#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! TEST-020: DB-backed governance follow-ups (GW-005 review).
//! - Generic secrets are never injection candidates on governed LLM hosts (SEC-37).
//! - An origin preflight mismatch writes `secrets.origin` and emits
//!   `NOTIFY aurora_secret_origin {secret_id, status}`.

use aurora_config::{KeyOrigin, LlmEndpoint, LlmSection};
use aurora_db::bus::{BusEvent, Channel, Listener};
use aurora_db::repo::gateway as repo;
use aurora_gateway::auth::{CredentialKind, RequestIdentity};
use aurora_gateway::governance::{BackstopInfo, DbGovSource, Governance, instance_from_config};
use aurora_gateway::resolve::{DbSource, Resolver};
use aurora_llm::{KeyOriginRule, ProviderRegistry};
use aurora_secrets::SecretRef;
use aurora_testkit::TestDb;
use aurora_testkit::wiremock::matchers::{method, path};
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const GOV_OPEN: &str = r#"{"llm":{"posture":"open","endpoints":{"anthropic":{"allowed":true}}}}"#;

fn llm_section(allowed_orgs: Vec<String>) -> LlmSection {
    LlmSection {
        enabled_providers: vec!["anthropic".into()],
        provider_preference: vec![],
        endpoints: vec![LlmEndpoint {
            id: "anthropic".into(),
            provider: "anthropic".into(),
            allowed_models: vec![],
            key_origin: Some(KeyOrigin {
                method: "provider_account".into(),
                allowed_organization_ids: allowed_orgs,
                ..Default::default()
            }),
            ..Default::default()
        }],
    }
}

fn gov_config(known_hosts: Vec<String>) -> aurora_config::Governance {
    aurora_config::Governance {
        allowed_key_scopes: vec!["organization".into(), "workspace".into()],
        llm_minimum_posture: aurora_config::LlmPosture::Open,
        require_key_origin: false,
        known_llm_hosts: known_hosts,
        origin_recheck_secs: 3600,
        egress_minimum_posture: aurora_config::EgressPosture::Open,
        baseline_allowlist: vec![],
        open_lane_minimum: aurora_config::OpenLanePosture::Enabled,
    }
}

struct Scope {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
}

async fn seed_scope(p: &sqlx::PgPool, governance: &str) -> Scope {
    let s = Scope {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id, governance) VALUES ($1,'acme','O','ext',$2::jsonb)",
    )
    .bind(s.org)
    .bind(governance)
    .execute(p)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'main','W')")
        .bind(s.ws)
        .bind(s.org)
        .execute(p)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','A','byo')",
    )
    .bind(s.agent)
    .bind(s.org)
    .bind(s.ws)
    .execute(p)
    .await
    .unwrap();
    s
}

/// Insert an LLM secret (`kind = 'llm'`) bound to the `anthropic` endpoint.
async fn insert_llm_secret(p: &sqlx::PgPool, s: &Scope, origin: Option<Value>) -> Uuid {
    let id = Uuid::now_v7();
    let vref = SecretRef::ext("mem", "k0", None, "1").unwrap().to_column();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode, \
         llm_endpoint_id, host_pattern, injection, value_source, value_ref, origin) \
         VALUES ($1,$2,$3,'llm-key','llm','anthropic','api_key','anthropic','api.anthropic.com',\
         '{}','reference',$4,$5)",
    )
    .bind(id)
    .bind(s.org)
    .bind(s.ws)
    .bind(vref)
    .bind(origin)
    .execute(p)
    .await
    .unwrap();
    id
}

/// Insert a generic (non-LLM) secret whose host pattern is `host_pattern`.
async fn insert_generic_secret(p: &sqlx::PgPool, s: &Scope, host_pattern: &str) -> Uuid {
    let id = Uuid::now_v7();
    let vref = SecretRef::ext("mem", "k1", None, "1").unwrap().to_column();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) \
         VALUES ($1,$2,$3,'generic-' || $1::text,'generic',$4,'{\"kind\":\"bearer\"}','reference',$5)",
    )
    .bind(id)
    .bind(s.org)
    .bind(s.ws)
    .bind(host_pattern)
    .bind(vref)
    .execute(p)
    .await
    .unwrap();
    id
}

fn identity(s: &Scope) -> RequestIdentity {
    RequestIdentity {
        credential_id: Uuid::now_v7(),
        kind: CredentialKind::Agent,
        agent_id: s.agent,
        workspace_id: s.ws,
        org_id: s.org,
        conversation_id: None,
        turn_id: None,
        context_label: None,
        principals: vec![s.agent],
        capabilities: vec![],
    }
}

async fn mock_anthropic(org_header: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("anthropic-organization-id", org_header)
                .set_body_string("{\"data\":[]}"),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn generic_secret_is_never_injected_on_governed_llm_hosts() {
    let db = TestDb::new().await;
    let p = db.pool();
    // Known LLM host (no configured endpoint) plus a configured endpoint host (mock provider).
    let provider = mock_anthropic("org_ok").await;
    let endpoint_addr = *provider.address();
    let s = seed_scope(p, GOV_OPEN).await;
    let openai = insert_generic_secret(p, &s, "api.openai.com").await;
    let endpoint_secret = insert_generic_secret(p, &s, &endpoint_addr.to_string()).await;
    // Control: a generic secret on an ordinary host is a candidate.
    let ordinary = insert_generic_secret(p, &s, "api.example.com").await;

    let llm = Arc::new(
        ProviderRegistry::from_config_with_anthropic_base(
            &llm_section(vec!["org_ok".into()]),
            &provider.uri(),
        )
        .unwrap(),
    );
    let gov = Arc::new(Governance::new(
        instance_from_config(&gov_config(vec!["api.openai.com".into()]), &llm),
        llm.clone(),
        Arc::new(DbGovSource(p.clone())),
    ));
    let resolver = Resolver::new(Arc::new(DbSource(p.clone())), llm).with_governance(gov);
    let id = identity(&s);

    let control = resolver.resolve(&id, "api.example.com", 443).await.unwrap();
    assert_eq!(control.secrets.len(), 1, "control secret must match");
    assert_eq!(control.secrets[0].id, ordinary);

    let openai_host = resolver.resolve(&id, "api.openai.com", 443).await.unwrap();
    assert!(openai_host.is_llm_host, "known host must be governed");
    assert!(
        openai_host.secrets.iter().all(|c| c.id != openai),
        "generic secret injected on a known LLM host"
    );
    assert!(openai_host.secrets.is_empty());

    let endpoint_host = resolver
        .resolve(&id, &endpoint_addr.ip().to_string(), endpoint_addr.port())
        .await
        .unwrap();
    assert!(endpoint_host.is_llm_host, "endpoint host must be governed");
    assert!(
        endpoint_host
            .secrets
            .iter()
            .all(|c| c.id != endpoint_secret),
        "generic secret injected on a configured endpoint host"
    );
    assert!(endpoint_host.secrets.is_empty());
}

#[tokio::test]
async fn preflight_mismatch_writes_origin_and_notifies() {
    let db = TestDb::new().await;
    let p = db.pool();
    // The provider reports a foreign account for the candidate key.
    let provider = mock_anthropic("org_bad").await;
    let s = seed_scope(p, GOV_OPEN).await;
    let stored = json!({"status": "verified", "organization_id": "org_ok", "value_version": 1});
    let secret = insert_llm_secret(p, &s, Some(stored)).await;

    let llm = Arc::new(
        ProviderRegistry::from_config_with_anthropic_base(
            &llm_section(vec!["org_ok".into()]),
            &provider.uri(),
        )
        .unwrap(),
    );
    let gov = Governance::new(
        instance_from_config(&gov_config(vec![]), &llm),
        llm.clone(),
        Arc::new(DbGovSource(p.clone())),
    );
    let rule = KeyOriginRule::from_config(&KeyOrigin {
        method: "provider_account".into(),
        allowed_organization_ids: vec!["org_ok".into()],
        ..Default::default()
    })
    .unwrap();
    let backstop = BackstopInfo {
        secret_id: secret,
        endpoint_id: "anthropic".into(),
        rule,
        version: 1,
        manage_url: "https://aurora.test/".into(),
    };

    let mut listener = Listener::connect(p, &[Channel::SecretOrigin])
        .await
        .unwrap();
    // Consume the initial resync marker so only the real notification is awaited.
    let first = tokio::time::timeout(Duration::from_secs(5), listener.next())
        .await
        .expect("listener resync");
    assert_eq!(first, BusEvent::Resync);

    let err = gov
        .preflight(&backstop, b"sk-foreign")
        .await
        .expect_err("mismatch must refuse");
    assert_eq!(err.code(), "llm_key_wrong_organization");

    // Write-back is spawned; poll the row until it lands.
    let mut origin: Option<Value> = None;
    for _ in 0..100 {
        let (o,): (Option<Value>,) = sqlx::query_as("SELECT origin FROM secrets WHERE id = $1")
            .bind(secret)
            .fetch_one(p)
            .await
            .unwrap();
        if o.as_ref().and_then(|v| v.get("status")) == Some(&json!("mismatch")) {
            origin = o;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let origin = origin.expect("origin status written back as mismatch");
    assert_eq!(origin["status"], "mismatch");
    assert_eq!(origin["organization_id"], "org_bad");
    assert_eq!(origin["value_version"], 1);

    // NOTIFY aurora_secret_origin {secret_id, status}.
    let payload = loop {
        match tokio::time::timeout(Duration::from_secs(5), listener.next())
            .await
            .expect("secret origin notification")
        {
            BusEvent::Notification {
                channel: Channel::SecretOrigin,
                payload,
            } => break payload,
            BusEvent::Notification { .. } | BusEvent::Resync => continue,
        }
    };
    assert_eq!(payload, json!({"secret_id": secret, "status": "mismatch"}));

    // Write-back for an unknown secret id is a no-op, not an error.
    repo::update_secret_origin(p, Uuid::now_v7(), &json!({"status": "mismatch"}))
        .await
        .unwrap();
}

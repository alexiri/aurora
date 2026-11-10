#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! DB-backed policy view: loading, cache, NOTIFY invalidation, request-log insert.

use aurora_db::bus::{Channel, notify};
use aurora_db::repo::gateway as repo;
use aurora_gateway::auth::{CredentialKind, RequestIdentity};
use aurora_gateway::invalidation::Hub;
use aurora_gateway::resolve::{DbSource, Resolver};
use aurora_secrets::SecretRef;
use aurora_testkit::TestDb;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn loads_view_and_invalidates_on_notify() {
    let db = TestDb::new().await;
    let p = db.pool();
    let (org, ws, agent, gen_id, secret) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'acme','O','ext')",
    )
    .bind(org)
    .execute(p)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'main','W')")
        .bind(ws)
        .bind(org)
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','A','byo')")
        .bind(agent)
        .bind(org)
        .bind(ws)
        .execute(p)
        .await
        .unwrap();
    let vref = SecretRef::ext("mem", "k1", None, "1").unwrap().to_column();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) \
         VALUES ($1,$2,$3,'s','generic','api.example.com','{\"kind\":\"bearer\"}','reference',$4)",
    )
    .bind(secret)
    .bind(org)
    .bind(ws)
    .bind(vref)
    .execute(p)
    .await
    .unwrap();
    sqlx::query("INSERT INTO policy_generations (id, org_id, workspace_id, number, status, default_action) VALUES ($1,$2,$3,1,'published','block')")
        .bind(gen_id)
        .bind(org)
        .bind(ws)
        .execute(p)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO policy_rules (id, generation_id, logical_id, priority, name, identities, targets, action) \
         VALUES ($1,$2,$3,10,'grant','[]',$4,'allow')",
    )
    .bind(Uuid::now_v7())
    .bind(gen_id)
    .bind(Uuid::now_v7())
    .bind(json!([{"type":"secret","id":secret}]))
    .execute(p)
    .await
    .unwrap();

    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            endpoints: vec![],
        })
        .unwrap(),
    );
    let resolver = Resolver::new(Arc::new(DbSource(p.clone())), llm);
    let hub = Hub::new();
    hub.register(resolver.cache());
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(hub.clone().run(p.clone(), rx));
    while !hub.is_connected() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let id = RequestIdentity {
        credential_id: Uuid::now_v7(),
        kind: CredentialKind::Agent,
        agent_id: agent,
        workspace_id: ws,
        org_id: org,
        conversation_id: None,
        turn_id: None,
        context_label: None,
        speaker_user_id: None,
        speaker_group_ids: Vec::new(),
        capabilities: vec![],
        byo: None,
    };
    let rv = resolver.resolve(&id, "api.example.com", 443).await.unwrap();
    assert_eq!(rv.ws_policy.ordered_rules.len(), 1);
    assert_eq!(rv.secrets.len(), 1);
    assert_eq!(
        (rv.org_slug.as_str(), rv.ws_slug.as_str()),
        ("acme", "main")
    );
    assert!(
        resolver
            .resolve(&id, "other.example.com", 443)
            .await
            .unwrap()
            .secrets
            .is_empty()
    );

    // A newly published rule is invisible (cached) until the NOTIFY arrives.
    sqlx::query(
        "INSERT INTO policy_rules (id, generation_id, logical_id, priority, name, identities, targets, action) \
         VALUES ($1,$2,$3,5,'stop','[]','[]','block')",
    )
    .bind(Uuid::now_v7())
    .bind(gen_id)
    .bind(Uuid::now_v7())
    .execute(p)
    .await
    .unwrap();
    assert_eq!(
        resolver
            .resolve(&id, "api.example.com", 443)
            .await
            .unwrap()
            .ws_policy
            .ordered_rules
            .len(),
        1
    );
    notify(
        p,
        Channel::GatewayInvalidate,
        &json!({"scope":"workspace","id":ws}),
    )
    .await
    .unwrap();
    let mut n = 0;
    for _ in 0..100 {
        n = resolver
            .resolve(&id, "api.example.com", 443)
            .await
            .unwrap()
            .ws_policy
            .ordered_rules
            .len();
        if n == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(n, 2);

    // Request log insert + secret touch.
    let row = repo::RequestLogRow {
        id: Uuid::now_v7(),
        ts: chrono::Utc::now(),
        org_id: org,
        workspace_id: ws,
        agent_id: agent,
        conversation_id: None,
        turn_id: None,
        credential_kind: "agent",
        lane: "mitm",
        method: "GET".into(),
        host: "api.example.com".into(),
        path: "/x".into(),
        app_id: None,
        tool_id: None,
        connection_id: None,
        secret_id: Some(secret),
        decision: "allow",
        rule_logical_id: None,
        rule_name: Some("grant".into()),
        upstream_status: Some(200),
        status: 200,
        error_code: None,
        injected: 1,
        latency_ms: 3,
        bytes_up: 0,
        bytes_down: 0,
        context_label: None,
        speaker_user_id: None,
    };
    repo::insert_request_log(p, &[row]).await.unwrap();
    let (cnt,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM request_log WHERE workspace_id = $1 AND rule_name = 'grant'",
    )
    .bind(ws)
    .fetch_one(p)
    .await
    .unwrap();
    assert_eq!(cnt, 1);
    repo::touch_secret_last_used(p, secret).await.unwrap();
    let (touched,): (bool,) =
        sqlx::query_as("SELECT last_used_at IS NOT NULL FROM secrets WHERE id = $1")
            .bind(secret)
            .fetch_one(p)
            .await
            .unwrap();
    assert!(touched);
    let _ = tx.send(true);
}

#[tokio::test]
async fn governance_reads_and_origin_write_back() {
    let db = TestDb::new().await;
    let p = db.pool();
    let org = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id, governance) VALUES ($1,'acme','O','ext','{\"llm\":{\"posture\":\"open\"}}')",
    )
    .bind(org)
    .execute(p)
    .await
    .unwrap();
    let g = repo::org_governance(p, org).await.unwrap().unwrap();
    assert_eq!(g["llm"]["posture"], "open");
    assert!(
        repo::org_governance(p, Uuid::now_v7())
            .await
            .unwrap()
            .is_none()
    );
    // No LLM secret matches: the write is a no-op, not an error.
    repo::update_secret_origin(p, Uuid::now_v7(), &json!({"status":"mismatch"}))
        .await
        .unwrap();
}

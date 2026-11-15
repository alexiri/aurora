#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_config::{KeyOrigin, LlmEndpoint, SecretStoreConfig};
use aurora_secrets::{MemoryStore, SecretVault, StoreRegistry};
use base64::Engine;
use http::Method;
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const GOOD_KEY: &str = "sk-good-secret-value";

struct World {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
    admin: String,
    mem: Arc<MemoryStore>,
    _litellm: MockServer,
    _anthropic: MockServer,
    /// (FIX-105) answers 503 on `/v1/models`.
    _flaky: MockServer,
    /// (P4-LLM-1) answers as api.openai.com (reached through `llm.dev_upstream_overrides`).
    _openai: MockServer,
    /// (P4-LLM-2) Google's token endpoint (`llm.dev_upstream_overrides`) and the service-account key it accepts.
    google: aurora_testkit::llm::google_token::MockGoogleToken,
    vkey: aurora_testkit::llm::google_token::TestKey,
    /// (P4-LLM-3) An Azure OpenAI resource (key `azure-key`) and the Entra authority that knows `azure_app`.
    azure: aurora_testkit::llm::azure::MockAzure,
    entra: aurora_testkit::llm::entra::MockEntra,
    azure_app: aurora_testkit::llm::entra::TestApp,
}

const AZURE_TENANT: &str = "11111111-2222-3333-4444-555555555555";

fn endpoint(id: &str, base: &str, rule: Option<KeyOrigin>) -> LlmEndpoint {
    LlmEndpoint {
        id: id.into(),
        provider: "litellm".into(),
        base_url: Some(base.into()),
        key_origin: rule,
        ..LlmEndpoint::default()
    }
}

fn azure_endpoint(id: &str, base: &str, tenant: Option<&str>) -> LlmEndpoint {
    LlmEndpoint {
        id: id.into(),
        provider: "azure_openai".into(),
        base_url: Some(base.into()),
        models: vec!["gpt-4o".into()],
        default_model: Some("gpt-4o".into()),
        key_origin: tenant.map(|t| KeyOrigin {
            method: "provider_account".into(),
            allowed_organization_ids: vec![t.into()],
            ..KeyOrigin::default()
        }),
        ..LlmEndpoint::default()
    }
}

async fn world() -> World {
    let litellm = MockServer::start().await;
    let anthropic = MockServer::start().await;
    // LiteLLM: bad key rejected, everything else lists models.
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer sk-bad"))
        .respond_with(ResponseTemplate::new(401))
        .with_priority(1)
        .mount(&litellm)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": [{"id": "gpt-a"}, {"id": "gpt-b"}]})),
        )
        .mount(&litellm)
        .await;
    // FIX-019: LiteLLM `/key/info` by key: a team key, a teamless key, a refusal and a failure.
    for (key, resp) in [
        (
            "sk-team",
            ResponseTemplate::new(200)
                .set_body_json(json!({"key": "x", "info": {"team_id": "t1", "user_id": "u1"}})),
        ),
        (
            "sk-teamless",
            ResponseTemplate::new(200)
                .set_body_json(json!({"key": "x", "info": {"team_id": null, "user_id": "u2"}})),
        ),
        ("sk-info401", ResponseTemplate::new(401)),
        ("sk-info500", ResponseTemplate::new(500)),
    ] {
        Mock::given(method("GET"))
            .and(path("/key/info"))
            .and(header("authorization", format!("Bearer {key}").as_str()))
            .respond_with(resp)
            .mount(&litellm)
            .await;
    }
    // Anthropic: account by key.
    for (key, org) in [
        ("sk-ant-good", Some("org-good")),
        ("sk-ant-other", Some("org-bad")),
        ("sk-ant-none", None),
    ] {
        let mut r = ResponseTemplate::new(200)
            .set_body_json(json!({"data": [{"id": "claude-x"}], "has_more": false}));
        if let Some(o) = org {
            r = r.insert_header("anthropic-organization-id", o);
        }
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("x-api-key", key))
            .respond_with(r)
            .mount(&anthropic)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-api-key", "sk-ant-bad"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&anthropic)
        .await;
    let _ = query_param("limit", "1");
    let flaky = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&flaky)
        .await;

    // OpenAI: account by key, reported in the response headers of the model list.
    let openai = MockServer::start().await;
    for (key, org) in [("sk-oai-good", "org-good"), ("sk-oai-other", "org-bad")] {
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", format!("Bearer {key}").as_str()))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("openai-organization", org)
                    .set_body_json(json!({"data": [{"id": "gpt-5"}]})),
            )
            .mount(&openai)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .with_priority(5)
        .mount(&openai)
        .await;
    let vkey = aurora_testkit::llm::google_token::TestKey::generate(
        "key-proj",
        "sa@key-proj.iam.gserviceaccount.com",
    );
    let google = aurora_testkit::llm::google_token::MockGoogleToken::start(&vkey).await;
    let gu = google.base().to_owned();
    let azure_app = aurora_testkit::llm::entra::TestApp {
        tenant_id: AZURE_TENANT.into(),
        client_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
        client_secret: "entra-client-secret-value".into(),
    };
    let entra = aurora_testkit::llm::entra::MockEntra::start(&azure_app).await;
    let azure = aurora_testkit::llm::azure::MockAzure::start("azure-key").await;
    let (eu, zu) = (entra.base().to_owned(), azure.base().to_owned());
    let (lu, au, fu) = (litellm.uri(), anthropic.uri(), flaky.uri());
    let ou = openai.uri();
    let h = Harness::with_config(move |c| {
        c.llm.enabled_providers = [
            "litellm",
            "anthropic",
            "openai",
            "openai_compatible",
            "vertex",
            "azure_openai",
        ]
        .map(String::from)
        .to_vec();
        c.llm
            .dev_upstream_overrides
            .insert("https://oauth2.googleapis.com".into(), gu.clone());
        c.llm
            .dev_upstream_overrides
            .insert("https://login.microsoftonline.com".into(), eu.clone());
        c.llm
            .dev_upstream_overrides
            .insert("https://api.openai.com".into(), ou.clone());
        c.llm.endpoints = vec![
            endpoint(
                "central",
                &lu,
                Some(KeyOrigin {
                    method: "endpoint".into(),
                    ..KeyOrigin::default()
                }),
            ),
            endpoint("dead", "http://127.0.0.1:1", None),
            // (FIX-105) a static model list must not turn an endpoint that answers 503 into a
            // verified key.
            LlmEndpoint {
                models: vec!["gpt-a".into()],
                ..endpoint("flaky", &fu, None)
            },
            LlmEndpoint {
                models: vec!["gpt-a".into()],
                ..endpoint(
                    "flaky_rule",
                    &fu,
                    Some(KeyOrigin {
                        method: "endpoint".into(),
                        ..KeyOrigin::default()
                    }),
                )
            },
            endpoint(
                "team",
                &lu,
                Some(KeyOrigin {
                    method: "litellm_team".into(),
                    ..KeyOrigin::default()
                }),
            ),
            endpoint(
                "team_req",
                &lu,
                Some(KeyOrigin {
                    method: "litellm_team".into(),
                    allowed_team_ids: vec!["t1".into()],
                    ..KeyOrigin::default()
                }),
            ),
            endpoint(
                "dead_rule",
                "http://127.0.0.1:1",
                Some(KeyOrigin {
                    method: "endpoint".into(),
                    ..KeyOrigin::default()
                }),
            ),
            LlmEndpoint {
                id: "oai".into(),
                provider: "openai".into(),
                key_origin: Some(KeyOrigin {
                    method: "provider_account".into(),
                    allowed_organization_ids: vec!["org-good".into()],
                    ..KeyOrigin::default()
                }),
                ..LlmEndpoint::default()
            },
            // (P4-LLM-2) Vertex: the key's project must be `key-proj` (`vx`) / another one (`vx_other`).
            LlmEndpoint {
                id: "vx".into(),
                provider: "vertex".into(),
                project_id: Some("inference-proj".into()),
                location: Some("us-east5".into()),
                models: vec!["claude-x".into()],
                key_origin: Some(KeyOrigin {
                    method: "provider_account".into(),
                    allowed_workspace_ids: vec!["key-proj".into()],
                    ..KeyOrigin::default()
                }),
                ..LlmEndpoint::default()
            },
            LlmEndpoint {
                id: "vx_other".into(),
                provider: "vertex".into(),
                project_id: Some("inference-proj".into()),
                location: Some("global".into()),
                models: vec!["claude-x".into()],
                key_origin: Some(KeyOrigin {
                    method: "provider_account".into(),
                    allowed_workspace_ids: vec!["another-proj".into()],
                    ..KeyOrigin::default()
                }),
                ..LlmEndpoint::default()
            },
            // (P4-LLM-3) Azure: no rule, an allowed tenant, another tenant.
            azure_endpoint("az", &zu, None),
            azure_endpoint("az_tenant", &zu, Some(AZURE_TENANT)),
            azure_endpoint(
                "az_other",
                &zu,
                Some("99999999-2222-3333-4444-555555555555"),
            ),
            LlmEndpoint {
                id: "compat".into(),
                provider: "openai_compatible".into(),
                base_url: Some(lu.clone()),
                auth_header: Some("api-key".into()),
                ..LlmEndpoint::default()
            },
            LlmEndpoint {
                id: "anthropic".into(),
                provider: "anthropic".into(),
                key_origin: Some(KeyOrigin {
                    method: "provider_account".into(),
                    allowed_organization_ids: vec!["org-good".into()],
                    ..KeyOrigin::default()
                }),
                ..LlmEndpoint::default()
            },
        ];
        c.secrets.stores.push(SecretStoreConfig {
            id: "memory".into(),
            kind: "memory".into(),
            ..SecretStoreConfig::default()
        });
    })
    .await;
    // Registry with the Anthropic mock; vault with a preloadable memory store.
    let reg =
        aurora_llm::ProviderRegistry::from_config_with_anthropic_base(&h.state.config.llm, &au)
            .unwrap();
    let k = base64::engine::general_purpose::STANDARD.encode([b'a'; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{k}"), "k1").unwrap());
    let mut sr = StoreRegistry::from_config(&h.state.config.secrets, keyring).unwrap();
    let mem = Arc::new(MemoryStore::new("memory"));
    sr.register(mem.clone());
    let mut h = h;
    h.state = h
        .state
        .clone()
        .with_llm(Arc::new(reg))
        .with_vault(Arc::new(SecretVault::new(sr)));
    h.app = test_router(h.state.clone());

    let org = h.org().await;
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(org)
        .bind(json!({"llm": {"endpoints": {
            "central": {"key_scopes": ["organization"]},
            "dead": {}, "dead_rule": {}, "anthropic": {}, "team": {}, "team_req": {},
            "flaky": {}, "flaky_rule": {}, "oai": {}, "compat": {}, "vx": {}, "vx_other": {},
            "az": {}, "az_tenant": {}, "az_other": {}
        }}}))
        .execute(h.pool())
        .await
        .unwrap();
    let owner = h.user("o@x.io", false).await;
    let admin = h.user("a@x.io", false).await;
    let member = h.user("m@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, admin, "admin", "active").await;
    h.member(org, member, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, member, "member").await;
    let (_, owner) = h.good_session(owner).await;
    let (_, admin) = h.good_session(admin).await;
    let (_, member) = h.good_session(member).await;
    World {
        h,
        org,
        ws,
        owner,
        member,
        admin,
        mem,
        _litellm: litellm,
        _anthropic: anthropic,
        _flaky: flaky,
        _openai: openai,
        google,
        vkey,
        azure,
        entra,
        azure_app,
    }
}

impl World {
    async fn call(&self, m: Method, uri: &str, tok: &str, body: Option<Value>) -> (u16, Value) {
        let c = cookie(tok);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", c.as_str()), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }
    fn ws_url(&self) -> String {
        format!("/v1/workspaces/{}/secrets", self.ws)
    }
    fn org_url(&self) -> String {
        format!("/v1/orgs/{}/secrets", self.org)
    }
}

fn no_value(v: &Value, needles: &[&str]) {
    let s = v.to_string();
    for n in needles {
        assert!(!s.contains(n), "response leaks {n}: {s}");
    }
}

async fn wait_audit(w: &World, action: &str) {
    for _ in 0..50 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
        if n > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no audit row for {action}");
}

#[tokio::test]
async fn generic_crud_never_returns_values_and_audits() {
    let w = world().await;
    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(json!({
                "name": "github", "kind": "generic", "host_pattern": "api.github.com",
                "injection": {"kind": "header", "name": "X-Token"}, "value": "ghp_supersecret"
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    no_value(&v, &["ghp_supersecret"]);
    assert_eq!(v["scope"], "workspace");
    assert_eq!(v["value_source"], "managed");
    assert_eq!(
        v["injection"],
        json!({"kind": "header", "name": "X-Token", "format": "{value}"})
    );
    assert_eq!(v["value_version"], 1);
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), l.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(n.payload().contains("workspace"));
    let id = v["id"].as_str().unwrap().to_owned();

    // stored ciphertext is not the plaintext
    let vr: String = sqlx::query_scalar("SELECT value_ref FROM secrets WHERE id = $1::uuid")
        .bind(&id)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert!(!vr.contains("ghp_supersecret"));

    // duplicate name
    let (s, v2) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(json!({
            "name": "github", "kind": "generic", "host_pattern": "x.example.com", "value": "v"})),
        )
        .await;
    assert_eq!(
        (s, v2["error"]["code"].as_str()),
        (409, Some("name_taken")),
        "{v2}"
    );

    // member: cannot read (I-27: reads follow the matrix), cannot write; the owner reads
    let (s, v) = w
        .call(Method::GET, &format!("/v1/secrets/{id}"), &w.member, None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    let (s, _) = w.call(Method::GET, &w.ws_url(), &w.member, None).await;
    assert_eq!(s, 403);
    let (s, _) = w
        .call(Method::GET, &format!("/v1/secrets/{id}"), &w.owner, None)
        .await;
    assert_eq!(s, 200);
    let (s, _) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.member,
            Some(json!({
        "name": "n2", "kind": "generic", "host_pattern": "a.example.com", "value": "v"})),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = w
        .call(
            Method::DELETE,
            &format!("/v1/secrets/{id}"),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 403);

    // list
    let (s, v) = w
        .call(
            Method::GET,
            &format!("{}?kind=generic", w.ws_url()),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    no_value(&v, &["ghp_supersecret"]);

    // patch + value update
    let (s, v) = w
        .call(
            Method::PATCH,
            &format!("/v1/secrets/{id}"),
            &w.owner,
            Some(json!({"path_pattern": "/v3/**"})),
        )
        .await;
    assert_eq!(
        (s, v["path_pattern"].as_str()),
        (200, Some("/v3/**")),
        "{v}"
    );
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/secrets/{id}/value"),
            &w.owner,
            Some(json!({"value": "ghp_second"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["value_version"], 2);
    no_value(&v, &["ghp_second"]);

    // invalid inputs
    for body in [
        json!({"name": "a", "kind": "generic", "host_pattern": "a.*.*", "value": "v"}),
        json!({"name": "a", "kind": "generic", "host_pattern": "a.com", "value": "v", "reference": {"store_id": "memory", "path": "p"}}),
        json!({"name": "a", "kind": "generic", "host_pattern": "a.com"}),
        json!({"name": "a", "kind": "generic", "host_pattern": "a.com", "value": "v", "injection": {"kind": "header", "name": "Host"}}),
    ] {
        let (s, v) = w
            .call(Method::POST, &w.ws_url(), &w.owner, Some(body))
            .await;
        assert_eq!(s, 400, "{v}");
    }

    let (s, _) = w
        .call(Method::DELETE, &format!("/v1/secrets/{id}"), &w.owner, None)
        .await;
    assert_eq!(s, 204);
    let (s, _) = w
        .call(Method::GET, &format!("/v1/secrets/{id}"), &w.owner, None)
        .await;
    assert_eq!(s, 404);
    for a in [
        "secret.create",
        "secret.update",
        "secret.value_update",
        "secret.delete",
    ] {
        wait_audit(&w, a).await;
    }
    let meta: Vec<Value> =
        sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action LIKE 'secret.%'")
            .fetch_all(w.h.pool())
            .await
            .unwrap();
    no_value(&Value::Array(meta), &["ghp_supersecret", "ghp_second"]);
}

#[tokio::test]
async fn generic_secret_on_governed_llm_host_is_refused() {
    let w = world().await;
    for host in ["api.openai.com", "*.openai.com", "*"] {
        let (s, v) = w
            .call(
                Method::POST,
                &w.ws_url(),
                &w.owner,
                Some(json!({
                "name": "x", "kind": "generic", "host_pattern": host, "value": "v"})),
            )
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("llm_host_requires_llm_key")),
            "{host}: {v}"
        );
    }
    // PATCH is also guarded.
    let (_, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(json!({
        "name": "ok", "kind": "generic", "host_pattern": "a.example.com", "value": "v"})),
        )
        .await;
    let id = v["id"].as_str().unwrap();
    let (s, v) = w
        .call(
            Method::PATCH,
            &format!("/v1/secrets/{id}"),
            &w.owner,
            Some(json!({"host_pattern": "api.openai.com"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_host_requires_llm_key"))
    );
}

#[tokio::test]
async fn llm_key_admission_validation_and_origin() {
    let w = world().await;
    let llm = |ep: &str, val: &str| json!({"name": format!("k-{ep}-{}", val.len()), "kind": "llm", "llm": {"endpoint_id": ep}, "value": val});

    // central: workspace scope not permitted, org scope permitted (owner/admin only).
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("central", GOOD_KEY)),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("llm_key_not_permitted")),
        "{v}"
    );
    let (s, _) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.member,
            Some(llm("central", GOOD_KEY)),
        )
        .await;
    assert_eq!(s, 403);
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.admin,
            Some(llm("central", GOOD_KEY)),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    no_value(&v, &[GOOD_KEY]);
    assert_eq!(v["kind"], "llm");
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["method"], "endpoint");
    assert_eq!(v["host_pattern"], w._litellm.address().to_string().as_str());
    assert_eq!(v["injection"], json!({"kind": "bearer"}));
    let id = v["id"].as_str().unwrap().to_owned();

    // models via the stored key: `can_own` / owner-admin only (I-27); agent model pickers use
    // `GET /v1/agents/{agent}/models`
    let (s, _) = w
        .call(
            Method::GET,
            &format!("/v1/secrets/{id}/models"),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 403);
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/secrets/{id}/models"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 2);

    // invalid key
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(llm("central", "sk-bad")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_invalid")),
        "{v}"
    );

    // anthropic (workspace scope allowed): good, wrong org, unverifiable
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("anthropic", "sk-ant-good")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["organization_id"], "org-good");
    assert_eq!(v["injection"]["name"], "x-api-key");
    let aid = v["id"].as_str().unwrap().to_owned();
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("anthropic", "sk-ant-other")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_wrong_organization")),
        "{v}"
    );
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("anthropic", "sk-ant-none")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_unverifiable")),
        "{v}"
    );
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("anthropic", "sk-ant-bad")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_invalid")),
        "{v}"
    );
    // nothing was saved for the refused ones
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets WHERE workspace_id = $1")
        .bind(w.ws)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);

    // value change re-verifies: wrong-org replacement is refused, value_version unchanged
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/secrets/{aid}/value"),
            &w.owner,
            Some(json!({"value": "sk-ant-other"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_wrong_organization"))
    );
    let (_, v) = w
        .call(Method::GET, &format!("/v1/secrets/{aid}"), &w.owner, None)
        .await;
    assert_eq!(v["value_version"], 1);
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/secrets/{aid}/value"),
            &w.owner,
            Some(json!({"value": "sk-ant-good"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["value_version"], 2);
    assert_eq!(v["origin"]["value_version"], 2);

    // explicit verify
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{aid}/verify"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(
        (s, v["origin"]["status"].as_str()),
        (200, Some("verified")),
        "{v}"
    );
    let (s, _) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{aid}/verify"),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 403);

    // LLM host/injection are read-only
    let (s, _) = w.call(Method::POST, &w.org_url(), &w.owner, Some(json!({
        "name": "z", "kind": "llm", "llm": {"endpoint_id": "central"}, "host_pattern": "x.com", "value": GOOD_KEY}))).await;
    assert_eq!(s, 400);
    wait_audit(&w, "secret.verify").await;
}

// FIX-019 (D-136, C-502): `litellm_team` with a team key, a teamless key, and the reason a
// refusal carries.
#[tokio::test]
async fn litellm_team_teamless_and_refusal_detail() {
    let w = world().await;
    let llm = |ep: &str, val: &str| json!({"name": format!("k-{ep}-{val}"), "kind": "llm", "llm": {"endpoint_id": ep}, "value": val});

    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(llm("team", "sk-team")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["team_id"], "t1");
    assert_eq!(v["origin"]["detail"], Value::Null);

    // No required team ids: a teamless key verifies with `team_id = null`.
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(llm("team", "sk-teamless")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["team_id"], Value::Null);
    let teamless = v["id"].as_str().unwrap().to_owned();

    // Required team ids: a team key in the list verifies; a teamless key is refused.
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(llm("team_req", "sk-team")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(llm("team_req", "sk-teamless")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_unverifiable")),
        "{v}"
    );
    assert_eq!(
        v["error"]["details"]["detail"], "key_reports_no_team",
        "{v}"
    );

    // `/key/info` refusing or failing says which step failed, never the key.
    for (key, want) in [
        ("sk-info401", "key_info_http_401"),
        ("sk-info500", "key_info_http_500"),
    ] {
        let (s, v) = w
            .call(Method::POST, &w.org_url(), &w.owner, Some(llm("team", key)))
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("llm_key_unverifiable")),
            "{key}: {v}"
        );
        assert_eq!(v["error"]["details"]["detail"], want, "{v}");
        no_value(&v, &[key]);
    }

    // Re-verify persists the reason on the key row: the rule now requires team ids.
    sqlx::query("UPDATE secrets SET llm_endpoint_id = 'team_req' WHERE id = $1::uuid")
        .bind(&teamless)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{teamless}/verify"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["origin"]["status"], "unverifiable");
    assert_eq!(v["origin"]["detail"], "key_reports_no_team");
    let (_, v) = w
        .call(
            Method::GET,
            &format!("/v1/secrets/{teamless}"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(v["origin"]["detail"], "key_reports_no_team", "{v}");
}

/// (FIX-105, CRED-6) An endpoint that answers 503 on `/v1/models` is unreachable for key
/// verification even when a static model list is configured: with a key-origin rule the key is
/// refused (422 `llm_endpoint_unreachable`, nothing saved); without one it is saved `pending`,
/// never `verified`.
#[tokio::test]
async fn a_static_model_list_does_not_verify_a_key_the_endpoint_never_saw() {
    let w = world().await;
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(json!({
                "name": "flaky-rule", "kind": "llm", "llm": {"endpoint_id": "flaky_rule"},
                "value": "sk-mistyped"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_endpoint_unreachable")),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets WHERE name = 'flaky-rule'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing saved");
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(json!({
                "name": "flaky", "kind": "llm", "llm": {"endpoint_id": "flaky"},
                "value": "sk-mistyped"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "pending", "{v}");
}

#[tokio::test]
async fn unreachable_endpoint_with_and_without_rule() {
    let w = world().await;
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(json!({
        "name": "dead", "kind": "llm", "llm": {"endpoint_id": "dead"}, "value": "sk-x"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "pending");
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.owner,
            Some(json!({
        "name": "dead2", "kind": "llm", "llm": {"endpoint_id": "dead_rule"}, "value": "sk-x"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_endpoint_unreachable")),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets WHERE name = 'dead2'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    // models of an unreachable endpoint: 502
    let id = {
        let (_, v) = w.call(Method::GET, &w.org_url(), &w.owner, None).await;
        v["items"][0]["id"].as_str().unwrap().to_owned()
    };
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/secrets/{id}/models"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (502, Some("llm_endpoint_unreachable")),
        "{v}"
    );
}

#[tokio::test]
async fn reference_mode_via_memory_store() {
    let w = world().await;
    w.mem.preload("kv/team/api", Some("key"), b"ref-value-123");
    w.mem.preload("kv/other/api", Some("key"), b"nope");
    w.mem.allow_reference(w.org, "kv/team/*");
    let body = |p: &str| {
        json!({
        "name": format!("ref-{p}").replace('/', "-"), "kind": "generic", "host_pattern": "api.example.com",
        "reference": {"store_id": "memory", "path": p, "field": "key"}})
    };
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(body("kv/team/api")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["value_source"], "reference");
    assert_eq!(
        v["reference"],
        json!({"store_id": "memory", "path": "kv/team/api", "field": "key"})
    );
    no_value(&v, &["ref-value-123"]);
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(body("kv/other/api")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("reference_not_permitted")),
        "{v}"
    );
    // inline store has no reference support
    let (s, _) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(json!({
        "name": "r3", "kind": "generic", "host_pattern": "a.example.com",
        "reference": {"store_id": "inline", "path": "x"}})),
        )
        .await;
    assert_eq!(s, 400);
    // unavailable store -> 503
    w.mem.set_unavailable(true);
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(body("kv/team/api2")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (503, Some("secret_store_unavailable")),
        "{v}"
    );
    w.mem.set_unavailable(false);

    let (s, v) = w
        .call(Method::GET, "/v1/secret-stores", &w.member, None)
        .await;
    assert_eq!(s, 200);
    let items = v["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|i| i["id"] == "inline" && i["capabilities"]["references"] == false)
    );
    assert!(
        items
            .iter()
            .any(|i| i["id"] == "memory" && i["capabilities"]["references"] == true)
    );
}

/// (P4-LLM-1, C-801, C-802, C-804) An `openai` key validates through the strict model listing, its account comes from
/// the `openai-organization` response header and must satisfy the endpoint's `provider_account` rule; a compatible
/// key validates through `models_path`; a mode the endpoint does not admit is refused; the provider listing reports
/// modes, shapes and harnesses.
#[tokio::test]
async fn openai_and_compatible_keys_validate_and_the_listing_reports_modes() {
    let w = world().await;
    let llm = |ep: &str, val: &str, mode: Option<&str>| {
        let mut l = json!({"endpoint_id": ep});
        if let Some(m) = mode {
            l["auth_mode"] = json!(m);
        }
        json!({"name": format!("k-{ep}-{}-{}", val.len(), mode.unwrap_or("")), "kind": "llm", "llm": l, "value": val})
    };
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("oai", "sk-oai-good", None)),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["method"], "provider_account");
    assert_eq!(v["origin"]["organization_id"], "org-good");
    assert_eq!(v["host_pattern"], w._openai.address().to_string().as_str());
    assert_eq!(v["injection"], json!({"kind": "bearer"}));
    no_value(&v, &["sk-oai-good"]);
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("oai", "sk-oai-other", None)),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_wrong_organization")),
        "{v}"
    );
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("oai", "sk-oai-bad", None)),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_invalid")),
        "{v}"
    );
    // a compatible key (api-key header) validates by listing
    let (s, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(llm("compat", "sk-compat", None)),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["method"], "endpoint");
    // modes the endpoint does not admit
    for (ep, mode) in [
        ("oai", "aws_sigv4"),
        ("compat", "azure_entra"),
        ("oai", "bogus"),
    ] {
        let (s, v) = w
            .call(
                Method::POST,
                &w.ws_url(),
                &w.owner,
                Some(llm(ep, "sk-oai-good", Some(mode))),
            )
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("llm_key_invalid")),
            "{ep} {mode}: {v}"
        );
        assert_eq!(
            v["error"]["details"]["reason"], "auth_mode_unsupported",
            "{v}"
        );
    }

    let (s, v) = w
        .call(Method::GET, "/v1/llm/providers", &w.member, None)
        .await;
    assert_eq!(s, 200, "{v}");
    let oa = v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "openai")
        .unwrap();
    assert_eq!(oa["auth_modes"], json!(["api_key"]));
    assert_eq!(
        oa["auth_mode_details"],
        json!([{"id": "api_key", "value_shape": "string", "recommended": true}])
    );
    assert_eq!(oa["dialects"], json!(["openai_chat", "openai_responses"]));
    let ep = |id: &str| {
        v["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()
            .clone()
    };
    let (oai, compat, anthropic) = (ep("oai"), ep("compat"), ep("anthropic"));
    assert_eq!(oai["auth_modes"], json!(["api_key"]));
    assert_eq!(oai["recommended_auth_mode"], "api_key");
    // harnesses whose dialects intersect: codex speaks responses, claude-code messages (D-260(a))
    assert_eq!(
        oai["harnesses"],
        json!(
            ["codex"]
                .iter()
                .filter(|h| w
                    .h
                    .state
                    .config
                    .hosted
                    .enabled_harnesses
                    .iter()
                    .any(|e| e == *h))
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(
        compat["harnesses"],
        json!([]),
        "chat-only: no shipped harness"
    );
    assert!(
        anthropic["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h == "claude-code")
    );
}

#[tokio::test]
async fn providers_listing_with_effective_governance() {
    let w = world().await;
    let (s, v) = w
        .call(Method::GET, "/v1/llm/providers", &w.member, None)
        .await;
    assert_eq!(s, 200);
    assert!(
        v["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["permitted"].is_null())
    );
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/llm/providers?org_id={}", w.org),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let central = v["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "central")
        .unwrap();
    assert_eq!(central["permitted"], true);
    assert_eq!(central["key_scopes"], json!(["organization"]));
    assert_eq!(central["origin_method"], "endpoint");
    assert!(
        v["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "litellm")
    );
}

#[tokio::test]
async fn proposal_accept_creates_grant_and_derived_rule_reject_and_delete_cleanup() {
    let w = world().await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(w.org).bind(w.ws)
        .execute(w.h.pool()).await.unwrap();
    let mk = |name: &str| {
        let id = Uuid::now_v7();
        (id, name.to_owned())
    };
    let (p1, n1) = mk("prop-1");
    let (p2, n2) = mk("prop-2");
    for (id, name) in [(p1, n1), (p2, n2)] {
        sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref, status, proposed_by_agent_id) VALUES ($1,$2,$3,$4,'generic','api.example.com','{\"kind\":\"bearer\"}','managed','pending:','proposed',$5)")
            .bind(id).bind(w.org).bind(w.ws).bind(name).bind(agent)
            .execute(w.h.pool()).await.unwrap();
    }
    // not proposed -> 409
    let (_, v) = w
        .call(
            Method::POST,
            &w.ws_url(),
            &w.owner,
            Some(json!({
        "name": "act", "kind": "generic", "host_pattern": "a.example.com", "value": "v"})),
        )
        .await;
    let active = v["id"].as_str().unwrap().to_owned();
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{active}/accept"),
            &w.owner,
            Some(json!({"value": "x"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_proposed"))
    );
    // member cannot accept
    let (s, _) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{p1}/accept"),
            &w.member,
            Some(json!({"value": "x"})),
        )
        .await;
    assert_eq!(s, 403);

    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{p1}/accept"),
            &w.owner,
            Some(json!({"value": "proposal-secret"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "active");
    no_value(&v, &["proposal-secret"]);
    let (gid,): (Uuid,) =
        sqlx::query_as("SELECT id FROM grants WHERE agent_id = $1 AND secret_id = $2")
            .bind(agent)
            .bind(p1)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let rule: Value = sqlx::query_scalar(
        "SELECT rule FROM derived_rules WHERE source_kind='grant' AND source_id = $1",
    )
    .bind(gid)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rule["action"], "allow");
    assert_eq!(rule["targets"], json!([{"type": "secret", "id": p1}]));
    assert_eq!(rule["identities"], json!([{"kind": "agent", "id": agent}]));
    // the derived rule parses with the policy library
    let _: aurora_policy::Rule = serde_json::from_value(rule).unwrap();

    // deleting the secret removes grant and derived rule
    let (s, _) = w
        .call(Method::DELETE, &format!("/v1/secrets/{p1}"), &w.owner, None)
        .await;
    assert_eq!(s, 204);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM derived_rules WHERE source_id = $1")
        .bind(gid)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);

    // reject
    let (s, _) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{p2}/reject"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/secrets/{p2}/reject"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_proposed"))
    );
    wait_audit(&w, "secret.accept").await;
    wait_audit(&w, "secret.reject").await;
}

#[tokio::test]
async fn org_secret_visibility_and_foreign_access() {
    let w = world().await;
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.admin,
            Some(json!({
        "name": "orgkey", "kind": "generic", "host_pattern": "api.example.com", "value": "v"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["scope"], "organization");
    let id = v["id"].as_str().unwrap();
    // org secrets are owner/admin only, for reads as for writes (I-27)
    let (s, v) = w.call(Method::GET, &w.org_url(), &w.member, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    let (s, _) = w
        .call(Method::GET, &format!("/v1/secrets/{id}"), &w.member, None)
        .await;
    assert_eq!(s, 403);
    let (s, _) = w.call(Method::GET, &w.org_url(), &w.admin, None).await;
    assert_eq!(s, 200);
    let (s, _) = w
        .call(
            Method::PUT,
            &format!("/v1/secrets/{id}/value"),
            &w.member,
            Some(json!({"value": "n"})),
        )
        .await;
    assert_eq!(s, 403);
    // outsider
    let o = w.h.user("out@x.io", false).await;
    let (_, tok) = w.h.good_session(o).await;
    let (s, _) = w
        .call(Method::GET, &format!("/v1/secrets/{id}"), &tok, None)
        .await;
    assert_eq!(s, 404);
}

/// (D-178 Q-R5-21, FIX-115) An org-level secret's creator (and, for LLM keys, origin) is shown
/// to org admins only: a workspace owner who lists or reads it to grant it gets the secret
/// without those fields; a workspace secret is unchanged.
#[tokio::test]
async fn org_secret_provenance_is_for_org_admins_only() {
    let w = world().await;
    let (s, v) = w
        .call(
            Method::POST,
            &w.org_url(),
            &w.admin,
            Some(json!({
        "name": "orgkey2", "kind": "generic", "host_pattern": "api.example.com", "value": "v"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    // Admin: creator visible.
    let (s, v) = w
        .call(Method::GET, &format!("/v1/secrets/{id}"), &w.admin, None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["created_by"].is_object(), "{v}");
    // A plain org member who owns a workspace (can discover org secrets to grant them): no
    // creator, no origin.
    let wo = w.h.user("wso@x.io", false).await;
    w.h.member(w.org, wo, "member", "active").await;
    w.h.bind_ws(w.ws, wo, "owner").await;
    let (_, wo) = w.h.good_session(wo).await;
    let (s, v) = w
        .call(Method::GET, &format!("/v1/secrets/{id}"), &wo, None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["created_by"].is_null() && v["origin"].is_null(), "{v}");
    assert_eq!(v["name"], "orgkey2");
    let (s, v) = w.call(Method::GET, &w.org_url(), &wo, None).await;
    assert_eq!(s, 200, "{v}");
    let item = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == id.as_str())
        .unwrap();
    assert!(
        item["created_by"].is_null() && item["origin"].is_null(),
        "{v}"
    );
    let (_, v) = w.call(Method::GET, &w.org_url(), &w.admin, None).await;
    let item = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == id.as_str())
        .unwrap();
    assert!(item["created_by"].is_object(), "{v}");
}

/// (P4-LLM-2, C-801, C-804, SEC-97, acceptance 4) A `gcp_service_account` key: shape refusals name the C-801 reason and
/// call nothing; a valid key is verified by one mint and its origin is the key's own project and e-mail; the
/// `provider_account` rule on another project refuses it; a rejected mint is `llm_key_invalid`.
#[tokio::test]
async fn vertex_service_account_keys_are_checked_minted_and_attributed() {
    use aurora_testkit::llm::google_token::Behaviour;
    let w = world().await;
    let save = |ep: &str, value: String, name: &str| {
        json!({"name": name, "kind": "llm",
               "llm": {"endpoint_id": ep, "auth_mode": "gcp_service_account"}, "value": value})
    };
    let url = w.ws_url();
    let post = |body: Value| w.call(Method::POST, &url, &w.owner, Some(body));
    // Shape refusals: nothing is called.
    let json_with = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_str(&w.vkey.json()).unwrap();
        f(&mut v);
        v.to_string()
    };
    for (name, value, reason) in [
        (
            "wrong-uri",
            w.vkey
                .json_with("service_account", "https://evil.example/token"),
            "token_uri_not_google",
        ),
        (
            "not-sa",
            w.vkey
                .json_with("authorized_user", "https://oauth2.googleapis.com/token"),
            "not_service_account",
        ),
        (
            "not-json",
            "just a string".to_owned(),
            "not_service_account",
        ),
        (
            "bad-key",
            json_with(&|v| {
                v["private_key"] =
                    "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----".into()
            }),
            "missing_field",
        ),
        (
            "extra",
            json_with(&|v| v["region"] = "us".into()),
            "unknown_field",
        ),
    ] {
        let (s, v) = post(save("vx", value, name)).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("llm_key_invalid")),
            "{name}: {v}"
        );
        assert_eq!(v["error"]["details"]["reason"], reason, "{name}: {v}");
        no_value(&v, &["BEGIN PRIVATE KEY"]);
    }
    assert_eq!(
        w.google.requests(),
        0,
        "shape refusals make no outbound call"
    );
    // A valid key: one mint, origin from the key.
    let (s, v) = post(save("vx", w.vkey.json(), "good")).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(w.google.requests(), 1);
    assert!(w.google.refused().is_empty());
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["workspace_id"], "key-proj");
    assert_eq!(
        v["origin"]["user_id"],
        "sa@key-proj.iam.gserviceaccount.com"
    );
    assert_eq!(v["llm"]["auth_mode"], "gcp_service_account");
    assert_eq!(v["host_pattern"], "us-east5-aiplatform.googleapis.com");
    no_value(&v, &["BEGIN PRIVATE KEY", "ya29"]);
    // The provider_account rule with another project.
    let (s, v) = post(save("vx_other", w.vkey.json(), "other")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_wrong_organization")),
        "{v}"
    );
    // A rejected mint.
    w.google.set_behaviour(Behaviour::InvalidGrant);
    let (s, v) = post(save("vx", w.vkey.json(), "rejected")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_invalid")),
        "{v}"
    );
    // The api_key mode is not admitted on vertex.
    let (s, v) =
        post(json!({"name": "ak", "kind": "llm", "llm": {"endpoint_id": "vx"}, "value": "sk-x"}))
            .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_invalid")),
        "{v}"
    );
    assert_eq!(v["error"]["details"]["reason"], "auth_mode_unsupported");
}

/// (P4-LLM-3, C-801, C-802, C-804, SEC-97, acceptance 3) Azure OpenAI: a resource key is validated by `GET /openai/v1/models`
/// (`api-key`); an Entra application is shape-checked, minted and its tenant and application become the origin; a
/// refused application is `mint_rejected`; a `provider_account` rule judges the tenant; an `api_key` under such a rule is
/// `llm_key_unverifiable`; the providers listing carries the endpoint's admitted modes.
#[tokio::test]
async fn azure_keys_and_entra_applications_are_validated_and_attributed() {
    use aurora_testkit::llm::entra::Behaviour;
    let w = world().await;
    let url = w.ws_url();
    let save = |ep: &str, mode: &str, value: String, name: &str| {
        json!({"name": name, "kind": "llm",
               "llm": {"endpoint_id": ep, "auth_mode": mode}, "value": value})
    };
    let post = |body: Value| w.call(Method::POST, &url, &w.owner, Some(body));
    // A resource key.
    let (s, v) = post(save("az", "api_key", "azure-key".into(), "k1")).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["method"], "endpoint");
    let seen = w.azure.requests();
    assert_eq!(seen[0].path, "/openai/v1/models");
    assert_eq!(seen[0].header("api-key"), Some("azure-key"));
    assert_eq!(w.entra.requests(), 0);
    let (s, v) = post(save("az", "api_key", "wrong".into(), "k2")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_invalid")),
        "{v}"
    );
    // Entra: shape refusals call nothing.
    let good = w.azure_app.json();
    let with = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_str(&good).unwrap();
        f(&mut v);
        v.to_string()
    };
    for (name, value, reason) in [
        (
            "common",
            with(&|v| v["tenant_id"] = "common".into()),
            "tenant_not_specific",
        ),
        (
            "extra",
            with(&|v| v["authority"] = "https://evil.example".into()),
            "unknown_field",
        ),
        (
            "missing",
            with(&|v| {
                v.as_object_mut().unwrap().remove("client_secret");
            }),
            "missing_field",
        ),
        ("not-json", "just a string".to_owned(), "missing_field"),
    ] {
        let (s, v) = post(save("az", "azure_entra", value, name)).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("llm_key_invalid")),
            "{name}: {v}"
        );
        assert_eq!(v["error"]["details"]["reason"], reason, "{name}: {v}");
        no_value(&v, &[&w.azure_app.client_secret]);
    }
    assert_eq!(
        w.entra.requests(),
        0,
        "shape refusals make no outbound call"
    );
    // A valid application: minted, then models with the token; the tenant and application are the origin.
    let (s, v) = post(save("az_tenant", "azure_entra", good.clone(), "e1")).await;
    assert_eq!(s, 201, "{v}");
    assert!(w.entra.refused().is_empty(), "{:?}", w.entra.refused());
    assert_eq!(v["origin"]["status"], "verified");
    assert_eq!(v["origin"]["method"], "provider_account");
    assert_eq!(v["origin"]["organization_id"], AZURE_TENANT);
    assert_eq!(v["origin"]["user_id"], w.azure_app.client_id);
    assert_eq!(v["llm"]["auth_mode"], "azure_entra");
    no_value(&v, &[&w.azure_app.client_secret]);
    let last = w.azure.requests().pop().unwrap();
    assert!(
        last.header("authorization")
            .is_some_and(|a| a.starts_with("Bearer "))
    );
    assert!(last.header("api-key").is_none());
    // Another tenant in the rule.
    let (s, v) = post(save("az_other", "azure_entra", good.clone(), "e2")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_wrong_organization")),
        "{v}"
    );
    // A resource key cannot report its account.
    let (s, v) = post(save("az_tenant", "api_key", "azure-key".into(), "k3")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_unverifiable")),
        "{v}"
    );
    // A refused application.
    w.entra.set_behaviour(Behaviour::InvalidClient);
    let (s, v) = post(save("az", "azure_entra", good.clone(), "e3")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_key_invalid")),
        "{v}"
    );
    assert_eq!(v["error"]["details"]["reason"], "mint_rejected", "{v}");
    // An unreachable authority is no verdict on the key: saved `pending` without a rule, refused with one (C-801).
    w.entra.set_behaviour(Behaviour::Unavailable);
    let (s, v) = post(save("az", "azure_entra", good.clone(), "e4")).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "pending");
    let (s, v) = post(save("az_tenant", "azure_entra", good, "e5")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("llm_endpoint_unreachable")),
        "{v}"
    );
    // The listing: modes per endpoint (an Entra-only endpoint under `provider_account`).
    let (s, v) = w
        .call(Method::GET, "/v1/llm/providers", &w.member, None)
        .await;
    assert_eq!(s, 200, "{v}");
    let azure = v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "azure_openai")
        .unwrap();
    assert_eq!(azure["auth_modes"], json!(["api_key", "azure_entra"]));
    assert_eq!(
        azure["auth_mode_details"][1],
        json!({"id": "azure_entra", "value_shape": "azure_entra", "recommended": false})
    );
    let mode_of = |ep: &str| -> Value {
        v["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == ep)
            .unwrap()["auth_modes"]
            .clone()
    };
    assert_eq!(mode_of("az"), json!(["api_key", "azure_entra"]));
    assert_eq!(mode_of("az_tenant"), json!(["azure_entra"]));
}

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::config::Config;
use crate::types::{Decision, WorkspaceRef};
use crate::{Client, Error};

fn err(code: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({"error": {"code": code, "message": "m"}}))
}

fn token_ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "api_key": "auk_secret",
        "workspace": {"id":"w1","slug":"main","name":"Main","org_id":"o1","org_slug":"acme"}
    }))
}

#[tokio::test]
async fn device_flow_pending_then_success() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/cli"))
        .and(body_json(json!({"hostname": "box"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code":"dc","user_code":"ABCD-EFGH","verification_url":"http://x/cli",
            "interval":5,"expires_in":600})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/cli/token"))
        .respond_with(err("authorization_pending"))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/cli/token"))
        .respond_with(token_ok())
        .with_priority(2)
        .mount(&s)
        .await;
    let c = Client::new(&s.uri(), None).unwrap();
    let st = c.auth_cli_start(Some("box")).await.unwrap();
    assert_eq!(st.user_code, "ABCD-EFGH");
    let t = c
        .poll_cli_token(
            &st.device_code,
            Duration::from_millis(5),
            Duration::from_millis(5),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    assert_eq!(t.api_key, "auk_secret");
    assert_eq!(t.workspace.org_slug, "acme");
}

#[tokio::test]
async fn device_flow_slow_down_increases_interval() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/cli/token"))
        .respond_with(err("slow_down"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/cli/token"))
        .respond_with(token_ok())
        .with_priority(2)
        .mount(&s)
        .await;
    let c = Client::new(&s.uri(), None).unwrap();
    let t0 = std::time::Instant::now();
    c.poll_cli_token(
        "dc",
        Duration::from_millis(10),
        Duration::from_millis(150),
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    // 10ms + (10+150)ms
    assert!(t0.elapsed() >= Duration::from_millis(170));
}

#[tokio::test]
async fn device_flow_denied_is_terminal() {
    let s = MockServer::start().await;
    Mock::given(path("/v1/auth/cli/token"))
        .respond_with(err("access_denied"))
        .expect(1)
        .mount(&s)
        .await;
    let c = Client::new(&s.uri(), None).unwrap();
    let e = c
        .poll_cli_token(
            "dc",
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("access_denied"));
}

#[tokio::test]
async fn bearer_and_agents_pagination() {
    let s = MockServer::start().await;
    let agent = |h: &str| json!({"id": format!("id-{h}"), "handle": h, "name": h, "kind": "byo", "extra": 1});
    Mock::given(method("GET"))
        .and(path("/v1/workspaces/w1/agents"))
        .and(header("authorization", "Bearer auk_k"))
        .and(query_param("cursor", "c2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items":[agent("b")],"next_cursor":null})),
        )
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/workspaces/w1/agents"))
        .and(header("authorization", "Bearer auk_k"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items":[agent("a")],"next_cursor":"c2"})),
        )
        .with_priority(2)
        .mount(&s)
        .await;
    let c = Client::new(&s.uri(), Some("auk_k".into())).unwrap();
    let agents = c.list_agents("w1").await.unwrap();
    assert_eq!(
        agents.iter().map(|a| a.handle.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[tokio::test]
async fn pending_and_decision() {
    let s = MockServer::start().await;
    let ap = json!({"id":"ap1","org_id":"o","workspace_id":"w","agent":null,"method":"POST","host":"h.com",
        "path":"/x","summary":{"title":"T","fields":[],"text":null,"link":null,"group":null,"source":"generic"},
        "status":"pending","deadline":"2026-11-05T21:58:00.123Z"});
    Mock::given(path("/v1/approvals/pending"))
        .and(query_param("wait", "30"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":[ap.clone()]})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/approvals/ap1/decision"))
        .and(body_json(json!({"decision":"deny"})))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({"error":{"code":"already_decided","message":"done"}})),
        )
        .mount(&s)
        .await;
    let c = Client::new(&s.uri(), Some("auk_k".into())).unwrap();
    let items = c.pending_approvals(99).await.unwrap();
    assert_eq!(items[0].id, "ap1");
    let e = c.decide("ap1", Decision::Deny).await.unwrap_err();
    assert!(matches!(e, Error::Api { status: 409, .. }));
    assert_eq!(e.code(), Some("already_decided"));
}

#[test]
fn config_is_0600_and_roundtrips() {
    let dir = std::env::temp_dir().join(format!("aurora-cfg-{}", uuid::Uuid::now_v7()));
    let p = dir.join("sub").join("config.toml");
    let mut cfg = Config {
        url: "http://x".into(),
        api_key: "auk_1".into(),
        workspace: WorkspaceRef {
            id: "w".into(),
            slug: "s".into(),
            name: "n".into(),
            org_id: "o".into(),
            org_slug: "os".into(),
        },
        agents: Default::default(),
    };
    cfg.save(&p).unwrap();
    cfg.agents.insert(
        "bot".into(),
        crate::config::AgentEntry {
            token: "aug_t".into(),
        },
    );
    cfg.save(&p).unwrap();
    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(Config::load(&p).unwrap(), cfg);
    std::fs::remove_dir_all(&dir).unwrap();
}

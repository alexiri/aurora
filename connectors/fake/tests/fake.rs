//! Fake connector: ops executor, control API, and the SDK loops against a wiremock core.

use std::sync::Arc;
use std::time::Duration;

use aurora_channels::{Block, ButtonItem, ButtonStyle, Card, Content};
use aurora_connector_fake::{FakeConnector, control};
use aurora_connector_sdk::{Connector, RetryPolicy, RuntimeConfig};
use aurora_protocol::connector::*;
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

type R = Result<(), Box<dyn std::error::Error>>;

fn op(kind: &str, payload: Value) -> Operation {
    Operation {
        op_id: Uuid::now_v7(),
        kind: OpKind::from_wire(kind).unwrap_or(OpKind::MessagePost),
        integration_id: None,
        presence_id: None,
        attempt: 1,
        maybe_sent: false,
        payload,
    }
}

fn seed(f: &FakeConnector) {
    let mut p = f.platform.lock().unwrap_or_else(|e| e.into_inner());
    p.upsert_user(crate::user("U1", false));
    p.upsert_user(crate::user("B1", true));
    p.upsert_space(crate::space("C1", "channel", "general"));
}

fn user(r: &str, bot: bool) -> aurora_connector_fake::platform::FakeUser {
    aurora_connector_fake::platform::FakeUser {
        reference: r.into(),
        display_name: r.into(),
        handle: r.to_lowercase(),
        email: Some(format!("{r}@example.com")),
        email_verified: true,
        is_bot: bot,
    }
}

fn space(r: &str, kind: &str, label: &str) -> aurora_connector_fake::platform::FakeSpace {
    aurora_connector_fake::platform::FakeSpace {
        reference: r.into(),
        kind: kind.into(),
        label: label.into(),
        members: Default::default(),
    }
}

fn post(text: &str, marker: &str) -> Value {
    json!({
        "target": {"space_ref": "C1"}, "sender": "shared",
        "content": {"text": text}, "op_marker": marker
    })
}

fn posted(f: &FakeConnector) -> Vec<aurora_connector_fake::platform::PostedMessage> {
    f.platform
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .messages
        .clone()
}

#[tokio::test]
async fn post_defuses_broadcasts_and_threads() -> R {
    let f = FakeConnector::default();
    seed(&f);
    let r = f
        .execute(op(
            "message.post",
            post("hi @channel @all @here <@user:U1> <@here>", "m1"),
        ))
        .await;
    assert_eq!(r.outcome, Outcome::Ok);
    let m = &posted(&f)[0];
    assert!(m.text.contains("@\u{200b}channel"));
    assert!(m.text.contains("@\u{200b}all"));
    assert!(m.text.contains("@u1"));
    // only the neutral token becomes a live broadcast
    assert!(m.text.ends_with(" @here"), "{}", m.text);
    // channel post with threads: a new thread rooted at the message
    assert_eq!(
        r.data.as_ref().map(|d| d["thread_ref"].clone()),
        Some(json!(m.message_ref))
    );
    Ok(())
}

#[tokio::test]
async fn card_degrades_per_declared_capabilities() -> R {
    let card = Card::new(vec![
        Block::text("Approve?"),
        Block::buttons(vec![ButtonItem::new(
            "Approve",
            ButtonStyle::Primary,
            "tok".into(),
        )])?,
    ])?;
    let content = serde_json::to_value(Content::card(card))?;
    let payload = json!({"target": {"space_ref": "C1"}, "sender": "shared", "content": content, "op_marker": "m"});

    let full = FakeConnector::default();
    seed(&full);
    full.execute(op("message.post", payload.clone())).await;
    assert!(
        posted(&full)[0]
            .card
            .as_ref()
            .is_some_and(|c| c.blocks.len() == 2)
    );

    let plain = FakeConnector::new(&["commands"]);
    seed(&plain);
    plain.execute(op("message.post", payload)).await;
    let m = &posted(&plain)[0];
    assert!(
        m.card
            .as_ref()
            .is_none_or(|c| c.blocks.iter().all(|b| !matches!(b, Block::Buttons { .. })))
    );
    // no threads capability -> empty thread ref
    assert_eq!(m.thread_ref, "");
    Ok(())
}

#[tokio::test]
async fn maybe_sent_reclaim_does_not_duplicate() -> R {
    let f = FakeConnector::default();
    seed(&f);
    let first = f
        .execute(op("message.post", post("once", "marker-1")))
        .await;
    let mut again = op("message.post", post("once", "marker-1"));
    again.maybe_sent = true;
    let second = f.execute(again).await;
    assert_eq!(posted(&f).len(), 1);
    assert_eq!(first.data, second.data);
    Ok(())
}

#[tokio::test]
async fn neutral_errors_and_capability_gating() -> R {
    let f = FakeConnector::new(&["threads"]);
    seed(&f);
    let code = |r: &OpResult| r.error.as_ref().map(|e| e.code.clone());
    // reactions not declared
    let r = f
        .execute(op(
            "reaction.add",
            json!({"space_ref":"C1","message_ref":"m","sender":"shared","emoji":"eyes"}),
        ))
        .await;
    assert_eq!(code(&r), Some(ErrorCode::Unsupported));
    // unknown space
    let mut bad = post("x", "m");
    bad["target"] = json!({"space_ref": "nope"});
    assert_eq!(
        code(&f.execute(op("message.post", bad)).await),
        Some(ErrorCode::NotFound)
    );
    // rate limit then recovery
    {
        let mut p = f.platform.lock().unwrap_or_else(|e| e.into_inner());
        p.failures.rate_limit_next = 1;
        p.failures.rate_limit_retry_after_secs = Some(7);
    }
    let r = f.execute(op("message.post", post("x", "a"))).await;
    assert_eq!(r.error.as_ref().and_then(|e| e.retry_after_secs), Some(7));
    assert_eq!(
        f.execute(op("message.post", post("x", "b"))).await.outcome,
        Outcome::Ok
    );
    // dead credentials
    f.platform
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .failures
        .dead_credentials = true;
    assert_eq!(
        code(&f.execute(op("message.post", post("x", "c"))).await),
        Some(ErrorCode::CredentialDead)
    );
    Ok(())
}

struct EchoEvents;
impl Respond for EchoEvents {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let ids: Vec<Value> = body["events"]
            .as_array()
            .map(|a| a.iter().map(|e| e["event_id"].clone()).collect())
            .unwrap_or_default();
        ResponseTemplate::new(200)
            .set_body_json(json!({"accepted": ids, "duplicates": [], "rejected": []}))
    }
}

async fn core(integration: Uuid, claims: Vec<Value>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/register"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "connector_id": "fake", "replica_id": Uuid::from_u128(1), "status": "active",
            "poll": {"config_secs": 1, "ops_wait_secs": 0}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/connector/config"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "etag": "e", "lease_secs": 30, "presences": [], "space_bindings": [],
            "integrations": [{"id": integration, "org_id": Uuid::nil(), "tenant_ref": "t",
                "tenant_label": "T", "credentials": {}, "status": "active"}]})))
        .mount(&server)
        .await;
    for c in claims {
        Mock::given(method("POST"))
            .and(path("/v1/connector/ops/claim"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ops": [c]})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/v1/connector/ops/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ops": []})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/ops/results"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/events"))
        .respond_with(EchoEvents)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/retire"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;
    server
}

fn rt_cfg(url: &str) -> RuntimeConfig {
    let mut c = RuntimeConfig::new(url, "aun_t");
    c.retry = RetryPolicy {
        max_attempts: 3,
        initial: Duration::from_millis(10),
        max: Duration::from_millis(30),
    };
    c.config_interval = Some(Duration::from_millis(50));
    c.ops_wait_secs = Some(0);
    c.op_lease = Duration::from_millis(300);
    c
}

async fn serve_control(f: Arc<FakeConnector>) -> Result<String, Box<dyn std::error::Error>> {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = l.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(l, control::router(f)).await;
    });
    Ok(format!("http://{addr}"))
}

async fn requests(server: &MockServer, p: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == p)
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

#[tokio::test]
async fn control_api_injects_events_through_sdk() -> R {
    let integ = Uuid::from_u128(5);
    let server = core(integ, vec![]).await;
    let f = Arc::new(FakeConnector::default());
    let ctl = serve_control(f.clone()).await?;
    let (stop, rx) = watch::channel(false);
    let conn: Arc<dyn Connector> = f.clone();
    let h = tokio::spawn(aurora_connector_sdk::run(conn, rt_cfg(&server.uri()), rx));
    for _ in 0..100 {
        if !f.open_integrations().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let http = reqwest::Client::new();
    http.post(format!("{ctl}/fake/users"))
        .json(&json!({"ref":"U1","display_name":"Ann","email":"a@x.io","email_verified":true}))
        .send()
        .await?;
    http.post(format!("{ctl}/fake/users"))
        .json(&json!({"ref":"B1","display_name":"Bot","is_bot":true}))
        .send()
        .await?;
    http.post(format!("{ctl}/fake/spaces"))
        .json(&json!({"ref":"C1","kind":"channel","label":"general"}))
        .send()
        .await?;
    let r: Value = http
        .post(format!("{ctl}/fake/messages"))
        .json(&json!({
        "space_ref":"C1","author_ref":"U1","text":"hey <@bot:B1> look"}))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(r["ack"], "accepted");
    let r: Value = http
        .post(format!("{ctl}/fake/bot/added"))
        .json(&json!({"space_ref":"C1","bot_ref":"B1","actor_ref":"U1"}))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(r["ack"], "accepted");

    let evs: Vec<Value> = requests(&server, "/v1/connector/events")
        .await
        .into_iter()
        .flat_map(|b| b["events"].as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(evs[0]["kind"], "message.posted");
    assert_eq!(evs[0]["integration_id"], json!(integ));
    let p = &evs[0]["payload"];
    assert_eq!(p["bot_mentioned"], true);
    assert_eq!(p["addressed_bot_ref"], "B1");
    assert_eq!(p["author"]["email_verified"], true);
    assert_eq!(evs[1]["kind"], "bot.added");

    let listed: Value = http
        .get(format!("{ctl}/fake/posted?space_ref=C1"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(listed["messages"].as_array().map(Vec::len), Some(1));
    stop.send(true)?;
    h.await??;
    Ok(())
}

#[tokio::test]
async fn lost_result_is_reclaimed_without_duplicate_post() -> R {
    let integ = Uuid::from_u128(5);
    let op_id = Uuid::from_u128(77);
    let claim = |maybe| {
        json!({"op_id": op_id, "kind": "message.post", "integration_id": integ,
        "attempt": if maybe {2} else {1}, "maybe_sent": maybe, "payload": post("hello", "marker-77")})
    };
    let server = core(integ, vec![claim(false), claim(true)]).await;
    let f = Arc::new(FakeConnector::default());
    seed(&f);
    f.platform
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .failures
        .lose_results = 1;
    let (stop, rx) = watch::channel(false);
    let conn: Arc<dyn Connector> = f.clone();
    let h = tokio::spawn(aurora_connector_sdk::run(conn, rt_cfg(&server.uri()), rx));
    let mut results = vec![];
    for _ in 0..200 {
        results = requests(&server, "/v1/connector/ops/results").await;
        if !results.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    stop.send(true)?;
    h.await??;
    assert_eq!(results.len(), 1, "only the re-claim reports");
    assert_eq!(results[0]["results"][0]["outcome"], "ok");
    assert_eq!(posted(&f).len(), 1);
    Ok(())
}

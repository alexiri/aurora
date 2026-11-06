//! SDK against a wiremock core.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use aurora_connector_sdk::*;
use aurora_protocol::connector::*;
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type R = Result<(), Box<dyn std::error::Error>>;

struct ProbeConn(Arc<Probe>);

struct Probe {
    opened: Mutex<Vec<Uuid>>,
    sink: Mutex<Option<EventSink>>,
    exec_delay: Duration,
}

impl Probe {
    fn new(exec_delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            opened: Mutex::new(vec![]),
            sink: Mutex::new(None),
            exec_delay,
        })
    }
}

#[async_trait]
impl Connector for ProbeConn {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            package: ManifestPackage {
                id: "probe".into(),
                version: "1".into(),
                contract: 1,
                publisher: "t".into(),
            },
            about: ManifestAbout {
                name: "Probe".into(),
                ..Default::default()
            },
            capabilities: Default::default(),
            integration_setup: Value::Null,
            presence_setup: Value::Null,
            limits: Default::default(),
            public_callbacks: vec![],
        }
    }
    async fn open_integration(
        &self,
        integration: Integration,
        _p: Vec<Presence>,
        sink: EventSink,
    ) -> Result<IntegrationHandle, ConnectorError> {
        if let Ok(mut g) = self.0.opened.lock() {
            g.push(integration.id);
        }
        if let Ok(mut g) = self.0.sink.lock() {
            *g = Some(sink);
        }
        Ok(IntegrationHandle::new(integration.id))
    }
    async fn update_integration(
        &self,
        _h: &IntegrationHandle,
        _i: Integration,
        _p: Vec<Presence>,
    ) -> Result<(), ConnectorError> {
        Ok(())
    }
    async fn execute(&self, op: Operation) -> OpResult {
        tokio::time::sleep(self.0.exec_delay).await;
        OpResult::ok(op.op_id, json!({"message_ref": "m1", "thread_ref": ""}))
    }
}

fn cfg(url: &str) -> RuntimeConfig {
    let mut c = RuntimeConfig::new(url, "aun_test");
    c.retry = RetryPolicy {
        max_attempts: 4,
        initial: Duration::from_millis(10),
        max: Duration::from_millis(40),
    };
    c.config_interval = Some(Duration::from_millis(50));
    c.ops_wait_secs = Some(0);
    c
}

fn register_mock(replica: Uuid) -> Mock {
    Mock::given(method("POST"))
        .and(path("/v1/connector/register"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "connector_id": "probe", "replica_id": replica, "status": "active",
            "poll": {"config_secs": 1, "ops_wait_secs": 0}
        })))
}

async fn fixtures(server: &MockServer, replica: Uuid, integration: Uuid) {
    register_mock(replica).mount(server).await;
    Mock::given(method("GET"))
        .and(path("/v1/connector/config"))
        .and(header_exists("x-aurora-replica"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "etag": "e1", "lease_secs": 30,
            "integrations": [{
                "id": integration, "org_id": Uuid::nil(), "tenant_ref": "t",
                "tenant_label": "T", "credentials": {}, "status": "active"
            }],
            "presences": [], "space_bindings": []
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/retire"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(server)
        .await;
}

async fn wait_for<F: Fn() -> bool>(f: F) -> bool {
    for _ in 0..200 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn bodies(server: &MockServer, p: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == p)
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

fn spawn_run(
    probe: &Arc<Probe>,
    c: RuntimeConfig,
) -> (
    watch::Sender<bool>,
    tokio::task::JoinHandle<Result<(), RuntimeError>>,
) {
    let (tx, rx) = watch::channel(false);
    let conn: Arc<dyn Connector> = Arc::new(ProbeConn(probe.clone()));
    (tx, tokio::spawn(run(conn, c, rx)))
}

#[tokio::test]
async fn register_then_config_opens_integration_once() -> R {
    let server = MockServer::start().await;
    let (replica, integ) = (Uuid::from_u128(1), Uuid::from_u128(2));
    fixtures(&server, replica, integ).await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/ops/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ops": []})))
        .mount(&server)
        .await;
    let probe = Probe::new(Duration::ZERO);
    let (tx, h) = spawn_run(&probe, cfg(&server.uri()));
    assert!(wait_for(|| probe.opened.lock().map(|g| g.len()).unwrap_or(0) >= 1).await);
    tokio::time::sleep(Duration::from_millis(200)).await; // several polls
    tx.send(true)?;
    h.await??;
    assert_eq!(
        *probe.opened.lock().map_err(|e| e.to_string())?,
        vec![integ]
    );
    let reg = bodies(&server, "/v1/connector/register").await;
    assert_eq!(reg[0]["manifest"]["package"]["id"], "probe");
    let hdr = server.received_requests().await.unwrap_or_default();
    let cfgreq = hdr.iter().find(|r| r.url.path() == "/v1/connector/config");
    assert_eq!(
        cfgreq
            .and_then(|r| r.headers.get("x-aurora-replica"))
            .map(|v| v.to_str().unwrap_or("")),
        Some(replica.to_string().as_str())
    );
    assert_eq!(
        cfgreq
            .and_then(|r| r.headers.get("x-aurora-protocol"))
            .map(|v| v.to_str().unwrap_or("")),
        Some("connector/1")
    );
    Ok(())
}

async fn claim_once(server: &MockServer, ops: Value) {
    Mock::given(method("POST"))
        .and(path("/v1/connector/ops/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ops": ops})))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/ops/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ops": []})))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/ops/results"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn claim_execute_result_and_unknown_kind_degrades() -> R {
    let server = MockServer::start().await;
    fixtures(&server, Uuid::from_u128(1), Uuid::from_u128(2)).await;
    let (known, unknown) = (Uuid::from_u128(10), Uuid::from_u128(11));
    claim_once(
        &server,
        json!([
            {"op_id": known, "kind": "message.post", "attempt": 1, "maybe_sent": false, "payload": {}},
            {"op_id": unknown, "kind": "poll.create", "payload": {}}
        ]),
    )
    .await;
    let probe = Probe::new(Duration::ZERO);
    let (tx, h) = spawn_run(&probe, cfg(&server.uri()));
    let mut results = vec![];
    for _ in 0..200 {
        results = bodies(&server, "/v1/connector/ops/results").await;
        if results.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tx.send(true)?;
    h.await??;
    let all: Vec<&Value> = results
        .iter()
        .flat_map(|b| b["results"].as_array().into_iter().flatten())
        .collect();
    let k = all.iter().find(|r| r["op_id"] == json!(known));
    let u = all.iter().find(|r| r["op_id"] == json!(unknown));
    assert_eq!(k.map(|r| r["outcome"].clone()), Some(json!("ok")));
    assert_eq!(
        k.map(|r| r["data"]["message_ref"].clone()),
        Some(json!("m1"))
    );
    assert_eq!(u.map(|r| r["outcome"].clone()), Some(json!("error")));
    assert_eq!(
        u.map(|r| r["error"]["code"].clone()),
        Some(json!("unsupported"))
    );
    Ok(())
}

#[tokio::test]
async fn op_past_its_lease_is_abandoned() -> R {
    let server = MockServer::start().await;
    fixtures(&server, Uuid::from_u128(1), Uuid::from_u128(2)).await;
    claim_once(
        &server,
        json!([{"op_id": Uuid::from_u128(20), "kind": "message.post", "payload": {}}]),
    )
    .await;
    let probe = Probe::new(Duration::from_secs(5));
    let mut c = cfg(&server.uri());
    c.op_lease = Duration::from_millis(100);
    let (tx, h) = spawn_run(&probe, c);
    tokio::time::sleep(Duration::from_millis(600)).await;
    tx.send(true)?;
    h.await??;
    assert!(
        bodies(&server, "/v1/connector/ops/results")
            .await
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn event_post_retried_until_acked() -> R {
    let server = MockServer::start().await;
    fixtures(&server, Uuid::from_u128(1), Uuid::from_u128(2)).await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/ops/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ops": []})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/events"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_json(json!({"code": "unavailable", "message": "x"})),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accepted": ["ev1"], "duplicates": ["ev2"], "rejected": []
        })))
        .mount(&server)
        .await;
    let probe = Probe::new(Duration::ZERO);
    let (tx, h) = spawn_run(&probe, cfg(&server.uri()));
    assert!(wait_for(|| probe.sink.lock().map(|g| g.is_some()).unwrap_or(false)).await);
    let sink = probe
        .sink
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("no sink")?;
    let mk = |id: &str| InboundEvent {
        event_id: id.into(),
        integration_id: Uuid::from_u128(2),
        kind: EventKind::ReactionAdded,
        occurred_at: "t".into(),
        payload: json!({}),
    };
    let (a, b) = tokio::join!(sink.post(mk("ev1")), sink.post(mk("ev2")));
    assert_eq!(a?, EventAck::Accepted);
    assert_eq!(b?, EventAck::Duplicate);
    let posts = bodies(&server, "/v1/connector/events").await;
    assert!(posts.len() >= 3, "retried: {}", posts.len());
    // every retry carries the same event ids (idempotency keys)
    assert!(
        posts
            .iter()
            .all(|p| p["events"][0]["event_id"] == posts[0]["events"][0]["event_id"])
    );
    tx.send(true)?;
    h.await??;
    Ok(())
}

#[tokio::test]
async fn rejected_token_stops_runtime() -> R {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/register"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"code": "unauthorized", "message": "no"})),
        )
        .mount(&server)
        .await;
    let probe = Probe::new(Duration::ZERO);
    let (_tx, h) = spawn_run(&probe, cfg(&server.uri()));
    assert!(matches!(h.await?, Err(RuntimeError::Unauthorized(_))));
    Ok(())
}

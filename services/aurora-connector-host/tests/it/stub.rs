//! A stub Aurora API: the host plane (`/v1/connector-host/*`) and just enough of the connector
//! contract (`/v1/connector/*`) for the SDK runtime of each instance.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aurora_protocol::host::{Assignment, AssignmentsResponse};
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ConnectorRegistration {
    pub authorization: String,
    pub connector: String,
    pub body: Value,
}

#[derive(Default)]
struct Inner {
    registrations: Vec<Value>,
    host_tokens: Vec<String>,
    assignment_queries: Vec<String>,
    polls: usize,
    statuses: Vec<Value>,
    connector_regs: Vec<ConnectorRegistration>,
    retired: BTreeMap<String, usize>,
}

#[derive(Clone)]
struct S {
    inner: Arc<Mutex<Inner>>,
    current: watch::Sender<(i64, Vec<Assignment>)>,
}

pub struct Stub {
    pub url: String,
    s: S,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Deserialize)]
struct Q {
    revision: Option<i64>,
    wait: Option<u64>,
}

async fn register(State(s): State<S>, h: HeaderMap, Json(b): Json<Value>) -> impl IntoResponse {
    let mut g = lock(&s.inner);
    g.registrations.push(b);
    g.host_tokens.push(auth(&h));
    Json(json!({"poll_interval_secs": 1, "rejected": []}))
}

fn auth(h: &HeaderMap) -> String {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn assignments(
    State(s): State<S>,
    h: HeaderMap,
    uri: axum::http::Uri,
    Query(q): Query<Q>,
) -> impl IntoResponse {
    {
        let mut g = lock(&s.inner);
        g.polls += 1;
        g.host_tokens.push(auth(&h));
        g.assignment_queries
            .push(uri.query().unwrap_or_default().to_owned());
    }
    let mut rx = s.current.subscribe();
    let wait = Duration::from_secs(q.wait.unwrap_or(0).min(30));
    let differs = |cur: &(i64, Vec<Assignment>)| Some(cur.0) != q.revision;
    let _ = tokio::time::timeout(wait, rx.wait_for(differs)).await;
    let (revision, connectors) = rx.borrow().clone();
    (
        [("cache-control", "no-store")],
        Json(AssignmentsResponse {
            revision,
            connectors,
        }),
    )
}

async fn status(State(s): State<S>, Json(b): Json<Value>) -> StatusCode {
    lock(&s.inner).statuses.push(b);
    StatusCode::NO_CONTENT
}

async fn c_register(State(s): State<S>, h: HeaderMap, Json(b): Json<Value>) -> impl IntoResponse {
    let connector = h
        .get("aurora-connector")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    lock(&s.inner).connector_regs.push(ConnectorRegistration {
        authorization: auth(&h),
        connector: connector.clone(),
        body: b,
    });
    Json(json!({
        "connector_id": connector, "replica_id": Uuid::new_v4(), "status": "active",
        "poll": {"config_secs": 1, "ops_wait_secs": 0}
    }))
}

async fn c_config() -> impl IntoResponse {
    Json(
        json!({"etag": "e1", "lease_secs": 30, "integrations": [], "presences": [], "space_bindings": []}),
    )
}

async fn c_claim() -> impl IntoResponse {
    tokio::time::sleep(Duration::from_millis(100)).await;
    Json(json!({"ops": []}))
}

async fn c_retire(State(s): State<S>, h: HeaderMap) -> impl IntoResponse {
    let id = h
        .get("aurora-connector")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    *lock(&s.inner).retired.entry(id).or_default() += 1;
    Json(json!({"ok": true}))
}

impl Stub {
    pub async fn start() -> Self {
        let s = S {
            inner: Arc::default(),
            current: watch::channel((1, Vec::new())).0,
        };
        let app = Router::new()
            .route("/v1/connector-host/register", post(register))
            .route("/v1/connector-host/assignments", get(assignments))
            .route("/v1/connector-host/status", post(status))
            .route("/v1/connector/register", post(c_register))
            .route("/v1/connector/config", get(c_config))
            .route("/v1/connector/ops/claim", post(c_claim))
            .route("/v1/connector/retire", post(c_retire))
            .with_state(s.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { url, s }
    }

    /// Replaces the assignments and bumps the revision.
    pub fn set_assignments(&self, list: Vec<Assignment>) {
        self.s.current.send_modify(|c| {
            c.0 += 1;
            c.1 = list;
        });
    }

    pub fn registrations(&self) -> Vec<Value> {
        lock(&self.s.inner).registrations.clone()
    }

    pub fn host_tokens(&self) -> Vec<String> {
        lock(&self.s.inner).host_tokens.clone()
    }

    pub fn assignment_polls(&self) -> usize {
        lock(&self.s.inner).polls
    }

    pub fn assignment_queries(&self) -> Vec<String> {
        lock(&self.s.inner).assignment_queries.clone()
    }

    pub fn connector_registrations(&self, id: &str) -> Vec<ConnectorRegistration> {
        lock(&self.s.inner)
            .connector_regs
            .iter()
            .filter(|r| r.connector == id)
            .cloned()
            .collect()
    }

    pub fn retired(&self, id: &str) -> usize {
        lock(&self.s.inner).retired.get(id).copied().unwrap_or(0)
    }

    pub fn status_bodies(&self) -> Vec<Value> {
        lock(&self.s.inner).statuses.clone()
    }

    fn last_report(&self, id: &str) -> Option<Value> {
        lock(&self.s.inner)
            .statuses
            .iter()
            .rev()
            .flat_map(|b| b["connectors"].as_array().cloned().unwrap_or_default())
            .find(|c| c["id"] == id)
    }

    pub fn last_state(&self, id: &str) -> Option<String> {
        self.last_report(id)
            .and_then(|c| c["state"].as_str().map(str::to_owned))
    }

    pub fn last_message(&self, id: &str) -> Option<String> {
        self.last_report(id)
            .and_then(|c| c["message"].as_str().map(str::to_owned))
    }
}

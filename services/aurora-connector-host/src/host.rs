//! The host loop: register (heartbeat), long-poll assignments, and keep exactly one connector
//! instance running per assigned managed connector (docs/13 §3.1).
//!
//! * Only types compiled into this host (the table it was constructed with) ever run. An
//!   assignment naming anything else is reported as an error and nothing is started (SEC-69).
//! * An instance is restarted when its type, display name, settings or secrets change, or when
//!   its runtime has stopped; it is stopped when its assignment disappears (disabled or deleted).
//! * Assignment `secrets` are never logged. Status messages are sanitized and have every secret
//!   value of the assignment scrubbed out of them (C-457, SEC-70).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use aurora_connector_sdk::{RuntimeConfig, run};
use aurora_protocol::host::{
    Assignment, ConnectorReport, HostRegisterRequest, HostStatusRequest, HostType, InstanceState,
    MAX_ASSIGNMENTS_WAIT_SECS, MAX_STATUS_MESSAGE_CHARS,
};
use axum::Router;
use axum::body::Body;
use axum::extract::State as AxState;
use axum::http::{Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tower::ServiceExt;
use uuid::Uuid;

use crate::api::{ApiError, HostApi};
use crate::catalogue::CompiledType;

/// Heartbeat interval until `register` advertises one.
const DEFAULT_POLL: Duration = Duration::from_secs(30);
/// Pause after a failed call before the next attempt.
const RETRY_PAUSE: Duration = Duration::from_secs(2);
const MAX_RETRY_PAUSE: Duration = Duration::from_secs(30);

/// Adjusts the SDK runtime configuration of each instance.
pub type RuntimeTweak = Arc<dyn Fn(&mut RuntimeConfig) + Send + Sync>;

#[derive(Clone)]
pub struct HostConfig {
    /// API origin, e.g. `http://aurora-api:4101`.
    pub api_url: String,
    /// The host's `auh_…` anchor token. Never logged.
    pub token: String,
    pub version: String,
    /// Long-poll `wait` (0–30 s).
    pub assignments_wait_secs: u32,
    /// How long an instance gets to stop before it is aborted.
    pub stop_timeout: Duration,
    /// Tunes each instance's SDK runtime (tests shorten the polling).
    pub runtime_tweak: Option<RuntimeTweak>,
}

impl HostConfig {
    pub fn new(api_url: &str, token: &str) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            assignments_wait_secs: 25,
            stop_timeout: Duration::from_secs(10),
            runtime_tweak: None,
        }
    }
}

impl fmt::Debug for HostConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostConfig")
            .field("api_url", &self.api_url)
            .field("token", &"[redacted]")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What an instance is doing, as reported to the core.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Running,
    Error(String),
}

/// What decides whether a running instance must be restarted.
#[derive(Clone, PartialEq, Eq)]
struct Key {
    type_id: String,
    display_name: String,
    settings: Value,
    secrets: BTreeMap<String, String>,
}

impl Key {
    fn of(a: &Assignment) -> Self {
        Self {
            type_id: a.type_id.clone(),
            display_name: a.display_name.clone(),
            settings: a.settings.clone(),
            secrets: a.secrets.clone(),
        }
    }
}

struct Instance {
    key: Key,
    state: Arc<Mutex<State>>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl Instance {
    fn failed(a: &Assignment, msg: &str) -> Self {
        Self {
            key: Key::of(a),
            state: Arc::new(Mutex::new(State::Error(sanitize(msg, &a.secrets)))),
            stop: watch::channel(false).0,
            task: None,
        }
    }

    fn finished(&self) -> bool {
        self.task.as_ref().is_some_and(JoinHandle::is_finished)
    }
}

/// Control characters become spaces, every secret value is scrubbed, and the message is cut to
/// the contract's 300 characters.
pub fn sanitize(msg: &str, secrets: &BTreeMap<String, String>) -> String {
    let mut out = msg.to_owned();
    for s in secrets.values().filter(|s| !s.is_empty()) {
        out = out.replace(s.as_str(), "[redacted]");
    }
    let flat: String = out
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.trim().chars().take(MAX_STATUS_MESSAGE_CHARS).collect()
}

/// Which table of per-instance routers a dispatcher serves.
#[derive(Clone, Copy)]
enum Plane {
    Callbacks,
    Test,
}

#[derive(Default)]
struct InstanceRoutes {
    callbacks: Option<Router>,
    test: Option<Router>,
}

type Routes = Arc<RwLock<HashMap<String, InstanceRoutes>>>;

#[derive(Clone)]
struct Dispatch {
    routes: Routes,
    plane: Plane,
    prefix: &'static str,
}

/// Forwards `/<prefix>/<id>/<rest>` to the instance `id` as `/<rest>`.
async fn dispatch(AxState(d): AxState<Dispatch>, req: Request<Body>) -> Response {
    let path = req.uri().path().to_owned();
    let Some(tail) = path.strip_prefix(d.prefix) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (id, rest) = tail.split_once('/').unwrap_or((tail, ""));
    let router = {
        let g = d.routes.read().unwrap_or_else(|e| e.into_inner());
        g.get(id).and_then(|r| match d.plane {
            Plane::Callbacks => r.callbacks.clone(),
            Plane::Test => r.test.clone(),
        })
    };
    let Some(router) = router else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let pq = match req.uri().query() {
        Some(q) => format!("/{rest}?{q}"),
        None => format!("/{rest}"),
    };
    let Ok(uri) = pq.parse::<Uri>() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let (mut parts, body) = req.into_parts();
    parts.uri = uri;
    match router.oneshot(Request::from_parts(parts, body)).await {
        Ok(r) => r,
        Err(never) => match never {},
    }
}

pub struct Host {
    cfg: HostConfig,
    api: HostApi,
    types: Vec<CompiledType>,
    instance_id: Uuid,
    routes: Routes,
    ready: Arc<AtomicBool>,
}

impl Host {
    pub fn new(cfg: HostConfig, types: Vec<CompiledType>) -> Result<Self, ApiError> {
        let api = HostApi::new(&cfg.api_url, &cfg.token)?;
        Ok(Self {
            cfg,
            api,
            types,
            instance_id: Uuid::new_v4(),
            routes: Routes::default(),
            ready: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The `instance_id` this host sends with `register`, `assignments` and `status`.
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    /// True once `register` has succeeded.
    pub fn ready(&self) -> Arc<AtomicBool> {
        self.ready.clone()
    }

    /// Public callbacks: `/connectors/<id>/…` goes to instance `<id>` as `/…`.
    pub fn callback_router(&self) -> Router {
        Router::new().fallback(dispatch).with_state(Dispatch {
            routes: self.routes.clone(),
            plane: Plane::Callbacks,
            prefix: "/connectors/",
        })
    }

    /// Test control APIs: `/test/<id>/…` (only instances that have one). Mount it only when
    /// `test_types` is on.
    pub fn test_router(&self) -> Router {
        Router::new().fallback(dispatch).with_state(Dispatch {
            routes: self.routes.clone(),
            plane: Plane::Test,
            prefix: "/test/",
        })
    }

    fn catalogue(&self) -> HostRegisterRequest {
        HostRegisterRequest {
            instance_id: self.instance_id,
            version: self.cfg.version.clone(),
            types: self
                .types
                .iter()
                .map(|t| HostType {
                    type_id: t.type_id.clone(),
                    manifest: t.manifest.clone(),
                    manifest_signature: t.signature.clone(),
                })
                .collect(),
        }
    }

    /// Announces the catalogue. Returns the heartbeat interval.
    async fn register(&self) -> Result<Duration, ApiError> {
        let r = self.api.register(&self.catalogue()).await?;
        for rej in &r.rejected {
            tracing::warn!(type_id = %rej.type_id, reason = ?rej.reason, "the core rejected a connector type");
        }
        self.ready.store(true, Ordering::Relaxed);
        Ok(Duration::from_secs(u64::from(r.poll_interval_secs.max(1))))
    }

    /// Runs until `shutdown` turns true, then stops every instance.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        let mut pause = RETRY_PAUSE;
        let poll = loop {
            match self.register().await {
                Ok(p) => break p,
                Err(e) => {
                    tracing::warn!(error = %e, "register failed; retrying");
                    if sleep_or_stop(pause, &mut shutdown).await {
                        return;
                    }
                    pause = (pause * 2).min(MAX_RETRY_PAUSE);
                }
            }
        };
        tracing::info!(instance_id = %self.instance_id, types = self.types.len(), "registered");
        let mut poll = if poll.is_zero() { DEFAULT_POLL } else { poll };
        let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + poll, poll);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut instances: BTreeMap<String, Instance> = BTreeMap::new();
        let mut desired: Vec<Assignment> = Vec::new();
        let mut revision: Option<i64> = None;
        let mut pause = RETRY_PAUSE;
        let wait = self
            .cfg
            .assignments_wait_secs
            .min(MAX_ASSIGNMENTS_WAIT_SECS);
        loop {
            let event = {
                let fetch = self.api.assignments(revision, wait, self.instance_id);
                tokio::select! {
                    _ = shutdown.wait_for(|s| *s) => Event::Stop,
                    _ = heartbeat.tick() => Event::Heartbeat,
                    r = fetch => Event::Polled(r),
                }
            };
            match event {
                Event::Stop => break,
                Event::Heartbeat => {
                    match self.register().await {
                        Ok(p) if p != poll => {
                            poll = p;
                            heartbeat =
                                tokio::time::interval_at(tokio::time::Instant::now() + p, p);
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "heartbeat failed"),
                    }
                    // Restarts instances whose runtime stopped on its own.
                    self.reconcile(&mut instances, &desired).await;
                    self.report(&instances).await;
                }
                Event::Polled(r) => match r {
                    Ok(resp) => {
                        pause = RETRY_PAUSE;
                        revision = Some(resp.revision);
                        desired = resp.connectors;
                        self.reconcile(&mut instances, &desired).await;
                        self.report(&instances).await;
                    }
                    Err(e) => {
                        // The running set is kept: a failing store or API must not stop connectors.
                        if e.is_auth() {
                            tracing::error!(error = %e, "the API refused the connector host token");
                        } else {
                            tracing::warn!(error = %e, "assignments poll failed");
                        }
                        if sleep_or_stop(pause, &mut shutdown).await {
                            break;
                        }
                        pause = (pause * 2).min(MAX_RETRY_PAUSE);
                    }
                },
            }
        }
        for (id, inst) in std::mem::take(&mut instances) {
            self.stop(&id, inst).await;
        }
        self.ready.store(false, Ordering::Relaxed);
        tracing::info!("connector host stopped");
    }

    async fn reconcile(&self, instances: &mut BTreeMap<String, Instance>, desired: &[Assignment]) {
        let gone: Vec<String> = instances
            .keys()
            .filter(|id| !desired.iter().any(|a| &a.id == *id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(inst) = instances.remove(&id) {
                tracing::info!(connector = %id, "stopping connector");
                self.stop(&id, inst).await;
            }
        }
        for a in desired {
            if let Some(cur) = instances.get(&a.id)
                && cur.key == Key::of(a)
                && !cur.finished()
            {
                continue;
            }
            if let Some(old) = instances.remove(&a.id) {
                tracing::info!(connector = %a.id, "restarting connector");
                self.stop(&a.id, old).await;
            }
            let inst = self.start(a);
            instances.insert(a.id.clone(), inst);
        }
        let running = instances
            .values()
            .filter(|i| *lock(&i.state) == State::Running)
            .count();
        metrics::gauge!("aurora_connector_host_instances", "state" => "running")
            .set(running as f64);
        metrics::gauge!("aurora_connector_host_instances", "state" => "error")
            .set((instances.len() - running) as f64);
    }

    /// Starts one instance. Only a type of this host's own table is ever built (SEC-69).
    fn start(&self, a: &Assignment) -> Instance {
        let Some(t) = self.types.iter().find(|t| t.type_id == a.type_id) else {
            tracing::warn!(connector = %a.id, "assignment names a type this host does not run");
            return Instance::failed(a, "this connector type is not available in this host");
        };
        let built = match (t.build)(a, &self.cfg.api_url) {
            Ok(b) => b,
            Err(msg) => {
                let inst = Instance::failed(a, &msg);
                if let State::Error(clean) = &*lock(&inst.state) {
                    tracing::warn!(connector = %a.id, "connector settings refused: {clean}");
                }
                return inst;
            }
        };
        self.routes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                a.id.clone(),
                InstanceRoutes {
                    callbacks: built.callbacks,
                    test: built.test_control,
                },
            );
        let mut rt = RuntimeConfig::managed(&self.cfg.api_url, &self.cfg.token, &a.id);
        rt.manifest_signature = t.signature.clone();
        rt.version = self.cfg.version.clone();
        if let Some(tweak) = &self.cfg.runtime_tweak {
            tweak(&mut rt);
        }
        let (stop, stop_rx) = watch::channel(false);
        let state = Arc::new(Mutex::new(State::Running));
        let task = {
            let (state, id, secrets) = (state.clone(), a.id.clone(), a.secrets.clone());
            let connector = built.connector;
            tokio::spawn(async move {
                match run(connector, rt, stop_rx).await {
                    Ok(()) => tracing::info!(connector = %id, "connector stopped"),
                    Err(e) => {
                        tracing::error!(connector = %id, error = %sanitize(&e.to_string(), &secrets), "connector runtime stopped");
                        *lock(&state) = State::Error(sanitize(&e.to_string(), &secrets));
                    }
                }
            })
        };
        tracing::info!(connector = %a.id, type_id = %a.type_id, "connector started");
        Instance {
            key: Key::of(a),
            state,
            stop,
            task: Some(task),
        }
    }

    async fn stop(&self, id: &str, inst: Instance) {
        self.routes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        let _ = inst.stop.send(true);
        if let Some(mut task) = inst.task
            && tokio::time::timeout(self.cfg.stop_timeout, &mut task)
                .await
                .is_err()
        {
            tracing::warn!(connector = %id, "connector did not stop in time; aborting");
            task.abort();
        }
    }

    async fn report(&self, instances: &BTreeMap<String, Instance>) {
        let connectors: Vec<ConnectorReport> = instances
            .iter()
            .map(|(id, i)| {
                let (state, message) = match lock(&i.state).clone() {
                    State::Running => (InstanceState::Running, None),
                    State::Error(m) => (InstanceState::Error, Some(m)),
                };
                ConnectorReport {
                    id: id.clone(),
                    state,
                    message,
                }
            })
            .collect();
        let req = HostStatusRequest {
            instance_id: self.instance_id,
            connectors,
        };
        if let Err(e) = self.api.status(&req).await {
            tracing::warn!(error = %e, "status report failed");
        }
    }
}

enum Event {
    Stop,
    Heartbeat,
    Polled(Result<aurora_protocol::host::AssignmentsResponse, ApiError>),
}

async fn sleep_or_stop(d: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => false,
        _ = shutdown.wait_for(|s| *s) => true,
    }
}

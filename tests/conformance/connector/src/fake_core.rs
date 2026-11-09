//! A fake core: an in-memory implementation of the connector plane
//! (`/v1/connector/*`, docs/13 §4) for driving any connector.
//!
//! The fake core is strict. Every request body is parsed and checked with the
//! protocol types' [`Validate`]; auth, protocol header and replica header are
//! checked on every call; the replica-scoped file rules of §4 are enforced.
//! Anything non-conformant is answered with an error *and* recorded in
//! [`FakeCore::violations`], which each scenario asserts to be empty.

// Handlers early-return ready-made axum responses as the `Err` side; boxing would only add noise
// to test-support code.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use aurora_protocol::common::Validate;
use aurora_protocol::connector::*;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};
use uuid::Uuid;

/// A file the core stores (inbound upload, or outbound attachment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub attachment_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// The `file.fetch` op this arrived for (uploads only).
    pub op_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpState {
    Queued,
    Claimed,
    Done,
}

#[derive(Debug, Clone)]
struct OpRecord {
    op: Operation,
    state: OpState,
    /// Every claim handed to the connector, in order.
    claims: Vec<Operation>,
    /// The last claim lapsed (`expire_lease`): the next one carries `maybe_sent`.
    lapsed: bool,
    result: Option<OpResult>,
}

struct State_ {
    token: String,
    status: ConnectorStatus,
    replica: Option<Uuid>,
    registrations: Vec<RegisterRequest>,
    version: u64,
    integrations: Vec<Integration>,
    presences: Vec<Presence>,
    bindings: Vec<Binding>,
    events: Vec<InboundEvent>,
    event_ids: HashSet<String>,
    duplicates: Vec<String>,
    event_posts: u32,
    ops: Vec<OpRecord>,
    late_results: Vec<OpResult>,
    credentials: HashMap<String, (u64, Value)>,
    credential_writes: Vec<CredentialsRequest>,
    link_starts: Vec<LinkStartRequest>,
    uploads: Vec<StoredFile>,
    outbound: BTreeMap<Uuid, StoredFile>,
    fail_events: u32,
    violations: Vec<String>,
    retired: bool,
    retire_calls: u32,
    /// `(alias, real)` pairs: platforms whose ids the suite cannot choose (see `set_alias`).
    aliases: Vec<(String, String)>,
}

struct Inner {
    st: Mutex<State_>,
    notify: Notify,
    events_gate: watch::Sender<bool>,
}

fn lock(m: &Mutex<State_>) -> MutexGuard<'_, State_> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The running fake core. Dropping it stops the server.
pub struct FakeCore {
    inner: Arc<Inner>,
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for FakeCore {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Polls `f` until it returns `Some` or `timeout` elapses.
pub async fn eventually<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= end {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

impl FakeCore {
    /// Starts on an ephemeral loopback port. `token` is the anchor token (`aun_…`).
    pub async fn start(token: &str) -> anyhow::Result<Self> {
        let (events_gate, _) = watch::channel(true);
        let inner = Arc::new(Inner {
            st: Mutex::new(State_ {
                token: token.to_owned(),
                status: ConnectorStatus::AwaitingRegistration,
                replica: None,
                registrations: vec![],
                version: 1,
                integrations: vec![],
                presences: vec![],
                bindings: vec![],
                events: vec![],
                event_ids: HashSet::new(),
                duplicates: vec![],
                event_posts: 0,
                ops: vec![],
                late_results: vec![],
                credentials: HashMap::new(),
                credential_writes: vec![],
                link_starts: vec![],
                uploads: vec![],
                outbound: BTreeMap::new(),
                fail_events: 0,
                violations: vec![],
                retired: false,
                retire_calls: 0,
                aliases: vec![],
            }),
            notify: Notify::new(),
            events_gate,
        });
        let app = Router::new()
            .route("/v1/connector/register", post(register))
            .route("/v1/connector/config", get(config))
            .route("/v1/connector/events", post(events))
            .route("/v1/connector/ops/claim", post(claim))
            .route("/v1/connector/ops/results", post(results))
            .route("/v1/connector/credentials", post(credentials))
            .route("/v1/connector/link-start", post(link_start))
            .route(
                "/v1/connector/files/{id}",
                put(put_file)
                    .get(get_file)
                    .layer(DefaultBodyLimit::disable()),
            )
            .route("/v1/connector/retire", post(retire))
            .route("/test-assets/avatar.png", get(avatar_png))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            inner,
            addr,
            server,
        })
    }

    /// The API origin to hand the connector (`AURORA_CORE_URL`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn token(&self) -> String {
        lock(&self.inner.st).token.clone()
    }

    fn st(&self) -> MutexGuard<'_, State_> {
        lock(&self.inner.st)
    }

    // ------------------------------------------------------------ setup

    /// Adds an integration (served to the connector once it is approved) with a
    /// shared bot `bot_ref`. Returns its id.
    pub fn add_integration(&self, bot_ref: &str) -> Uuid {
        let id = Uuid::now_v7();
        let mut st = self.st();
        let n = st.integrations.len() + 1;
        st.integrations.push(Integration {
            id,
            org_id: Uuid::now_v7(),
            tenant_ref: format!("tenant-{n}"),
            tenant_label: "Conformance Tenant".into(),
            config: json!({}),
            credentials: json!({"token": "platform-token"}),
            credentials_version: 1,
            shared_bot: Some(SharedBot {
                platform_user_ref: bot_ref.to_owned(),
                handle: "daneel".into(),
                display_name: "Daneel".into(),
            }),
            status: "active".into(),
        });
        st.credentials.insert(
            format!("integration:{id}"),
            (1, json!({"token": "platform-token"})),
        );
        st.version += 1;
        id
    }

    /// Adds a fully specified integration (a platform whose setup needs real
    /// `config`/`credentials`/`tenant_ref`). Returns its id.
    pub fn add_integration_with(&self, integration: Integration) -> Uuid {
        let id = integration.id;
        let mut st = self.st();
        st.credentials.insert(
            format!("integration:{id}"),
            (
                integration.credentials_version,
                integration.credentials.clone(),
            ),
        );
        st.integrations.push(integration);
        st.version += 1;
        id
    }

    /// Adds a presence to what `GET /config` serves (a dedicated bot the core knows).
    pub fn add_presence(&self, presence: Presence) {
        let mut st = self.st();
        st.presences.retain(|p| p.id != presence.id);
        st.presences.push(presence);
        st.version += 1;
    }

    /// Stops serving an integration and its presences, as if their rows were deleted.
    pub fn remove_integration(&self, id: Uuid) {
        let mut st = self.st();
        st.integrations.retain(|i| i.id != id);
        st.presences.retain(|p| p.integration_id != id);
        st.version += 1;
    }

    /// Queues a self-contained `presence.deprovision` (RFE-024, C-360/C-361) as the core
    /// does after a delete: no integration id, everything the connector needs in the payload
    /// and the integration's credentials in `credentials`. Built from the served integration
    /// `integration`, so call it before [`Self::remove_integration`].
    pub fn enqueue_deprovision(
        &self,
        integration: Uuid,
        presence: Uuid,
        reason: &str,
        platform_user_ref: &str,
        handle: Option<&str>,
    ) -> Uuid {
        self.deprovision_op(
            integration,
            presence,
            reason,
            platform_user_ref,
            handle,
            true,
        )
    }

    /// As [`Self::enqueue_deprovision`] but the op arrives without `credentials` (the core
    /// never does this; a connector must refuse it).
    pub fn enqueue_deprovision_without_credentials(
        &self,
        integration: Uuid,
        presence: Uuid,
        reason: &str,
        platform_user_ref: &str,
    ) -> Uuid {
        self.deprovision_op(
            integration,
            presence,
            reason,
            platform_user_ref,
            None,
            false,
        )
    }

    fn deprovision_op(
        &self,
        integration: Uuid,
        presence: Uuid,
        reason: &str,
        platform_user_ref: &str,
        handle: Option<&str>,
        with_credentials: bool,
    ) -> Uuid {
        let (payload, credentials) = {
            let st = self.st();
            // An integration that is not served yields an op the connector must refuse.
            let i = st.integrations.iter().find(|i| i.id == integration);
            (
                json!({
                    "presence_id": presence, "reason": reason,
                    "platform_user_ref": platform_user_ref, "handle": handle,
                    "tenant_ref": i.map(|i| i.tenant_ref.clone()).unwrap_or_default(),
                    "tenant_label": i.map(|i| i.tenant_label.clone()).unwrap_or_default(),
                    "config": i.map(|i| i.config.clone()).unwrap_or_else(|| json!({})),
                }),
                i.map(|i| i.credentials.clone())
                    .unwrap_or_else(|| json!({})),
            )
        };
        let mut payload = payload;
        let aliases = self.st().aliases.clone();
        translate(&mut payload, &aliases, false);
        let id = Uuid::now_v7();
        self.st().ops.push(OpRecord {
            op: Operation {
                op_id: id,
                kind: OpKind::PresenceDeprovision,
                integration_id: None,
                presence_id: Some(presence),
                attempt: 1,
                maybe_sent: false,
                payload,
                credentials: with_credentials.then_some(credentials),
            },
            state: OpState::Queued,
            claims: vec![],
            lapsed: false,
            result: None,
        });
        self.inner.notify.notify_waiters();
        id
    }

    /// Registers `alias` for the platform id `real`, for platforms whose ids the
    /// suite cannot choose. Scenarios talk in aliases (`U1`, `C1`, `B1`): they are
    /// translated to real ids in operations the scenario enqueues, and back to
    /// aliases in the events, results and duplicates the scenario reads. An alias
    /// matches a whole string value, or the id part of a `<@user:…>`, `<@bot:…>`,
    /// `<#space:…>` token.
    pub fn set_alias(&self, alias: &str, real: &str) {
        let mut st = self.st();
        st.aliases.retain(|(a, _)| a != alias);
        st.aliases.push((alias.to_owned(), real.to_owned()));
    }

    /// A cheap read-only view for platform drivers that need to see what the
    /// connector delivered to the core (e.g. to answer "did the core accept it").
    pub fn probe(&self) -> CoreProbe {
        CoreProbe {
            inner: self.inner.clone(),
        }
    }

    /// An instance admin approves the connector; its integrations are served.
    pub fn approve(&self) {
        let mut st = self.st();
        st.status = ConnectorStatus::Active;
        st.version += 1;
    }

    pub fn status(&self) -> ConnectorStatus {
        self.st().status.clone()
    }

    pub fn replica_id(&self) -> Option<Uuid> {
        self.st().replica
    }

    /// Every `register` request received, oldest first.
    pub fn registrations(&self) -> Vec<RegisterRequest> {
        self.st().registrations.clone()
    }

    pub fn retired(&self) -> bool {
        self.st().retired
    }

    /// Protocol violations observed from the connector (must stay empty).
    pub fn violations(&self) -> Vec<String> {
        self.st().violations.clone()
    }

    // ------------------------------------------------------------- ops

    /// Queues an operation for the connector; returns its id (= the `op_marker`
    /// a `message.post` should carry).
    pub fn enqueue(&self, kind: &str, integration: Option<Uuid>, payload: Value) -> Uuid {
        self.enqueue_with_id(Uuid::now_v7(), kind, integration, payload)
    }

    /// As [`Self::enqueue`] for an operation about a presence (`presence_id` set).
    pub fn enqueue_for_presence(
        &self,
        kind: &str,
        integration: Option<Uuid>,
        presence: Uuid,
        payload: Value,
    ) -> Uuid {
        let id = Uuid::now_v7();
        self.enqueue_with_id(id, kind, integration, payload);
        if let Some(r) = self.st().ops.iter_mut().find(|r| r.op.op_id == id) {
            r.op.presence_id = Some(presence);
        }
        id
    }

    /// As [`Self::enqueue`] with a caller-chosen op id.
    pub fn enqueue_with_id(
        &self,
        op_id: Uuid,
        kind: &str,
        integration: Option<Uuid>,
        mut payload: Value,
    ) -> Uuid {
        let aliases = self.st().aliases.clone();
        translate(&mut payload, &aliases, false);
        let op = Operation {
            op_id,
            kind: OpKind::from_wire(kind).unwrap_or(OpKind::MessagePost),
            integration_id: integration,
            presence_id: None,
            attempt: 1,
            maybe_sent: false,
            payload,
            credentials: None,
        };
        self.st().ops.push(OpRecord {
            op,
            state: OpState::Queued,
            claims: vec![],
            lapsed: false,
            result: None,
        });
        self.inner.notify.notify_waiters();
        op_id
    }

    /// Waits for the result of `op_id`.
    pub async fn wait_result(&self, op_id: Uuid, timeout: Duration) -> Option<OpResult> {
        eventually(timeout, || {
            let st = self.st();
            let found = st
                .ops
                .iter()
                .find(|o| o.op.op_id == op_id)
                .and_then(|o| o.result.clone());
            found.map(|r| alias_result(&st.aliases, r))
        })
        .await
    }

    /// Waits until `op_id` has been claimed `n` times (lease history).
    pub async fn wait_claims(
        &self,
        op_id: Uuid,
        n: usize,
        timeout: Duration,
    ) -> Option<Vec<Operation>> {
        eventually(timeout, || {
            self.st()
                .ops
                .iter()
                .find(|o| o.op.op_id == op_id)
                .filter(|o| o.claims.len() >= n)
                .map(|o| o.claims.clone())
        })
        .await
    }

    /// Lets the claim lease of `op_id` lapse now: the op becomes claimable
    /// again and its next claim carries `maybe_sent = true`. Returns false when
    /// the op is not currently claimed.
    pub fn expire_lease(&self, op_id: Uuid) -> bool {
        let mut st = self.st();
        let Some(o) = st.ops.iter_mut().find(|o| o.op.op_id == op_id) else {
            return false;
        };
        if o.state != OpState::Claimed {
            return false;
        }
        o.state = OpState::Queued;
        o.lapsed = true;
        drop(st);
        self.inner.notify.notify_waiters();
        true
    }

    /// Results accepted so far, in arrival order.
    pub fn results(&self) -> Vec<OpResult> {
        let st = self.st();
        let mut r: Vec<OpResult> = st.ops.iter().filter_map(|o| o.result.clone()).collect();
        r.sort_by_key(|x| st.ops.iter().position(|o| o.op.op_id == x.op_id));
        let aliases = st.aliases.clone();
        drop(st);
        r.into_iter().map(|x| alias_result(&aliases, x)).collect()
    }

    /// Results reported for ops that were not claimed at the time (ignored).
    pub fn late_results(&self) -> Vec<OpResult> {
        let st = self.st();
        let aliases = st.aliases.clone();
        let r = st.late_results.clone();
        drop(st);
        r.into_iter().map(|x| alias_result(&aliases, x)).collect()
    }

    // ---------------------------------------------------------- events

    /// Events accepted into the inbox, in order.
    pub fn events(&self) -> Vec<InboundEvent> {
        let st = self.st();
        aliased_events(&st.aliases, &st.events)
    }

    /// `event_id`s answered as duplicates.
    pub fn duplicates(&self) -> Vec<String> {
        let st = self.st();
        aliased_ids(&st.aliases, &st.duplicates)
    }

    /// How many `POST /events` requests arrived.
    pub fn event_posts(&self) -> u32 {
        self.st().event_posts
    }

    /// Waits for an accepted event matching `pred`.
    pub async fn wait_event(
        &self,
        timeout: Duration,
        pred: impl Fn(&InboundEvent) -> bool,
    ) -> Option<InboundEvent> {
        eventually(timeout, || {
            let st = self.st();
            aliased_events(&st.aliases, &st.events)
                .into_iter()
                .find(|e| pred(e))
        })
        .await
    }

    /// While held, `POST /events` blocks before answering (the core has not yet
    /// persisted the batch, so the connector must not ack the platform).
    pub fn hold_events(&self, hold: bool) {
        self.inner.events_gate.send_replace(!hold);
    }

    /// The next `n` `POST /events` requests answer 503 without persisting.
    pub fn fail_next_event_posts(&self, n: u32) {
        self.st().fail_events = n;
    }

    // ----------------------------------------------------------- files

    /// Stores an outbound attachment (what a `message.post` op will reference).
    pub fn add_attachment(&self, filename: &str, content_type: &str, bytes: &[u8]) -> Uuid {
        let id = Uuid::now_v7();
        self.st().outbound.insert(
            id,
            StoredFile {
                attachment_id: id,
                filename: filename.to_owned(),
                content_type: content_type.to_owned(),
                bytes: bytes.to_vec(),
                op_id: None,
            },
        );
        id
    }

    /// Files uploaded with `PUT /files/{op_id}`.
    pub fn uploads(&self) -> Vec<StoredFile> {
        self.st().uploads.clone()
    }

    // ------------------------------------------------------ link-start

    pub fn link_starts(&self) -> Vec<LinkStartRequest> {
        self.st().link_starts.clone()
    }

    pub fn credential_writes(&self) -> Vec<CredentialsRequest> {
        self.st().credential_writes.clone()
    }
}

// ------------------------------------------------------------------ http

type S = State<Arc<Inner>>;

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"code": code, "message": message}))).into_response()
}

fn violation(st: &mut State_, status: StatusCode, what: String) -> Response {
    let r = err(status, "invalid", &what);
    st.violations.push(what);
    r
}

/// Bearer token, protocol header and (when `replica` is true) the replica header.
fn check_auth(st: &mut State_, h: &HeaderMap, replica: bool) -> Result<(), Response> {
    let bearer = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if bearer != Some(st.token.as_str()) {
        // A bad token is the caller's failure, not a conformance violation of a
        // connector that was given the right one; still recorded for visibility.
        st.violations.push("missing or wrong bearer token".into());
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized", "bad token"));
    }
    let proto = h
        .get(aurora_protocol::version::HEADER)
        .and_then(|v| v.to_str().ok());
    if proto != Some(PROTOCOL_HEADER_VALUE) {
        return Err(violation(
            st,
            StatusCode::BAD_REQUEST,
            format!("protocol header must be {PROTOCOL_HEADER_VALUE}, got {proto:?}"),
        ));
    }
    if replica {
        let got = h
            .get(HDR_REPLICA)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<Uuid>().ok());
        if got.is_none() || got != st.replica {
            st.violations
                .push(format!("wrong or missing {HDR_REPLICA}: {got:?}"));
            return Err(err(StatusCode::FORBIDDEN, "forbidden", "unknown replica"));
        }
    }
    Ok(())
}

fn parse<T: DeserializeOwned>(st: &mut State_, body: &Bytes, what: &str) -> Result<T, Response> {
    serde_json::from_slice(body).map_err(|e| {
        violation(
            st,
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{what}: undecodable body: {e}"),
        )
    })
}

fn parse_valid<T: DeserializeOwned + Validate>(
    st: &mut State_,
    body: &Bytes,
    what: &str,
) -> Result<T, Response> {
    let v: T = parse(st, body, what)?;
    v.validate()
        .map_err(|e| violation(st, StatusCode::UNPROCESSABLE_ENTITY, format!("{what}: {e}")))?;
    Ok(v)
}

/// A manifest widening (new capability or public callback) needs re-approval (docs/13 §3).
fn widened(old: &ConnectorManifest, new: &ConnectorManifest) -> bool {
    let caps: HashSet<&str> = old.enabled_capabilities().into_iter().collect();
    new.enabled_capabilities()
        .into_iter()
        .any(|c| !caps.contains(c))
        || new
            .public_callbacks
            .iter()
            .any(|c| !old.public_callbacks.contains(c))
}

async fn register(State(i): S, h: HeaderMap, body: Bytes) -> Response {
    let mut st = lock(&i.st);
    // register is the one call without a replica header.
    if let Err(r) = check_auth(&mut st, &h, false) {
        return r;
    }
    let req: RegisterRequest = match parse_valid(&mut st, &body, "register") {
        Ok(r) => r,
        Err(r) => return r,
    };
    if req.manifest.package.contract != 1 {
        return err(StatusCode::BAD_REQUEST, "invalid", "unsupported contract");
    }
    let first = st.registrations.is_empty();
    let grew = st
        .registrations
        .last()
        .is_some_and(|prev| widened(&prev.manifest, &req.manifest));
    if first || grew {
        st.status = ConnectorStatus::PendingApproval;
        st.version += 1;
    }
    let replica = Uuid::now_v7();
    st.replica = Some(replica);
    let connector_id = req.manifest.package.id.clone();
    st.registrations.push(req);
    st.retired = false;
    Json(RegisterResponse {
        connector_id,
        replica_id: replica,
        status: st.status.clone(),
        // Fast polls keep the suite quick; a real core advertises 15 / 25.
        poll: PollConfig {
            config_secs: 1,
            ops_wait_secs: 2,
        },
    })
    .into_response()
}

/// A public 8x8 PNG standing in for an agent avatar served by the Aurora API.
pub const AVATAR_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x08, 0x08, 0x02, 0x00, 0x00, 0x00, 0x4b, 0x6d, 0x29,
    0xdc, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x38, 0x61, 0x63, 0x83,
    0x15, 0x31, 0x0c, 0x2d, 0x09, 0x00, 0x65, 0x99, 0x50, 0x01, 0x52, 0x0f, 0xaf, 0x53, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

async fn avatar_png() -> Response {
    ([(header::CONTENT_TYPE, "image/png")], AVATAR_PNG).into_response()
}

async fn config(State(i): S, h: HeaderMap) -> Response {
    let mut st = lock(&i.st);
    if let Err(r) = check_auth(&mut st, &h, true) {
        return r;
    }
    let etag = format!("\"v{}\"", st.version);
    if h.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    let active = st.status == ConnectorStatus::Active && !st.retired;
    let body = ConfigResponse {
        etag: etag.clone(),
        lease_secs: 60,
        integrations: if active {
            st.integrations.clone()
        } else {
            vec![]
        },
        presences: if active { st.presences.clone() } else { vec![] },
        space_bindings: if active { st.bindings.clone() } else { vec![] },
    };
    ([(header::ETAG, etag)], Json(body)).into_response()
}

async fn events(State(i): S, h: HeaderMap, body: Bytes) -> Response {
    {
        let mut st = lock(&i.st);
        if let Err(r) = check_auth(&mut st, &h, true) {
            return r;
        }
        st.event_posts += 1;
    }
    let mut gate = i.events_gate.subscribe();
    let _ = gate.wait_for(|open| *open).await;
    let mut st = lock(&i.st);
    if st.fail_events > 0 {
        st.fail_events -= 1;
        return err(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "inbox down");
    }
    let req: EventsRequest = match parse_valid(&mut st, &body, "events") {
        Ok(r) => r,
        Err(r) => return r,
    };
    let mut resp = EventsResponse::default();
    for ev in req.events {
        let served = st.integrations.iter().any(|x| x.id == ev.integration_id)
            && st.status == ConnectorStatus::Active;
        if !served {
            resp.rejected.push(RejectedEvent {
                event_id: ev.event_id,
                code: ErrorCode::Forbidden,
            });
            continue;
        }
        if ev.parse().is_err() {
            st.violations.push(format!(
                "event {} has an invalid {} payload",
                ev.event_id,
                ev.kind.as_str()
            ));
            resp.rejected.push(RejectedEvent {
                event_id: ev.event_id,
                code: ErrorCode::Invalid,
            });
            continue;
        }
        if st.event_ids.contains(&ev.event_id) {
            st.duplicates.push(ev.event_id.clone());
            resp.duplicates.push(ev.event_id);
            continue;
        }
        st.event_ids.insert(ev.event_id.clone());
        resp.accepted.push(ev.event_id.clone());
        st.events.push(ev);
    }
    drop(st);
    i.notify.notify_waiters();
    Json(resp).into_response()
}

async fn claim(State(i): S, h: HeaderMap, body: Bytes) -> Response {
    let req: ClaimRequest = {
        let mut st = lock(&i.st);
        if let Err(r) = check_auth(&mut st, &h, true) {
            return r;
        }
        match parse_valid(&mut st, &body, "ops/claim") {
            Ok(r) => r,
            Err(r) => return r,
        }
    };
    let end = Instant::now() + Duration::from_secs(u64::from(req.wait_secs));
    loop {
        let notified = i.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let mut st = lock(&i.st);
            let served: HashSet<Uuid> = st.integrations.iter().map(|x| x.id).collect();
            let active = st.status == ConnectorStatus::Active && !st.retired;
            let mut out = vec![];
            if active {
                for o in &mut st.ops {
                    if out.len() >= req.max as usize {
                        break;
                    }
                    let owned = o.op.integration_id.is_none_or(|x| served.contains(&x));
                    if o.state != OpState::Queued || !owned {
                        continue;
                    }
                    let mut claimed = o.op.clone();
                    claimed.attempt = o.claims.len() as u32 + 1;
                    claimed.maybe_sent = o.lapsed;
                    o.claims.push(claimed.clone());
                    o.state = OpState::Claimed;
                    out.push(claimed);
                }
            }
            if !out.is_empty() || Instant::now() >= end {
                return Json(ClaimResponse { ops: out }).into_response();
            }
        }
        let left = end.saturating_duration_since(Instant::now());
        let _ = tokio::time::timeout(left.min(Duration::from_millis(250)), notified).await;
    }
}

async fn results(State(i): S, h: HeaderMap, body: Bytes) -> Response {
    let mut st = lock(&i.st);
    if let Err(r) = check_auth(&mut st, &h, true) {
        return r;
    }
    let req: ResultsRequest = match parse_valid(&mut st, &body, "ops/results") {
        Ok(r) => r,
        Err(r) => return r,
    };
    for r in req.results {
        match st.ops.iter_mut().find(|o| o.op.op_id == r.op_id) {
            Some(o) if o.state == OpState::Claimed => {
                o.state = OpState::Done;
                o.result = Some(r);
            }
            Some(_) => st.late_results.push(r),
            None => {
                st.violations
                    .push(format!("result for unknown op {}", r.op_id));
                return err(StatusCode::NOT_FOUND, "not_found", "unknown op");
            }
        }
    }
    drop(st);
    i.notify.notify_waiters();
    Json(OkResponse { ok: true }).into_response()
}

async fn credentials(State(i): S, h: HeaderMap, body: Bytes) -> Response {
    let mut st = lock(&i.st);
    if let Err(r) = check_auth(&mut st, &h, true) {
        return r;
    }
    let req: CredentialsRequest = match parse_valid(&mut st, &body, "credentials") {
        Ok(r) => r,
        Err(r) => return r,
    };
    let key = match (req.target.integration_id, req.target.presence_id) {
        (Some(id), _) => format!("integration:{id}"),
        (_, Some(id)) => format!("presence:{id}"),
        _ => return err(StatusCode::BAD_REQUEST, "invalid", "no target"),
    };
    let Some(cur) = st.credentials.get(&key).map(|c| c.0) else {
        return err(StatusCode::NOT_FOUND, "not_found", "unknown target");
    };
    if cur != req.expected_version {
        return err(StatusCode::CONFLICT, "stale_version", "credentials changed");
    }
    let version = cur + 1;
    st.credentials
        .insert(key, (version, req.credentials.clone()));
    st.credential_writes.push(req);
    Json(CredentialsResponse { version }).into_response()
}

async fn link_start(State(i): S, h: HeaderMap, body: Bytes) -> Response {
    let mut st = lock(&i.st);
    if let Err(r) = check_auth(&mut st, &h, true) {
        return r;
    }
    let req: LinkStartRequest = match parse(&mut st, &body, "link-start") {
        Ok(r) => r,
        Err(r) => return r,
    };
    if !st.integrations.iter().any(|x| x.id == req.integration_id) {
        return err(StatusCode::NOT_FOUND, "not_found", "not your integration");
    }
    if req.platform_user_ref.is_empty() {
        return violation(
            &mut st,
            StatusCode::BAD_REQUEST,
            "empty platform_user_ref".into(),
        );
    }
    st.link_starts.push(req);
    Json(LinkStartResponse {
        url: format!(
            "https://aurora.test/link/channel?token=aui_{}",
            Uuid::now_v7().simple()
        ),
        expires_at: "2099-01-01T00:30:00Z".into(),
    })
    .into_response()
}

/// `PUT /files/{op_id}`: only for a claimed `file.fetch` op of this replica.
async fn put_file(State(i): S, Path(op_id): Path<Uuid>, h: HeaderMap, body: Bytes) -> Response {
    let mut st = lock(&i.st);
    if let Err(r) = check_auth(&mut st, &h, true) {
        return r;
    }
    let Some(rec) = st.ops.iter().find(|o| o.op.op_id == op_id) else {
        return err(StatusCode::NOT_FOUND, "not_found", "unknown op");
    };
    if rec.op.kind != OpKind::FileFetch || rec.state != OpState::Claimed {
        return err(
            StatusCode::NOT_FOUND,
            "not_found",
            "not a claimed file.fetch op",
        );
    }
    let max = rec
        .op
        .payload
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if body.len() as u64 > max {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "over max_bytes");
    }
    let filename = h
        .get(HDR_FILENAME)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let content_type = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let (Some(filename), Some(content_type)) = (filename, content_type) else {
        return violation(
            &mut st,
            StatusCode::BAD_REQUEST,
            format!("file upload needs {HDR_FILENAME} and content-type headers"),
        );
    };
    let attachment_id = Uuid::now_v7();
    st.uploads.push(StoredFile {
        attachment_id,
        filename,
        content_type,
        bytes: body.to_vec(),
        op_id: Some(op_id),
    });
    Json(FilePutResponse { attachment_id }).into_response()
}

/// `GET /files/{attachment_id}`: only attachments referenced by a claimed
/// `message.post` op of this replica; everything else is 404.
async fn get_file(State(i): S, Path(id): Path<Uuid>, h: HeaderMap) -> Response {
    let mut st = lock(&i.st);
    if let Err(r) = check_auth(&mut st, &h, true) {
        return r;
    }
    let referenced = st.ops.iter().any(|o| {
        o.op.kind == OpKind::MessagePost
            && o.state == OpState::Claimed
            && o.op.payload["files"]
                .as_array()
                .is_some_and(|fs| fs.iter().any(|f| f["attachment_id"] == json!(id)))
    });
    match st.outbound.get(&id) {
        Some(f) if referenced => (
            [(header::CONTENT_TYPE, f.content_type.clone())],
            f.bytes.clone(),
        )
            .into_response(),
        _ => err(StatusCode::NOT_FOUND, "not_found", "no such attachment"),
    }
}

async fn retire(State(i): S, h: HeaderMap) -> Response {
    let mut st = lock(&i.st);
    if let Err(r) = check_auth(&mut st, &h, true) {
        return r;
    }
    st.retired = true;
    st.retire_calls += 1;
    st.version += 1;
    Json(OkResponse { ok: true }).into_response()
}

// ------------------------------------------------------------- aliases

fn swap(s: &str, pairs: &[(&str, &str)]) -> Option<String> {
    for (from, to) in pairs {
        if s == *from {
            return Some((*to).to_owned());
        }
    }
    let mut out = s.to_owned();
    let mut changed = false;
    for (from, to) in pairs {
        for tag in ["<@user:", "<@bot:", "<#space:"] {
            let needle = format!("{tag}{from}>");
            if out.contains(&needle) {
                out = out.replace(&needle, &format!("{tag}{to}>"));
                changed = true;
            }
        }
    }
    changed.then_some(out)
}

fn walk(v: &mut Value, pairs: &[(&str, &str)]) {
    match v {
        Value::String(s) => {
            if let Some(n) = swap(s, pairs) {
                *s = n;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| walk(x, pairs)),
        Value::Object(m) => m.values_mut().for_each(|x| walk(x, pairs)),
        _ => {}
    }
}

/// `to_alias`: real → alias (what the scenario reads); else alias → real.
fn translate(v: &mut Value, aliases: &[(String, String)], to_alias: bool) {
    if aliases.is_empty() {
        return;
    }
    let pairs: Vec<(&str, &str)> = aliases
        .iter()
        .map(|(a, r)| {
            if to_alias {
                (r.as_str(), a.as_str())
            } else {
                (a.as_str(), r.as_str())
            }
        })
        .collect();
    walk(v, &pairs);
}

fn alias_str(aliases: &[(String, String)], s: &str) -> String {
    let mut v = Value::String(s.to_owned());
    translate(&mut v, aliases, true);
    v.as_str().unwrap_or(s).to_owned()
}

fn aliased_ids(aliases: &[(String, String)], ids: &[String]) -> Vec<String> {
    ids.iter().map(|i| alias_str(aliases, i)).collect()
}

fn aliased_events(aliases: &[(String, String)], events: &[InboundEvent]) -> Vec<InboundEvent> {
    events
        .iter()
        .map(|e| {
            let mut e = e.clone();
            e.event_id = alias_str(aliases, &e.event_id);
            translate(&mut e.payload, aliases, true);
            e
        })
        .collect()
}

fn alias_result(aliases: &[(String, String)], mut r: OpResult) -> OpResult {
    if let Some(d) = r.data.as_mut() {
        translate(d, aliases, true);
    }
    r
}

/// A read-only handle on a [`FakeCore`] (see [`FakeCore::probe`]). Answers use
/// the same aliases as the core's own accessors.
#[derive(Clone)]
pub struct CoreProbe {
    inner: Arc<Inner>,
}

impl CoreProbe {
    pub fn events(&self) -> Vec<InboundEvent> {
        let st = lock(&self.inner.st);
        aliased_events(&st.aliases, &st.events)
    }

    pub fn duplicates(&self) -> Vec<String> {
        let st = lock(&self.inner.st);
        aliased_ids(&st.aliases, &st.duplicates)
    }

    pub fn registrations(&self) -> Vec<RegisterRequest> {
        lock(&self.inner.st).registrations.clone()
    }

    /// The alias for a platform id (the id itself when none).
    pub fn alias_of(&self, real: &str) -> String {
        alias_str(&lock(&self.inner.st).aliases, real)
    }

    /// See [`FakeCore::add_presence`].
    pub fn add_presence(&self, presence: Presence) {
        let mut st = lock(&self.inner.st);
        st.presences.retain(|p| p.id != presence.id);
        st.presences.push(presence);
        st.version += 1;
    }

    pub fn set_alias(&self, alias: &str, real: &str) {
        let mut st = lock(&self.inner.st);
        st.aliases.retain(|(a, _)| a != alias);
        st.aliases.push((alias.to_owned(), real.to_owned()));
    }
}

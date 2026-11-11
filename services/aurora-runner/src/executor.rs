//! Executor: one step at a time per sandbox (FIFO), bounded global concurrency
//! for lifecycle steps.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use aurora_core::limits::ATTACH_IN_FILE_MAX;
use aurora_protocol::runner::AttachmentRef;
use aurora_protocol::runner::{
    HomeSyncKind, NAMESPACE_AURORA_REPORT, ReportBody, SandboxStart, SandboxState, SandboxStatus,
    SandboxStop, StopReason, WorkItem, WorkItemBody,
};
use aurora_protocol::supervisor::{FileIn, MAX_FILE_PART_DECODED};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::api::ApiClient;
use crate::backend::{Backend, BackendError, ContainerRef, HoldCallback, StartRequest};
use crate::hub::{ChannelHub, Frame, HubError};
use crate::reporter::{ReportSink, report};

/// Time allowed for downloading one inbound attachment (all retries included).
const ATTACHMENT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `file.in` frames may wait for the supervisor to acknowledge earlier ones.
const ATTACHMENT_BACKLOG_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ExecutorSettings {
    /// Global bound on concurrent start/stop steps.
    pub max_concurrency: usize,
    /// How long a connected supervisor may flush after `shutdown` (`11 §Stop`: 10 s).
    pub shutdown_flush: Duration,
    /// `runner.image` (docs/11, docs/15): the sandbox image this runner starts. `None` keeps the
    /// image named in the start item (the API's `aurora-sandbox:<version>`).
    pub image: Option<String>,
    /// (FIX-025, D-158) Bound on a whole start when the item carries no `start_timeout_secs`.
    pub start_timeout: Duration,
    /// (FIX-025, D-158) Longest gap between two `starting` progress reports while a start runs:
    /// each one renews the API's start lease.
    pub progress_every: Duration,
}

impl Default for ExecutorSettings {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            shutdown_flush: Duration::from_secs(10),
            image: None,
            start_timeout: Duration::from_secs(270),
            progress_every: Duration::from_secs(20),
        }
    }
}

#[derive(Default)]
struct Queues {
    /// Sandboxes with an active worker; the queue holds items not yet started.
    queues: HashMap<Uuid, VecDeque<WorkItem>>,
    in_flight: HashSet<Uuid>,
    running: HashSet<Uuid>,
}

/// What the executor needs to fetch inbound attachments (set once the runner has registered).
#[derive(Clone)]
struct Inbound {
    api: Arc<ApiClient>,
    /// Per-file download cap.
    max_bytes: u64,
}

struct Inner {
    inbound: Mutex<Option<Inbound>>,
    backend: Arc<dyn Backend>,
    hub: Arc<dyn ChannelHub>,
    sink: ReportSink,
    settings: ExecutorSettings,
    permits: Semaphore,
    q: Mutex<Queues>,
    /// (P4-PRV-1, D-145) `share_id -> port` observed from the `previews` home-sync parts this
    /// runner has forwarded: the only source for resolving a `preview.start { share_id }` work
    /// item to the supervisor's `preview.start { port }` frame (no command text ever travels).
    preview_ports: Mutex<HashMap<Uuid, u16>>,
}

#[derive(Clone)]
pub struct Executor(Arc<Inner>);

/// Deterministic id for runner-originated lifecycle reports, so a redelivered
/// item yields the same report id.
fn lifecycle_report_id(item_id: Uuid, step: &str) -> Uuid {
    Uuid::new_v5(
        &NAMESPACE_AURORA_REPORT,
        format!("item:{item_id}:{step}").as_bytes(),
    )
}

/// (FIX-025) Last reported start phase, shared by the backend callbacks and the heartbeat.
struct ProgressState {
    phase: String,
    detail: String,
    last_sent: Option<tokio::time::Instant>,
    /// `held` already reported (once per start).
    held: bool,
}

struct HeldReporter {
    sink: ReportSink,
    item_id: Uuid,
    sandbox_id: Uuid,
    boot_id: Uuid,
    every: Duration,
    state: Mutex<ProgressState>,
}

impl HeldReporter {
    fn lock(&self) -> MutexGuard<'_, ProgressState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn send_progress(&self, phase: &str, detail: &str) {
        let mut st = status(
            SandboxState::Starting,
            None,
            Some(phase),
            (!detail.is_empty()).then(|| aurora_core::sanitize::reason(detail, 500)),
        );
        st.events = Vec::new();
        // Progress reports are not deduplicated: each renews the lease (D-158).
        self.sink.send(report(
            Uuid::now_v7(),
            self.sandbox_id,
            self.boot_id,
            ReportBody::SandboxStatus(st),
        ));
    }

    /// Re-sends the last phase when nothing was reported for `every` (lease heartbeat).
    fn heartbeat(&self) {
        let due = {
            let mut s = self.lock();
            let due = s.last_sent.is_none_or(|t| t.elapsed() >= self.every);
            if due {
                s.last_sent = Some(tokio::time::Instant::now());
            }
            due.then(|| (s.phase.clone(), s.detail.clone()))
        };
        if let Some((p, d)) = due {
            self.send_progress(&p, &d);
        }
    }

    fn last(&self) -> (String, String) {
        let s = self.lock();
        (s.phase.clone(), s.detail.clone())
    }
}

impl HoldCallback for HeldReporter {
    fn on_hold(&self) {
        {
            let mut s = self.lock();
            if s.held {
                return;
            }
            s.held = true;
            s.phase = "waiting_capacity".into();
            s.last_sent = Some(tokio::time::Instant::now());
        }
        self.sink.send(report(
            lifecycle_report_id(self.item_id, "held"),
            self.sandbox_id,
            self.boot_id,
            ReportBody::SandboxStatus(status(SandboxState::Held, None, None, None)),
        ));
    }

    fn progress(&self, phase: &str, detail: &str) {
        let send = {
            let mut s = self.lock();
            let changed = s.phase != phase || s.detail != detail;
            let stale = s.last_sent.is_none_or(|t| t.elapsed() >= self.every);
            s.phase = phase.to_owned();
            s.detail = detail.to_owned();
            if changed || stale {
                s.last_sent = Some(tokio::time::Instant::now());
            }
            changed || stale
        };
        if send {
            self.send_progress(phase, detail);
        }
    }
}

fn status(
    state: SandboxState,
    container_ref: Option<String>,
    reason: Option<&str>,
    message: Option<String>,
) -> SandboxStatus {
    SandboxStatus {
        state,
        container_ref,
        reason_code: reason.and_then(|r| aurora_protocol::runner::Code::new(r).ok()),
        message,
        observed: None,
        events: Vec::new(),
    }
}

impl Executor {
    pub fn new(
        backend: Arc<dyn Backend>,
        hub: Arc<dyn ChannelHub>,
        sink: ReportSink,
        settings: ExecutorSettings,
    ) -> Self {
        Self(Arc::new(Inner {
            inbound: Mutex::new(None),
            permits: Semaphore::new(settings.max_concurrency.max(1)),
            backend,
            hub,
            sink,
            settings,
            q: Mutex::default(),
            preview_ports: Mutex::default(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Queues> {
        self.0
            .q
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Enables inbound attachment delivery (S-11). Each download is capped at
    /// `max_attachment_bytes` (the runner's advertised capability), further bounded by the
    /// protocol's per-file inbound limit. Without an API client `turn.deliver` is forwarded as is.
    pub fn set_inbound_api(&self, api: Arc<ApiClient>, max_attachment_bytes: u64) {
        let max_bytes = max_attachment_bytes.min(ATTACH_IN_FILE_MAX);
        *self
            .0
            .inbound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Inbound { api, max_bytes });
    }

    /// Sandboxes this replica currently believes to be running.
    pub fn running_count(&self) -> u32 {
        self.lock().running.len() as u32
    }

    pub fn seed_running(&self, ids: impl IntoIterator<Item = Uuid>) {
        self.lock().running.extend(ids);
    }

    /// True while the sandbox has queued or executing work.
    pub fn is_busy(&self, sandbox_id: Uuid) -> bool {
        self.lock().queues.contains_key(&sandbox_id)
    }

    pub fn is_idle(&self) -> bool {
        self.lock().queues.is_empty()
    }

    /// Wait until nothing is queued or executing, up to `timeout`.
    pub async fn drain(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.is_idle() {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }

    /// Enqueue an item behind earlier items of the same sandbox. Redelivered
    /// items that are still queued/executing are ignored.
    pub fn submit(&self, item: WorkItem) {
        let sandbox = item.sandbox_id;
        let spawn = {
            let mut q = self.lock();
            if !q.in_flight.insert(item.item_id) {
                tracing::debug!(item_id = %item.item_id, "duplicate work item ignored");
                return;
            }
            match q.queues.get_mut(&sandbox) {
                Some(queue) => {
                    queue.push_back(item);
                    false
                }
                None => {
                    q.queues.insert(sandbox, VecDeque::from([item]));
                    true
                }
            }
        };
        if spawn {
            let this = self.clone();
            tokio::spawn(async move { this.worker(sandbox).await });
        }
    }

    async fn worker(&self, sandbox: Uuid) {
        loop {
            let item = {
                let mut q = self.lock();
                match q.queues.get_mut(&sandbox).and_then(VecDeque::pop_front) {
                    Some(i) => i,
                    None => {
                        q.queues.remove(&sandbox);
                        return;
                    }
                }
            };
            let id = item.item_id;
            // A panicking step must not wedge the sandbox queue.
            let this = self.clone();
            let res = tokio::spawn(async move { this.handle(item).await }).await;
            if res.is_err() {
                tracing::error!(item_id = %id, "work item handler panicked");
            }
            self.lock().in_flight.remove(&id);
        }
    }

    async fn handle(&self, item: WorkItem) {
        let (item_id, sandbox, boot) = (item.item_id, item.sandbox_id, item.boot_id);
        match item.body {
            WorkItemBody::SandboxStart(s) => self.start(item_id, sandbox, boot, s).await,
            WorkItemBody::SandboxStop(s) => self.stop(item_id, sandbox, boot, s).await,
            // (P4-PRV-1, D-145) Resolved to the supervisor's `preview.start { port }` frame: the
            // port comes from the `previews` home part this runner already forwarded. A share the
            // runner has no port for (part not applied, stale) is dropped. Sent directly (not
            // through `forward`, which skips unknown kinds): this frame is the private
            // runner->supervisor signal of `16 §W5` and carries no command text.
            WorkItemBody::PreviewStart(p) => {
                let port = self
                    .0
                    .preview_ports
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&p.share_id)
                    .copied();
                match port {
                    Some(port) => {
                        if let Err(e) = self
                            .0
                            .hub
                            .send(
                                sandbox,
                                Frame {
                                    kind: "preview.start".to_owned(),
                                    body: serde_json::json!({ "port": port }),
                                },
                            )
                            .await
                        {
                            tracing::warn!(%sandbox, error = %e, "preview.start not delivered");
                        }
                    }
                    None => {
                        tracing::warn!(%sandbox, share = %p.share_id, "preview.start without a known port; dropped")
                    }
                }
            }
            other => self.forward(sandbox, other).await,
        }
    }

    fn emit(&self, item_id: Uuid, step: &str, sandbox: Uuid, boot: Uuid, st: SandboxStatus) {
        self.0.sink.send(report(
            lifecycle_report_id(item_id, step),
            sandbox,
            boot,
            ReportBody::SandboxStatus(st),
        ));
    }

    async fn start(&self, item_id: Uuid, sandbox: Uuid, boot: Uuid, s: SandboxStart) {
        // (FIX-025, D-158) One bound for the whole start, from the moment the item is handled
        // (waiting for a start slot included); the API's lease outlives it (C-580).
        let bound = s
            .start_timeout_secs
            .map_or(self.0.settings.start_timeout, |t| {
                Duration::from_secs(u64::from(t))
            });
        let deadline = tokio::time::Instant::now() + bound;
        let hold = HeldReporter {
            sink: self.0.sink.clone(),
            item_id,
            sandbox_id: sandbox,
            boot_id: boot,
            every: self.0.settings.progress_every,
            state: Mutex::new(ProgressState {
                phase: "queued".into(),
                detail: String::new(),
                last_sent: None,
                held: false,
            }),
        };
        hold.progress("queued", "waiting for a start slot on the runner");
        let b = &self.0.backend;
        let mut spec = s.spec;
        if let Some(image) = &self.0.settings.image {
            spec.image.clone_from(image);
        }
        let req = StartRequest {
            sandbox_id: sandbox,
            boot_id: boot,
            spec,
        };
        let work = async {
            let Ok(permit) = self.0.permits.acquire().await else {
                return None;
            };
            hold.progress("provisioning", "preparing the home volume");
            let r = match b
                .provision_home(sandbox, &req.spec.home_ref, req.spec.home_gib)
                .await
            {
                Ok(()) => b.start(&req, &hold).await,
                Err(e) => Err(e),
            };
            Some((permit, r))
        };
        let result = {
            tokio::pin!(work);
            let mut beat = tokio::time::interval(self.0.settings.progress_every);
            beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    r = &mut work => break r.map(|(_permit, r)| r),
                    () = tokio::time::sleep_until(deadline) => {
                        let (phase, detail) = hold.last();
                        let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
                        break Some(Err(BackendError::StartTimeout(format!(
                            "the sandbox did not start within {} s (last phase: {phase}{detail})",
                            bound.as_secs()
                        ))));
                    }
                    _ = beat.tick() => hold.heartbeat(),
                }
            }
        };
        let Some(result) = result else { return };
        match result {
            Ok(ContainerRef(reference)) => {
                self.lock().running.insert(sandbox);
                let observed = match b.inspect(sandbox).await {
                    Ok(Some(o)) => o.observed,
                    _ => None,
                };
                // (FIX-025) Tell the API where the supervisor must connect (the hint of
                // `supervisor_unreachable` when it never does).
                let message = b.supervisor_endpoint().await.map(|ep| {
                    format!("waiting for the supervisor to connect to the runner at {ep}")
                });
                let mut st = status(SandboxState::Running, Some(reference), None, message);
                st.observed = observed;
                self.emit(item_id, "running", sandbox, boot, st);
            }
            Err(e) => {
                tracing::warn!(%sandbox, %boot, reason = e.reason_code(), error = %e.sanitized_message(), "sandbox start failed");
                self.lock().running.remove(&sandbox);
                if matches!(e, BackendError::StartTimeout(_)) {
                    // The abandoned start may have left its container: remove this boot's.
                    if let Ok(Some(o)) = b.inspect(sandbox).await
                        && o.boot_id.is_none_or(|ob| ob == boot)
                        && let Err(err) = b.stop(&o.container, Duration::ZERO).await
                    {
                        tracing::warn!(%sandbox, %boot, error = %err, "cleanup after start timeout failed");
                    }
                }
                let mut st = status(
                    SandboxState::Failed,
                    None,
                    Some(e.reason_code()),
                    Some(e.sanitized_message()),
                );
                st.events = crate::backend::bound_events(b.start_events(&req).await);
                self.emit(item_id, "failed", sandbox, boot, st);
            }
        }
    }

    async fn stop(&self, item_id: Uuid, sandbox: Uuid, boot: Uuid, s: SandboxStop) {
        let Ok(_permit) = self.0.permits.acquire().await else {
            return;
        };
        let grace = Duration::from_secs(u64::from(s.grace_secs));
        // The supervisor gets `shutdown` first and a bounded time to flush.
        if self.0.hub.is_connected(sandbox) {
            let frame = Frame {
                kind: "shutdown".into(),
                body: serde_json::json!({ "reason": reason_str(&s.reason), "grace_secs": s.grace_secs }),
            };
            if self.0.hub.send(sandbox, frame).await.is_ok() {
                let deadline =
                    tokio::time::Instant::now() + self.0.settings.shutdown_flush.min(grace);
                while self.0.hub.is_connected(sandbox) && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
        let b = &self.0.backend;
        let outcome = match b.inspect(sandbox).await {
            // Never stop a newer boot on behalf of a stale item.
            Ok(Some(o)) if o.boot_id.is_none_or(|ob| ob == boot) => {
                b.stop(&o.container, grace).await
            }
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => {
                self.lock().running.remove(&sandbox);
                self.emit(
                    item_id,
                    "stopped",
                    sandbox,
                    boot,
                    status(SandboxState::Stopped, None, None, None),
                );
            }
            Err(e) => {
                tracing::warn!(%sandbox, %boot, error = %e, "sandbox stop failed");
                let st = status(
                    SandboxState::Failed,
                    None,
                    Some("agent_stop_failed"),
                    Some(e.sanitized_message()),
                );
                self.emit(item_id, "failed", sandbox, boot, st);
            }
        }
    }

    /// Socket-bound items go to the supervisor, in order, through the hub.
    async fn forward(&self, sandbox: Uuid, body: WorkItemBody) {
        let kind = body.kind().to_owned();
        let mut value = match serde_json::to_value(&body) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(%kind, error = %e, "cannot encode work item");
                return;
            }
        };
        if let Some(o) = value.as_object_mut() {
            o.remove("kind");
        }
        if matches!(body, WorkItemBody::Other(_)) {
            tracing::warn!(%kind, "unknown work item kind skipped");
            return;
        }
        // (P4-PRV-1, D-145) Remember the share->port mapping of the `previews` home part this
        // runner forwarded: `preview.start { share_id }` work items resolve against it.
        if let WorkItemBody::HomeSync(h) = &body
            && h.part.kind == HomeSyncKind::Previews
        {
            let mut ports = self
                .0
                .preview_ports
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for s in h
                .part
                .entries
                .iter()
                .filter_map(|e| e.get("shares").and_then(|s| s.as_array()))
                .flatten()
            {
                if let (Some(id), Some(port)) = (
                    s.get("share_id")
                        .and_then(|v| v.as_str())
                        .and_then(|v| v.parse::<Uuid>().ok()),
                    s.get("port").and_then(|v| v.as_u64()),
                ) {
                    ports.insert(id, u16::try_from(port).unwrap_or(0));
                }
            }
        }
        // Channel order is seq order: every `file.in` frame precedes the `turn.deliver` it belongs to.
        if let WorkItemBody::TurnDeliver(d) = &body {
            self.deliver_attachments(sandbox, d.turn_id, &d.attachments)
                .await;
        }
        if let Err(e) = self
            .0
            .hub
            .send(
                sandbox,
                Frame {
                    kind: kind.clone(),
                    body: value,
                },
            )
            .await
        {
            tracing::warn!(%sandbox, %kind, error = %e, "work item not delivered");
        }
    }
}

/// Why an attachment could not be streamed. The turn is delivered regardless.
#[derive(Debug)]
enum InboundError {
    Download(crate::api::ApiError),
    Channel(HubError),
}

impl Executor {
    /// Downloads each attachment and streams it to the supervisor as `file.in` frames. A failure
    /// is logged (never the contents) and skipped: the supervisor reports a missing file to the
    /// model when the turn starts.
    async fn deliver_attachments(&self, sandbox: Uuid, turn_id: Uuid, list: &[AttachmentRef]) {
        let inbound = self
            .0
            .inbound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(inbound) = inbound else { return };
        for a in list
            .iter()
            .take(aurora_protocol::runner::MAX_TURN_ATTACHMENTS)
        {
            match self.stream_attachment(&inbound, sandbox, turn_id, a).await {
                Ok(n) => {
                    tracing::info!(%sandbox, %turn_id, attachment_id = %a.attachment_id, bytes = n, "inbound attachment delivered");
                }
                Err(InboundError::Channel(e)) => {
                    tracing::warn!(%sandbox, %turn_id, attachment_id = %a.attachment_id, error = %e, "inbound attachment not delivered");
                    return; // the channel is gone; later files would fail the same way
                }
                Err(InboundError::Download(e)) => {
                    tracing::warn!(%sandbox, %turn_id, attachment_id = %a.attachment_id, error = %e, "inbound attachment download failed");
                }
            }
        }
    }

    async fn stream_attachment(
        &self,
        inbound: &Inbound,
        sandbox: Uuid,
        turn_id: Uuid,
        a: &AttachmentRef,
    ) -> Result<usize, InboundError> {
        let bytes = inbound
            .api
            .download_attachment(a.attachment_id, inbound.max_bytes, ATTACHMENT_TIMEOUT)
            .await
            .map_err(InboundError::Download)?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        let mut chunks: Vec<&[u8]> = bytes.chunks(MAX_FILE_PART_DECODED).collect();
        if chunks.is_empty() {
            chunks.push(&[]); // an empty file is one empty last chunk
        }
        let count = chunks.len();
        for (index, chunk) in chunks.into_iter().enumerate() {
            let frame = FileIn {
                turn_id,
                attachment_id: a.attachment_id,
                filename: a.filename.clone(),
                index: u32::try_from(index).unwrap_or(u32::MAX),
                last: index + 1 == count,
                size,
                sha256: sha256.clone(),
                data_b64: base64::engine::general_purpose::STANDARD.encode(chunk),
            };
            let body = serde_json::to_value(&frame)
                .map_err(|_| InboundError::Channel(HubError::TooLarge))?;
            self.send_with_backpressure(sandbox, "file.in", body)
                .await?;
        }
        Ok(bytes.len())
    }

    /// The hub bounds unacknowledged bytes; wait for the supervisor's acks instead of failing.
    async fn send_with_backpressure(
        &self,
        sandbox: Uuid,
        kind: &str,
        body: serde_json::Value,
    ) -> Result<(), InboundError> {
        let deadline = tokio::time::Instant::now() + ATTACHMENT_BACKLOG_WAIT;
        loop {
            let frame = Frame {
                kind: kind.to_owned(),
                body: body.clone(),
            };
            match self.0.hub.send(sandbox, frame).await {
                Ok(()) => return Ok(()),
                Err(HubError::Backlog) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(InboundError::Channel(e)),
            }
        }
    }
}

fn reason_str(r: &StopReason) -> &str {
    r.as_str()
}

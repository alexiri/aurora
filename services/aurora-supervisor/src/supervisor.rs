//! Supervisor core: turn execution over the channel and a harness adapter
//! (`docs/12-supervisor-harness.md` §Process model and startup, §Harness interface).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use aurora_protocol::runner::{
    Code, HarnessId, HarnessInfo, HomeSynced, SendNowOutcome, SendNowResult, SessionReport,
    SteerOutcome, SteerResult, SupervisorReady, TurnDeliver, TurnEvent, TurnMessage, TurnRef,
    TurnResult, TurnStatus,
};
use aurora_protocol::supervisor::{RunnerToSupervisor, SupervisorToRunner};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::{Instant, interval, sleep_until};
use uuid::Uuid;

use crate::channel::ChannelSender;
use crate::harness::{
    HarnessAdapter, HarnessCapabilities, HarnessEvent, HarnessSession, McpBridgeSpec,
    SessionOptions, SteerAck, SteerInput, TurnInput,
};
use crate::harvest::Harvester;
use crate::home::{HomeConfig, HomeProjector, PartOutcome};
use crate::proxy::{Credential, ProxyHandle, ProxyMode, spawn_proxy};

const ABORT_GRACE: Duration = Duration::from_secs(5);
const MAX_ERROR_MESSAGE: usize = 2_000;

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub working_dir: PathBuf,
    /// Gateway proxy `host:port`.
    pub gateway_proxy: String,
    pub boot_token: String,
    pub proxy_ip: IpAddr,
    pub progress_interval: Duration,
    pub shutdown_grace: Duration,
    pub version: String,
    pub mcp_bridge_command: Vec<String>,
    pub harvest_interval: Duration,
}

impl SupervisorConfig {
    pub fn new(gateway_proxy: impl Into<String>, boot_token: impl Into<String>) -> Self {
        Self {
            working_dir: PathBuf::from("/workspace"),
            gateway_proxy: gateway_proxy.into(),
            boot_token: boot_token.into(),
            proxy_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            progress_interval: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(5),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            mcp_bridge_command: Vec::new(),
            harvest_interval: Duration::from_secs(10),
        }
    }
}

type SharedSession = Arc<Mutex<Box<dyn HarnessSession>>>;

struct SessionEntry {
    session: SharedSession,
    cred: Arc<Credential>,
    epoch: u64,
    reported_ref: Arc<StdMutex<Option<String>>>,
    _proxy: ProxyHandle,
}

struct TurnHandle {
    conversation_id: Uuid,
    session: SharedSession,
    aborted: Arc<AtomicBool>,
    abort: Arc<Notify>,
    pending_steers: Arc<StdMutex<HashSet<Uuid>>>,
}

pub struct Supervisor {
    cfg: SupervisorConfig,
    out: ChannelSender,
    adapter: Arc<dyn HarnessAdapter>,
    caps: HarnessCapabilities,
    sessions: HashMap<Uuid, SessionEntry>,
    turns: HashMap<Uuid, TurnHandle>,
    ready: bool,
    home: Arc<StdMutex<HomeCtl>>,
}

/// Home projection state: staged sync, plus the memory harvest bookkeeping.
struct HomeCtl {
    projector: HomeProjector,
    harvester: Harvester,
}

/// Maps a terminal harness error code to a turn error code (`12 §Provider refusal classification`).
pub fn classify_error(code: &str) -> &'static str {
    match code {
        "provider_auth" => "provider_auth",
        "provider_rate_limited" => "provider_rate_limited",
        "provider_quota" => "provider_quota",
        "provider_overloaded" => "provider_overloaded",
        "provider_refusal" => "provider_refusal",
        "gateway_refused" => "gateway_refused",
        _ => "harness_failed",
    }
}

fn code(s: &str) -> Code {
    // Static short strings; the fallback is unreachable for our fixed vocabulary.
    Code::new(s)
        .unwrap_or_else(|_| Code::new("harness_failed").unwrap_or_else(|_| unreachable_code()))
}

#[allow(clippy::panic)]
fn unreachable_code() -> Code {
    panic!("fixed code vocabulary exceeds bound")
}

impl Supervisor {
    pub fn new(
        cfg: SupervisorConfig,
        out: ChannelSender,
        adapter: Arc<dyn HarnessAdapter>,
    ) -> Self {
        let caps = adapter.capabilities();
        let home = Arc::new(StdMutex::new(HomeCtl {
            projector: HomeProjector::new(HomeConfig {
                root: cfg.working_dir.clone(),
                skills_dir: caps.skills_dir.clone(),
                instruction_files: caps.instruction_files.clone(),
            }),
            harvester: Harvester::new(&cfg.working_dir),
        }));
        Self {
            home,
            cfg,
            out,
            adapter,
            caps,
            sessions: HashMap::new(),
            turns: HashMap::new(),
            ready: false,
        }
    }

    async fn send(&self, msg: SupervisorToRunner) {
        if self.out.send(msg).await.is_err() {
            tracing::warn!("channel closed; dropping outgoing message");
        }
    }

    /// Runs until `inbound` closes or a `shutdown` frame has been handled.
    pub async fn run(mut self, mut inbound: mpsc::Receiver<RunnerToSupervisor>) {
        let ready = SupervisorReady {
            version: self.cfg.version.clone(),
            harness: HarnessInfo {
                id: HarnessId::from_wire(self.adapter.id()).unwrap_or(HarnessId::Fake),
                capabilities: self.caps.names().into_iter().map(code).collect(),
            },
        };
        self.send(SupervisorToRunner::Ready(ready)).await;
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Uuid>();
        // Instruction files are a projection re-rendered on every boot.
        let home = self.home.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(h) = home.lock()
                && let Err(e) = h.projector.render_installed()
            {
                tracing::warn!(error = %e, "boot render of instruction files failed");
            }
        })
        .await;
        let mut harvest_tick = interval(self.cfg.harvest_interval);
        harvest_tick.tick().await;
        loop {
            tokio::select! {
                _ = harvest_tick.tick() => { self.harvest().await; }
                msg = inbound.recv() => {
                    let Some(msg) = msg else { break };
                    if let RunnerToSupervisor::Shutdown(s) = &msg {
                        let grace = s.grace_secs.map_or(self.cfg.shutdown_grace, |g| Duration::from_secs(u64::from(g)));
                        self.shutdown(grace, &mut done_rx).await;
                        break;
                    }
                    self.handle(msg, &done_tx).await;
                }
                Some(turn_id) = done_rx.recv() => { self.turns.remove(&turn_id); }
            }
        }
    }

    async fn shutdown(&mut self, grace: Duration, done_rx: &mut mpsc::UnboundedReceiver<Uuid>) {
        for t in self.turns.values() {
            t.aborted.store(true, Ordering::SeqCst);
            t.abort.notify_one();
            let _ = t.session.lock().await.abort().await;
        }
        let deadline = Instant::now() + grace;
        while !self.turns.is_empty() {
            tokio::select! {
                Some(id) = done_rx.recv() => { self.turns.remove(&id); }
                _ = sleep_until(deadline) => break,
            }
        }
        for (_, entry) in self.sessions.drain() {
            if let Ok(m) = Arc::try_unwrap(entry.session) {
                let _ = m.into_inner().release().await;
            }
        }
    }

    async fn handle(&mut self, msg: RunnerToSupervisor, done_tx: &mpsc::UnboundedSender<Uuid>) {
        match msg {
            RunnerToSupervisor::TurnDeliver(d) => self.deliver(*d, done_tx).await,
            RunnerToSupervisor::TurnMessage(m) => self.steer(m).await,
            RunnerToSupervisor::TurnSendNow(r) => self.send_now(r).await,
            RunnerToSupervisor::TurnAbort(r) => {
                if let Some(t) = self.turns.get(&r.turn_id) {
                    t.aborted.store(true, Ordering::SeqCst);
                    t.abort.notify_one();
                    let _ = t.session.lock().await.abort().await;
                }
            }
            RunnerToSupervisor::SessionRelease(r) => {
                if self
                    .turns
                    .values()
                    .any(|t| t.conversation_id == r.conversation_id)
                {
                    return; // a running turn is never interrupted for a release
                }
                if let Some(e) = self.sessions.remove(&r.conversation_id)
                    && let Ok(m) = Arc::try_unwrap(e.session)
                {
                    let _ = m.into_inner().release().await;
                }
            }
            RunnerToSupervisor::HomeSync(h) => self.home_sync(h).await,
            RunnerToSupervisor::MemoryWriteResult(r) => {
                if !r.ok
                    && let Ok(mut h) = self.home.lock()
                {
                    let why = r.error.as_ref().map_or("rejected", |e| e.message.as_str());
                    let why: String = why.chars().take(200).collect();
                    h.harvester.push_notice(format!(
                        "memory `{}` was not saved: {why}",
                        r.key.chars().take(128).collect::<String>()
                    ));
                }
            }
            RunnerToSupervisor::TurnNotice(_)
            | RunnerToSupervisor::ToolResult(_)
            | RunnerToSupervisor::FileResult(_) => {
                tracing::debug!("frame handled by SUP-002; ignored");
            }
            RunnerToSupervisor::Ack(_) | RunnerToSupervisor::Shutdown(_) => {}
            RunnerToSupervisor::Other(u) => {
                tracing::warn!(kind = u.kind.as_str(), "unknown frame kind dropped");
            }
        }
    }

    /// Scans `memory/` for agent edits and uploads them. Returns the number uploaded.
    async fn harvest(&self) -> usize {
        let home = self.home.clone();
        let writes = tokio::task::spawn_blocking(move || {
            home.lock()
                .map(|mut h| h.harvester.scan())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        let n = writes.len();
        for w in writes {
            self.send(SupervisorToRunner::MemoryWrite(w)).await;
        }
        n
    }

    async fn home_sync(&mut self, h: aurora_protocol::runner::HomeSync) {
        let home = self.home.clone();
        let hc = h.clone();
        let staged = tokio::task::spawn_blocking(move || {
            home.lock()
                .map_err(|_| "home state poisoned".to_owned())
                .and_then(|mut g| g.projector.apply_part(&hc).map_err(|e| e.to_string()))
        })
        .await;
        let generation = match staged {
            Ok(Ok(PartOutcome::Staged)) => return,
            Ok(Ok(PartOutcome::ReadyToCommit(g))) => g,
            Ok(Err(e)) => {
                tracing::warn!(generation = h.generation, error = %e, "home sync part failed");
                return;
            }
            Err(_) => return,
        };
        // Agent edits are uploaded before the canonical render supersedes them.
        self.harvest().await;
        let home = self.home.clone();
        let committed = tokio::task::spawn_blocking(move || {
            home.lock()
                .map_err(|_| "home state poisoned".to_owned())
                .and_then(|mut g| g.projector.commit(generation).map_err(|e| e.to_string()))
        })
        .await;
        match committed {
            Ok(Ok(())) => {
                self.ready = true;
                self.send(SupervisorToRunner::HomeSynced(HomeSynced { generation }))
                    .await;
            }
            Ok(Err(e)) => tracing::warn!(generation, error = %e, "home sync commit failed"),
            Err(_) => {}
        }
    }

    async fn fail_turn(&self, turn_id: Uuid, error_code: &str, message: &str) {
        self.send(SupervisorToRunner::TurnResult(TurnResult {
            turn_id,
            status: TurnStatus::Failed,
            error_code: Some(code(error_code)),
            error_message: Some(message.to_owned()),
            usage: None,
        }))
        .await;
    }

    async fn deliver(&mut self, d: TurnDeliver, done_tx: &mpsc::UnboundedSender<Uuid>) {
        if !self.ready {
            return self
                .fail_turn(d.turn_id, "not_ready", "home sync incomplete")
                .await;
        }
        if self
            .turns
            .values()
            .any(|t| t.conversation_id == d.conversation_id)
        {
            return self
                .fail_turn(
                    d.turn_id,
                    "turn_in_progress",
                    "conversation already has a running turn",
                )
                .await;
        }
        // Reuse the session unless the runner moved to a new epoch.
        if self
            .sessions
            .get(&d.conversation_id)
            .is_some_and(|e| e.epoch != d.session.epoch)
            && let Some(old) = self.sessions.remove(&d.conversation_id)
            && let Ok(m) = Arc::try_unwrap(old.session)
        {
            let _ = m.into_inner().release().await;
        }
        if !self.sessions.contains_key(&d.conversation_id) {
            match self.start_session(&d).await {
                Ok(e) => {
                    self.sessions.insert(d.conversation_id, e);
                }
                Err(e) => {
                    let c = classify_error(&e.code);
                    return self.fail_turn(d.turn_id, c, &e.message).await;
                }
            }
        }
        let Some(entry) = self.sessions.get(&d.conversation_id) else {
            return;
        };
        let handle = TurnHandle {
            conversation_id: d.conversation_id,
            session: entry.session.clone(),
            aborted: Arc::new(AtomicBool::new(false)),
            abort: Arc::new(Notify::new()),
            pending_steers: Arc::default(),
        };
        let task = TurnTask {
            turn_id: d.turn_id,
            conversation_id: d.conversation_id,
            epoch: d.session.epoch,
            credential: d.context_credential.expose().to_owned(),
            input: TurnInput {
                turn_id: d.turn_id,
                text: d.input.text,
                speaker_label: d.input.speaker_label,
                door: d.input.door,
                recall: d.input.recall,
                attachment_paths: d
                    .attachments
                    .iter()
                    .map(|a| {
                        self.cfg
                            .working_dir
                            .join(format!(".aurora/inbox/{}/{}", d.turn_id, a.filename))
                    })
                    .collect(),
                notices: {
                    let mut n = d.input.notices;
                    if let Ok(mut h) = self.home.lock() {
                        n.extend(h.harvester.take_notices());
                    }
                    n
                },
            },
            session: handle.session.clone(),
            cred: entry.cred.clone(),
            reported_ref: entry.reported_ref.clone(),
            aborted: handle.aborted.clone(),
            abort: handle.abort.clone(),
            pending_steers: handle.pending_steers.clone(),
            out: self.out.clone(),
            progress: self.cfg.progress_interval,
        };
        self.turns.insert(d.turn_id, handle);
        let done = done_tx.clone();
        let turn_id = task.turn_id;
        tokio::spawn(async move {
            task.run().await;
            let _ = done.send(turn_id);
        });
    }

    async fn start_session(
        &self,
        d: &TurnDeliver,
    ) -> Result<SessionEntry, crate::harness::HarnessError> {
        let cred = Credential::new("x", self.cfg.boot_token.clone());
        let bind = SocketAddr::new(self.cfg.proxy_ip, 0);
        let proxy = spawn_proxy(
            bind,
            self.cfg.gateway_proxy.clone(),
            cred.clone(),
            ProxyMode::Mitm,
        )
        .await
        .map_err(|e| {
            crate::harness::HarnessError::new("harness_failed", format!("session proxy: {e}"))
        })?;
        let url = proxy.url();
        let mut env: Vec<(String, String)> =
            ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"]
                .iter()
                .map(|k| ((*k).to_owned(), url.clone()))
                .collect();
        env.push(("NO_PROXY".into(), "localhost,127.0.0.1".into()));
        env.extend(d.model.client_env.iter().cloned());
        let session = self
            .adapter
            .start_session(SessionOptions {
                conversation_id: d.conversation_id,
                working_dir: self.cfg.working_dir.clone(),
                resume_ref: d.session.resume_ref.clone(),
                model: d.model.clone(),
                proxy_url: url,
                mcp_bridge: McpBridgeSpec {
                    command: self.cfg.mcp_bridge_command.clone(),
                },
                env,
            })
            .await?;
        Ok(SessionEntry {
            session: Arc::new(Mutex::new(session)),
            cred,
            epoch: d.session.epoch,
            reported_ref: Arc::new(StdMutex::new(d.session.resume_ref.clone())),
            _proxy: proxy,
        })
    }

    async fn steer(&self, m: TurnMessage) {
        let refuse = SteerResult {
            followup_id: m.followup_id,
            outcome: SteerOutcome::Refused,
        };
        let Some(t) = self.turns.get(&m.turn_id).filter(|_| self.caps.steer) else {
            return self.send(SupervisorToRunner::SteerResult(refuse)).await;
        };
        if let Ok(mut p) = t.pending_steers.lock() {
            p.insert(m.followup_id);
        }
        let ack = t
            .session
            .lock()
            .await
            .steer(SteerInput {
                followup_id: m.followup_id,
                text: m.text,
                speaker_label: m.speaker_label,
            })
            .await;
        if !matches!(ack, Ok(SteerAck::Accepted)) {
            if let Ok(mut p) = t.pending_steers.lock() {
                p.remove(&m.followup_id);
            }
            self.send(SupervisorToRunner::SteerResult(refuse)).await;
        }
        // Accepted: `delivered` on message.joined, `missed` at turn end (turn task).
    }

    async fn send_now(&self, r: TurnRef) {
        let outcome = match self.turns.get(&r.turn_id) {
            None => SendNowOutcome::NothingRunning,
            Some(_) if !self.caps.background_tool => SendNowOutcome::Unsupported,
            Some(t) => t
                .session
                .lock()
                .await
                .background_running_tool()
                .await
                .unwrap_or(SendNowOutcome::NothingRunning),
        };
        self.send(SupervisorToRunner::SendNowResult(SendNowResult {
            turn_id: r.turn_id,
            outcome,
        }))
        .await;
    }
}

struct TurnTask {
    turn_id: Uuid,
    conversation_id: Uuid,
    epoch: u64,
    credential: String,
    input: TurnInput,
    session: SharedSession,
    cred: Arc<Credential>,
    reported_ref: Arc<StdMutex<Option<String>>>,
    aborted: Arc<AtomicBool>,
    abort: Arc<Notify>,
    pending_steers: Arc<StdMutex<HashSet<Uuid>>>,
    out: ChannelSender,
    progress: Duration,
}

impl TurnTask {
    async fn send(&self, msg: SupervisorToRunner) {
        let _ = self.out.send(msg).await;
    }

    async fn report_session(&mut self, new_ref: Option<String>) {
        let changed = new_ref.filter(|r| {
            self.reported_ref.lock().is_ok_and(|mut g| {
                let differs = g.as_deref() != Some(r.as_str());
                if differs {
                    *g = Some(r.clone());
                }
                differs
            })
        });
        if let Some(r) = changed {
            self.send(SupervisorToRunner::Session(SessionReport {
                conversation_id: self.conversation_id,
                session_ref: r,
                epoch: self.epoch,
            }))
            .await;
        }
    }

    async fn run(mut self) {
        let turn_ref = TurnRef {
            turn_id: self.turn_id,
        };
        self.cred
            .set_context(Some(std::mem::take(&mut self.credential)));
        self.send(SupervisorToRunner::TurnStarted(turn_ref.clone()))
            .await;

        let started = {
            let mut s = self.session.lock().await;
            s.run_turn(self.input.clone()).await
        };
        let (status, err_code, err_msg, usage) = match started {
            Err(e) => (
                TurnStatus::Failed,
                Some(classify_error(&e.code)),
                Some(e.message),
                None,
            ),
            Ok(stream) => self.pump(stream, &turn_ref).await,
        };
        self.cred.set_context(None);

        let current_ref = self.session.lock().await.session_ref();
        self.report_session(current_ref).await;
        let missed: Vec<Uuid> = self
            .pending_steers
            .lock()
            .map(|mut p| p.drain().collect())
            .unwrap_or_default();
        for followup_id in missed {
            self.send(SupervisorToRunner::SteerResult(SteerResult {
                followup_id,
                outcome: SteerOutcome::Missed,
            }))
            .await;
        }
        self.send(SupervisorToRunner::TurnResult(TurnResult {
            turn_id: self.turn_id,
            status,
            error_code: err_code.map(code),
            error_message: err_msg.map(|m| m.chars().take(MAX_ERROR_MESSAGE).collect()),
            usage,
        }))
        .await;
    }

    #[allow(clippy::type_complexity)]
    async fn pump(
        &mut self,
        mut stream: mpsc::Receiver<HarnessEvent>,
        turn_ref: &TurnRef,
    ) -> (
        TurnStatus,
        Option<&'static str>,
        Option<String>,
        Option<serde_json::Value>,
    ) {
        let mut n = 0u64;
        let mut tick = interval(self.progress);
        tick.tick().await;
        let mut abort_deadline: Option<Instant> = None;
        loop {
            let deadline = async {
                match abort_deadline {
                    Some(d) => sleep_until(d).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                ev = stream.recv() => {
                    let aborted = self.aborted.load(Ordering::SeqCst);
                    match ev {
                        None => return if aborted {
                            (TurnStatus::Aborted, None, None, None)
                        } else {
                            (TurnStatus::Failed, Some("harness_failed"), Some("harness ended without a result".into()), None)
                        },
                        Some(HarnessEvent::Done { usage }) => return if aborted {
                            (TurnStatus::Aborted, None, None, usage)
                        } else {
                            (TurnStatus::Done, None, None, usage)
                        },
                        Some(HarnessEvent::Error { code: c, message }) => return if aborted {
                            (TurnStatus::Aborted, None, None, None)
                        } else {
                            (TurnStatus::Failed, Some(classify_error(&c)), Some(message), None)
                        },
                        Some(HarnessEvent::SessionRef { session_ref }) => {
                            self.report_session(Some(session_ref)).await;
                        }
                        Some(ev) => {
                            if let HarnessEvent::MessageJoined { followup_id } = &ev {
                                let was_pending = self.pending_steers.lock().is_ok_and(|mut p| p.remove(followup_id));
                                if was_pending {
                                    self.send(SupervisorToRunner::SteerResult(SteerResult {
                                        followup_id: *followup_id,
                                        outcome: SteerOutcome::Delivered,
                                    })).await;
                                }
                            }
                            if let Some(wire) = ev.to_wire() {
                                n += 1;
                                self.send(SupervisorToRunner::TurnEvent(TurnEvent { turn_id: self.turn_id, n, event: wire })).await;
                            }
                        }
                    }
                }
                _ = tick.tick() => {
                    self.send(SupervisorToRunner::TurnProgress(turn_ref.clone())).await;
                }
                _ = self.abort.notified(), if abort_deadline.is_none() => {
                    abort_deadline = Some(Instant::now() + ABORT_GRACE);
                }
                _ = deadline => return (TurnStatus::Aborted, None, None, None),
            }
        }
    }
}

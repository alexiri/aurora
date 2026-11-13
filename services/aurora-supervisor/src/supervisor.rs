//! Supervisor core: turn execution over the channel and a harness adapter
//! (`docs/12-supervisor-harness.md` §Process model and startup, §Harness interface).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use aurora_protocol::runner::{
    Code, Gate, GateOutcome, HarnessId, HarnessInfo, HomeSynced, PortsState, SendNowOutcome,
    SendNowResult, SessionPurged, SessionReport, SteerOutcome, SteerResult, SupervisorReady,
    TurnDeliver, TurnEvent, TurnMessage, TurnRef, TurnResult, TurnStatus,
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
use crate::inbox::Inbox;
use crate::process::{ProcessConfig, ProcessManager};
use crate::proxy::{Credential, GatewayUpstream, ProxyHandle, ProxyMode, spawn_proxy};
use crate::tools::{ToolHub, ToolsConfig};

const ABORT_GRACE: Duration = Duration::from_secs(5);
const MAX_ERROR_MESSAGE: usize = 2_000;

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub working_dir: PathBuf,
    /// Gateway proxy (always dialled with TLS, `docs/12 §Loopback proxies`).
    pub gateway_proxy: GatewayUpstream,
    pub boot_token: String,
    pub proxy_ip: IpAddr,
    pub progress_interval: Duration,
    pub shutdown_grace: Duration,
    pub version: String,
    /// Base bridge command (e.g. `[exe, "mcp-bridge"]`); `--session <conversation id>` is appended.
    pub mcp_bridge_command: Vec<String>,
    pub harvest_interval: Duration,
    pub tools: ToolsConfig,
    pub process: ProcessConfig,
    /// (P2-DBL-2, D-89) Loopback port of the database relay (`crate::pgrelay`); the libpq service
    /// file points at it. `None`: no service file, no relay, no `PGSERVICEFILE`. Port 0 binds any
    /// free port (tests that exercise the relay directly).
    pub pg: Option<u16>,
}

impl SupervisorConfig {
    pub fn new(gateway_proxy: GatewayUpstream, boot_token: impl Into<String>) -> Self {
        Self {
            working_dir: PathBuf::from("/workspace"),
            gateway_proxy,
            boot_token: boot_token.into(),
            proxy_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            progress_interval: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(5),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            mcp_bridge_command: Vec::new(),
            harvest_interval: Duration::from_secs(10),
            tools: ToolsConfig::default(),
            process: ProcessConfig::new(PathBuf::from("/workspace")),
            pg: None,
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
    hub: Arc<ToolHub>,
    procs: Arc<ProcessManager>,
    inbox: Inbox,
    /// (RFE-010) Session purges received in the home sync, executed before any turn is accepted.
    pending_purges: Vec<(Uuid, String, Option<String>)>,
    /// (P4-HAR, D-153) Adapter of another harness, for the purge of a session it created.
    purge_lookup: PurgeLookup,
    /// (P2-DBL-2) The loopback PostgreSQL relay, running while the home sync carries databases.
    db_relay: Option<ProxyHandle>,
    /// The relay's credential: boot token, turn context credential during exactly one turn.
    db_turns: Arc<crate::pgrelay::DbTurns>,
    /// (P4-PRV-1, D-144) The active preview shares, from the `previews` home-sync part: held in
    /// memory only (the home is agent-writable), replaced whole on every sync
    /// (`12 §Previews`): `share_id -> (port, start_command, cwd)`.
    preview_shares: HashMap<Uuid, PreviewShareEntry>,
    /// (P4-PRV-2, D-147) The preview byte-stream engine (`16 §W18`): the allow-list is this
    /// share list, never the port discovery.
    streams: crate::streams::StreamsHandle,
    /// (P4-TERM, D-161) The terminal PTY engine (`16 §W19`): gated on the boot document's
    /// `terminal` capability.
    ptys: crate::pty::PtyHandle,
    /// (P4-PRV-2) Last reported listening loopback ports (`ports.state`); sent only on change.
    reported_ports: BTreeSet<u16>,
}

/// One share of the in-memory allow-list.
#[derive(Debug, Clone)]
pub struct PreviewShareEntry {
    pub port: u16,
    pub start_command: Option<String>,
    pub cwd: Option<String>,
}

/// Resolves a harness id to its adapter (the compile-time registry, `crate::adapters`).
pub type PurgeLookup = Arc<
    dyn Fn(&str) -> Result<Arc<dyn HarnessAdapter>, crate::harness::HarnessError> + Send + Sync,
>;

/// Home projection state: staged sync, plus the memory harvest bookkeeping.
struct HomeCtl {
    projector: HomeProjector,
    harvester: Harvester,
}

/// The frame a supervisor sends **instead of `ready`** when it cannot run the harness the boot
/// document names (unknown id, runtime missing from the image): the runner forwards it as the
/// sandbox's `failed` status with `reason_code` (`harness_unsupported`), and the queued turns of
/// the agent fail with that reason (`12 §Harness selection`, SEC-66). No fallback harness exists.
pub fn boot_refusal(reason_code: &str, message: &str) -> SupervisorToRunner {
    SupervisorToRunner::SandboxStatus(aurora_protocol::runner::SandboxStatus {
        state: aurora_protocol::runner::SandboxState::Failed,
        container_ref: None,
        reason_code: Some(code(reason_code)),
        // (FIX-025) one line, bounded, credentials masked.
        message: Some(aurora_core::sanitize::reason(message, 500)),
        observed: None,
        events: Vec::new(),
    })
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
        streams: crate::streams::StreamsHandle,
        ptys: crate::pty::PtyHandle,
    ) -> Self {
        let caps = adapter.capabilities();
        let home = Arc::new(StdMutex::new(HomeCtl {
            projector: HomeProjector::new(HomeConfig {
                root: cfg.working_dir.clone(),
                skills_dir: caps.skills_dir.clone(),
                instruction_files: caps.instruction_files.clone(),
                pg: cfg.pg,
            }),
            harvester: Harvester::new(&cfg.working_dir),
        }));
        let mut process = cfg.process.clone();
        if cfg.pg.is_some() {
            process.env.extend(crate::pg::env(&cfg.working_dir));
        }
        let procs = ProcessManager::new(process, out.clone());
        let mut tools = cfg.tools.clone();
        tools.home = cfg.working_dir.clone();
        let hub = ToolHub::new(
            tools,
            cfg.working_dir.join(".aurora/tools.json"),
            out.clone(),
            procs.clone(),
        );
        Self {
            db_turns: crate::pgrelay::DbTurns::new(cfg.boot_token.clone()),
            db_relay: None,
            inbox: Inbox::new(&cfg.working_dir),
            home,
            hub,
            procs,
            cfg,
            out,
            adapter,
            caps,
            sessions: HashMap::new(),
            turns: HashMap::new(),
            ready: false,
            pending_purges: Vec::new(),
            purge_lookup: Arc::new(crate::adapters::adapter_for),
            preview_shares: HashMap::new(),
            streams,
            ptys,
            reported_ports: BTreeSet::new(),
        }
    }

    /// Replaces how another harness's adapter is found for a purge (tests).
    #[must_use]
    pub fn purge_lookup(mut self, f: PurgeLookup) -> Self {
        self.purge_lookup = f;
        self
    }

    /// Starts the loopback database relay when the installed home carries databases and stops it
    /// when none remain (boot and after every committed sync).
    async fn sync_db_relay(&mut self) {
        let Some(port) = self.cfg.pg else { return };
        let path = self.cfg.working_dir.join(".aurora/render_inputs.json");
        let any = std::fs::read(path)
            .map(|b| !crate::home::installed_databases(&b).is_empty())
            .unwrap_or(false);
        if !any {
            self.db_relay = None;
        } else if self.db_relay.is_none() {
            let bind = SocketAddr::new(self.cfg.proxy_ip, port);
            match crate::pgrelay::spawn_db_relay(
                bind,
                self.cfg.gateway_proxy.clone(),
                self.db_turns.clone(),
            )
            .await
            {
                Ok(h) => self.db_relay = Some(h),
                Err(e) => tracing::warn!(error = %e, "database relay did not start"),
            }
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
        self.procs.spawn_background();
        if let Some(path) = self.cfg.tools.socket.clone() {
            let hub = self.hub.clone();
            tokio::spawn(async move {
                if let Err(e) = hub.serve(&path).await {
                    tracing::error!(error = %e, "tools socket failed");
                }
            });
        }
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
        self.sync_db_relay().await;
        let mut harvest_tick = interval(self.cfg.harvest_interval);
        harvest_tick.tick().await;
        loop {
            tokio::select! {
                // (P4-PRV-2) The harvest tick also drives port discovery (`ports.state`): the
                // first tick fires immediately, so a boot's listeners are reported without
                // waiting a full interval (`11 §Port discovery`).
                _ = harvest_tick.tick() => {
                    self.harvest().await;
                    self.send_ports().await;
                }
                msg = inbound.recv() => {
                    let Some(msg) = msg else { break };
                    if let RunnerToSupervisor::Shutdown(s) = &msg {
                        let grace = s.grace_secs.map_or(self.cfg.shutdown_grace, |g| Duration::from_secs(u64::from(g)));
                        self.shutdown(grace, &mut done_rx).await;
                        break;
                    }
                    self.handle(msg, &done_tx).await;
                }
                Some(turn_id) = done_rx.recv() => { self.finish_turn(turn_id); }
            }
        }
    }

    fn finish_turn(&mut self, turn_id: Uuid) {
        if let Some(t) = self.turns.remove(&turn_id) {
            self.hub.set_turn(t.conversation_id, None);
        }
    }

    async fn shutdown(&mut self, grace: Duration, done_rx: &mut mpsc::UnboundedReceiver<Uuid>) {
        for t in self.turns.values() {
            t.aborted.store(true, Ordering::SeqCst);
            t.abort.notify_one();
            let _ = t.session.lock().await.abort().await;
        }
        // Every terminal ends with the sandbox (`12 §Terminal`, `16 §W19`).
        self.ptys.close_all().await;
        let deadline = Instant::now() + grace;
        while !self.turns.is_empty() {
            tokio::select! {
                Some(id) = done_rx.recv() => { self.finish_turn(id); }
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
            RunnerToSupervisor::SessionPurge(p) => {
                self.purge(p.conversation_id, p.session_ref, p.harness)
                    .await;
            }
            RunnerToSupervisor::HomeSync(h) => self.home_sync(h).await,
            RunnerToSupervisor::PreviewStart(p) => self.preview_start(p.port).await,
            // (P4-PRV-2, W18) Stream frames ride the unsequenced side-channel in production;
            // a sequenced one is still honoured (it can only come from a test harness).
            RunnerToSupervisor::StreamOpen(o) => self.stream_open(o).await,
            RunnerToSupervisor::StreamCredit(c) => {
                let _ = self.streams.credit(c.stream_id, c.bytes).await;
            }
            RunnerToSupervisor::StreamClose(c) => {
                let _ = self.streams.stream_closed(c.stream_id).await;
            }
            // (P4-TERM, W19) PTY frames beside the streams; a sequenced one is honoured only
            // from a test harness, like the stream frames.
            RunnerToSupervisor::PtyOpen(o) => {
                let _ = self.ptys.pty_open(o).await;
            }
            RunnerToSupervisor::PtyResize(r) => {
                let _ = self.ptys.resize(r.pty_id, r.cols, r.rows).await;
            }
            RunnerToSupervisor::PtyCredit(c) => {
                let _ = self.ptys.credit(c.pty_id, c.bytes).await;
            }
            RunnerToSupervisor::PtyClose(c) => {
                let _ = self.ptys.pty_closed(c.pty_id).await;
            }
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
            RunnerToSupervisor::ToolResult(r) => self.hub.on_result(r),
            RunnerToSupervisor::FileIn(f) => self.inbox.accept(f).await,
            RunnerToSupervisor::FileResult(r) => self.hub.on_file_result(r),
            RunnerToSupervisor::TurnNotice(_) => {
                tracing::debug!("frame handled by SUP-002; ignored");
            }
            RunnerToSupervisor::Ack(_) | RunnerToSupervisor::Shutdown(_) => {}
            RunnerToSupervisor::Other(u) => {
                tracing::warn!(kind = u.kind.as_str(), "unknown frame kind dropped");
            }
        }
    }

    /// (P4-PRV-2) Scans `/proc/net/tcp{,6}` for LISTEN loopback ports and reports them via
    /// `ports.state` only on change (`11 §Port discovery`). Runs on the harvest tick (the
    /// first fires immediately); `listening_ports` is what the dashboard's "Add share"
    /// dialog offers and what derives a share's `listening` status. Informational only:
    /// the stream allow-list is the share list, never this discovery.
    async fn send_ports(&mut self) {
        let ports = tokio::task::spawn_blocking(crate::ports::scan)
            .await
            .unwrap_or_default();
        if ports != self.reported_ports {
            self.reported_ports = ports.clone();
            self.send(SupervisorToRunner::PortsState(PortsState {
                ports: ports.into_iter().collect(),
            }))
            .await;
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

    /// (RFE-010) Abort and release any live session of the conversation, delete the stored
    /// session files and report `session.purged`. A failure keeps the purge pending upstream.
    ///
    /// (P4-HAR, D-153) `harness` is the harness that created the session: after a switch its
    /// adapter (looked up in the compile-time registry) deletes the files, not this sandbox's own.
    /// An adapter whose runtime is gone fails the purge, which stays pending.
    async fn purge(&mut self, conversation_id: Uuid, session_ref: String, harness: Option<String>) {
        let foreign = harness.as_deref().filter(|h| *h != self.adapter.id());
        let adapter: Result<Arc<dyn HarnessAdapter>, crate::harness::HarnessError> = match foreign {
            Some(h) => (self.purge_lookup)(h),
            None => Ok(self.adapter.clone()),
        };
        let live: Vec<Uuid> = if foreign.is_some() {
            Vec::new() // live sessions belong to this sandbox's own harness
        } else {
            self.turns
                .iter()
                .filter(|(_, t)| t.conversation_id == conversation_id)
                .map(|(id, _)| *id)
                .collect()
        };
        for id in live {
            if let Some(t) = self.turns.get(&id) {
                t.aborted.store(true, Ordering::SeqCst);
                t.abort.notify_one();
                let _ = t.session.lock().await.abort().await;
            }
            // The turn task ends on its terminal event; the session is dropped below regardless.
            self.finish_turn(id);
        }
        if foreign.is_none()
            && let Some(e) = self.sessions.remove(&conversation_id)
            && let Ok(m) = Arc::try_unwrap(e.session)
        {
            let _ = m.into_inner().release().await;
        }
        let done = match adapter {
            Ok(a) => a.purge_session(&self.cfg.working_dir, &session_ref).await,
            Err(e) => Err(e),
        };
        let ok = match done {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(%conversation_id, code = %e.code, "session purge failed");
                false
            }
        };
        self.pending_purges
            .retain(|(c, r, _)| !(*c == conversation_id && *r == session_ref && ok));
        self.send(SupervisorToRunner::SessionPurged(SessionPurged {
            conversation_id,
            session_ref,
            ok,
        }))
        .await;
    }

    async fn run_pending_purges(&mut self) {
        for (c, r, h) in std::mem::take(&mut self.pending_purges) {
            self.purge(c, r, h).await;
        }
    }

    async fn home_sync(&mut self, h: aurora_protocol::runner::HomeSync) {
        // (P4-PRV-1, D-144) The `previews` part never touches the home (it is agent-writable):
        // its single entry replaces the in-memory share list. An older API sends nothing.
        if h.part.kind == aurora_protocol::runner::HomeSyncKind::Previews {
            self.preview_shares = h
                .part
                .entries
                .iter()
                .filter_map(|e| e.get("shares")?.as_array())
                .flatten()
                .filter_map(|s| {
                    let id = s.get("share_id")?.as_str()?.parse::<Uuid>().ok()?;
                    let port = s.get("port")?.as_u64()?;
                    if port == 0 || port > u16::MAX as u64 {
                        return None;
                    }
                    Some((
                        id,
                        PreviewShareEntry {
                            port: port as u16,
                            start_command: s
                                .get("start_command")
                                .and_then(|v| v.as_str())
                                .map(str::to_owned),
                            cwd: s.get("cwd").and_then(|v| v.as_str()).map(str::to_owned),
                        },
                    ))
                })
                .collect();
            // (P4-PRV-2, D-147) The stream allow-list is the share list: replaced as a whole,
            // streams on ports that left it are closed.
            self.streams.set_shares(&self.preview_shares);
            tracing::debug!(
                shares = self.preview_shares.len(),
                "preview share list applied"
            );
            return;
        }
        if h.part.kind == aurora_protocol::runner::HomeSyncKind::RenderInputs {
            for e in &h.part.entries {
                let Some(list) = e.get("purges").and_then(serde_json::Value::as_array) else {
                    continue;
                };
                for p in list {
                    let c = p
                        .get("conversation_id")
                        .and_then(|v| v.as_str())
                        .and_then(|v| v.parse::<Uuid>().ok());
                    let r = p.get("session_ref").and_then(|v| v.as_str());
                    let harness = p.get("harness").and_then(|v| v.as_str()).map(str::to_owned);
                    if let (Some(c), Some(r)) = (c, r)
                        && !self
                            .pending_purges
                            .iter()
                            .any(|(pc, pr, _)| *pc == c && pr == r)
                    {
                        self.pending_purges.push((c, r.to_owned(), harness));
                    }
                }
            }
        }
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
                // Deleted conversations' session files go before the sandbox can run any turn.
                self.run_pending_purges().await;
                self.sync_db_relay().await;
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

    /// (P4-PRV-1, D-145) On-demand start of a share's app: the forwarded `preview.start { port }`
    /// frame looks the port up in the **in-memory share list** (never the home, never the frame):
    /// with a `start_command` and the port not listening, the command starts as a background
    /// process `preview:<port>` (an ordinary process; `12 §Previews`). An unknown port, a share
    /// without a command or a port that is already listening is a no-op. The share list is empty
    /// while the agent is frozen, so a frozen sandbox never starts anything.
    async fn preview_start(&mut self, port: u16) {
        let Some(share) = self
            .preview_shares
            .values()
            .find(|s| s.port == port)
            .cloned()
        else {
            tracing::debug!(port, "preview.start for an unknown port; ignored");
            return;
        };
        let Some(command) = share.start_command.as_deref().map(str::trim) else {
            tracing::debug!(port, "preview.start for a share without a command; ignored");
            return;
        };
        if command.is_empty() {
            return;
        }
        // Already listening (the supervisor's own listeners are never in the share list).
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            tracing::debug!(port, "preview port already listening; ignored");
            return;
        }
        match self
            .procs
            .start(&format!("preview:{port}"), command, share.cwd.as_deref())
        {
            Ok(id) => tracing::info!(%port, %id, "preview app started"),
            Err(e) => {
                // Logged only: the command is the agent's own; the next request retries within
                // the attempt window (`11 §Port previews`).
                tracing::warn!(%port, error = %e, "preview start command failed");
            }
        }
    }

    /// (P4-PRV-2, D-147, `16 §W18`) A sequenced `stream.open` (tests / older runners only —
    /// in production stream frames ride the unsequenced side-channel straight into the engine).
    /// The engine enforces the share-list allow-list and answers `stream.opened`/`refused`.
    async fn stream_open(&mut self, o: aurora_protocol::runner::StreamOpen) {
        let _ = self.streams.stream_open(o).await;
    }

    async fn deliver(&mut self, d: TurnDeliver, done_tx: &mpsc::UnboundedSender<Uuid>) {
        // The runner sent this turn's `file.in` frames first (S-11); settle them now so partial
        // files never outlive the delivery, even when the turn is refused below.
        let inbound = self.inbox.resolve(d.turn_id, &d.attachments).await;
        if !self.ready {
            return self
                .fail_turn(d.turn_id, "not_ready", "home sync incomplete")
                .await;
        }
        self.run_pending_purges().await;
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
                attachment_paths: inbound.paths,
                notices: {
                    let mut n = d.input.notices;
                    n.extend(inbound.notices);
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
            gate: d.gate,
            procs: self.procs.clone(),
            db_turns: self.db_turns.clone(),
        };
        self.hub.set_turn(d.conversation_id, Some(d.turn_id));
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
        if self.cfg.pg.is_some() {
            env.extend(crate::pg::env(&self.cfg.working_dir));
        }
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
                    command: if self.cfg.mcp_bridge_command.is_empty() {
                        Vec::new()
                    } else {
                        let mut c = self.cfg.mcp_bridge_command.clone();
                        c.extend(["--session".to_owned(), d.conversation_id.to_string()]);
                        c
                    },
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
    gate: Option<Gate>,
    procs: Arc<ProcessManager>,
    db_turns: Arc<crate::pgrelay::DbTurns>,
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
        // A gated turn stays `dispatched` while its gate runs; only a waking gate proceeds, and the
        // API finalizes quiet/failed turns from `gate.result` (no `turn.*` frames follow).
        if let Some(g) = self.gate.take() {
            let r = self
                .procs
                .run_gate(self.turn_id, &g.command, g.timeout_secs)
                .await;
            let wake = r.outcome == GateOutcome::Wake;
            if wake && let Some(o) = &r.output {
                self.input.notices.push(format!("Gate output:\n{o}"));
            }
            self.send(SupervisorToRunner::GateResult(r)).await;
            if !wake {
                return;
            }
        }
        let turn_ref = TurnRef {
            turn_id: self.turn_id,
        };
        self.db_turns.begin(self.turn_id, self.credential.clone());
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
        self.db_turns.end(self.turn_id);

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

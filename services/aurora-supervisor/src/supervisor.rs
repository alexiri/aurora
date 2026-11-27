//! Supervisor core: turn execution over the channel and a harness adapter
//! (`docs/12-supervisor-harness.md` §Process model and startup, §Harness interface).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use aurora_protocol::runner::{
    Code, Gate, GateOutcome, HarnessId, HarnessInfo, HomeSynced, NumberedEvent, PortsState,
    SendNowOutcome, SendNowResult, SessionPurged, SessionReleased, SessionReport, SteerOutcome,
    SteerResult, SupervisorReady, TurnDeliver, TurnEvent, TurnEvents, TurnMessage, TurnRef,
    TurnResult, TurnStatus,
};
use aurora_protocol::supervisor::{RunnerToSupervisor, SupervisorToRunner};
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::{Instant, interval, sleep_until};
use uuid::Uuid;

use crate::channel::ChannelSender;
use crate::harness::{
    BackgroundWork, HarnessAdapter, HarnessCapabilities, HarnessEvent, HarnessSession,
    McpBridgeSpec, SessionOptions, SteerAck, SteerInput, TurnInput,
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
    /// (FIX-265) The longest one release request holds the supervisor's loop, however many
    /// sessions it covers. What is not released by then is reported as not stopped and goes on
    /// in its own task. Longer than the grace a process that ignores HUP is given.
    pub release_deadline: Duration,
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
    /// (FIX-138 item 1b) `/run/aurora/harness.env` for the terminal's harness wrappers: the
    /// agent's own harness's section is rewritten at each session start (the terminal open
    /// rewrites the sections too; the state is shared with the PTY engine). `None` (tests): no file.
    pub harness_env: Option<Arc<crate::harness_env::HarnessEnv>>,
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
            release_deadline: Duration::from_secs(4),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            mcp_bridge_command: Vec::new(),
            harvest_interval: Duration::from_secs(10),
            tools: ToolsConfig::default(),
            process: ProcessConfig::new(PathBuf::from("/workspace")),
            pg: None,
            harness_env: None,
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
    /// (FIX-253, part B) Conversations whose idle child is being watched, with the turn the
    /// watch follows: `conversation -> anchor turn`. The drain reports the watch's events
    /// against that anchor (`turn.idle.events`); the entry goes when the watch is done. With
    /// the turn: the number of the watch's next event.
    idle_turns: Arc<StdMutex<HashMap<Uuid, (Uuid, u64)>>>,
    /// (FIX-265) The background list last reported for each conversation that ever had one:
    /// from then on every change is reported, the empty list too.
    reported_background: Arc<StdMutex<HashMap<Uuid, Vec<BackgroundWork>>>>,
    /// (FIX-265) Sessions that are not released yet, because their release failed or takes
    /// longer than [`SupervisorConfig::release_deadline`]: the conversation's next
    /// `session.release` goes on with them.
    unreleased: Vec<(Uuid, Unreleased)>,
    /// (FIX-258) Conversations with a session that was not released within the deadline (a
    /// release that answered `ok: false`, or a replaced session) and that were not told
    /// since: once nothing of theirs runs, `session.released { ok: true }` is sent unasked.
    owes_released: HashSet<Uuid>,
    /// (FIX-258) Conversations whose release task died: nothing can say any more that all of
    /// theirs ended, so they are not told unasked until a request was answered `ok: true`.
    lost_releases: HashSet<Uuid>,
}

/// How often the supervisor looks whether an unfinished release has finished.
const LATE_RELEASE_TICK: Duration = Duration::from_millis(250);

/// A session the supervisor is done with but has not seen released.
enum Unreleased {
    /// Its release is still running, in a task that gives the session back if it fails.
    Running(Releasing),
    /// Its release failed.
    Failed(SharedSession),
}

/// [`release_shared`] in its own task, so that nobody has to wait for it to the end:
/// `None` once released.
type Releasing = tokio::task::JoinHandle<Option<SharedSession>>;

fn start_release(session: SharedSession) -> Releasing {
    tokio::spawn(async move { release_shared(session).await.1 })
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
        let mut projector = HomeProjector::new(HomeConfig {
            root: cfg.working_dir.clone(),
            skills_dir: Some(caps.skills_dir.clone()),
            instruction_files: caps.instruction_files.clone(),
            pg: cfg.pg,
            // (C-653) A supervisor-local fact, computed once at start: the wrapper is there.
            agent_cli: crate::agent_cli::agent_cli_available(std::path::Path::new(
                crate::agent_cli::WRAPPER,
            )),
        });
        // (FIX-258) The same rule as for its processes: a root supervisor runs them as this uid.
        if rustix::process::getuid().is_root()
            && let Some(uid) = cfg.process.uid
        {
            projector.set_agent_uid(uid);
        }
        let home = Arc::new(StdMutex::new(HomeCtl {
            projector,
            harvester: Harvester::new(&cfg.working_dir),
        }));
        let mut process = cfg.process.clone();
        // (P5-SKILLDIR, D-454) Set last, so it wins over an image or profile value.
        process.env.retain(|(k, _)| k != "SKILLS_DIR");
        process.env.push((
            "SKILLS_DIR".into(),
            crate::harness::skills_dir_abs(&cfg.working_dir, &caps.skills_dir),
        ));
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
            idle_turns: Arc::new(StdMutex::new(HashMap::new())),
            reported_background: Arc::default(),
            unreleased: Vec::new(),
            owes_released: HashSet::new(),
            lost_releases: HashSet::new(),
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

    /// The background-process manager (FIX-114: `main` forwards SIGTERM to its groups at exit).
    pub fn process_manager(&self) -> Arc<ProcessManager> {
        self.procs.clone()
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
        // Its own interval: it keeps its beat however busy the other arms are.
        let mut late_tick = interval(LATE_RELEASE_TICK);
        late_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = late_tick.tick(), if !self.owes_released.is_empty() => {
                    self.acknowledge_late_releases().await;
                }
                // (P4-PRV-2) The harvest tick also drives port discovery (`ports.state`): the
                // first tick fires immediately, so a boot's listeners are reported without
                // waiting a full interval (`11 §Port discovery`).
                _ = harvest_tick.tick() => {
                    self.harvest().await;
                    self.send_ports().await;
                    // (FIX-253, part B) What the watched idle children did since the last tick.
                    self.drain_idle().await;
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

    /// (FIX-253, part B) Reports what the watched idle children did since the last drain, as
    /// `turn.idle.events` anchored on the turn each watch follows, with a `background` entry
    /// when the list changed (FIX-265). A watch whose reader stopped, whose background work is
    /// gone and that has nothing left to report is dropped.
    async fn drain_idle(&mut self) {
        let anchors: Vec<(Uuid, (Uuid, u64))> = self
            .idle_turns
            .lock()
            .map(|m| m.iter().map(|(c, w)| (*c, *w)).collect())
            .unwrap_or_default();
        for (conversation_id, (turn_id, mut next)) in anchors {
            // A running turn reports for itself, at its end.
            if self
                .turns
                .values()
                .any(|t| t.conversation_id == conversation_id)
            {
                continue;
            }
            let Some(entry) = self.sessions.get(&conversation_id) else {
                if let Ok(mut m) = self.idle_turns.lock() {
                    m.remove(&conversation_id);
                }
                continue;
            };
            let mut s = entry.session.lock().await;
            // Read before the drain: a watch that was over then has nothing more to add.
            let watching = s.idle_watching();
            let events: Vec<HarnessEvent> = s.take_idle_events();
            let work = s.background_work();
            drop(s);
            let changed = self.reported_background.lock().is_ok_and(|mut m| {
                m.get_mut(&conversation_id).is_some_and(|reported| {
                    let changed = *reported != work;
                    reported.clone_from(&work);
                    changed
                })
            });
            let mut wire = number_idle(&events, &mut next);
            if changed {
                wire.push(NumberedEvent {
                    n: next,
                    event: background_wire(&work),
                });
                next += 1;
            }
            if wire.is_empty() {
                if work.is_empty()
                    && !watching
                    && let Ok(mut m) = self.idle_turns.lock()
                {
                    m.remove(&conversation_id);
                }
                continue;
            }
            if let Ok(mut m) = self.idle_turns.lock()
                && let Some(watch) = m.get_mut(&conversation_id)
                && watch.0 == turn_id
            {
                watch.1 = next;
            }
            self.send(SupervisorToRunner::TurnIdleEvents(TurnEvents {
                turn_id,
                events: wire,
            }))
            .await;
        }
    }

    async fn shutdown(&mut self, grace: Duration, done_rx: &mut mpsc::UnboundedReceiver<Uuid>) {
        // (FIX-114, `12` §Process model) SIGTERM to every registered process group.
        self.procs.terminate_all();
        for t in self.turns.values() {
            t.aborted.store(true, Ordering::SeqCst);
            t.abort.notify_one();
            let _ = t.session.lock().await.abort().await;
        }
        let deadline = Instant::now() + grace;
        // Every terminal and preview stream ends with the sandbox (`12 §Terminal`, `16 §W19`,
        // `12 §Previews`), within the grace: a wedged engine must not hold shutdown (FIX-091).
        let _ = tokio::time::timeout_at(
            deadline,
            futures::future::join(self.ptys.close_all(), self.streams.close_all()),
        )
        .await;
        while !self.turns.is_empty() {
            tokio::select! {
                Some(id) = done_rx.recv() => { self.finish_turn(id); }
                _ = sleep_until(deadline) => break,
            }
        }
        // All at once: each release waits for what its harness started to end.
        let rest: Vec<Releasing> = self
            .sessions
            .drain()
            .map(|(_, e)| start_release(e.session))
            .chain(self.unreleased.drain(..).map(|(_, u)| match u {
                Unreleased::Running(r) => r,
                Unreleased::Failed(s) => start_release(s),
            }))
            .collect();
        let _ =
            tokio::time::timeout(self.cfg.release_deadline, futures::future::join_all(rest)).await;
    }

    /// Waits for releases until `deadline`, all of them against that one instant; each that
    /// has not released its session by then is kept for the conversation's next
    /// `session.release`. `true` when none is.
    async fn settle(
        &mut self,
        conversation_id: Uuid,
        releases: Vec<Releasing>,
        deadline: Instant,
    ) -> bool {
        let mut released = true;
        for mut release in releases {
            let kept = match tokio::time::timeout_at(deadline, &mut release).await {
                Ok(Ok(None)) => continue,
                Ok(Ok(Some(session))) => Some(Unreleased::Failed(session)),
                // The task died and its session with it: nothing is left to try again, and
                // nothing says that it was released.
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "a release task failed");
                    self.lost_releases.insert(conversation_id);
                    None
                }
                Err(_) => Some(Unreleased::Running(release)),
            };
            released = false;
            if let Some(kept) = kept {
                self.owes_released.insert(conversation_id);
                self.unreleased.push((conversation_id, kept));
            }
        }
        released
    }

    /// Releases a session that is no longer used, waiting no longer than the release
    /// deadline.
    async fn release_replaced(&mut self, conversation_id: Uuid, session: SharedSession) {
        let deadline = Instant::now() + self.cfg.release_deadline;
        self.settle(conversation_id, vec![start_release(session)], deadline)
            .await;
    }

    /// Whether a session of the conversation is still not released, after dropping those
    /// whose release has finished meanwhile.
    async fn has_unreleased(&mut self, conversation_id: Uuid) -> bool {
        let mut left = false;
        for (c, u) in std::mem::take(&mut self.unreleased) {
            let u = match u {
                Unreleased::Running(r) if r.is_finished() => match r.await {
                    Ok(Some(session)) => Unreleased::Failed(session),
                    Ok(None) => continue,
                    Err(e) => {
                        tracing::error!(error = %e, "a release task failed");
                        self.lost_releases.insert(c);
                        continue;
                    }
                },
                u => u,
            };
            left |= c == conversation_id;
            self.unreleased.push((c, u));
        }
        left
    }

    /// Releases one conversation's harness session; one with a running turn is never interrupted
    /// (the release waits for it — the request comes again). The outcome is reported
    /// (`session.released`): the API records the **Background activity stopped** notice only on
    /// an acknowledged release (FIX-253 review), and `ok` says that nothing the conversation's
    /// harness started is left (FIX-265). What did not end is kept, and the request, sent again,
    /// goes on with it.
    async fn release_session(&mut self, conversation_id: Uuid) {
        let deadline = Instant::now() + self.cfg.release_deadline;
        let releases = self.begin_release(conversation_id).await;
        self.finish_release(conversation_id, releases, deadline)
            .await;
    }

    /// Starts every release the conversation needs, so that they all run at once; `None`
    /// while one of its turns runs (a running turn is never interrupted).
    async fn begin_release(&mut self, conversation_id: Uuid) -> Option<Vec<Releasing>> {
        // A failed release that is still running counts as such once it has finished.
        self.has_unreleased(conversation_id).await;
        if self
            .turns
            .values()
            .any(|t| t.conversation_id == conversation_id)
        {
            return None;
        }
        let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut self.unreleased)
            .into_iter()
            .partition(|(c, _)| *c == conversation_id);
        self.unreleased = others;
        let mut releases: Vec<Releasing> = mine
            .into_iter()
            .map(|(_, u)| match u {
                Unreleased::Running(r) => r,
                Unreleased::Failed(s) => start_release(s),
            })
            .collect();
        if let Some(e) = self.sessions.remove(&conversation_id) {
            releases.push(start_release(e.session));
        }
        Some(releases)
    }

    /// Waits for what [`Self::begin_release`] started, until `deadline`, and reports the
    /// outcome (`None`: not released, a turn runs).
    async fn finish_release(
        &mut self,
        conversation_id: Uuid,
        releases: Option<Vec<Releasing>>,
        deadline: Instant,
    ) {
        // (FIX-253 fixes2) `ok` only when every release really ran and succeeded. No
        // session: nothing of it to release.
        let ok = match releases {
            None => false,
            Some(releases) => self.settle(conversation_id, releases, deadline).await,
        };
        if ok {
            // The API has it from this answer: nothing is owed, and a release task that
            // died earlier no longer says anything about a later one.
            self.owes_released.remove(&conversation_id);
            self.lost_releases.remove(&conversation_id);
            self.forget_watch(conversation_id);
        }
        self.send_released(conversation_id, ok).await;
    }

    /// (FIX-253, part B) The watch dies with the session, and what was reported of it.
    fn forget_watch(&self, conversation_id: Uuid) {
        if let Ok(mut m) = self.idle_turns.lock() {
            m.remove(&conversation_id);
        }
        if let Ok(mut m) = self.reported_background.lock() {
            m.remove(&conversation_id);
        }
    }

    async fn send_released(&self, conversation_id: Uuid, ok: bool) {
        self.send(SupervisorToRunner::SessionReleased(SessionReleased {
            conversation_id,
            ok,
        }))
        .await;
    }

    /// (FIX-258) Says `session.released { ok: true }` unasked for each conversation that is
    /// owed it, once, as soon as that is true and not before: every unfinished release of it
    /// has ended well, none of its turns runs, and its live session, if it has one, lists no
    /// background work and has no child that is still ending.
    async fn acknowledge_late_releases(&mut self) {
        let owed: Vec<Uuid> = self.owes_released.iter().copied().collect();
        for conversation_id in owed {
            // A failed release stays unreleased: only a `session.release` tries it again.
            if self.has_unreleased(conversation_id).await
                || self
                    .turns
                    .values()
                    .any(|t| t.conversation_id == conversation_id)
            {
                continue;
            }
            if self.lost_releases.contains(&conversation_id) {
                self.owes_released.remove(&conversation_id);
                continue;
            }
            let live = match self.sessions.get(&conversation_id) {
                Some(e) => {
                    let s = e.session.lock().await;
                    if !s.background_work().is_empty() || s.still_ending() {
                        continue;
                    }
                    true
                }
                None => false,
            };
            self.owes_released.remove(&conversation_id);
            if !live {
                self.forget_watch(conversation_id);
            } else if let Ok(mut m) = self.reported_background.lock()
                && let Some(reported) = m.get_mut(&conversation_id)
            {
                // The live session's watch goes on, and reports what it starts from here.
                reported.clear();
            }
            self.send_released(conversation_id, true).await;
        }
    }

    /// (P5-GUIDE-7, C-654) A sync changed what new sessions are told (core descriptors or the
    /// instruction files): every idle session is released and picks the new list up at its next
    /// message; a session with a turn running keeps its own until then.
    async fn release_idle_sessions(&mut self) {
        let ids: Vec<Uuid> = self.sessions.keys().copied().collect();
        // One deadline for them all, so all are started before any is waited for: a slow one
        // uses up the wait, never the time the others have to finish.
        let deadline = Instant::now() + self.cfg.release_deadline;
        let mut started = Vec::new();
        for id in ids {
            started.push((id, self.begin_release(id).await));
        }
        for (id, releases) in started {
            self.finish_release(id, releases, deadline).await;
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
                self.release_session(r.conversation_id).await;
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
            // (P5-FILES-1) `exec.*` frames travel only on the unsequenced side-channel
            // (`channel.rs` routes them to the pump); a sequenced one is ignored.
            RunnerToSupervisor::ExecOpen(_) | RunnerToSupervisor::ExecEof(_) => {}
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
                .map(|mut h| {
                    // (P6-MEMLOCK, C-1149) The projector's locked map: the harvest never uploads
                    // a locked memory, it restores the recorded bytes instead.
                    let locked = h.projector.locked_memories();
                    h.harvester.scan(&locked)
                })
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
        {
            self.release_replaced(conversation_id, e.session).await;
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
                .and_then(|mut g| {
                    let before = g.projector.catalogue_digest();
                    g.projector.commit(generation).map_err(|e| e.to_string())?;
                    // A first install has no sessions to release (`before` is `None`).
                    Ok(before.is_some() && before != g.projector.catalogue_digest())
                })
        })
        .await;
        match committed {
            Ok(Ok(changed)) => {
                if changed {
                    self.release_idle_sessions().await;
                }
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
        {
            // (FIX-253, part B) The old session's watch dies with it.
            if let Ok(mut m) = self.idle_turns.lock() {
                m.remove(&d.conversation_id);
            }
            self.release_replaced(d.conversation_id, old.session).await;
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
            idle_turns: self.idle_turns.clone(),
            reported_background: self.reported_background.clone(),
            unreleased: self.has_unreleased(d.conversation_id).await,
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
        // (P5-GUIDE-7, C-655) The conversation the harness serves: `aurora-agent … aurora` names
        // it on the tools socket (the socket still decides by the caller's process, FIX-151).
        // Set only for harness processes: the terminal, SSH and `process_start` environments are
        // built elsewhere and strip `AURORA_*`.
        env.push(("AURORA_SESSION".into(), d.conversation_id.to_string()));
        // (P5-SKILLDIR, D-454, C-1140) The skills folder, the same name on every harness; last
        // so it wins over an image or profile value.
        env.retain(|(k, _)| k != "SKILLS_DIR");
        env.push((
            "SKILLS_DIR".into(),
            crate::harness::skills_dir_abs(&self.cfg.working_dir, &self.caps.skills_dir),
        ));
        let core_only = self
            .home
            .lock()
            .map(|h| h.projector.bridge_core_only())
            .unwrap_or(false);
        // The tools the harness does not see are found with `aurora-agent` (C-654).
        // The terminal's `claude`/`codex` wrappers reach the LLM with the same model the agent
        // uses (placeholders only, no secret; `crate::harness_env`).
        if let Some(h) = &self.cfg.harness_env
            && let Err(e) = h.apply_turn(self.adapter.id(), &d.model)
        {
            tracing::warn!(error = %e, "could not update harness.env for the terminal wrappers");
        }
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
                        if core_only {
                            c.push("--core-only".to_owned());
                        }
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
    /// (FIX-253, part B) The supervisor's `idle_turns` map, so the watch started here is known
    /// to the drain.
    idle_turns: Arc<StdMutex<HashMap<Uuid, (Uuid, u64)>>>,
    /// (FIX-265) The supervisor's record of the lists it reported.
    reported_background: Arc<StdMutex<HashMap<Uuid, Vec<BackgroundWork>>>>,
    /// (FIX-265) An earlier session of the conversation is not released yet, so an empty list
    /// of this one does not say that nothing runs.
    unreleased: bool,
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
        let (status, err_code, err_msg, usage, n) = match started {
            Err(e) => (
                TurnStatus::Failed,
                Some(classify_error(&e.code)),
                Some(e.message),
                None,
                0u64,
            ),
            Ok(stream) => self.pump(stream, &turn_ref).await,
        };
        // (FIX-253, D-490) Nothing happens unseen: the harness child's own background work is
        // listed at turn end, while the turn still owns the event stream on the API side, so the
        // conversation gets one visible entry (with Stop) for what keeps running after the reply.
        let (backgrounds, may_say_more) = {
            let s = self.session.lock().await;
            (s.background_work(), s.wants_idle_watch())
        };
        // (FIX-265) Once a conversation has had a list, every turn's end says what it is now,
        // the empty list too.
        let reported = self.reported_background.lock().is_ok_and(|mut m| {
            let had = m.contains_key(&self.conversation_id);
            if !backgrounds.is_empty() || (had && !self.unreleased) {
                m.insert(self.conversation_id, backgrounds.clone());
                return true;
            }
            false
        });
        if reported {
            self.send(SupervisorToRunner::TurnEvent(TurnEvent {
                turn_id: self.turn_id,
                n: n + 1,
                event: background_wire(&backgrounds),
            }))
            .await;
        }
        if reported || may_say_more {
            // (FIX-253, part B) The watch begins: what the child does while nobody holds its
            // stream is reported as `turn.idle.events` against this turn until the work, the
            // child or the watch budget ends it.
            let mut s = self.session.lock().await;
            s.start_idle_reader();
            drop(s);
            if let Ok(mut m) = self.idle_turns.lock() {
                m.insert(self.conversation_id, (self.turn_id, 1));
            }
        }
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
        u64,
    ) {
        let mut n = 0u64;
        let mut tick = interval(self.progress);
        tick.tick().await;
        let mut abort_deadline: Option<Instant> = None;
        let out = |n: u64,
                   s: TurnStatus,
                   c: Option<&'static str>,
                   m: Option<String>,
                   u: Option<serde_json::Value>| (s, c, m, u, n);
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
                        None => return out(n, if aborted {
                            TurnStatus::Aborted
                        } else {
                            TurnStatus::Failed
                        }, if aborted { None } else { Some("harness_failed") }, if aborted { None } else { Some("harness ended without a result".into()) }, None),
                        Some(HarnessEvent::Done { usage }) => return out(n, if aborted {
                            TurnStatus::Aborted
                        } else {
                            TurnStatus::Done
                        }, None, None, usage),
                        Some(HarnessEvent::Error { code: c, message }) => return out(n, if aborted {
                            TurnStatus::Aborted
                        } else {
                            TurnStatus::Failed
                        }, if aborted { None } else { Some(classify_error(&c)) }, if aborted { None } else { Some(message) }, None),
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
                _ = deadline => return out(n, TurnStatus::Aborted, None, None, None),
            }
        }
    }
}

/// (FIX-253 fixes2) One shared session's release: `true` only when the release really ran and
/// succeeded. A session another live clone still holds was not released, and neither was one
/// whose release failed (FIX-265): it comes back, so the caller can keep it and the next Stop
/// can try again.
async fn release_shared(session: SharedSession) -> (bool, Option<SharedSession>) {
    match Arc::try_unwrap(session) {
        Ok(m) => {
            let mut s = m.into_inner();
            match s.release().await {
                Ok(()) => (true, None),
                Err(e) => {
                    tracing::warn!(code = %e.code, message = %e.message, "release failed");
                    (false, Some(Arc::new(Mutex::new(s))))
                }
            }
        }
        Err(arc) => (false, Some(arc)),
    }
}

/// (FIX-253, D-490) The `background` event: what still runs, oldest first.
fn background_wire(work: &[BackgroundWork]) -> aurora_protocol::runner::HarnessEventWire {
    let items: Vec<Value> = work
        .iter()
        .map(|w| json!({ "command": w.command, "started_at": w.started_at }))
        .collect();
    aurora_protocol::runner::HarnessEventWire {
        kind: code("background"),
        data: Map::from_iter([("items".to_owned(), json!(items))]),
    }
}

/// (FIX-253) Numbers one drain of a watched idle child's events from `next`, which it advances:
/// the numbers are the events' places in the whole watch, so the API can tell a redelivered
/// event from a new one even when it carries no id of its own.
fn number_idle(events: &[HarnessEvent], next: &mut u64) -> Vec<NumberedEvent> {
    events
        .iter()
        .filter_map(HarnessEvent::to_wire)
        .map(|event| {
            let n = *next;
            *next += 1;
            NumberedEvent { n, event }
        })
        .collect()
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn idle_numbers_run_on_across_drains() {
        let activity = || HarnessEvent::Activity { line: "x".into() };
        let mut next = 1;
        let first = number_idle(&[activity(), activity()], &mut next);
        let second = number_idle(&[activity()], &mut next);
        let ns: Vec<u64> = first.iter().chain(&second).map(|e| e.n).collect();
        assert_eq!(ns, [1, 2, 3]);
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;
    use crate::harness::{BackgroundWork, HarnessError, HarnessEventStream};
    use aurora_protocol::runner::SendNowOutcome;
    use std::path::Path;

    /// A session whose release fails, and one whose release succeeds: the two failure and the
    /// success paths of [`release_shared`].
    struct StubSession {
        fails: bool,
    }

    #[async_trait::async_trait]
    impl HarnessSession for StubSession {
        fn session_ref(&self) -> Option<String> {
            None
        }
        async fn run_turn(&mut self, _: TurnInput) -> Result<HarnessEventStream, HarnessError> {
            Err(HarnessError::new("stub", "no turns"))
        }
        async fn steer(&mut self, _: SteerInput) -> Result<SteerAck, HarnessError> {
            Ok(SteerAck::Refused)
        }
        async fn background_running_tool(&mut self) -> Result<SendNowOutcome, HarnessError> {
            Ok(SendNowOutcome::Unsupported)
        }
        async fn abort(&mut self) -> Result<(), HarnessError> {
            Ok(())
        }
        /// Fails once, then succeeds.
        async fn release(&mut self) -> Result<(), HarnessError> {
            if std::mem::take(&mut self.fails) {
                Err(HarnessError::new("stub", "release failed"))
            } else {
                Ok(())
            }
        }
        fn background_work(&self) -> Vec<BackgroundWork> {
            Vec::new()
        }
    }

    fn stub(fails: bool) -> SharedSession {
        Arc::new(Mutex::new(
            Box::new(StubSession { fails }) as Box<dyn HarnessSession>
        ))
    }

    fn _unused(_: &Path) {}

    /// A release that failed is not an acknowledgement: `ok` stays false, and (FIX-265) the
    /// session comes back, so that the next Stop tries again.
    #[tokio::test]
    async fn a_failed_release_is_not_reported_as_released() {
        let (ok, back) = release_shared(stub(true)).await;
        assert!(!ok, "a release that errored must not report ok");
        let Some(back) = back else {
            panic!("an errored release must be retried");
        };
        let (ok, back) = release_shared(back).await;
        assert!(ok, "the retry released it");
        assert!(back.is_none());
    }

    /// A session another clone still holds was not released: the clone comes back for the entry.
    #[tokio::test]
    async fn a_session_still_held_by_another_clone_is_not_released_and_comes_back() {
        let session = stub(false);
        let held = Arc::clone(&session);
        let (ok, back) = release_shared(session).await;
        assert!(!ok, "an unreleased session must not report ok");
        let Some(back) = back else {
            panic!("the still-held session must come back");
        };
        // The returned session is the same live one, not a fresh failure.
        assert!(Arc::ptr_eq(&held, &back));
    }

    /// The success path: the release ran and succeeded.
    #[tokio::test]
    async fn a_successful_release_is_reported() {
        let (ok, back) = release_shared(stub(false)).await;
        assert!(ok);
        assert!(back.is_none());
    }
}

//! Adapter `claude-code`: drives the pinned Claude Code CLI in headless streaming-JSON mode
//! (`docs/12-supervisor-harness.md` §Adapter claude-code).
//!
//! # Pinned CLI
//! [`PINNED_VERSION`] (`@anthropic-ai/claude-code`, installed by `images/sandbox/Containerfile`
//! under `/opt/aurora/harness/claude-code`). Flags and event shapes below were read from the
//! `claude --help` of 2.1.318 and the documented stream-json format; the fixtures in `fixtures/`
//! are **hand-authored, not recorded**. The live conformance suite (`AURORA_LIVE_CLAUDE=1`) is
//! what verifies them against a real run.
//!
//! One child per live session: `claude -p --input-format stream-json --output-format stream-json
//! --verbose --include-partial-messages --permission-mode bypassPermissions --strict-mcp-config
//! --mcp-config '<json>' [--effort L] [--resume <session id>]`, cwd `/workspace`, cleared
//! environment. Each turn writes one stream-json user message to stdin and reads stdout until the
//! `result` line (see [`stream`] for the mapping). Abort kills the child; the next turn restarts it
//! with `--resume <session id>`. `release` kills the idle child and keeps the session id.
//!
//! `steer` is `true` (FIX-225, probed against 2.1.318 and 2.1.319): a second user line written to
//! stdin during a run is handed to the model at the next tool boundary and the run goes on to its
//! single `result`; `--replay-user-messages` makes the CLI echo each user line it takes, which is
//! the join. The same line to an idle CLI would start a run, so `steer()` refuses between turns and
//! the turn's end kills a CLI still holding an unechoed steer (see [`Pump::close_to_steers`]).
//! `background_tool` is `false`: the stream-json mode has no interface that moves a running tool to
//! the background (`docs/12 §Turn controls by harness`).
//!
//! The first turn on a fresh child is held until `system/init` lists the `aurora` MCP server as
//! connected with its tools; if the CLI emits no init before the first message, the message is sent
//! and an init that lacks the tools fails the turn. Without an init, the wait is `INIT_GRACE` (2 s):
//! CLI 2.1.319 emits none until it has a message.

pub mod config;
pub mod launcher;
pub mod stream;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::SendNowOutcome;
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::Instant;

use crate::harness::{
    BackgroundWork, HarnessAdapter, HarnessCapabilities, HarnessError, HarnessEvent,
    HarnessEventStream, HarnessSession, SessionOptions, SteerAck, SteerInput, TurnInput,
};
use launcher::{ChildIo, Launcher, ProcessLauncher};
use stream::StreamMapper;

pub const PINNED_VERSION: &str = "2.1.319";
const DEFAULT_HOLD: Duration = Duration::from_secs(30);
/// How long a fresh child gets to emit `system/init` before the first message is sent anyway.
/// CLI 2.1.319 emits no init until it has a user message, so waiting the full hold costs every
/// first turn 30 s; the init that follows the message is still validated in-turn.
const INIT_GRACE: Duration = Duration::from_secs(2);
/// (FIX-253, part B) How long the idle reader keeps watching a child after a turn that left
/// background work: half an hour after the watch began, what the child did by then is what was
/// recorded — a longer watch is the next turn's business (the supervisor restarts the watch
/// after every turn that still leaves work).
const IDLE_WATCH: Duration = Duration::from_secs(30 * 60);
/// (FIX-253, part B) The idle reader's cadence: how often it re-checks its stop conditions while
/// no line arrives, and so the worst delay a starting turn can see before the reader yields the
/// stream (the reader also wakes on the turn's notify, so the usual delay is nil).
const IDLE_TICK: Duration = Duration::from_millis(100);
/// (FIX-253, part B) The bound on the events one watch keeps for the next drain. An idle child's
/// own run is short; the bound only keeps a runaway child from growing the buffer without end.
const IDLE_BUFFER: usize = 256;

#[derive(Clone)]
pub struct ClaudeCodeAdapter {
    launcher: Arc<dyn Launcher>,
    hold_timeout: Duration,
    /// Directory holding the CLI's `.claude/` state; `None` = `$HOME`.
    config_home: Option<std::path::PathBuf>,
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::with_launcher(Arc::new(ProcessLauncher::default()))
    }
}

impl ClaudeCodeAdapter {
    pub fn with_launcher(launcher: Arc<dyn Launcher>) -> Self {
        Self {
            launcher,
            hold_timeout: DEFAULT_HOLD,
            config_home: None,
        }
    }

    /// Where the CLI keeps `.claude/` (default `$HOME`); used by [`HarnessAdapter::purge_session`].
    #[must_use]
    pub fn config_home(mut self, dir: std::path::PathBuf) -> Self {
        self.config_home = Some(dir);
        self
    }

    #[must_use]
    pub fn hold_timeout(mut self, d: Duration) -> Self {
        self.hold_timeout = d;
        self
    }
}

#[async_trait]
impl HarnessAdapter for ClaudeCodeAdapter {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            resume: true,
            steer: true,
            background_tool: false,
            thinking_deltas: true,
            tool_events: true,
            platform_tools: true,
            dialects: config::DIALECTS.iter().map(|d| (*d).to_owned()).collect(),
            skills_dir: ".claude/skills".into(),
            instruction_files: vec!["CLAUDE.md".into(), "AGENTS.md".into()],
            model_roles: aurora_protocol::runner::harness_model_roles("claude-code")
                .iter()
                .map(|r| (*r).to_owned())
                .collect(),
        }
    }

    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        let mut s = ClaudeSession {
            session_ref: Arc::new(StdMutex::new(opts.resume_ref.clone())),
            opts,
            launcher: self.launcher.clone(),
            hold_timeout: self.hold_timeout,
            live: None,
            running: Arc::new(AtomicBool::new(false)),
            abort: None,
        };
        s.ensure_child().await?;
        Ok(Box::new(s))
    }

    /// Removes everything the CLI stored for the session: the transcript (`projects/*/<ref>.jsonl`
    /// and its sidecar directory), todos, session env and file history. Idempotent.
    async fn purge_session(
        &self,
        working_dir: &std::path::Path,
        session_ref: &str,
    ) -> Result<(), HarnessError> {
        if !crate::harness::is_safe_session_ref(session_ref) {
            return Err(HarnessError::new(
                "invalid_session_ref",
                "unsafe session reference",
            ));
        }
        let mut roots = Vec::new();
        match &self.config_home {
            Some(h) => roots.push(h.join(".claude")),
            None => {
                if let Some(h) = std::env::var_os("HOME") {
                    roots.push(std::path::PathBuf::from(h).join(".claude"));
                }
            }
        }
        roots.push(working_dir.join(".claude"));
        roots.dedup();
        let session_ref = session_ref.to_owned();
        tokio::task::spawn_blocking(move || purge_claude_files(&roots, &session_ref))
            .await
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))?
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))
    }
}

fn remove_any(p: &std::path::Path) -> std::io::Result<()> {
    let r = match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    match r {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn purge_claude_files(roots: &[std::path::PathBuf], session_ref: &str) -> std::io::Result<()> {
    for root in roots {
        // projects/<encoded cwd>/<ref>.jsonl and projects/<encoded cwd>/<ref>/
        if let Ok(rd) = std::fs::read_dir(root.join("projects")) {
            for proj in rd.flatten() {
                remove_any(&proj.path().join(format!("{session_ref}.jsonl")))?;
                remove_any(&proj.path().join(session_ref))?;
            }
        }
        remove_any(&root.join("session-env").join(session_ref))?;
        remove_any(&root.join("file-history").join(session_ref))?;
        if let Ok(rd) = std::fs::read_dir(root.join("todos")) {
            let prefix = format!("{session_ref}-");
            for f in rd.flatten() {
                if f.file_name().to_string_lossy().starts_with(&prefix) {
                    remove_any(&f.path())?;
                }
            }
        }
    }
    Ok(())
}

struct Live {
    stdin: mpsc::Sender<String>,
    lines: Arc<Mutex<mpsc::Receiver<String>>>,
    kill: KillSlot,
    ready: Arc<AtomicBool>,
    dead: Arc<AtomicBool>,
    /// The CLI's per-model usage report is cumulative for this child (P5-MODELS-2).
    usage_base: stream::Baseline,
    /// (FIX-225) Steers handed to this child and not yet echoed.
    steers: stream::Steers,
    /// (FIX-253, D-490) The child's own background commands, as mapped from the turn's tool calls.
    backgrounds: stream::BackgroundSink,
    /// (FIX-253, part B) The idle reader's events since the last drain.
    idle_buf: Arc<StdMutex<Vec<HarnessEvent>>>,
    /// (FIX-253, part B) Wakes the idle reader when a turn starts, so it yields the stream at
    /// once instead of at the tick.
    turn_start: Arc<Notify>,
    /// (FIX-253, part B) Whether the idle reader is watching this child.
    idle_active: Arc<AtomicBool>,
}

type KillSlot = Arc<StdMutex<Option<Box<dyn FnOnce() + Send>>>>;

/// Kills the child once and marks it dead (the next turn starts a new one with `--resume`).
fn kill_child(dead: &AtomicBool, kill: &KillSlot) {
    dead.store(true, Ordering::SeqCst);
    let k = kill.lock().ok().and_then(|mut k| k.take());
    if let Some(k) = k {
        k();
    }
}

impl Live {
    fn kill(&mut self) {
        kill_child(&self.dead, &self.kill);
    }
}

pub struct ClaudeSession {
    opts: SessionOptions,
    launcher: Arc<dyn Launcher>,
    hold_timeout: Duration,
    session_ref: Arc<StdMutex<Option<String>>>,
    live: Option<Live>,
    running: Arc<AtomicBool>,
    abort: Option<Arc<Notify>>,
}

impl ClaudeSession {
    fn current_ref(&self) -> Option<String> {
        self.session_ref.lock().ok().and_then(|r| r.clone())
    }

    /// (FIX-253, part B) Watch the idle child: while its background work lasts, read the lines it
    /// writes after the turn — its own runs' text and tool calls — into the drain buffer. The
    /// turn's stream and the watch share one receiver behind a mutex, and the watch never holds
    /// the mutex across a turn: a starting turn takes the lock (it writes the turn's message only
    /// once it holds it), the reader re-checks the running flag every tick and wakes on the
    /// turn's notify, so every line is read by exactly one of them and the turn is never blocked
    /// by more than the tick. The watch ends when the work is gone, the child dies, or
    /// [`IDLE_WATCH`] runs out.
    fn start_idle_reader(&mut self) {
        let Some(live) = &self.live else {
            return;
        };
        if self.running.load(Ordering::SeqCst) || live.dead.load(Ordering::SeqCst) {
            return;
        }
        if live.idle_active.swap(true, Ordering::SeqCst) {
            return; // already watching
        }
        let lines = live.lines.clone();
        let backgrounds = live.backgrounds.clone();
        let usage_base = live.usage_base.clone();
        let steers = live.steers.clone();
        let buf = live.idle_buf.clone();
        let turn_start = live.turn_start.clone();
        let dead = live.dead.clone();
        let active = live.idle_active.clone();
        let running = self.running.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let mut mapper = StreamMapper::with_baseline(usage_base, steers)
                .with_backgrounds(backgrounds.clone());
            loop {
                if running.load(Ordering::SeqCst)
                    || dead.load(Ordering::SeqCst)
                    || started.elapsed() >= IDLE_WATCH
                    || backgrounds.lock().is_ok_and(|b| b.is_empty())
                {
                    break;
                }
                let mut lines = lines.lock().await;
                if running.load(Ordering::SeqCst) || dead.load(Ordering::SeqCst) {
                    break;
                }
                let line = tokio::select! {
                    biased;
                    () = turn_start.notified() => break,
                    l = tokio::time::timeout(IDLE_TICK, lines.recv()) => match l {
                        Ok(Some(l)) => l,
                        Ok(None) => break, // the child's stream closed
                        Err(_) => continue, // the tick: re-check the stop conditions
                    },
                };
                // A `result` here is the idle run's own end: it ends nothing of ours — another
                // run may follow while the background work lasts.
                let m = mapper.map_line(&line);
                drop(lines);
                let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                for e in m.events {
                    if b.len() < IDLE_BUFFER {
                        b.push(e);
                    }
                }
            }
            active.store(false, Ordering::SeqCst);
        });
    }

    async fn ensure_child(&mut self) -> Result<(), HarnessError> {
        if self
            .live
            .as_ref()
            .is_some_and(|l| !l.dead.load(Ordering::SeqCst))
        {
            return Ok(());
        }
        let spec = config::launch_spec(&self.opts, self.current_ref().as_deref())?;
        let ChildIo { stdin, lines, kill } = self.launcher.launch(spec).await?;
        self.live = Some(Live {
            stdin,
            lines: Arc::new(Mutex::new(lines)),
            kill: Arc::new(StdMutex::new(Some(kill))),
            ready: Arc::default(),
            dead: Arc::default(),
            usage_base: Arc::default(),
            steers: Arc::default(),
            backgrounds: Arc::default(),
            idle_buf: Arc::default(),
            turn_start: Arc::new(Notify::new()),
            idle_active: Arc::new(AtomicBool::new(false)),
        });
        Ok(())
    }
}

struct Pump {
    tx: mpsc::Sender<HarnessEvent>,
    lines: Arc<Mutex<mpsc::Receiver<String>>>,
    stdin: mpsc::Sender<String>,
    ready: Arc<AtomicBool>,
    dead: Arc<AtomicBool>,
    session_ref: Arc<StdMutex<Option<String>>>,
    abort: Arc<Notify>,
    hold_timeout: Duration,
    user_line: String,
    usage_base: stream::Baseline,
    steers: stream::Steers,
    backgrounds: stream::BackgroundSink,
    kill: KillSlot,
}

fn err(code: &str, message: &str) -> HarnessEvent {
    HarnessEvent::Error {
        code: code.into(),
        message: message.into(),
    }
}

impl Pump {
    /// Returns the single terminal event.
    async fn drive(&self) -> HarnessEvent {
        let aborted = || err("aborted", "turn aborted");
        let mut lines = self.lines.lock().await;
        let mut mapper = StreamMapper::with_baseline(self.usage_base.clone(), self.steers.clone())
            .with_backgrounds(self.backgrounds.clone());
        if !self.ready.load(Ordering::SeqCst) {
            let started = Instant::now();
            let mut deadline = started + self.hold_timeout.min(INIT_GRACE);
            let mut saw_init = false;
            loop {
                let next = tokio::select! {
                    biased;
                    () = self.abort.notified() => return aborted(),
                    n = tokio::time::timeout_at(deadline, lines.recv()) => n,
                };
                let line = match next {
                    Err(_) if saw_init => {
                        return err("harness_failed", "MCP tools were not listed by the harness");
                    }
                    Err(_) => break, // no init before the first message: validated in-turn
                    Ok(None) => return self.exited(),
                    Ok(Some(l)) => l,
                };
                let m = mapper.map_line(&line);
                self.note_ref(&m.events);
                if let Some(init) = m.init {
                    if !saw_init {
                        // An init is coming: give the MCP server the full hold to list its tools.
                        deadline = started + self.hold_timeout;
                    }
                    saw_init = true;
                    if init.mcp_ready {
                        self.ready.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                if let Some(t) = m.terminal {
                    return t;
                }
            }
        }
        mapper.reset_turn();
        if self.stdin.send(self.user_line.clone()).await.is_err() {
            return self.exited();
        }
        // The CLI has the turn's own message in its stdin queue ahead of any steer: from here on a
        // steer is taken as a message of the run (a steer sent before it would be the run's first).
        if let Ok(mut s) = self.steers.lock() {
            s.accepting = true;
        }
        loop {
            let line = tokio::select! {
                biased;
                () = self.abort.notified() => return aborted(),
                l = lines.recv() => l,
            };
            let Some(line) = line else {
                return self.exited();
            };
            let m = mapper.map_line(&line);
            if let Some(init) = &m.init {
                if init.mcp_ready {
                    self.ready.store(true, Ordering::SeqCst);
                } else if !self.ready.load(Ordering::SeqCst) {
                    return err("harness_failed", "MCP tools were not listed by the harness");
                }
            }
            self.note_ref(&m.events);
            for e in m.events {
                if self.tx.send(e).await.is_err() {
                    return aborted(); // receiver gone
                }
            }
            if let Some(t) = m.terminal {
                return t;
            }
        }
    }

    /// Closes the door to steers when the turn ends. A steer written to the CLI and not echoed by
    /// now would start a run of its own after this turn's `result` (probed: the CLI runs a user line
    /// that reaches an idle session as a new turn), so the child is killed: the steer is lost (the
    /// supervisor reports it `missed`, the follow-up runs as the next turn) and the next turn
    /// restarts the CLI with `--resume`.
    fn close_to_steers(&self) {
        let stray = self
            .steers
            .lock()
            .map(|mut s| {
                s.accepting = false;
                std::mem::take(&mut s.pending)
            })
            .unwrap_or_default();
        if !stray.is_empty() {
            kill_child(&self.dead, &self.kill);
        }
    }

    fn exited(&self) -> HarnessEvent {
        self.dead.store(true, Ordering::SeqCst);
        err("harness_failed", "claude-code exited unexpectedly")
    }

    fn note_ref(&self, events: &[HarnessEvent]) {
        for e in events {
            if let HarnessEvent::SessionRef { session_ref } = e
                && let Ok(mut r) = self.session_ref.lock()
            {
                *r = Some(session_ref.clone());
            }
        }
    }
}

#[async_trait]
impl HarnessSession for ClaudeSession {
    fn session_ref(&self) -> Option<String> {
        self.current_ref()
    }

    async fn run_turn(&mut self, input: TurnInput) -> Result<HarnessEventStream, HarnessError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(HarnessError::new(
                "harness_failed",
                "a turn is already running",
            ));
        }
        // (FIX-253, part B) A turn is starting: the idle reader (if any) yields the stream. It
        // re-checks the flag every tick besides, so even a reader between ticks is never in the
        // way for more than `IDLE_TICK`, and the turn's message is written only once it holds
        // the stream: nothing is read twice, nothing is lost.
        if let Some(live) = &self.live {
            live.turn_start.notify_waiters();
        }
        if let Err(e) = self.ensure_child().await {
            self.running.store(false, Ordering::SeqCst);
            return Err(e);
        }
        let Some(live) = &self.live else {
            self.running.store(false, Ordering::SeqCst);
            return Err(HarnessError::new("harness_failed", "no live child"));
        };
        let (tx, rx) = mpsc::channel(64);
        let abort = Arc::new(Notify::new());
        self.abort = Some(abort.clone());
        let pump = Pump {
            tx: tx.clone(),
            lines: live.lines.clone(),
            stdin: live.stdin.clone(),
            ready: live.ready.clone(),
            dead: live.dead.clone(),
            session_ref: self.session_ref.clone(),
            abort,
            hold_timeout: self.hold_timeout,
            user_line: config::user_message_line(&config::render_turn(&input)),
            usage_base: live.usage_base.clone(),
            steers: live.steers.clone(),
            backgrounds: live.backgrounds.clone(),
            kill: live.kill.clone(),
        };
        let running = self.running.clone();
        tokio::spawn(async move {
            let end = pump.drive().await;
            pump.close_to_steers();
            running.store(false, Ordering::SeqCst);
            let _ = tx.send(end).await;
        });
        Ok(rx)
    }

    /// A second stream-json user line written during a run is taken by the CLI at the next tool
    /// boundary (probed with 2.1.318 and 2.1.319, `claude_code_steer_*` tests) and echoed
    /// (`--replay-user-messages`); the echo is the join (`MessageJoined`). The run still ends with
    /// its single `result`. Refused between turns: the same line to an idle CLI would start a run.
    async fn steer(&mut self, input: SteerInput) -> Result<SteerAck, HarnessError> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(SteerAck::Refused);
        }
        let Some(live) = &self.live else {
            return Ok(SteerAck::Refused);
        };
        let text = config::render_steer(&input);
        // Registered before it is written, so its echo can never be missed; the door is checked in
        // the same critical section as the registration, so a turn that just ended is never handed
        // one (the turn's end takes the same lock and kills the child if one is still pending).
        let registered = live.steers.lock().is_ok_and(|mut s| {
            if s.accepting {
                s.pending.push((input.followup_id, text.clone()));
                true
            } else {
                false
            }
        });
        if !registered {
            return Ok(SteerAck::Refused);
        }
        if live
            .stdin
            .send(config::user_message_line(&text))
            .await
            .is_err()
        {
            if let Ok(mut s) = live.steers.lock() {
                s.pending.retain(|(id, _)| *id != input.followup_id);
            }
            return Ok(SteerAck::Refused);
        }
        Ok(SteerAck::Accepted)
    }

    async fn background_running_tool(&mut self) -> Result<SendNowOutcome, HarnessError> {
        Ok(SendNowOutcome::Unsupported)
    }

    async fn abort(&mut self) -> Result<(), HarnessError> {
        if self.running.load(Ordering::SeqCst) {
            if let Some(a) = &self.abort {
                a.notify_one();
            }
            if let Some(l) = &mut self.live {
                l.kill();
            }
        }
        Ok(())
    }

    async fn release(mut self: Box<Self>) -> Result<(), HarnessError> {
        if let Some(a) = &self.abort
            && self.running.load(Ordering::SeqCst)
        {
            a.notify_one();
        }
        if let Some(l) = &mut self.live {
            l.kill();
        }
        Ok(())
    }

    /// (FIX-253, part B) Watch the idle child after a turn that left background work. See
    /// [`ClaudeSession::start_idle_reader`].
    fn start_idle_reader(&mut self) {
        ClaudeSession::start_idle_reader(self);
    }

    /// (FIX-253, part B) What the watched idle child did since the last drain. While a turn runs
    /// the watch is stopped, so there is nothing new; the supervisor drains between turns.
    fn take_idle_events(&mut self) -> Vec<HarnessEvent> {
        let Some(live) = &self.live else {
            return Vec::new();
        };
        if self.running.load(Ordering::SeqCst) {
            return Vec::new();
        }
        live.idle_buf
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }

    /// (FIX-253, D-490) The live child's own background commands. A dead child's background work
    /// died with it, so a dead (or released) child reports none.
    fn background_work(&self) -> Vec<BackgroundWork> {
        let Some(live) = &self.live else {
            return Vec::new();
        };
        if live.dead.load(Ordering::SeqCst) {
            return Vec::new();
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        live.backgrounds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, started_at)| *started_at <= now)
            .map(|(command, started_at)| BackgroundWork {
                command: command.clone(),
                started_at: *started_at,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;

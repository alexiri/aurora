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
//! `steer` is `false` (not verified against the pinned version): follow-ups become separate turns
//! and `steer()` always refuses. `background_tool` is `false`.
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
    HarnessAdapter, HarnessCapabilities, HarnessError, HarnessEvent, HarnessEventStream,
    HarnessSession, SessionOptions, SteerAck, SteerInput, TurnInput,
};
use launcher::{ChildIo, Launcher, ProcessLauncher};
use stream::StreamMapper;

pub const PINNED_VERSION: &str = "2.1.319";
const DEFAULT_HOLD: Duration = Duration::from_secs(30);
/// How long a fresh child gets to emit `system/init` before the first message is sent anyway.
/// CLI 2.1.319 emits no init until it has a user message, so waiting the full hold costs every
/// first turn 30 s; the init that follows the message is still validated in-turn.
const INIT_GRACE: Duration = Duration::from_secs(2);

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
            steer: false,
            background_tool: false,
            thinking_deltas: true,
            tool_events: true,
            platform_tools: true,
            dialects: vec!["anthropic_messages".into()],
            skills_dir: Some(".claude/skills".into()),
            instruction_files: vec!["CLAUDE.md".into(), "AGENTS.md".into()],
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
    kill: Option<Box<dyn FnOnce() + Send>>,
    ready: Arc<AtomicBool>,
    dead: Arc<AtomicBool>,
}

impl Live {
    fn kill(&mut self) {
        self.dead.store(true, Ordering::SeqCst);
        if let Some(k) = self.kill.take() {
            k();
        }
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
            kill: Some(kill),
            ready: Arc::default(),
            dead: Arc::default(),
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
        let mut mapper = StreamMapper::new();
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
        };
        let running = self.running.clone();
        tokio::spawn(async move {
            let end = pump.drive().await;
            running.store(false, Ordering::SeqCst);
            let _ = tx.send(end).await;
        });
        Ok(rx)
    }

    async fn steer(&mut self, _input: SteerInput) -> Result<SteerAck, HarnessError> {
        Ok(SteerAck::Refused)
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
}

#[cfg(test)]
mod tests;

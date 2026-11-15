//! Adapter `codex`: drives the pinned OpenAI Codex CLI non-interactively
//! (`docs/12-supervisor-harness.md` §Adapter codex, D-122, SEC-66).
//!
//! # Pinned CLI
//! [`PINNED_VERSION`] (release `rust-v0.167.0`, `codex-<arch>-unknown-linux-musl.tar.gz`,
//! checksum-verified by `images/sandbox/Containerfile`, installed under
//! `/opt/aurora/harness/codex`). The flags, configuration keys and event shapes were read from
//! that binary's `exec --help`, `features list` and a real run against a scripted Responses
//! upstream; the conformance suite (`tests/conformance.rs`, mock upstream) and the env-gated live
//! suite (`AURORA_TEST_HARNESS_CODEX_LIVE=1`) verify them. The 0.167 CLI refuses
//! `wire_api = "chat"`, so the adapter declares `openai_responses` only.
//!
//! One `codex exec --json [resume <thread id>] -C /workspace -` child per **turn** (Codex has no
//! live-session protocol in `exec` mode), cwd `/workspace`, scrubbed environment, the prompt on
//! stdin. The thread id from `thread.started` is the session reference, so the next turn
//! resumes it. Abort kills the child; `release` keeps the reference.
//!
//! `steer` is `false`: follow-ups become separate turns and `steer()` always refuses.
//! `background_tool` is `false`.
//!
//! No credential is ever rendered: `config.toml` and the environment carry only the placeholder
//! `OPENAI_API_KEY`, the per-session proxy adds the turn's context credential and the gateway
//! injects the real key; `auth.json` is removed at every start (SEC-66).

pub mod config;
pub mod launcher;
pub mod stream;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use aurora_protocol::runner::SendNowOutcome;
use tokio::sync::{Notify, mpsc};

use crate::harness::{
    HarnessAdapter, HarnessCapabilities, HarnessError, HarnessEvent, HarnessEventStream,
    HarnessSession, SessionOptions, SteerAck, SteerInput, TurnInput,
};
use launcher::{Launcher, ProcessLauncher};
use stream::StreamMapper;

pub const PINNED_VERSION: &str = "0.167.0";

#[derive(Clone)]
pub struct CodexAdapter {
    launcher: Arc<dyn Launcher>,
    /// Directory `CODEX_HOME` points at; `None` = `<working dir>/.codex` (the sandbox home).
    codex_home: Option<PathBuf>,
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::with_launcher(Arc::new(ProcessLauncher::default()))
    }
}

impl CodexAdapter {
    pub fn with_launcher(launcher: Arc<dyn Launcher>) -> Self {
        Self {
            launcher,
            codex_home: None,
        }
    }

    /// Overrides `CODEX_HOME` (tests).
    #[must_use]
    pub fn codex_home(mut self, dir: PathBuf) -> Self {
        self.codex_home = Some(dir);
        self
    }

    fn home_for(&self, working_dir: &Path) -> PathBuf {
        self.codex_home
            .clone()
            .unwrap_or_else(|| working_dir.join(".codex"))
    }
}

/// A Codex thread id names files on disk and becomes an argv word: a short path-safe token that
/// cannot be read as an option.
fn safe_ref(r: &str) -> bool {
    crate::harness::is_safe_session_ref(r) && !r.starts_with('-')
}

/// Writes `config.toml` (owned by Aurora, rewritten before **every** `codex exec`: the home lives
/// in the agent-writable workspace because Codex keeps its rollouts beside the configuration, so
/// the file is re-rendered and its content verified immediately before each spawn, SEC-66) and
/// removes any `auth.json`: Aurora never offers sign-in and a planted file must not win over the
/// API key.
fn write_home(home: &Path, config_toml: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::create_dir_all(home)?;
    let path = home.join("config.toml");
    // Replace whatever is there (a read-only file, a symlink, a directory) and create the new
    // file exclusively so a link planted in between is not followed.
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(&path)?,
        Ok(_) => std::fs::remove_file(&path)?,
        Err(_) => {}
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    f.write_all(config_toml.as_bytes())?;
    f.sync_all()?;
    drop(f);
    if std::fs::read_to_string(&path)? != config_toml {
        return Err(std::io::Error::other("config.toml changed while written"));
    }
    match std::fs::remove_file(home.join("auth.json")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[async_trait]
impl HarnessAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            resume: true,
            steer: false,
            background_tool: false,
            // `exec --json` reports reasoning summaries as completed items, not as a stream.
            thinking_deltas: false,
            tool_events: true,
            platform_tools: true,
            dialects: vec!["openai_responses".into()],
            // A subdirectory of Codex's own skills dir: the read-only projection stays away from
            // the `.system` skills Codex seeds beside it.
            skills_dir: Some(".codex/skills/aurora".into()),
            instruction_files: vec!["AGENTS.md".into()],
        }
    }

    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        if let Some(r) = &opts.resume_ref
            && !safe_ref(r)
        {
            return Err(HarnessError::new(
                "invalid_session_ref",
                "unsafe session reference",
            ));
        }
        let home = self.home_for(&opts.working_dir);
        let toml = config::render_config(&opts)?;
        write_home(&home, &toml).map_err(|e| {
            HarnessError::new("harness_failed", format!("cannot prepare CODEX_HOME: {e}"))
        })?;
        Ok(Box::new(CodexSession {
            session_ref: Arc::new(StdMutex::new(opts.resume_ref.clone())),
            opts,
            home,
            launcher: self.launcher.clone(),
            running: Arc::new(AtomicBool::new(false)),
            abort: None,
            kill: Arc::new(StdMutex::new(None)),
        }))
    }

    /// Removes the rollout files of one session (`sessions/**/rollout-*-<ref>.jsonl`, the
    /// archived copy and the shell snapshot). Idempotent; other sessions are untouched. Codex's
    /// SQLite index keeps a metadata row until the thread table is rebuilt (documented limit).
    async fn purge_session(
        &self,
        working_dir: &Path,
        session_ref: &str,
    ) -> Result<(), HarnessError> {
        if !safe_ref(session_ref) {
            return Err(HarnessError::new(
                "invalid_session_ref",
                "unsafe session reference",
            ));
        }
        let mut roots = vec![self.home_for(working_dir)];
        if self.codex_home.is_none()
            && let Some(h) = std::env::var_os("HOME")
        {
            roots.push(PathBuf::from(h).join(".codex"));
        }
        roots.dedup();
        let session_ref = session_ref.to_owned();
        tokio::task::spawn_blocking(move || purge_codex_files(&roots, &session_ref))
            .await
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))?
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))
    }
}

fn remove_file_if_present(p: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// `rollout-<YYYY-MM-DDThh-mm-ss>-<ref>.jsonl` for exactly this reference: the timestamp is a
/// fixed 19-character field, so a short reference cannot match the tail of another thread id.
fn is_rollout_of(name: &str, session_ref: &str) -> bool {
    let Some(rest) = name
        .strip_prefix("rollout-")
        .and_then(|r| r.strip_suffix(".jsonl"))
    else {
        return false;
    };
    rest.get(..19).is_some_and(|ts| {
        ts.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'T'))
    }) && rest.get(19..).and_then(|t| t.strip_prefix('-')) == Some(session_ref)
}

/// Removes `rollout-<timestamp>-<ref>.jsonl` below `dir` (the date-sharded `sessions/` tree).
fn remove_rollouts(dir: &Path, session_ref: &str, depth: u8) -> std::io::Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth < 5 {
            remove_rollouts(&p, session_ref, depth + 1)?;
        } else if ft.is_file() {
            let n = e.file_name();
            if is_rollout_of(&n.to_string_lossy(), session_ref) {
                remove_file_if_present(&p)?;
            }
        }
    }
    Ok(())
}

fn purge_codex_files(roots: &[PathBuf], session_ref: &str) -> std::io::Result<()> {
    for root in roots {
        remove_rollouts(&root.join("sessions"), session_ref, 0)?;
        remove_rollouts(&root.join("archived_sessions"), session_ref, 0)?;
        if let Ok(rd) = std::fs::read_dir(root.join("shell_snapshots")) {
            let prefix = format!("{session_ref}.");
            for f in rd.flatten() {
                if f.file_name().to_string_lossy().starts_with(&prefix) {
                    remove_file_if_present(&f.path())?;
                }
            }
        }
    }
    Ok(())
}

pub struct CodexSession {
    opts: SessionOptions,
    home: PathBuf,
    launcher: Arc<dyn Launcher>,
    session_ref: Arc<StdMutex<Option<String>>>,
    running: Arc<AtomicBool>,
    abort: Option<Arc<Notify>>,
    kill: Arc<StdMutex<Option<KillSwitch>>>,
}

/// Stops the child of the running turn.
type KillSwitch = Box<dyn FnOnce() + Send>;

impl CodexSession {
    fn current_ref(&self) -> Option<String> {
        self.session_ref.lock().ok().and_then(|r| r.clone())
    }

    fn kill_child(&self) {
        if let Ok(mut k) = self.kill.lock()
            && let Some(k) = k.take()
        {
            k();
        }
    }
}

fn err(code: &str, message: &str) -> HarnessEvent {
    HarnessEvent::Error {
        code: code.into(),
        message: message.into(),
    }
}

/// Reads the child's lines until the single terminal event.
async fn drive(
    mut lines: mpsc::Receiver<String>,
    tx: &mpsc::Sender<HarnessEvent>,
    session_ref: &Arc<StdMutex<Option<String>>>,
    abort: &Notify,
) -> HarnessEvent {
    let mut mapper = StreamMapper::new();
    loop {
        let line = tokio::select! {
            biased;
            () = abort.notified() => return err("aborted", "turn aborted"),
            l = lines.recv() => l,
        };
        let Some(line) = line else {
            return err("harness_failed", "codex exited unexpectedly");
        };
        let m = mapper.map_line(&line);
        for e in m.events {
            if let HarnessEvent::SessionRef { session_ref: r } = &e
                && let Ok(mut s) = session_ref.lock()
            {
                *s = Some(r.clone());
            }
            if tx.send(e).await.is_err() {
                return err("aborted", "turn aborted"); // receiver gone
            }
        }
        if let Some(t) = m.terminal {
            return t;
        }
    }
}

#[async_trait]
impl HarnessSession for CodexSession {
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
        // The agent can write the home between turns: render the file again right before the
        // spawn (SEC-66, `docs/12` §Adapter codex).
        let rendered = config::render_config(&self.opts).and_then(|toml| {
            write_home(&self.home, &toml).map_err(|e| {
                HarnessError::new("harness_failed", format!("cannot prepare CODEX_HOME: {e}"))
            })
        });
        if let Err(e) = rendered {
            self.running.store(false, Ordering::SeqCst);
            return Err(e);
        }
        let spec = config::launch_spec(
            &self.opts,
            &self.home,
            self.current_ref().as_deref(),
            &config::render_turn(&input),
        );
        let io = match self.launcher.launch(spec).await {
            Ok(io) => io,
            Err(e) => {
                self.running.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };
        if let Ok(mut k) = self.kill.lock() {
            *k = Some(io.kill);
        }
        let (tx, rx) = mpsc::channel(64);
        let abort = Arc::new(Notify::new());
        self.abort = Some(abort.clone());
        let session_ref = self.session_ref.clone();
        let running = self.running.clone();
        let kill = self.kill.clone();
        tokio::spawn(async move {
            let end = drive(io.lines, &tx, &session_ref, &abort).await;
            // The child is done (or being abandoned): make sure nothing lingers.
            if let Ok(mut k) = kill.lock()
                && let Some(k) = k.take()
            {
                k();
            }
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
            self.kill_child();
        }
        Ok(())
    }

    async fn release(self: Box<Self>) -> Result<(), HarnessError> {
        if let Some(a) = &self.abort
            && self.running.load(Ordering::SeqCst)
        {
            a.notify_one();
        }
        self.kill_child();
        Ok(())
    }
}

#[cfg(test)]
mod tests;

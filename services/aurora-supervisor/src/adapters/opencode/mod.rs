//! Adapter `opencode`: drives the pinned OpenCode CLI non-interactively
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, D-361, D-362, D-363, SEC-133).
//!
//! # Pinned CLI
//! [`PINNED_VERSION`] (release `opencode-linux-<x64|arm64>.tar.gz`, checksum-verified by
//! `images/sandbox/Containerfile`, installed under `/opt/aurora/harness/opencode`), the latest v1.19.x
//! line (D-363): the v1 config schema applies, not the v2 docs. The flags, the rendered files, the
//! variables and the event shapes were read from that binary and from runs against the mock
//! Messages, Responses and Chat upstreams of `aurora-testkit`; the conformance suite
//! (`tests/conformance.rs`) and the env-gated live suite (`AURORA_TEST_HARNESS_OPENCODE_LIVE=1`)
//! verify them.
//!
//! One `opencode run --format json -m aurora/<model> [--session <id>] [--variant <effort>]` child per
//! **turn**, **without `--attach`**: the run is in-process, so the MCP bridge's parent chain ends in
//! this child (D-301). The prompt is written to **stdin**, never passed as an argument
//! (`MAX_ARG_STRLEN` is 128 KiB). All of OpenCode's files live in the conversation's own directory
//! `<home>/.aurora/harness/opencode/sessions/<ref>/` (`config/`, `data/`, `cache/`, `state/`),
//! so a purge removes exactly that directory. The configuration is wiped and rendered again, and the
//! credential stores are removed, before **every** start. The session ref is the conversation's id
//! (or the resume reference); OpenCode's own `ses_…` id is kept next to it and found again with
//! `opencode db` after a restart. Abort kills the child and the session stays resumable.
//!
//! `steer` is `false`: follow-ups become separate turns and `steer()` always refuses.
//! `background_tool` is `false`. Both tested in this mode (FIX-225, `docs/12 §Turn controls by
//! harness`): `opencode_cannot_take_a_steer_or_background_a_tool_in_run_mode`. `opencode serve`
//! (`prompt_async`) and `opencode acp` take messages mid-run; not built (D-469(d)).
//!
//! No credential is ever rendered: the environment carries only the placeholder
//! (`ANTHROPIC_API_KEY` or `OPENAI_API_KEY` = `aurora-placeholder`), the per-session proxy adds the
//! turn's context credential and the gateway injects the real key.

pub mod config;
pub mod launcher;
pub mod stream;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::SendNowOutcome;
use tokio::sync::{Notify, mpsc};

use crate::adapters::render;
use crate::harness::{
    HarnessAdapter, HarnessCapabilities, HarnessError, HarnessEvent, HarnessEventStream,
    HarnessSession, SessionOptions, SteerAck, SteerInput, TurnInput,
};
use launcher::{Launcher, ProcessLauncher, StderrTail};
use stream::StreamMapper;

pub const PINNED_VERSION: &str = "1.19.6";
/// (C-1140) Where OpenCode reads skills, relative to the home folder.
pub const SKILLS_DIR: &str = ".agents/skills";
/// The harness id (`KNOWN_HARNESSES`).
pub const ID: &str = "opencode";

/// After the answer is complete the process exits at once; a child of the agent's own commands that
/// keeps the output pipe open must not hold the turn: after this long the process is stopped.
const EXIT_GRACE: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(10)
};

#[derive(Clone)]
pub struct OpenCodeAdapter {
    launcher: Arc<dyn Launcher>,
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        Self::with_launcher(Arc::new(ProcessLauncher::default()))
    }
}

impl OpenCodeAdapter {
    pub fn with_launcher(launcher: Arc<dyn Launcher>) -> Self {
        Self { launcher }
    }
}

/// Wipes and renders the root's configuration, checks the state directories one by one and removes
/// the credential stores (SEC-66, SEC-133).
fn render_root(opts: &SessionOptions, root: &Path) -> Result<(), HarnessError> {
    let fail = |e: std::io::Error| {
        HarnessError::new(
            "harness_failed",
            format!("cannot render OpenCode's configuration: {e}"),
        )
    };
    render::ensure_dir(root).map_err(fail)?;
    // Whatever an agent put in the configuration directory (agents, plugins, a second
    // `opencode.json`) is gone: only the rendered files are there.
    render::remove_any(&root.join("config")).map_err(fail)?;
    for d in config::STATE_DIRS {
        render::ensure_dir(&root.join(d)).map_err(fail)?;
    }
    let files = config::render_files(opts)?;
    render::render_dir(root, &files, &config::CREDENTIAL_FILES).map_err(fail)
}

#[async_trait]
impl HarnessAdapter for OpenCodeAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            resume: true,
            steer: false,
            background_tool: false,
            // Text arrives whole and thinking is never requested (no `--thinking`).
            thinking_deltas: false,
            tool_events: true,
            platform_tools: true,
            dialects: config::DIALECTS.iter().map(|d| (*d).to_owned()).collect(),
            // The project's and the home's `.agents/skills`, with `AGENTS.md` as the instructions.
            skills_dir: SKILLS_DIR.into(),
            instruction_files: vec!["AGENTS.md".into()],
            // `roles.small` -> `small_model`, `subagent_role` -> `agent.general.model`.
            model_roles: aurora_protocol::runner::harness_model_roles(ID)
                .iter()
                .map(|r| (*r).to_owned())
                .collect(),
        }
    }

    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        crate::adapters::claude_code::config::check_model(&opts.model)?;
        config::provider_of(&opts.model.dialect)?;
        if let Some(r) = &opts.resume_ref
            && !render::safe_ref(r)
        {
            return Err(HarnessError::new(
                "invalid_session_ref",
                "unsafe session reference",
            ));
        }
        let ref_id = opts
            .resume_ref
            .clone()
            .unwrap_or_else(|| opts.conversation_id.to_string());
        let root = render::session_dir(&opts.working_dir, ID, &ref_id);
        render_root(&opts, &root)?;
        Ok(Box::new(OpenCodeSession {
            session_ref: Arc::new(StdMutex::new(opts.resume_ref.clone())),
            started: Arc::new(AtomicBool::new(opts.resume_ref.is_some())),
            oc_session: Arc::new(StdMutex::new(None)),
            ref_id,
            opts,
            root,
            launcher: self.launcher.clone(),
            running: Arc::new(AtomicBool::new(false)),
            abort: None,
            kill: Arc::new(StdMutex::new(None)),
        }))
    }

    /// Removes the root of one session (`<home>/.aurora/harness/opencode/sessions/<ref>/`): its
    /// database, logs and rendered files. Idempotent; other sessions are untouched.
    async fn purge_session(
        &self,
        working_dir: &Path,
        session_ref: &str,
    ) -> Result<(), HarnessError> {
        if !render::safe_ref(session_ref) {
            return Err(HarnessError::new(
                "invalid_session_ref",
                "unsafe session reference",
            ));
        }
        let dir = render::session_dir(working_dir, ID, session_ref);
        tokio::task::spawn_blocking(move || render::remove_any(&dir))
            .await
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))?
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))
    }
}

/// Stops the child of the running turn.
type KillSwitch = Box<dyn FnOnce() + Send>;

pub struct OpenCodeSession {
    opts: SessionOptions,
    ref_id: String,
    root: PathBuf,
    launcher: Arc<dyn Launcher>,
    session_ref: Arc<StdMutex<Option<String>>>,
    /// OpenCode has created the session (a previous turn produced output) or a resume reference was
    /// given: the next turn continues it.
    started: Arc<AtomicBool>,
    /// OpenCode's own session id (`ses_…`), from the first event or `opencode db`.
    oc_session: Arc<StdMutex<Option<String>>>,
    running: Arc<AtomicBool>,
    abort: Option<Arc<Notify>>,
    kill: Arc<StdMutex<Option<KillSwitch>>>,
}

impl OpenCodeSession {
    fn kill_child(&self) {
        if let Ok(mut k) = self.kill.lock()
            && let Some(k) = k.take()
        {
            k();
        }
    }

    /// The `ses_…` id to continue, if the session exists: remembered, or read from the database of
    /// a restarted process. A resume reference whose data is gone starts a new session.
    async fn continue_id(&self) -> Option<String> {
        if let Some(s) = self.oc_session.lock().ok().and_then(|s| s.clone()) {
            return Some(s);
        }
        if !self.started.load(Ordering::SeqCst) {
            return None;
        }
        let spec = config::db_spec(&self.opts, &self.root, config::ROOT_SESSION_SQL);
        let id = self
            .launcher
            .query(spec)
            .await
            .ok()
            .and_then(|t| stream::session_from_rows(&t))?;
        if let Ok(mut s) = self.oc_session.lock() {
            *s = Some(id.clone());
        }
        Some(id)
    }
}

fn err(code: &str, message: &str) -> HarnessEvent {
    HarnessEvent::Error {
        code: code.into(),
        message: message.into(),
    }
}

/// What the child said on stderr, for a turn that ended without an OpenCode error event.
fn stderr_message(tail: &StderrTail, what: &str) -> String {
    let t = tail.lock().map(|t| t.join(" ")).unwrap_or_default();
    let t = t.trim();
    if t.is_empty() {
        format!("opencode {what}")
    } else {
        format!(
            "opencode {what}: {}",
            t.chars().take(300).collect::<String>()
        )
    }
}

struct Pump {
    lines: mpsc::Receiver<String>,
    stderr: StderrTail,
    exit: tokio::sync::oneshot::Receiver<Option<i32>>,
    kill: Arc<StdMutex<Option<KillSwitch>>>,
    mapper: StreamMapper,
    ref_id: String,
    session_ref: Arc<StdMutex<Option<String>>>,
    started: Arc<AtomicBool>,
    oc_session: Arc<StdMutex<Option<String>>>,
    abort: Arc<Notify>,
    launcher: Arc<dyn Launcher>,
    opts: SessionOptions,
    root: PathBuf,
    /// Wall-clock milliseconds before the child started: the turn's messages are newer.
    since_ms: u128,
}

impl Pump {
    /// Reads the child's lines until the turn ends: an error event, or the exit of the process.
    async fn drive(mut self, tx: &mpsc::Sender<HarnessEvent>) -> HarnessEvent {
        let mut announced = false;
        let mut finished_at: Option<tokio::time::Instant> = None;
        loop {
            let grace = async {
                match finished_at {
                    Some(t) => tokio::time::sleep_until(t + EXIT_GRACE).await,
                    None => std::future::pending().await,
                }
            };
            let line = tokio::select! {
                biased;
                () = self.abort.notified() => return err("aborted", "turn aborted"),
                () = grace => {
                    // The answer is complete but the process has not exited: stop it.
                    if let Ok(mut k) = self.kill.lock() && let Some(k) = k.take() { k(); }
                    return self.done().await;
                }
                l = self.lines.recv() => l,
            };
            let Some(line) = line else {
                return self.ended().await;
            };
            let m = self.mapper.map_line(&line);
            if m.json {
                // The session exists from the first event: later turns continue it.
                self.started.store(true, Ordering::SeqCst);
                if let Some(s) = &m.session
                    && let Ok(mut o) = self.oc_session.lock()
                    && o.is_none()
                {
                    *o = Some(s.clone());
                }
                if !announced {
                    announced = true;
                    if let Ok(mut s) = self.session_ref.lock() {
                        *s = Some(self.ref_id.clone());
                    }
                    let e = HarnessEvent::SessionRef {
                        session_ref: self.ref_id.clone(),
                    };
                    if tx.send(e).await.is_err() {
                        return err("aborted", "turn aborted"); // receiver gone
                    }
                }
            }
            for e in m.events {
                if tx.send(e).await.is_err() {
                    return err("aborted", "turn aborted");
                }
            }
            if let Some(e) = m.error {
                return e;
            }
            if m.finished && finished_at.is_none() {
                finished_at = Some(tokio::time::Instant::now());
            }
        }
    }

    /// The output closed: the exit status decides (exit 0 -> `Done`).
    async fn ended(&mut self) -> HarnessEvent {
        match tokio::time::timeout(Duration::from_secs(5), &mut self.exit).await {
            Ok(Ok(Some(0))) => self.done().await,
            Ok(Ok(Some(c))) => err(
                "harness_failed",
                &stderr_message(&self.stderr, &format!("exited with status {c}")),
            ),
            _ => err(
                "harness_failed",
                &stderr_message(&self.stderr, "exited unexpectedly"),
            ),
        }
    }

    /// `Done` with the per-model usage of the turn from `opencode db`, falling back to one entry
    /// for the default model summed from the steps.
    async fn done(&self) -> HarnessEvent {
        let session = self.oc_session.lock().ok().and_then(|s| s.clone());
        let mut usage = None;
        if let Some(s) = session {
            let spec = config::db_spec(
                &self.opts,
                &self.root,
                &config::usage_sql(&s, self.since_ms),
            );
            usage = self
                .launcher
                .query(spec)
                .await
                .ok()
                .and_then(|t| stream::usage_from_rows(&t));
        }
        HarnessEvent::Done {
            usage: Some(usage.unwrap_or_else(|| self.mapper.usage_fallback())),
        }
    }
}

#[async_trait]
impl HarnessSession for OpenCodeSession {
    fn session_ref(&self) -> Option<String> {
        self.session_ref.lock().ok().and_then(|r| r.clone())
    }

    async fn run_turn(&mut self, input: TurnInput) -> Result<HarnessEventStream, HarnessError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(HarnessError::new(
                "harness_failed",
                "a turn is already running",
            ));
        }
        // The agent can write the root between turns: render it again right before the spawn.
        if let Err(e) = render_root(&self.opts, &self.root) {
            self.running.store(false, Ordering::SeqCst);
            return Err(e);
        }
        let resume = self.continue_id().await;
        let spec = config::launch_spec(
            &self.opts,
            &self.root,
            resume.as_deref(),
            &config::render_turn(&input),
        );
        let since_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
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
        let pump = Pump {
            lines: io.lines,
            stderr: io.stderr,
            exit: io.exit,
            kill: self.kill.clone(),
            mapper: StreamMapper::new(&self.opts.model.model, resume.as_deref()),
            ref_id: self.ref_id.clone(),
            session_ref: self.session_ref.clone(),
            started: self.started.clone(),
            oc_session: self.oc_session.clone(),
            abort,
            launcher: self.launcher.clone(),
            opts: self.opts.clone(),
            root: self.root.clone(),
            since_ms,
        };
        let running = self.running.clone();
        let kill = self.kill.clone();
        tokio::spawn(async move {
            let end = pump.drive(&tx).await;
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

    async fn release(&mut self) -> Result<(), HarnessError> {
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

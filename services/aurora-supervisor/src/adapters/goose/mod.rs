//! Adapter `goose`: drives the pinned goose CLI non-interactively
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, D-361, D-362, SEC-133).
//!
//! # Pinned CLI
//! [`PINNED_VERSION`] (release `goose-<arch>-unknown-linux-gnu.tar.gz`, checksum-verified by
//! `images/sandbox/Containerfile`, installed under `/opt/aurora/harness/goose`). The flags, the
//! rendered files, the variables and the event shapes were read from that binary and from runs
//! against the mock Messages and Chat upstreams of `aurora-testkit`; the conformance suite
//! (`tests/conformance.rs`) and the env-gated live suite (`AURORA_TEST_HARNESS_GOOSE_LIVE=1`)
//! verify them.
//!
//! One `goose run -i - --output-format stream-json --name <ref> [--resume] --provider <p> --model <m>`
//! child per **turn** (the prompt on stdin), cwd `/workspace`, scrubbed environment. goose keeps all
//! of its files under `GOOSE_PATH_ROOT`, which is the conversation's own directory
//! `<home>/.aurora/harness/goose/sessions/<ref>/` (`config/config.yaml`, `data/sessions/sessions.db`,
//! `state/logs/`): purging a session removes exactly that directory. The rendered files
//! (`config/config.yaml` and the model catalogue under `data/model_catalog/`) are written and read
//! back before **every** start, and `config/secrets.yaml` is removed. The session ref is the
//! conversation's id (or the resume reference) and is goose's session name; `--resume` is passed once
//! the session exists (a name that does not exist is an error for `--resume`). Abort kills the child
//! and the session stays resumable.
//!
//! `steer` is `false`: follow-ups become separate turns and `steer()` always refuses.
//! `background_tool` is `false`. Both tested in this mode (FIX-225, `docs/12 §Turn controls by
//! harness`): `goose_cannot_take_a_steer_or_background_a_tool_in_run_mode`. `goose acp` / `goose
//! serve` keep live sessions; not built (D-469(d)).
//!
//! No credential is ever rendered: the environment carries only the placeholder (`ANTHROPIC_API_KEY`
//! or `LITELLM_API_KEY` = `aurora-placeholder`), the per-session proxy adds the turn's context
//! credential and the gateway injects the real key.

pub mod config;
pub mod launcher;
pub mod stream;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

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

pub const PINNED_VERSION: &str = "1.59.2";
/// (C-1140) Where goose reads skills, relative to the home folder.
pub const SKILLS_DIR: &str = ".agents/skills";
/// The harness id (`KNOWN_HARNESSES`).
pub const ID: &str = "goose";

#[derive(Clone)]
pub struct GooseAdapter {
    launcher: Arc<dyn Launcher>,
}

impl Default for GooseAdapter {
    fn default() -> Self {
        Self::with_launcher(Arc::new(ProcessLauncher::default()))
    }
}

impl GooseAdapter {
    pub fn with_launcher(launcher: Arc<dyn Launcher>) -> Self {
        Self { launcher }
    }
}

/// Renders the root's files and removes the credential stores (SEC-66, SEC-133).
fn render_root(opts: &SessionOptions, root: &Path) -> Result<(), HarnessError> {
    let files = config::render_files(opts)?;
    let forbidden: Vec<String> = config::CREDENTIAL_FILES
        .iter()
        .map(|f| format!("config/{f}"))
        .collect();
    let forbidden: Vec<&str> = forbidden.iter().map(String::as_str).collect();
    render::render_dir(&opts.working_dir, root, &files, &forbidden).map_err(|e| {
        HarnessError::new(
            "harness_failed",
            format!("cannot render goose's configuration: {e}"),
        )
    })
}

#[async_trait]
impl HarnessAdapter for GooseAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            resume: true,
            steer: false,
            background_tool: false,
            // The stream carries no thinking content.
            thinking_deltas: false,
            tool_events: true,
            platform_tools: true,
            dialects: config::DIALECTS.iter().map(|d| (*d).to_owned()).collect(),
            // goose reads `$HOME/.agents/skills` and a project's, with `AGENTS.md` (verified).
            skills_dir: SKILLS_DIR.into(),
            instruction_files: vec!["AGENTS.md".into()],
            // `subagent_role` -> `GOOSE_SUBAGENT_PROVIDER` / `GOOSE_SUBAGENT_MODEL`: any role can be it.
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
        Ok(Box::new(GooseSession {
            session_ref: Arc::new(StdMutex::new(opts.resume_ref.clone())),
            started: Arc::new(AtomicBool::new(opts.resume_ref.is_some())),
            ref_id,
            opts,
            root,
            launcher: self.launcher.clone(),
            running: Arc::new(AtomicBool::new(false)),
            abort: None,
            kill: Arc::new(StdMutex::new(None)),
        }))
    }

    /// Removes the root of one session (`<home>/.aurora/harness/goose/sessions/<ref>/`): its
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
        let home = working_dir.to_path_buf();
        let dir = render::session_dir(working_dir, ID, session_ref);
        tokio::task::spawn_blocking(move || render::remove_any(&home, &dir))
            .await
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))?
            .map_err(|e| HarnessError::new("purge_failed", e.to_string()))
    }
}

/// Stops the child of the running turn.
type KillSwitch = Box<dyn FnOnce() + Send>;

pub struct GooseSession {
    opts: SessionOptions,
    ref_id: String,
    root: PathBuf,
    launcher: Arc<dyn Launcher>,
    session_ref: Arc<StdMutex<Option<String>>>,
    /// goose has created the session (a previous turn produced output): the next turn resumes it.
    started: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    abort: Option<Arc<Notify>>,
    kill: Arc<StdMutex<Option<KillSwitch>>>,
}

impl GooseSession {
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

/// What the child said on stderr, for a turn that ended without a goose event.
fn stderr_message(tail: &StderrTail) -> String {
    let t = tail.lock().map(|t| t.join(" ")).unwrap_or_default();
    let t = t.trim();
    if t.is_empty() {
        "goose exited unexpectedly".to_owned()
    } else {
        format!(
            "goose exited unexpectedly: {}",
            t.chars().take(300).collect::<String>()
        )
    }
}

struct Pump {
    lines: mpsc::Receiver<String>,
    stderr: StderrTail,
    model: String,
    ref_id: String,
    session_ref: Arc<StdMutex<Option<String>>>,
    started: Arc<AtomicBool>,
    abort: Arc<Notify>,
}

impl Pump {
    /// Reads the child's lines until the single terminal event.
    async fn drive(mut self, tx: &mpsc::Sender<HarnessEvent>) -> HarnessEvent {
        let mut mapper = StreamMapper::new(&self.model);
        let mut announced = false;
        loop {
            let line = tokio::select! {
                biased;
                () = self.abort.notified() => return err("aborted", "turn aborted"),
                l = self.lines.recv() => l,
            };
            let Some(line) = line else {
                return err("harness_failed", &stderr_message(&self.stderr));
            };
            let m = mapper.map_line(&line);
            if m.json {
                // The session exists from the first event: later turns resume it.
                self.started.store(true, Ordering::SeqCst);
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
            if let Some(t) = m.terminal {
                return t;
            }
        }
    }
}

#[async_trait]
impl HarnessSession for GooseSession {
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
        let spec = config::launch_spec(
            &self.opts,
            &self.root,
            &self.ref_id,
            self.started.load(Ordering::SeqCst),
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
        let pump = Pump {
            lines: io.lines,
            stderr: io.stderr,
            model: self.opts.model.model.clone(),
            ref_id: self.ref_id.clone(),
            session_ref: self.session_ref.clone(),
            started: self.started.clone(),
            abort,
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

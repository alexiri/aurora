//! Adapter `pi`: drives the pinned Pi coding agent in RPC mode
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, D-361, D-362, SEC-133).
//!
//! # Pinned CLI
//! [`PINNED_VERSION`] (npm `@earendil-works/pi-coding-agent`, the tree pinned by
//! `images/sandbox/harness/pi/package-lock.json`, installed with `npm ci --ignore-scripts` under
//! `/opt/aurora/harness/pi`, Node.js 22.19 or newer). The flags, the rendered files and the event
//! shapes were read from that binary and from runs against the mock Messages, Responses and Chat
//! upstreams of `aurora-testkit` (`tests/conformance.rs`); the env-gated live suite
//! (`AURORA_TEST_HARNESS_PI_LIVE=1`) runs it through a real gateway and LiteLLM.
//!
//! One long-lived `pi --mode rpc --provider aurora --model <id> --session-dir <dir> --session-id <ref>
//! --no-approve` child per live session, cwd `/workspace`, scrubbed environment. A turn sends one RPC
//! `prompt` command and reads events until `agent_settled` (see [`stream`]). The session id is the
//! conversation's: all of Pi's state for it lives in
//! `<home>/.aurora/harness/pi/sessions/<ref>/`, which is exactly what `purge_session` removes; the
//! rendered files (`models.json`, `mcp.json`, `settings.json`) live in
//! `<home>/.aurora/harness/pi/config/`, are re-rendered and read back before every start and every
//! turn, and `auth.json` is removed each time (SEC-66, SEC-133).
//!
//! `steer` is `true`: a `prompt` command with `streamingBehavior: "steer"` sent during a run is queued
//! by Pi and delivered after the in-flight tool result and before the next model call. Sent while
//! idle Pi would start a run instead, so the adapter refuses a steer between turns itself and never
//! sends it. Abort sends the RPC `abort` and waits for `agent_settled { aborted: true }`.
//! `background_tool` is `false` (FIX-225: tested, no RPC command backgrounds a running tool). The steer
//! is covered by the `steerable` case of `pi_conforms_against_the_mock_upstream_of_every_dialect_it_declares`.
//!
//! No credential is ever rendered: `models.json` holds a `$NAME` reference to the placeholder
//! variable (`ANTHROPIC_API_KEY` or `OPENAI_API_KEY` = `aurora-placeholder`), the per-session proxy
//! adds the turn's context credential and the gateway injects the real key.

pub mod config;
pub mod launcher;
pub mod stream;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::SendNowOutcome;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::Instant;

use crate::adapters::render;
use crate::harness::{
    HarnessAdapter, HarnessCapabilities, HarnessError, HarnessEvent, HarnessEventStream,
    HarnessSession, SessionOptions, SteerAck, SteerInput, TurnInput,
};
use launcher::{ChildIo, Launcher, ProcessLauncher};
use stream::{PendingSteers, StreamMapper};

pub const PINNED_VERSION: &str = "1.5.2";
/// The harness id (`KNOWN_HARNESSES`).
pub const ID: &str = "pi";

/// How long an abort waits for Pi to settle before the child is killed.
const ABORT_GRACE: Duration = Duration::from_secs(5);
/// How long a steer waits for Pi's acknowledgement of the queued command.
const STEER_ACK: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct PiAdapter {
    launcher: Arc<dyn Launcher>,
}

impl Default for PiAdapter {
    fn default() -> Self {
        Self::with_launcher(Arc::new(ProcessLauncher::default()))
    }
}

impl PiAdapter {
    pub fn with_launcher(launcher: Arc<dyn Launcher>) -> Self {
        Self { launcher }
    }
}

/// `models.json` etc. into `<home>/.aurora/harness/pi/config/`, then the credential stores removed.
fn render_config(opts: &SessionOptions) -> Result<std::path::PathBuf, HarnessError> {
    let dir = render::config_dir(&opts.working_dir, ID);
    let files = config::render_files(opts)?;
    render::render_dir(&dir, &files, &config::CREDENTIAL_FILES).map_err(|e| {
        HarnessError::new(
            "harness_failed",
            format!("cannot render pi's configuration: {e}"),
        )
    })?;
    Ok(dir)
}

#[async_trait]
impl HarnessAdapter for PiAdapter {
    fn id(&self) -> &'static str {
        ID
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
            // Pi loads `$HOME/.agents/skills` without project trust (verified); a project's own
            // `.agents/skills` needs `--approve`, which is never passed.
            skills_dir: ".agents/skills".into(),
            instruction_files: vec!["AGENTS.md".into()],
            // Pi has no subagents: a role has no slot to go to.
            model_roles: vec![],
        }
    }

    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        crate::adapters::claude_code::config::check_model(&opts.model)?;
        config::api_of(&opts.model.dialect)?;
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
        let mut s = PiSession {
            session_ref: Arc::new(StdMutex::new(opts.resume_ref.clone())),
            ref_id,
            opts,
            launcher: self.launcher.clone(),
            live: None,
            running: Arc::default(),
            accepting: Arc::default(),
            abort: None,
            pending: PendingSteers::default(),
            steer_wait: Arc::default(),
            stray: Arc::default(),
        };
        s.ensure_child().await?;
        Ok(Box::new(s))
    }

    /// Removes the state directory of one session (`<home>/.aurora/harness/pi/sessions/<ref>/`): the
    /// session file and everything Pi kept beside it. Idempotent; other sessions are untouched.
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

/// Stops the child.
type KillSwitch = Box<dyn FnOnce() + Send>;

struct Live {
    stdin: mpsc::Sender<String>,
    lines: Arc<Mutex<mpsc::Receiver<String>>>,
    kill: Arc<StdMutex<Option<KillSwitch>>>,
    dead: Arc<AtomicBool>,
}

fn kill(live_kill: &StdMutex<Option<KillSwitch>>, dead: &AtomicBool) {
    dead.store(true, Ordering::SeqCst);
    if let Ok(mut k) = live_kill.lock()
        && let Some(k) = k.take()
    {
        k();
    }
}

/// The acknowledgement of a steer: Pi's `disposition`, or `None` when none came in time.
type SteerWait = Arc<StdMutex<Option<oneshot::Sender<Option<String>>>>>;

pub struct PiSession {
    opts: SessionOptions,
    /// The Pi session id (`--session-id`): the resume reference, or the conversation's id.
    ref_id: String,
    launcher: Arc<dyn Launcher>,
    session_ref: Arc<StdMutex<Option<String>>>,
    live: Option<Live>,
    running: Arc<AtomicBool>,
    /// True while a run is live and Pi has acknowledged the turn's prompt: only then can a steer be
    /// queued (before that, or after the run, Pi would start a run of its own).
    accepting: Arc<AtomicBool>,
    abort: Option<Arc<Notify>>,
    pending: PendingSteers,
    steer_wait: SteerWait,
    /// A steer started a run of its own (it raced the end of the turn): the next turn waits for that
    /// aborted run to settle first.
    stray: Arc<AtomicBool>,
}

impl PiSession {
    fn current_ref(&self) -> Option<String> {
        self.session_ref.lock().ok().and_then(|r| r.clone())
    }

    fn alive(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|l| !l.dead.load(Ordering::SeqCst))
    }

    /// Renders the configuration (always, before every start and turn) and starts the child if it
    /// is not running.
    async fn ensure_child(&mut self) -> Result<(), HarnessError> {
        let config_dir = render_config(&self.opts)?;
        if self.alive() {
            return Ok(());
        }
        let sessions = render::harness_dir(&self.opts.working_dir, ID).join("sessions");
        let dir = sessions.join(&self.ref_id);
        render::ensure_dir(&dir).map_err(|e| {
            HarnessError::new("harness_failed", format!("cannot prepare pi's state: {e}"))
        })?;
        let spec = config::launch_spec(&self.opts, &config_dir, &dir, &self.ref_id);
        let ChildIo { stdin, lines, kill } = self.launcher.launch(spec).await?;
        self.live = Some(Live {
            stdin,
            lines: Arc::new(Mutex::new(lines)),
            kill: Arc::new(StdMutex::new(Some(kill))),
            dead: Arc::default(),
        });
        self.accepting.store(false, Ordering::SeqCst);
        Ok(())
    }
}

struct Pump {
    tx: mpsc::Sender<HarnessEvent>,
    lines: Arc<Mutex<mpsc::Receiver<String>>>,
    stdin: mpsc::Sender<String>,
    kill: Arc<StdMutex<Option<KillSwitch>>>,
    dead: Arc<AtomicBool>,
    accepting: Arc<AtomicBool>,
    stray: Arc<AtomicBool>,
    session_ref: Arc<StdMutex<Option<String>>>,
    ref_id: String,
    abort: Arc<Notify>,
    pending: PendingSteers,
    steer_wait: SteerWait,
    prompt: String,
}

fn err(code: &str, message: &str) -> HarnessEvent {
    HarnessEvent::Error {
        code: code.into(),
        message: message.into(),
    }
}

impl Pump {
    /// Waits for the aborted run a raced steer started, so its lines do not leak into this turn.
    async fn settle_stray(&self, lines: &mut mpsc::Receiver<String>) {
        if !self.stray.swap(false, Ordering::SeqCst) {
            return;
        }
        let deadline = Instant::now() + ABORT_GRACE;
        loop {
            match tokio::time::timeout_at(deadline, lines.recv()).await {
                Ok(Some(l)) if l.contains("\"agent_settled\"") => return,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    kill(&self.kill, &self.dead);
                    return;
                }
            }
        }
    }

    /// Returns the single terminal event.
    async fn drive(&self) -> HarnessEvent {
        let mut lines = self.lines.lock().await;
        self.settle_stray(&mut lines).await;
        if self.dead.load(Ordering::SeqCst) {
            return self.exited();
        }
        let mut mapper = StreamMapper::new(self.pending.clone());
        mapper.reset_turn();
        if self
            .stdin
            .send(config::prompt_line(&self.prompt))
            .await
            .is_err()
        {
            return self.exited();
        }
        let mut acked = false;
        let mut abort_deadline: Option<Instant> = None;
        let end = loop {
            let line = tokio::select! {
                biased;
                () = self.abort.notified(), if abort_deadline.is_none() => {
                    // Ask Pi to stop and wait for it to settle; the steers it still holds are dropped.
                    let _ = self.stdin.send(config::ABORT_LINE.to_owned()).await;
                    abort_deadline = Some(Instant::now() + ABORT_GRACE);
                    continue;
                }
                () = async { tokio::time::sleep_until(abort_deadline.unwrap_or_else(Instant::now)).await },
                    if abort_deadline.is_some() => {
                    kill(&self.kill, &self.dead);
                    break err("aborted", "turn aborted");
                }
                l = lines.recv() => l,
            };
            let Some(line) = line else {
                break if abort_deadline.is_some() {
                    kill(&self.kill, &self.dead);
                    err("aborted", "turn aborted")
                } else {
                    self.exited()
                };
            };
            let m = mapper.map_line(&line);
            if let Some(r) = &m.response
                && r.command == "prompt"
            {
                if acked {
                    // The acknowledgement of a steer.
                    if let Ok(mut w) = self.steer_wait.lock()
                        && let Some(w) = w.take()
                    {
                        let _ = w.send(r.success.then(|| r.disposition.clone()).flatten());
                    }
                } else if !r.success || r.disposition.as_deref() != Some("started") {
                    break err(
                        "harness_failed",
                        &format!(
                            "pi refused the prompt: {}",
                            r.error.as_deref().unwrap_or("not started")
                        ),
                    );
                } else {
                    acked = true;
                    if let Ok(mut s) = self.session_ref.lock() {
                        *s = Some(self.ref_id.clone());
                    }
                    self.accepting
                        .store(abort_deadline.is_none(), Ordering::SeqCst);
                    let e = HarnessEvent::SessionRef {
                        session_ref: self.ref_id.clone(),
                    };
                    if self.tx.send(e).await.is_err() {
                        break err("aborted", "turn aborted"); // receiver gone
                    }
                }
            }
            for e in m.events {
                if self.tx.send(e).await.is_err() {
                    break;
                }
            }
            if let Some(t) = m.terminal {
                break t;
            }
        };
        // No steer can be queued from here on; one still waiting for its acknowledgement is lost.
        self.accepting.store(false, Ordering::SeqCst);
        if let Ok(mut p) = self.pending.lock() {
            p.clear();
        }
        if let Ok(mut w) = self.steer_wait.lock()
            && let Some(w) = w.take()
        {
            let _ = w.send(None);
        }
        end
    }

    fn exited(&self) -> HarnessEvent {
        self.dead.store(true, Ordering::SeqCst);
        err("harness_failed", "pi exited unexpectedly")
    }
}

#[async_trait]
impl HarnessSession for PiSession {
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
        // The agent can write the configuration between turns: render it again, and start the child
        // again if it died (the session is resumed from its directory).
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
            kill: live.kill.clone(),
            dead: live.dead.clone(),
            accepting: self.accepting.clone(),
            stray: self.stray.clone(),
            session_ref: self.session_ref.clone(),
            ref_id: self.ref_id.clone(),
            abort,
            pending: self.pending.clone(),
            steer_wait: self.steer_wait.clone(),
            prompt: config::render_turn(&input),
        };
        let running = self.running.clone();
        tokio::spawn(async move {
            let end = pump.drive().await;
            running.store(false, Ordering::SeqCst);
            let _ = tx.send(end).await;
        });
        Ok(rx)
    }

    async fn steer(&mut self, input: SteerInput) -> Result<SteerAck, HarnessError> {
        // Between turns (or before Pi has taken the turn's prompt) a steer would start a run of its
        // own: refuse it here, never send it.
        if !self.running.load(Ordering::SeqCst) || !self.accepting.load(Ordering::SeqCst) {
            return Ok(SteerAck::Refused);
        }
        let Some(live) = &self.live else {
            return Ok(SteerAck::Refused);
        };
        let text = config::render_steer(&input);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut w) = self.steer_wait.lock() {
            if w.is_some() {
                return Ok(SteerAck::Refused); // one steer in flight at a time
            }
            *w = Some(tx);
        }
        // Registered before it is sent, so a join can never be missed.
        if let Ok(mut p) = self.pending.lock() {
            p.push((input.followup_id, text.clone()));
        }
        let forget = |me: &Self| {
            if let Ok(mut p) = me.pending.lock() {
                p.retain(|(id, _)| *id != input.followup_id);
            }
            if let Ok(mut w) = me.steer_wait.lock() {
                *w = None;
            }
        };
        if live.stdin.send(config::steer_line(&text)).await.is_err() {
            forget(self);
            return Ok(SteerAck::Refused);
        }
        match tokio::time::timeout(STEER_ACK, rx).await {
            Ok(Ok(Some(d))) if d == "queued" => Ok(SteerAck::Accepted),
            Ok(Ok(Some(_started))) => {
                // The run had just ended and Pi started one for the steer: stop it, and let the
                // next turn wait for it to settle. The supervisor sends the text as a turn of its own.
                forget(self);
                let _ = live.stdin.send(config::ABORT_LINE.to_owned()).await;
                self.stray.store(true, Ordering::SeqCst);
                Ok(SteerAck::Refused)
            }
            _ => {
                forget(self);
                Ok(SteerAck::Refused)
            }
        }
    }

    async fn background_running_tool(&mut self) -> Result<SendNowOutcome, HarnessError> {
        Ok(SendNowOutcome::Unsupported)
    }

    async fn abort(&mut self) -> Result<(), HarnessError> {
        if self.running.load(Ordering::SeqCst)
            && let Some(a) = &self.abort
        {
            a.notify_one();
        }
        Ok(())
    }

    async fn release(self: Box<Self>) -> Result<(), HarnessError> {
        if let Some(a) = &self.abort
            && self.running.load(Ordering::SeqCst)
        {
            a.notify_one();
        }
        if let Some(l) = &self.live {
            kill(&l.kill, &l.dead);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

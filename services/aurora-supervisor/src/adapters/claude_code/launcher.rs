//! Child-process plumbing for the adapter: [`Launcher`] abstracts "start the CLI" so the session
//! logic is testable with scripted streams.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::config::LaunchSpec;
use super::sweep::{self, ChildTree};
use crate::harness::HarnessError;

pub use crate::adapters::codex::launcher::{MAX_LINE_BYTES, OVERLONG_LINE, forward_lines};

/// Default install location (`images/sandbox/Containerfile`).
pub const DEFAULT_BIN: &str = "/opt/aurora/harness/claude-code/node_modules/.bin/claude";

/// A running child: user-message lines in, stream-json lines out, and a kill switch.
pub struct ChildIo {
    pub stdin: mpsc::Sender<String>,
    /// Closed when the child's stdout ends.
    pub lines: mpsc::Receiver<String>,
    /// Starts ending the child without waiting for it.
    pub kill: Box<dyn FnOnce() + Send>,
    pub procs: Arc<dyn ChildProcs>,
}

/// (FIX-265) The child and everything it started.
#[async_trait]
pub trait ChildProcs: Send + Sync {
    /// `true` once nothing of it was left running.
    fn ended(&self) -> bool;
    /// Ends it and waits: `Ok` only when nothing of it is left. After an error it can be
    /// called again.
    async fn end(&self) -> Result<(), HarnessError>;
}

#[async_trait]
pub trait Launcher: Send + Sync {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError>;
}

/// A request to the task that owns the child: end it, and answer if asked to.
type EndRequest = Option<oneshot::Sender<Result<(), String>>>;

struct Watched {
    requests: mpsc::UnboundedSender<EndRequest>,
    ended: Arc<AtomicBool>,
}

#[async_trait]
impl ChildProcs for Watched {
    fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    async fn end(&self) -> Result<(), HarnessError> {
        let (tx, rx) = oneshot::channel();
        let answer = match self.requests.send(Some(tx)) {
            Ok(()) => rx.await.ok(),
            Err(_) => None,
        };
        match answer {
            // The task is gone: it leaves only once nothing is left, or nobody can ask.
            _ if self.ended() => Ok(()),
            Some(r) => r.map_err(|e| HarnessError::new("stop_failed", e)),
            None => Err(HarnessError::new(
                "stop_failed",
                "the harness process is no longer watched",
            )),
        }
    }
}

/// How often the leader's exit is checked without reaping it.
const EXIT_POLL: Duration = Duration::from_millis(200);

/// Owns the child until it and everything it started ended: on a request, when every handle is
/// dropped, or when the leader exits by itself (whatever it left behind dies with it). The child
/// is waited for only after that, so its pid and group stay its own meanwhile.
async fn watch_child(
    mut child: crate::spawn::OwnedChild,
    mut tree: Option<ChildTree>,
    leader: Option<i32>,
    mut requests: mpsc::UnboundedReceiver<EndRequest>,
    ended: Arc<AtomicBool>,
) {
    let mut left_behind_tried = false;
    loop {
        let (reply, hup, last) = tokio::select! {
            r = requests.recv() => match r {
                Some(reply) => (reply, true, false),
                None => (None, true, true),
            },
            () = tokio::time::sleep(EXIT_POLL), if !left_behind_tried => {
                if !leader.is_none_or(sweep::exited) {
                    continue;
                }
                left_behind_tried = true;
                (None, false, false)
            }
        };
        let result = match &mut tree {
            Some(t) => t.end(hup).await,
            None => Ok(()), // no pid: the child was already waited for
        };
        let ok = result.is_ok();
        if ok {
            let _ = child.wait().await;
            ended.store(true, Ordering::SeqCst);
        } else if let Err(e) = &result {
            tracing::warn!(target: "claude_code", "stop: {e}");
        }
        if let Some(reply) = reply {
            let _ = reply.send(result);
        }
        if ok || last {
            return;
        }
    }
}

/// Spawns the real CLI through [`crate::spawn`] (allow-listed environment plus the session env).
#[derive(Debug, Clone)]
pub struct ProcessLauncher {
    pub program: PathBuf,
}

impl Default for ProcessLauncher {
    fn default() -> Self {
        Self {
            program: std::env::var_os("AURORA_CLAUDE_BIN")
                .map_or_else(|| DEFAULT_BIN.into(), Into::into),
        }
    }
}

#[async_trait]
impl Launcher for ProcessLauncher {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError> {
        let mut cmd =
            crate::spawn::tokio_command(&self.program, spec.env.iter().map(|(k, v)| (k, v)));
        // (FIX-265) What this child starts inherits its own tag: a stop finds it wherever it
        // went in the process tree.
        let tag = uuid::Uuid::now_v7().to_string();
        cmd.args(&spec.args)
            .env(sweep::TAG_ENV, &tag)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // (FIX-253) The child leads its own process group: a `run_in_background` shell it
            // starts shares the group, so releasing the session stops the background work too —
            // the same pattern as `ExecProc` in `crate::exec`.
            .process_group(0);
        let mut child = crate::spawn::spawn(&mut cmd).map_err(|e| {
            HarnessError::new(
                "harness_failed",
                format!("cannot start {}: {e}", self.program.display()),
            )
        })?;
        let (Some(mut stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(HarnessError::new(
                "harness_failed",
                "child pipes unavailable",
            ));
        };
        let (in_tx, mut in_rx) = mpsc::channel::<String>(16);
        tokio::spawn(async move {
            while let Some(mut l) = in_rx.recv().await {
                l.push('\n');
                if stdin.write_all(l.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let (out_tx, out_rx) = mpsc::channel::<String>(256);
        tokio::spawn(forward_lines(stdout, MAX_LINE_BYTES, out_tx));
        tokio::spawn(async move {
            let mut r = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = r.next_line().await {
                tracing::debug!(target: "claude_code", "stderr: {l}");
            }
        });
        // (FIX-151) Platform-tool calls from this process's descendants act for this conversation;
        // the registration ends when the process does.
        let registration = crate::adapters::register_harness(&child, spec.conversation_id);
        // The child is the tree's root and its group (pgid == pid), read while it is unreaped.
        let leader = child.id().and_then(|p| i32::try_from(p).ok());
        let tree = leader.map(|p| ChildTree::new(p, &tag));
        let (requests, requests_rx) = mpsc::unbounded_channel::<EndRequest>();
        let ended = Arc::new(AtomicBool::new(false));
        let watched_ended = ended.clone();
        tokio::spawn(async move {
            let _registration = registration;
            watch_child(child, tree, leader, requests_rx, watched_ended).await;
        });
        let kill = requests.clone();
        Ok(ChildIo {
            stdin: in_tx,
            lines: out_rx,
            // A dropped `ChildIo` (every sender gone) ends the child the same way.
            kill: Box::new(move || {
                let _ = kill.send(None);
            }),
            procs: Arc::new(Watched { requests, ended }),
        })
    }
}

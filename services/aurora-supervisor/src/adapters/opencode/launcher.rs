//! Child-process plumbing for the OpenCode adapter: [`Launcher`] abstracts "start `opencode run`" and
//! "run `opencode db`" so the session logic is testable with scripted streams.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::config::LaunchSpec;
use crate::harness::HarnessError;

pub use crate::adapters::codex::launcher::{MAX_LINE_BYTES, OVERLONG_LINE, forward_lines};

/// Default install location (`images/sandbox/Containerfile`).
pub const DEFAULT_BIN: &str = "/opt/aurora/harness/opencode/opencode";

/// The binary the adapter runs: always the image location. Tests build `ProcessLauncher { program }`
/// themselves; the release supervisor reads no override from the environment (FIX-095).
pub fn opencode_bin() -> PathBuf {
    DEFAULT_BIN.into()
}

/// The last stderr lines of a child (what OpenCode says when it refuses to start).
pub type StderrTail = Arc<Mutex<Vec<String>>>;

/// How many stderr lines are kept.
const TAIL_LINES: usize = 8;

/// How long a database query may take.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);

/// A running `opencode run`: its stdout lines, the tail of its stderr, a kill switch and its exit
/// status (`None` = killed or unknown). The prompt has been written to its stdin, which is closed.
pub struct ChildIo {
    /// Closed when the child's stdout ends.
    pub lines: mpsc::Receiver<String>,
    pub stderr: StderrTail,
    pub kill: Box<dyn FnOnce() + Send>,
    pub exit: oneshot::Receiver<Option<i32>>,
}

#[async_trait]
pub trait Launcher: Send + Sync {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError>;
    /// Runs the spec to its end and returns its stdout (`opencode db`). A non-zero exit or a
    /// timeout is an error.
    async fn query(&self, spec: LaunchSpec) -> Result<String, HarnessError>;
}

/// Spawns the real CLI through [`crate::spawn`] (allow-listed environment plus the session env).
#[derive(Debug, Clone)]
pub struct ProcessLauncher {
    pub program: PathBuf,
}

impl Default for ProcessLauncher {
    fn default() -> Self {
        Self {
            program: opencode_bin(),
        }
    }
}

impl ProcessLauncher {
    fn command(&self, spec: &LaunchSpec) -> tokio::process::Command {
        let mut cmd =
            crate::spawn::tokio_command(&self.program, spec.env.iter().map(|(k, v)| (k, v)));
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }
}

#[async_trait]
impl Launcher for ProcessLauncher {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError> {
        let mut child = crate::spawn::spawn(&mut self.command(&spec)).map_err(|e| {
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
        let prompt = spec.stdin;
        tokio::spawn(async move {
            let _ = stdin.write_all(prompt.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
        let (out_tx, out_rx) = mpsc::channel::<String>(256);
        tokio::spawn(forward_lines(stdout, MAX_LINE_BYTES, out_tx));
        let tail: StderrTail = Arc::default();
        let tail2 = tail.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = r.next_line().await {
                tracing::debug!(target: "opencode", "stderr: {l}");
                if let Ok(mut t) = tail2.lock() {
                    t.push(l);
                    if t.len() > TAIL_LINES {
                        t.remove(0);
                    }
                }
            }
        });
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let (exit_tx, exit_rx) = oneshot::channel::<Option<i32>>();
        // (FIX-151, FIX-176) Platform-tool calls from this process's descendants act for this
        // conversation; the registration ends when the process does.
        let registration = crate::adapters::register_harness(&child, spec.conversation_id);
        tokio::spawn(async move {
            let _registration = registration;
            tokio::select! {
                st = child.wait() => { let _ = exit_tx.send(st.ok().and_then(|s| s.code())); }
                _ = kill_rx => { let _ = child.kill().await; let _ = exit_tx.send(None); }
            }
        });
        Ok(ChildIo {
            lines: out_rx,
            stderr: tail,
            kill: Box::new(move || {
                let _ = kill_tx.send(());
            }),
            exit: exit_rx,
        })
    }

    async fn query(&self, spec: LaunchSpec) -> Result<String, HarnessError> {
        let fail = |m: String| HarnessError::new("harness_failed", m);
        let mut child = crate::spawn::spawn(&mut self.command(&spec))
            .map_err(|e| fail(format!("cannot start {}: {e}", self.program.display())))?;
        drop(child.stdin.take());
        let Some(mut out) = child.stdout.take() else {
            return Err(fail("child pipes unavailable".into()));
        };
        let run = async {
            let mut buf = String::new();
            out.read_to_string(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            let st = child.wait().await.map_err(|e| e.to_string())?;
            if st.success() {
                Ok(buf)
            } else {
                Err(format!("exit status {st}"))
            }
        };
        match tokio::time::timeout(QUERY_TIMEOUT, run).await {
            Ok(Ok(s)) => Ok(s),
            Ok(Err(e)) => Err(fail(format!("opencode db failed: {e}"))),
            Err(_) => Err(fail("opencode db timed out".into())),
        }
    }
}

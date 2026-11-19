//! Child-process plumbing for the goose adapter: [`Launcher`] abstracts "start `goose run`" so the
//! session logic is testable with scripted streams.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::config::LaunchSpec;
use crate::harness::HarnessError;

pub use crate::adapters::codex::launcher::{MAX_LINE_BYTES, OVERLONG_LINE, forward_lines};

/// Default install location (`images/sandbox/Containerfile`).
pub const DEFAULT_BIN: &str = "/opt/aurora/harness/goose/goose";

/// The binary the adapter runs: always the image location. Tests build `ProcessLauncher { program }`
/// themselves; the release supervisor reads no override from the environment (FIX-095).
pub fn goose_bin() -> PathBuf {
    DEFAULT_BIN.into()
}

/// The last stderr lines of a child (what goose says when it refuses to start).
pub type StderrTail = Arc<Mutex<Vec<String>>>;

/// How many stderr lines are kept.
const TAIL_LINES: usize = 8;

/// A running `goose run`: its stdout lines, the tail of its stderr and a kill switch. The prompt
/// has been written to its stdin, which is closed.
pub struct ChildIo {
    /// Closed when the child's stdout ends.
    pub lines: mpsc::Receiver<String>,
    pub stderr: StderrTail,
    pub kill: Box<dyn FnOnce() + Send>,
}

#[async_trait]
pub trait Launcher: Send + Sync {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError>;
}

/// Spawns the real CLI through [`crate::spawn`] (allow-listed environment plus the session env).
#[derive(Debug, Clone)]
pub struct ProcessLauncher {
    pub program: PathBuf,
}

impl Default for ProcessLauncher {
    fn default() -> Self {
        Self {
            program: goose_bin(),
        }
    }
}

#[async_trait]
impl Launcher for ProcessLauncher {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError> {
        let mut cmd =
            crate::spawn::tokio_command(&self.program, spec.env.iter().map(|(k, v)| (k, v)));
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
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
                tracing::debug!(target: "goose", "stderr: {l}");
                if let Ok(mut t) = tail2.lock() {
                    t.push(l);
                    if t.len() > TAIL_LINES {
                        t.remove(0);
                    }
                }
            }
        });
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        // (FIX-151, FIX-176) Platform-tool calls from this process's descendants act for this
        // conversation; the registration ends when the process does.
        let registration = crate::adapters::register_harness(&child, spec.conversation_id);
        tokio::spawn(async move {
            let _registration = registration;
            tokio::select! {
                _ = child.wait() => {}
                _ = kill_rx => { let _ = child.kill().await; }
            }
        });
        Ok(ChildIo {
            lines: out_rx,
            stderr: tail,
            kill: Box::new(move || {
                let _ = kill_tx.send(());
            }),
        })
    }
}

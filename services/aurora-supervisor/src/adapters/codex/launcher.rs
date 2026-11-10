//! Child-process plumbing for the Codex adapter: [`Launcher`] abstracts "start `codex exec`" so
//! the session logic is testable with scripted streams.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::config::LaunchSpec;
use crate::harness::HarnessError;

/// Default install location (`images/sandbox/Containerfile`).
pub const DEFAULT_BIN: &str = "/opt/aurora/harness/codex/codex";

/// The binary the adapter runs (`AURORA_CODEX_BIN` overrides the image location, for tests).
pub fn codex_bin() -> PathBuf {
    std::env::var_os("AURORA_CODEX_BIN").map_or_else(|| DEFAULT_BIN.into(), Into::into)
}

/// A running `codex exec`: its stdout lines and a kill switch. The prompt has been written to
/// its stdin, which is closed.
pub struct ChildIo {
    /// Closed when the child's stdout ends.
    pub lines: mpsc::Receiver<String>,
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
            program: codex_bin(),
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
        tokio::spawn(async move {
            let mut r = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = r.next_line().await {
                if out_tx.send(l).await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let mut r = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = r.next_line().await {
                tracing::debug!(target: "codex", "stderr: {l}");
            }
        });
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                _ = kill_rx => { let _ = child.kill().await; }
            }
        });
        Ok(ChildIo {
            lines: out_rx,
            kill: Box::new(move || {
                let _ = kill_tx.send(());
            }),
        })
    }
}

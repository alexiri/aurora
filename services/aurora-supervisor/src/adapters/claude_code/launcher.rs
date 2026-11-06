//! Child-process plumbing for the adapter: [`Launcher`] abstracts "start the CLI" so the session
//! logic is testable with scripted streams.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use super::config::LaunchSpec;
use crate::harness::HarnessError;

/// Default install location (`images/sandbox/Containerfile`).
pub const DEFAULT_BIN: &str = "/opt/aurora/harness/claude-code/node_modules/.bin/claude";

/// A running child: user-message lines in, stream-json lines out, and a kill switch.
pub struct ChildIo {
    pub stdin: mpsc::Sender<String>,
    /// Closed when the child's stdout ends.
    pub lines: mpsc::Receiver<String>,
    pub kill: Box<dyn FnOnce() + Send>,
}

#[async_trait]
pub trait Launcher: Send + Sync {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError>;
}

/// Spawns the real CLI with a cleared environment (only `PATH`, `HOME` and the session env).
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
        let mut cmd = Command::new(&self.program);
        cmd.args(&spec.args)
            .env_clear()
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for k in ["PATH", "HOME", "LANG"] {
            if let Some(v) = std::env::var_os(k) {
                cmd.env(k, v);
            }
        }
        cmd.envs(spec.env.iter().map(|(k, v)| (k, v)));
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
                tracing::debug!(target: "claude_code", "stderr: {l}");
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
            stdin: in_tx,
            lines: out_rx,
            kill: Box::new(move || {
                let _ = kill_tx.send(());
            }),
        })
    }
}

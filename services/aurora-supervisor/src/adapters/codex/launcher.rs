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

/// The binary the adapter runs: always the image location. Tests build
/// `ProcessLauncher { program }` themselves; the release supervisor reads no override from the
/// environment (FIX-095).
pub fn codex_bin() -> PathBuf {
    DEFAULT_BIN.into()
}

/// The longest stdout line read (one JSONL event). A longer line is discarded without being
/// buffered and replaced by [`OVERLONG_LINE`].
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Delivered in place of a line longer than the bound; the stream mapper turns it into a failed
/// turn, never a success. (A raw NUL cannot occur in the CLI's JSON output.)
pub const OVERLONG_LINE: &str = "\0aurora:line-too-long";

/// Reads `\n`-terminated lines holding at most `max` bytes each in memory; an over-long line is
/// skipped to its end and reported as [`OVERLONG_LINE`]. Returns when the reader ends, errors or
/// `out` is closed.
pub async fn forward_lines<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    max: usize,
    out: mpsc::Sender<String>,
) {
    let mut r = BufReader::new(reader);
    let mut line: Vec<u8> = Vec::new();
    let mut overlong = false;
    loop {
        let Ok(chunk) = r.fill_buf().await else {
            return;
        };
        if chunk.is_empty() {
            // EOF: a final unterminated line still counts.
            if !overlong && !line.is_empty() {
                let _ = out.send(String::from_utf8_lossy(&line).into_owned()).await;
            } else if overlong {
                let _ = out.send(OVERLONG_LINE.to_owned()).await;
            }
            return;
        }
        let (take, ended) = chunk
            .iter()
            .position(|b| *b == b'\n')
            .map_or((chunk.len(), false), |i| (i, true));
        if !overlong {
            if line.len() + take > max {
                overlong = true;
                line = Vec::new(); // release what was buffered
            } else {
                line.extend_from_slice(&chunk[..take]);
            }
        }
        r.consume(take + usize::from(ended));
        if ended {
            let msg = if overlong {
                OVERLONG_LINE.to_owned()
            } else {
                String::from_utf8_lossy(&std::mem::take(&mut line)).into_owned()
            };
            overlong = false;
            if out.send(msg).await.is_err() {
                return;
            }
        }
    }
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
        tokio::spawn(forward_lines(stdout, MAX_LINE_BYTES, out_tx));
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

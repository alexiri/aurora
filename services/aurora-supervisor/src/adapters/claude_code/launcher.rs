//! Child-process plumbing for the adapter: [`Launcher`] abstracts "start the CLI" so the session
//! logic is testable with scripted streams.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::config::LaunchSpec;
use crate::harness::HarnessError;

pub use crate::adapters::codex::launcher::{MAX_LINE_BYTES, OVERLONG_LINE, forward_lines};

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

/// Grace between the group's HUP and the KILL (the same as `ExecProc` in `crate::exec`).
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// (FIX-253) Kills the whole process group: HUP, a short grace for a clean exit, then KILL. The
/// child leads the group, so its `run_in_background` shells die with it instead of being orphaned.
///
/// (FIX-253 fixes2) The KILL always follows the grace, whether or not the leader exited in it: a
/// descendant that ignores HUP (`nohup`, `trap "" HUP`) must not survive. The leader is still
/// unreaped while the group is signalled — a zombie's pid is never reused, so the KILL cannot
/// reach a stranger's group. `group` is the child's own pid (pgid == pid, `process_group(0)`),
/// taken while the child is unreaped.
async fn kill_group(mut child: tokio::process::Child, group: Option<rustix::process::Pid>) {
    if let Some(g) = group {
        let _ = rustix::process::kill_process_group(g, rustix::process::Signal::HUP);
    }
    tokio::time::sleep(KILL_GRACE).await;
    if let Some(g) = group {
        let _ = rustix::process::kill_process_group(g, rustix::process::Signal::KILL);
    }
    let _ = child.wait().await;
}

/// (FIX-253 fixes2) The KILL for a leader that is already gone by itself: whatever it left
/// behind in the group (a background shell) dies with it instead of being orphaned.
async fn kill_group_leftovers(group: Option<rustix::process::Pid>) {
    if let Some(g) = group {
        let _ = rustix::process::kill_process_group(g, rustix::process::Signal::KILL);
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
            .kill_on_drop(true)
            // (FIX-253) The child leads its own process group: a `run_in_background` shell it
            // starts shares the group, so releasing the session stops the background work too —
            // the same pattern as `ExecProc` in `crate::exec`.
            .process_group(0);
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
        tokio::spawn(forward_lines(stdout, MAX_LINE_BYTES, out_tx));
        tokio::spawn(async move {
            let mut r = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = r.next_line().await {
                tracing::debug!(target: "claude_code", "stderr: {l}");
            }
        });
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        // (FIX-151) Platform-tool calls from this process's descendants act for this conversation;
        // the registration ends when the process does.
        let registration = crate::adapters::register_harness(&child, spec.conversation_id);
        // (FIX-253 fixes2) The group the child leads, taken while it is unreaped: the group
        // kills are aimed at it, so the leader must never be reaped before they are sent.
        let group = child
            .id()
            .and_then(|p| rustix::process::Pid::from_raw(p as i32));
        tokio::spawn(async move {
            let _registration = registration;
            tokio::select! {
                res = child.wait() => {
                    let _ = res;
                    // The leader ended by itself: anything it left in the group dies with it
                    // (a descendant that ignores HUP does not survive its parent's exit).
                    kill_group_leftovers(group).await;
                }
                // A dropped `ChildIo` (the sender gone without a send) releases the same way.
                _ = kill_rx => kill_group(child, group).await,
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

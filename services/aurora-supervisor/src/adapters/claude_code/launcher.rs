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
async fn kill_group(
    mut child: tokio::process::Child,
    root: Option<rustix::process::Pid>,
    group: Option<rustix::process::Pid>,
) {
    // (FIX-253 fixes2) The descendants are collected BEFORE the signalling: the HUP may kill the
    // leader, and a dead parent's children are reparented (their ppid chain is lost to the walk)
    // while they live on. The snapshot pins each process instance by its starttime, so a KILL
    // two seconds later still only ever reaches what the walk saw.
    let pre = descendants(root);
    if let Some(g) = group {
        let _ = rustix::process::kill_process_group(g, rustix::process::Signal::HUP);
    }
    tokio::time::sleep(KILL_GRACE).await;
    kill_group_and_descendants(root, group, &pre).await;
    let _ = child.wait().await;
}

/// (FIX-253 fixes2) The KILL for a leader that is already gone by itself: whatever it left
/// behind (a background shell, detached or not) dies with it instead of being orphaned. The
/// leader is a zombie here (unreaped), so the walk still sees its children's ppid chain.
async fn kill_group_leftovers(
    root: Option<rustix::process::Pid>,
    group: Option<rustix::process::Pid>,
) {
    let sweep = descendants(root);
    kill_group_and_descendants(root, group, &sweep).await;
}

/// One descendant of the harness child: its pid and the /proc starttime that pins the process
/// instance (a dead pid's number may be reused; a reused one has another starttime and is never
/// touched).
#[cfg(target_os = "linux")]
struct Swept {
    pid: i32,
    starttime: u64,
}

/// One /proc snapshot of the child's descendants, then the KILLs: the group, and every
/// descendant of the snapshot — a background shell that started its own session (`setsid`) is in
/// no group of ours, so the group kill alone would miss it. Only pids of the snapshots are ever
/// signalled, each re-verified by its starttime: the supervisor itself and every stranger is an
/// ancestor or no descendant, never in the walk.
async fn kill_group_and_descendants(
    root: Option<rustix::process::Pid>,
    group: Option<rustix::process::Pid>,
    pre: &[Swept],
) {
    let post = descendants(root);
    if let Some(g) = group {
        let _ = rustix::process::kill_process_group(g, rustix::process::Signal::KILL);
    }
    for s in pre.iter().chain(post.iter()) {
        if Some(s.pid) == group.as_ref().map(|g| g.as_raw_pid()) {
            continue; // the group kill already reached the leader
        }
        if same_process(s) {
            if let Some(p) = rustix::process::Pid::from_raw(s.pid) {
                let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
            }
        }
    }
}

/// How deep the ppid chain is followed and how many processes one walk may ever return (a
/// harness cannot own more; a runaway /proc is never an excuse to touch strangers).
#[cfg(target_os = "linux")]
const WALK_MAX_DEPTH: usize = 32;
#[cfg(target_os = "linux")]
const WALK_MAX_COUNT: usize = 4096;

/// How often the leader's exit is checked without reaping it (the spawn task below).
#[cfg(target_os = "linux")]
const EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// (FIX-253 fixes2) The descendants of the harness child, from one /proc snapshot (pid → ppid,
/// then the breadth-first walk down the ppid chain, bounded in depth and count). Linux only: on
/// the other platforms there is no /proc and the group kill is the whole story (as before).
#[cfg(target_os = "linux")]
fn descendants(root: Option<rustix::process::Pid>) -> Vec<Swept> {
    use std::collections::{HashMap, VecDeque};
    let Some(root) = root else {
        return Vec::new();
    };
    let root = root.as_raw_pid();
    // One snapshot: every process's ppid and starttime (`/proc/<pid>/stat`; `comm` may hold
    // spaces and brackets, so the fields are read after the last ')').
    let mut parents: HashMap<i32, i32> = HashMap::new();
    let mut starts: HashMap<i32, u64> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten().take(WALK_MAX_COUNT) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue; // gone between the listing and the read
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            continue;
        };
        let f: Vec<&str> = fields.split_whitespace().collect();
        let (Some(ppid), Some(start)) = (f.get(1), f.get(19)) else {
            continue;
        };
        let (Ok(ppid), Ok(start)) = (ppid.parse::<i32>(), start.parse::<u64>()) else {
            continue;
        };
        parents.insert(pid, ppid);
        starts.insert(pid, start);
    }
    // The children index of the snapshot, then the walk down from the child.
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for (pid, ppid) in &parents {
        children.entry(*ppid).or_default().push(*pid);
    }
    let mut raw = Vec::new();
    let mut queue = VecDeque::from([(root, 0usize)]);
    while let Some((pid, depth)) = queue.pop_front() {
        if depth >= WALK_MAX_DEPTH || raw.len() >= WALK_MAX_COUNT {
            break;
        }
        for child in children.remove(&pid).unwrap_or_default() {
            raw.push(child);
            queue.push_back((child, depth + 1));
        }
    }
    raw.drain(..)
        .filter_map(|pid| {
            Some(Swept {
                pid,
                starttime: *starts.get(&pid)?,
            })
        })
        .collect()
}

/// `true` when `/proc/<pid>` is still the same process instance the walk saw (same starttime).
#[cfg(target_os = "linux")]
fn same_process(s: &Swept) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", s.pid)) else {
        return false;
    };
    stat.rsplit_once(')')
        .and_then(|(_, f)| f.split_whitespace().nth(19))
        .and_then(|t| t.parse::<u64>().ok())
        .is_some_and(|t| t == s.starttime)
}

/// `true` once the leader exited — still unreaped (a zombie keeps its pid, its group and its
/// children's ppid chain, so the release's kills never lose what they are aimed at).
#[cfg(target_os = "linux")]
fn leader_exited(pid: Option<rustix::process::Pid>) -> bool {
    let Some(p) = pid else {
        return true;
    };
    match std::fs::read_to_string(format!("/proc/{}/stat", p.as_raw_pid())) {
        Ok(stat) => stat
            .rsplit_once(')')
            .map_or(true, |(_, fields)| fields.starts_with(" Z")),
        Err(_) => true, // gone: only this task reaps the child, so nothing is left to signal
    }
}

/// The other platforms: no /proc, no walk.
#[cfg(not(target_os = "linux"))]
fn descendants(_root: Option<rustix::process::Pid>) -> Vec<rustix::process::Pid> {
    Vec::new()
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
        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        // (FIX-151) Platform-tool calls from this process's descendants act for this conversation;
        // the registration ends when the process does.
        let registration = crate::adapters::register_harness(&child, spec.conversation_id);
        // (FIX-253 fixes2) The child is both the walk root and the group (pgid == pid), taken
        // while it is unreaped: the kills and the descendant sweep are aimed at it.
        let root = child
            .id()
            .and_then(|p| rustix::process::Pid::from_raw(p as i32));
        let group = root;
        tokio::spawn(async move {
            let _registration = registration;
            #[cfg(target_os = "linux")]
            loop {
                // The leader's exit is polled, not waited: the child stays unreaped until the
                // kills are done, so the sweep still sees its descendants' ppid chain.
                tokio::select! {
                    _ = &mut kill_rx => { kill_group(child, root, group).await; return; }
                    _ = tokio::time::sleep(EXIT_POLL) => {
                        if leader_exited(group) {
                            kill_group_leftovers(root, group).await;
                            let _ = child.wait().await;
                            return;
                        }
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (root, group);
                tokio::select! {
                    res = child.wait() => {
                        let _ = res;
                        // The leader ended by itself: anything it left in the group dies with
                        // it (a descendant that ignores HUP does not survive its parent's exit).
                        kill_group_leftovers(None, group).await;
                    }
                    // A dropped `ChildIo` (the sender gone without a send) releases the same way.
                    _ = kill_rx => kill_group(child, None, group).await,
                }
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

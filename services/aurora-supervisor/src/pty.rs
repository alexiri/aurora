//! Terminal PTY sessions (phase 4, P4-TERM, D-161, SEC-87, C-619; `16 §W19` leg 2,
//! `12 §Terminal`).
//!
//! The supervisor accepts `pty.open { pty_id, window, cols, rows }` only for a boot document
//! that carries the `terminal` capability (the operator's `terminal.enabled`, C-623), spawns
//! the image shell (`/bin/sh`, or `$SHELL` if set) under a pseudo-terminal as the agent user
//! (uid/gid as the process manager uses, only when the supervisor itself is root), cwd
//! `/workspace`, own process group, and answers `pty.opened` / `pty.refused`. The environment
//! is the container environment minus every `AURORA_*` variable and minus the boot document
//! ([`crate::spawn::child_env`] posture): no platform credential, no token, no supervisor
//! handle ever appears in a terminal's environment (SEC-01/SEC-34).
//!
//! Bytes are relayed with the W18 credit machinery (`16 §W19`, binary tag `0x02` on the same
//! W6 channel), never logged, never counted beyond the informational `pty.close` totals,
//! never visible to the harness, the gateway or the loopback proxies. A terminal session is
//! **not** a process row, never a turn, never a transcript event and never stamps
//! `last_engaged_at`. A connection change (channel reset) ends every PTY; nothing is resumed.

use std::collections::{HashMap, VecDeque};
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;

use aurora_core::limits::{TERMINAL_CHUNK, TERMINAL_STREAM_WINDOW, TERMINALS_PER_SANDBOX};
use aurora_protocol::runner::{PtyCloseReason, PtyRefuseReason};
use aurora_protocol::supervisor::SupervisorToRunner;
use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use crate::channel::{self, PtyIn, StreamOut as PtyOut, StreamSender};
use crate::streams::{ClosedIds, Credit, DATA_QUEUE, Violations};

/// After the master closed (SIGHUP) a shell has this long to exit before its process group is
/// killed (`SIGKILL`): a shell that ignores HUP must not keep its terminal slot's process
/// alive (FIX-091).
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// The shell the PTY runs: the image's `$SHELL` when set and plausible, else `/bin/sh`
/// (`12 §Terminal`).
fn shell_of() -> String {
    match std::env::var("SHELL") {
        Ok(s) if s.starts_with('/') && !s.contains("..") => s,
        _ => "/bin/sh".to_owned(),
    }
}

/// The wire frame `[0x02][pty_id BE][payload]` (`16 §W19`): the tag and id are how the
/// runner's channel routes the payload to the right terminal.
fn frame_data(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(payload.len() + 5);
    b.push(0x02);
    b.extend_from_slice(&id.to_be_bytes());
    b.extend_from_slice(payload);
    b
}

/// Cloneable handle the supervisor keeps.
#[derive(Clone)]
pub struct PtyHandle {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
}

impl PtyHandle {
    /// A handle whose pump never runs and that refuses everything (tests, terminal off).
    pub fn noop() -> Self {
        let (cmd_tx, _) = mpsc::unbounded_channel();
        Self { cmd_tx }
    }

    /// Forwards a credit frame to a live PTY's task (supervisor→runner direction).
    pub async fn credit(&self, id: u32, bytes: u32) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::Credit { id, bytes, tx }).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Forwards a resize to a live PTY's task.
    pub async fn resize(&self, id: u32, cols: u16, rows: u16) -> bool {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(Cmd::Resize { id, cols, rows, tx })
            .is_err()
        {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// The runner closed its side of a PTY.
    pub async fn pty_closed(&self, id: u32) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::Closed { id, tx }).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Ends every PTY (the `shutdown` frame: `docs/12`, "closes everything on shutdown"):
    /// each shell's master closes (SIGHUP) and the runner is told with `pty.close`. Returns
    /// once the engine has done it.
    pub async fn close_all(&self) {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::CloseAll { tx }).is_ok() {
            let _ = rx.await;
        }
    }

    /// Opens a PTY from a sequenced `pty.open` (tests / older runners); the engine enforces
    /// the posture and answers. Waits for the pump to act.
    pub async fn pty_open(&self, o: aurora_protocol::runner::PtyOpen) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::Open { open: o, tx }).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Test hook: waits for the pump to process pending commands.
    #[cfg(test)]
    pub async fn settle(&self) {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.send(Cmd::Settle { tx });
        let _ = rx.await;
    }
}

enum Cmd {
    Credit {
        id: u32,
        bytes: u32,
        tx: oneshot::Sender<bool>,
    },
    Resize {
        id: u32,
        cols: u16,
        rows: u16,
        tx: oneshot::Sender<bool>,
    },
    Closed {
        id: u32,
        tx: oneshot::Sender<bool>,
    },
    Open {
        open: aurora_protocol::runner::PtyOpen,
        tx: oneshot::Sender<bool>,
    },
    /// The supervisor is shutting down: end every PTY (`pty.close { channel_lost }`).
    CloseAll {
        tx: oneshot::Sender<()>,
    },
    #[cfg(test)]
    Settle {
        tx: oneshot::Sender<()>,
    },
}

/// Report from a PTY task to the pump.
enum TaskMsg {
    /// The task ended: forward `pty.close` and free the id.
    Finished {
        id: u32,
        reason: PtyCloseReason,
        bytes_in: u64,
        bytes_out: u64,
    },
}

struct Live {
    data_tx: mpsc::Sender<Vec<u8>>,
    credit: Arc<Credit>,
    /// The latest requested size (a `watch`: the pump never waits on the task to resize).
    resize_tx: watch::Sender<(u16, u16)>,
    abort: AbortHandle,
}

/// The pump: owns the PTY table, services the channel's terminal side-channel.
pub struct PtyEngine {
    out: StreamSender,
    inbound: mpsc::UnboundedReceiver<PtyIn>,
    epoch: watch::Receiver<u64>,
    cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    task_rx: mpsc::UnboundedReceiver<TaskMsg>,
    task_tx: mpsc::UnboundedSender<TaskMsg>,
    shell: String,
    /// The boot document's `terminal` capability (`12 §Terminal`).
    enabled: bool,
    uid: Option<u32>,
    /// The shell's working directory (`/workspace` in the image; a parameter for the tests).
    cwd: std::path::PathBuf,
    /// This supervisor's own executable: the PTY shell is reached through a re-exec of it
    /// (`crate::pty_exec`; `std::env::current_exe()` in production).
    bootstrap: std::path::PathBuf,
    live: HashMap<u32, Live>,
    /// Recent unsequenced-frame violations (`16 §W19` on `16 §W18`): four within 60 s close the
    /// channel with 4400; reset with the connection.
    violations: Violations,
    /// Ids closed on this connection (data for them is dropped, not a violation).
    closed: ClosedIds,
}

impl PtyEngine {
    /// Spawns the pump on the channel's terminal ends; returns the supervisor's handle.
    /// `enabled` is the boot document's `terminal` capability; `uid` is the agent user the
    /// process manager uses (effective only when the supervisor itself is root). `cwd` is the
    /// shell's working directory (`/workspace` in the image; a parameter so the tests can
    /// point the PTY at a real directory). `bootstrap` is this supervisor's own executable
    /// (`std::env::current_exe()` in production): the PTY shell is reached through a
    /// re-exec of it (`crate::pty_exec`; the tests pass the built binary instead — the
    /// test process is not the supervisor).
    pub fn spawn(
        ends: channel::PtyEnds,
        enabled: bool,
        uid: Option<u32>,
        cwd: std::path::PathBuf,
        bootstrap: std::path::PathBuf,
    ) -> Arc<PtyHandle> {
        let channel::PtyEnds {
            sender,
            inbound,
            epoch,
        } = ends;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        let pump = Self {
            out: sender,
            inbound,
            epoch,
            cmd_rx,
            task_rx,
            task_tx: task_tx.clone(),
            shell: shell_of(),
            enabled,
            uid,
            cwd,
            bootstrap,
            live: HashMap::new(),
            violations: Violations::default(),
            closed: ClosedIds::default(),
        };
        tokio::spawn(pump.run());
        Arc::new(PtyHandle { cmd_tx })
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                msg = self.inbound.recv() => {
                    let Some(msg) = msg else { return };
                    match msg {
                        PtyIn::Frame(f) => self.on_frame(f).await,
                        PtyIn::Data { pty_id, payload } => {
                            self.on_data(pty_id, payload).await;
                        }
                        // A connection change ends every PTY (`16 §W19`); also sent after every
                        // (re)connect attempt, so this subsumes the epoch watch.
                        PtyIn::Reset => self.reset(PtyCloseReason::ChannelLost).await,
                    }
                }
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(Cmd::Credit { id, bytes, tx }) => {
                        let ok = if let Some(l) = self.live.get(&id) {
                            l.credit.add(bytes as usize);
                            true
                        } else {
                            false
                        };
                        let _ = tx.send(ok);
                    }
                    Some(Cmd::Resize { id, cols, rows, tx }) => {
                        let ok = if let Some(l) = self.live.get(&id) {
                            l.resize_tx.send_replace((cols, rows));
                            true
                        } else {
                            false
                        };
                        let _ = tx.send(ok);
                    }
                    Some(Cmd::Closed { id, tx }) => {
                        let ok = self.live.remove(&id).is_some();
                        if ok {
                            self.closed.insert(id);
                        }
                        let _ = tx.send(ok);
                    }
                    Some(Cmd::Open { open, tx }) => {
                        self.on_open(open).await;
                        let _ = tx.send(true);
                    }
                    Some(Cmd::CloseAll { tx }) => {
                        self.close_live(PtyCloseReason::ChannelLost).await;
                        let _ = tx.send(());
                    }
                    #[cfg(test)]
                    Some(Cmd::Settle { tx }) => {
                        let _ = tx.send(());
                    }
                    None => return,
                },
                msg = self.task_rx.recv() => match msg {
                    Some(TaskMsg::Finished { id, reason, bytes_in, bytes_out }) => {
                        // Only the first end of a PTY answers (kill may have raced us).
                        if self.live.remove(&id).is_some() {
                            self.closed.insert(id);
                            self.send_close(id, reason, bytes_in, bytes_out).await;
                        }
                    }
                    None => return,
                },
                _ = self.epoch.changed() => {
                    // Belt and braces: the channel also sends `Reset`; a double reset is a no-op.
                    self.reset(PtyCloseReason::ChannelLost).await;
                }
            }
        }
    }

    async fn on_frame(&mut self, f: aurora_protocol::supervisor::RunnerToSupervisor) {
        use aurora_protocol::supervisor::RunnerToSupervisor as M;
        match f {
            M::PtyOpen(o) => self.on_open(o).await,
            M::PtyResize(r) => {
                if let Some(l) = self.live.get(&r.pty_id) {
                    l.resize_tx.send_replace((r.cols, r.rows));
                }
            }
            M::PtyCredit(c) => {
                if let Some(l) = self.live.get(&c.pty_id) {
                    l.credit.add(c.bytes as usize);
                }
            }
            M::PtyClose(c) => {
                // The runner ended its side: drop ours without another close frame.
                if let Some(l) = self.live.remove(&c.pty_id) {
                    l.abort.abort();
                }
                self.closed.insert(c.pty_id);
            }
            M::Other(u) => {
                tracing::debug!(kind = u.kind.as_str(), "unknown pty frame ignored");
            }
            _ => {}
        }
    }

    /// `pty.open`: capability gate (C-623), per-sandbox cap, then spawn the shell in a PTY
    /// (`12 §Terminal`) and answer `pty.opened` / `pty.refused`.
    async fn on_open(&mut self, o: aurora_protocol::runner::PtyOpen) {
        if self.live.contains_key(&o.pty_id) {
            // A second open for a live id is a protocol violation (`16 §W19`).
            self.kill(o.pty_id, PtyCloseReason::Error).await;
            self.violate().await;
            return;
        }
        if !self.enabled {
            self.refuse(o.pty_id, PtyRefuseReason::Disabled).await;
            return;
        }
        if self.live.len() >= TERMINALS_PER_SANDBOX as usize {
            self.refuse(o.pty_id, PtyRefuseReason::Limit).await;
            return;
        }
        let pty = match PtySession::spawn(
            &self.shell,
            self.uid,
            &self.cwd,
            &self.bootstrap,
            o.cols,
            o.rows,
        ) {
            Ok(p) => p,
            Err(e) => {
                // The failure is the OS's, not the session's content: log the error kind only.
                tracing::warn!(error = e.kind().to_string(), "pty spawn failed");
                self.refuse(o.pty_id, PtyRefuseReason::Error).await;
                return;
            }
        };
        let opened = SupervisorToRunner::PtyOpened(aurora_protocol::runner::PtyOpened {
            pty_id: o.pty_id,
            window: TERMINAL_STREAM_WINDOW as u32,
        });
        if !self.send_frame(opened).await {
            return;
        }
        // The runner's announced window is our send allowance (direction shell → runner).
        let (data_tx, data_rx) = mpsc::channel(DATA_QUEUE);
        let credit = Arc::new(Credit::default());
        let (resize_tx, resize_rx) = watch::channel((o.cols, o.rows));
        let task = tokio::spawn(pty_task(
            o.pty_id,
            pty,
            o.window as usize,
            data_rx,
            credit.clone(),
            resize_rx,
            self.out.clone(),
            self.task_tx.clone(),
        ));
        // A reused id is live again: its old "closed" mark no longer applies.
        self.closed.remove(o.pty_id);
        self.live.insert(
            o.pty_id,
            Live {
                data_tx,
                credit,
                resize_tx,
                abort: task.abort_handle(),
            },
        );
    }

    /// Data from the runner: over-window or unknown-id is a protocol violation (`16 §W19`);
    /// repeated violations close the channel with 4400.
    async fn on_data(&mut self, pty_id: u32, payload: Vec<u8>) {
        let Some(l) = self.live.get_mut(&pty_id) else {
            if self.closed.contains(pty_id) {
                // A close race: closed on this connection. Dropped silently.
                tracing::debug!(pty_id, "pty data after close dropped");
            } else {
                tracing::warn!(pty_id, "pty data for unknown id");
                self.violate().await;
            }
            return;
        };
        if l.credit.outstanding() + payload.len() > TERMINAL_STREAM_WINDOW {
            tracing::warn!(pty_id, "pty data beyond the credit window");
            self.kill(pty_id, PtyCloseReason::Error).await;
            self.violate().await;
            return;
        }
        l.credit.reserve(payload.len());
        // Never wait on the terminal's task (FIX-091): a full queue means it is wedged.
        match l.data_tx.try_send(payload) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(pty_id, "pty queue full; closing the terminal");
                self.kill(pty_id, PtyCloseReason::Error).await;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!(pty_id, "pty queue closed; task is ending");
            }
        }
    }

    /// One terminal protocol violation; four within 60 s close the channel (`16 §W19`).
    async fn violate(&mut self) {
        if self.violations.record() {
            let _ = self.out.send(PtyOut::Abort).await;
        }
    }

    /// A connection change: every PTY ends, none are resumed; the violation count and the
    /// closed-id memory are per connection.
    async fn reset(&mut self, reason: PtyCloseReason) {
        self.close_live(reason).await;
        self.violations.clear();
        self.closed.clear();
    }

    /// Ends every live PTY (connection change or shutdown).
    async fn close_live(&mut self, reason: PtyCloseReason) {
        let ids: Vec<u32> = self.live.keys().copied().collect();
        for id in ids {
            self.kill(id, reason.clone()).await;
        }
    }

    /// Ends one PTY: abort the task (dropping the session ends the shell, see
    /// [`PtySession`]'s `Drop`), free the id, tell the runner.
    async fn kill(&mut self, id: u32, reason: PtyCloseReason) {
        if let Some(l) = self.live.remove(&id) {
            l.abort.abort();
            self.closed.insert(id);
            self.send_close(id, reason, 0, 0).await;
        }
    }

    async fn send_close(&mut self, id: u32, reason: PtyCloseReason, bytes_in: u64, bytes_out: u64) {
        let f = SupervisorToRunner::PtyClose(aurora_protocol::runner::PtyClose {
            pty_id: id,
            reason,
            bytes_in: (bytes_in > 0).then_some(bytes_in),
            bytes_out: (bytes_out > 0).then_some(bytes_out),
        });
        self.send_frame(f).await;
    }

    async fn refuse(&mut self, pty_id: u32, reason: PtyRefuseReason) {
        let f =
            SupervisorToRunner::PtyRefused(aurora_protocol::runner::PtyRefused { pty_id, reason });
        self.send_frame(f).await;
    }

    /// One unsequenced `pty.*` text frame; `false` when the channel (or encoding) failed.
    async fn send_frame(&mut self, msg: SupervisorToRunner) -> bool {
        let Some(text) = channel::encode_pty_frame(msg) else {
            tracing::warn!("pty frame dropped (encode)");
            return false;
        };
        self.out.send(PtyOut::Text(text)).await.is_ok()
            && self.out.send(PtyOut::Flush).await.is_ok()
    }
}

/// One live PTY: the shell's side of the pseudo-terminal (the master fd, async) plus the
/// child handle for reaping.
struct PtySession {
    master: AsyncFd<OwnedFd>,
    /// Taken by `Drop`, which hands it to a detached reaper.
    child: Option<tokio::process::Child>,
}

/// Ending a session (the task finished or was aborted): the master closes with the drop (the
/// kernel hangs the session up), and a detached task escalates — SIGHUP now, a short grace,
/// then SIGKILL to the session leader's process group — and reaps the child. Nothing on the
/// data path waits for the shell (FIX-091); a shell that ignores HUP cannot outlive its
/// terminal.
impl Drop for PtySession {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            // The bootstrap is its own session and group leader (`setsid`): pgid == pid.
            let group = child
                .id()
                .and_then(|p| rustix::process::Pid::from_raw(p as i32));
            if let Some(g) = group {
                let _ = rustix::process::kill_process_group(g, rustix::process::Signal::HUP);
            }
            if tokio::time::timeout(KILL_GRACE, child.wait())
                .await
                .is_err()
            {
                if let Some(g) = group {
                    let _ = rustix::process::kill_process_group(g, rustix::process::Signal::KILL);
                }
                let _ = child.wait().await;
            }
        });
    }
}

impl PtySession {
    /// Spawns the shell under a pseudo-terminal (`12 §Terminal`): `rustix::pty::openpt` +
    /// `grantpt`/`unlockpt`, `ptsname`, then the child with the slave as its stdio, own
    /// process group, agent uid when the supervisor is root, the given working directory
    /// (`/workspace` in the image), environment
    /// minus every `AURORA_*` variable and minus the boot document ([`crate::spawn`]).
    ///
    /// The child is a **bootstrap re-exec of this supervisor** (`pty_exec::SUBCOMMAND`,
    /// FIX-054): a shell spawned directly has no controlling terminal — std/tokio cannot
    /// `setsid`+`TIOCSCTTY` pre-exec without `unsafe` (`pre_exec`), and `#![forbid(unsafe_code)]`
    /// holds — so bash's `tcsetpgrp` fails (`cannot set terminal process group`/`no job
    /// control`). The bootstrap makes its own session, takes the slave as its controlling
    /// terminal and execs the shell in-process, so the shell *is* the session leader.
    fn spawn(
        shell: &str,
        uid: Option<u32>,
        cwd: &std::path::Path,
        bootstrap: &std::path::Path,
        cols: u16,
        rows: u16,
    ) -> std::io::Result<Self> {
        // `CLOEXEC` is a security property (FIX-059, SEC-87): without it every shell — and every
        // later child the supervisor spawns, the agent's harness included — inherits the
        // master, so the agent could read and type into the user's terminal and the session
        // would never see SIGHUP/EOF. The slave (below) is opened by std (CLOEXEC) and
        // reaches the child only as its dup'd stdio.
        let master = rustix::pty::openpt(
            rustix::pty::OpenptFlags::RDWR
                | rustix::pty::OpenptFlags::NOCTTY
                | rustix::pty::OpenptFlags::CLOEXEC,
        )?;
        rustix::pty::grantpt(&master)?;
        rustix::pty::unlockpt(&master)?;
        let name = rustix::pty::ptsname(&master, Vec::new())?;
        let name = name.to_string_lossy().into_owned();
        // `O_NOCTTY` (FIX-091 item 7, production outage 2026-11-13): the supervisor runs as PID 1,
        // a session leader without a controlling terminal, so opening the slave without it made
        // the slave the *supervisor's* ctty — and the FIX-054 bootstrap's `TIOCSCTTY` then failed
        // `EPERM` ("__pty-exec: Operation not permitted"). The slave must become the shell's ctty
        // only, in the bootstrap's own new session.
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NOCTTY.bits() as i32)
            .open(&name)?;
        // Seed `TIOCSWINSZ` before the shell starts so nothing reads a 0×0 window.
        winsize(&master, cols, rows);
        // The bootstrap posture (12 §Terminal): the container environment minus the boot
        // document and every `AURORA_*` variable (`spawn::child_env`, then an explicit
        // `AURORA_*` sweep — `12 §Terminal` is stricter than the process manager's own
        // filter: no supervisor handle, no `AURORA_*` anything), `TERM` for a sane shell,
        // cwd `/workspace`, agent uid when root. std applies environment, cwd and uid/gid
        // pre-exec, so the bootstrap starts with exactly the posture the shell must have.
        let extra = [
            ("TERM", "xterm-256color".to_owned()),
            ("COLORTERM", "truecolor".to_owned()),
        ];
        let mut cmd = crate::spawn::tokio_command(bootstrap, extra);
        for (k, _) in std::env::vars_os() {
            if k.to_str().is_some_and(|k| k.starts_with("AURORA_")) {
                cmd.env_remove(&k);
            }
        }
        cmd.arg(crate::pty_exec::SUBCOMMAND)
            .arg(shell)
            .stdin(std::process::Stdio::from(slave.try_clone()?))
            .stdout(std::process::Stdio::from(slave.try_clone()?))
            .stderr(std::process::Stdio::from(slave))
            .current_dir(cwd);
        if let Some(uid) = uid
            && rustix::process::getuid().is_root()
        {
            cmd.uid(uid).gid(uid);
        }
        // No `process_group(0)`: the bootstrap must *not* be a group leader, or `setsid`
        // would fail `EPERM` (it makes its own session — which subsumes the shell's own
        // process group).
        let child = cmd.spawn()?;
        // The master fd is used through `AsyncFd` (poll-based, safe code): nonblocking reads
        // return `EAGAIN` when the shell has not produced output, which `AsyncFd` waits for.
        rustix::fs::fcntl_setfl(&master, rustix::fs::OFlags::NONBLOCK)?;
        // `AsyncFd::new` is deprecated in favour of an `unsafe` constructor (tokio #8587);
        // for an owned `OwnedFd` the I/O-safety concern it guards against does not apply, and
        // this crate forbids `unsafe` (so the replacement is unavailable here).
        #[allow(deprecated)]
        let master = AsyncFd::new(master)?;
        Ok(Self {
            master,
            child: Some(child),
        })
    }

    /// Applies `TIOCSWINSZ` on a `pty.resize` (`12 §Terminal`).
    fn resize(&self, cols: u16, rows: u16) {
        winsize(self.master.get_ref(), cols, rows);
    }
}

/// `TIOCSWINSZ` (best effort; a failed resize keeps the old size).
fn winsize<Fd: std::os::fd::AsFd>(fd: Fd, cols: u16, rows: u16) {
    let _ = rustix::termios::tcsetwinsize(
        fd,
        rustix::termios::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    );
}

/// One PTY's copy loop: the pseudo-terminal ↔ the W6 channel's binary side-channel. Credit
/// gates the shell→runner direction (we stop reading the master at zero credit, which is the
/// backpressure toward the shell); runner→shell bytes are window-bounded by the pump. The
/// master read ending (`EIO` after the child exited and the slave closed, or 0) is the
/// shell's `eof`; other read/write failures are `hangup` (`16 §W19`).
#[allow(clippy::too_many_arguments)]
async fn pty_task(
    id: u32,
    pty: PtySession,
    send_credit: usize,
    mut data_rx: mpsc::Receiver<Vec<u8>>,
    credit_cell: Arc<Credit>,
    mut resize_rx: watch::Receiver<(u16, u16)>,
    out: StreamSender,
    report: mpsc::UnboundedSender<TaskMsg>,
) {
    tracing::debug!(pty_id = id, "terminal pty open");
    // Runner → shell bytes wait here until the shell reads them (window-bounded by the pump).
    // The write is a select arm like the others: a program that never reads stdin cannot stop
    // this task from draining credit, resizes and the queue, or from being aborted (FIX-091).
    let mut wbuf: VecDeque<u8> = VecDeque::new();
    let mut credit = send_credit;
    let mut bytes_in: u64 = 0;
    let mut bytes_out: u64 = 0;
    let mut buf = vec![0u8; TERMINAL_CHUNK];
    let reason = loop {
        tokio::select! {
            // Shell → runner. Only with credit (we stop reading the master at zero credit).
            // The wire frame is `[0x02][pty_id BE][payload]` — the runner's channel decodes it.
            read = pty.master.readable(), if credit > 0 => {
                let mut guard = match read {
                    Ok(g) => g,
                    Err(_) => break PtyCloseReason::Hangup,
                };
                // Read at most the remaining credit (FIX-059): nothing is read that cannot
                // be sent, so no byte is ever dropped; the kernel's PTY buffer provides the
                // backpressure and the shell blocks on write.
                let want = credit.min(TERMINAL_CHUNK);
                match rustix::io::read(guard.get_ref(), &mut buf[..want]) {
                    Ok(0) => break PtyCloseReason::Eof,
                    Ok(n) => {
                        // Epoll is edge-triggered: clear readiness only when the read
                        // drained the kernel buffer (a short read). A read capped by the
                        // credit may leave data behind, so readiness must stay set or the
                        // rest would never be noticed once credit returns.
                        if n < want {
                            guard.clear_ready();
                        }
                        if out.send(PtyOut::Binary(frame_data(id, &buf[..n]))).await.is_err() {
                            break PtyCloseReason::ChannelLost;
                        }
                        credit -= n;
                        bytes_out += n as u64;
                    }
                    // `EAGAIN`: the kernel buffer is empty (stale readiness, e.g. a
                    // credit-capped read drained it exactly). Clear readiness or
                    // `readable()` returns at once and the loop busy-spins.
                    Err(rustix::io::Errno::AGAIN) => { guard.clear_ready(); continue }
                    Err(_) => break PtyCloseReason::Hangup,
                }
            }
            // Runner → shell; credit returns only after the bytes were written onward.
            chunk = data_rx.recv() => {
                match chunk {
                    Some(chunk) => wbuf.extend(chunk),
                    // The pump dropped the queue (reset path aborts us anyway).
                    None => break PtyCloseReason::ChannelLost,
                }
            }
            write = pty.master.writable(), if !wbuf.is_empty() => {
                let mut guard = match write {
                    Ok(g) => g,
                    Err(_) => break PtyCloseReason::Hangup,
                };
                match rustix::io::write(guard.get_ref(), wbuf.as_slices().0) {
                    Ok(0) => break PtyCloseReason::Hangup,
                    Ok(n) => {
                        guard.clear_ready();
                        wbuf.drain(..n);
                        bytes_in += n as u64;
                        credit_cell.release(n);
                        let f = SupervisorToRunner::PtyCredit(
                            aurora_protocol::runner::PtyCredit {
                                pty_id: id,
                                bytes: n as u32,
                            },
                        );
                        if let Some(t) = channel::encode_pty_frame(f)
                            && out.send(PtyOut::Text(t)).await.is_err()
                        {
                            break PtyCloseReason::ChannelLost;
                        }
                    }
                    // Same as the read arm: clear stale readiness, no spin.
                    Err(rustix::io::Errno::AGAIN) => { guard.clear_ready(); continue }
                    Err(_) => break PtyCloseReason::Hangup,
                }
            }
            () = credit_cell.granted() => {
                credit = (credit + credit_cell.take()).min(TERMINAL_STREAM_WINDOW);
            }
            r = resize_rx.changed() => {
                if r.is_ok() {
                    let (cols, rows) = *resize_rx.borrow_and_update();
                    pty.resize(cols, rows);
                } else {
                    break PtyCloseReason::ChannelLost;
                }
            }
        }
    };
    // Informational byte totals only: never logged beyond this debug line's counters, never
    // stored (SEC-87).
    tracing::debug!(
        pty_id = id,
        ?reason,
        bytes_in,
        bytes_out,
        "terminal pty closed"
    );
    let _ = report.send(TaskMsg::Finished {
        id,
        reason,
        bytes_in,
        bytes_out,
    });
}

//! `exec.*` sessions (phase 5, P5-FILES-1, D-316; `16 §W20` leg 2, `12 §SSH channels`).
//!
//! The pipes-only engine: one child per `exec.open`, stdin from `[0x02]` data, stdout as
//! `[0x02]` and stderr as `[0x03]` data back (one credit window per `pty_id`, exactly W19's
//! machinery), `exec.eof` closes stdin once everything before it was written, and a child that
//! ended is answered with `exec.exit { status | signal }` (after its pipes drained) followed by
//! `pty.close { eof }`. P5-FILES-1 implements mode `sftp` only (the image's `sftp-server` at a
//! fixed path, no argv, agent uid/gid, cwd `/workspace`, the container environment minus every
//! `AURORA_*` variable and the boot document).
//!
//! (P4-SSH-2) `shell` (the image's shell reading commands from stdin) and `exec` (`<shell> -c
//! <command>`) run over the same pipes; with `tty` they run under a PTY instead (the terminal's
//! `PtySession` and `__pty-exec` bootstrap: own session, controlling terminal, window size from
//! `tty`), where stdout and stderr are one stream (`0x02`) and the exit status is read from the
//! reaped bootstrap, which is the shell itself. The image shell is `bash` when present, else
//! `sh`. A `command` is never logged. `sftp` is unchanged and never has a `tty`.
//!
//! The table lives inside the PTY pump (`crate::pty::PtyEngine`): `pty_id`s are one id space and
//! `pty.credit`/`pty.close`/binary data arrive on the same side-channel, so a single pump
//! demultiplexes by id. Bytes are never logged (byte totals only, debug).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use aurora_core::limits::{EXECS_PER_SANDBOX, TERMINAL_CHUNK, TERMINAL_STREAM_WINDOW};
use aurora_protocol::runner::{
    EXEC_COMMAND_MAX, ExecExit, ExecMode, ExecOpen, ExecRefuseReason, PtyCloseReason, PtyCredit,
};
use aurora_protocol::supervisor::SupervisorToRunner;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;

use crate::channel::{self, StreamOut, StreamSender};
use crate::pty::{PtySession, ResolvedShell, ShellPaths};
use crate::streams::{Credit, DATA_QUEUE};

/// The `TERM` of a `tty` whose `term` is not up to 64 printable ASCII characters.
const DEFAULT_TERM: &str = "xterm-256color";

/// The image's `sftp-server` (`12 §SSH channels`: fixed; never a path a client names).
pub const SFTP_SERVER: &str = "/usr/libexec/openssh/sftp-server";

/// After stdin closed (or the task was aborted) a child has this long to exit before its
/// process group is killed (FIX-091 posture).
const KILL_GRACE: Duration = Duration::from_secs(2);

/// One message from the pump to an exec's task, in wire order (so an `exec.eof` can never
/// overtake the data before it).
pub(crate) enum ExecIn {
    Data(Vec<u8>),
    Eof,
}

/// A finished exec reported to the pump.
pub(crate) struct ExecDone {
    pub id: u32,
    /// `None` when the task ended before the child's exit was seen (channel loss).
    pub exit: Option<ExecExit>,
    pub reason: PtyCloseReason,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

struct Live {
    data_tx: mpsc::Sender<ExecIn>,
    credit: Arc<Credit>,
    abort: AbortHandle,
    /// `tty` execs only: the window size the PTY task applies.
    resize: Option<watch::Sender<(u16, u16)>>,
}

/// What [`ExecTable::data`] did with a payload.
pub(crate) enum DataOutcome {
    Queued,
    /// The id is not a live exec.
    Unknown,
    /// More than the announced window outstanding (a protocol violation).
    OverWindow,
    /// The task's queue is full: it is wedged.
    Full,
}

/// (P5-SKILLDIR, D-454) The environment Aurora adds to a terminal or SSH child on top of the
/// client's accepted `LANG`/`LC_*`: `SKILLS_DIR`, set last so nothing overrides it. It is not in
/// the accepted-name list, so a client cannot set it, and the `AURORA_*` sweep has no exception.
pub(crate) fn with_skills_dir(
    client: &std::collections::BTreeMap<String, String>,
    skills_dir: Option<&str>,
) -> std::collections::BTreeMap<String, String> {
    let mut env = client.clone();
    if let Some(s) = skills_dir {
        env.insert("SKILLS_DIR".to_owned(), s.to_owned());
    }
    env
}

/// The live execs of one supervisor.
pub(crate) struct ExecTable {
    /// (P5-SKILLDIR) `SKILLS_DIR` for every child (absolute); `None` in tests that do not set it.
    skills_dir: Option<String>,
    live: HashMap<u32, Live>,
    sftp_server: std::path::PathBuf,
    shells: ShellPaths,
    /// This supervisor's own executable: a `tty` exec reaches its shell through the
    /// `__pty-exec` re-exec (`crate::pty_exec`).
    bootstrap: std::path::PathBuf,
}

impl ExecTable {
    pub fn new(
        sftp_server: std::path::PathBuf,
        shells: ShellPaths,
        bootstrap: std::path::PathBuf,
        skills_dir: Option<String>,
    ) -> Self {
        Self {
            skills_dir,
            live: HashMap::new(),
            sftp_server,
            shells,
            bootstrap,
        }
    }

    /// `pty.resize` for a `tty` exec; ignored for any other id.
    pub fn resize(&self, id: u32, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        if let Some(tx) = self.live.get(&id).and_then(|l| l.resize.as_ref()) {
            tx.send_replace((cols, rows));
        }
    }

    /// The image shell: `bash` when present, else `sh` (fixed paths, never from the wire).
    fn image_shell(&self) -> std::path::PathBuf {
        if self.shells.bash.is_file() {
            self.shells.bash.clone()
        } else {
            self.shells.sh.clone()
        }
    }

    pub fn contains(&self, id: u32) -> bool {
        self.live.contains_key(&id)
    }

    pub fn ids(&self) -> Vec<u32> {
        self.live.keys().copied().collect()
    }

    pub fn credit(&self, id: u32, bytes: u32) -> bool {
        self.live.get(&id).is_some_and(|l| {
            l.credit.add(bytes as usize);
            true
        })
    }

    pub fn eof(&self, id: u32) {
        if let Some(l) = self.live.get(&id) {
            // A full queue here means the task is wedged; the data path's `Full` handling
            // ends it, so a dropped eof cannot linger.
            let _ = l.data_tx.try_send(ExecIn::Eof);
        }
    }

    pub fn data(&mut self, id: u32, payload: Vec<u8>) -> DataOutcome {
        let Some(l) = self.live.get(&id) else {
            return DataOutcome::Unknown;
        };
        if l.credit.outstanding() + payload.len() > TERMINAL_STREAM_WINDOW {
            return DataOutcome::OverWindow;
        }
        l.credit.reserve(payload.len());
        match l.data_tx.try_send(ExecIn::Data(payload)) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => DataOutcome::Queued,
            Err(mpsc::error::TrySendError::Full(_)) => DataOutcome::Full,
        }
    }

    /// Drops an exec (its task is aborted, which ends the child); `true` when it was live.
    pub fn remove(&mut self, id: u32) -> bool {
        match self.live.remove(&id) {
            Some(l) => {
                l.abort.abort();
                true
            }
            None => false,
        }
    }

    /// Frees the slot of a task that finished by itself.
    pub fn finished(&mut self, id: u32) -> bool {
        self.live.remove(&id).is_some()
    }

    /// `exec.open`: the posture checks, then the child. `Ok(())` once the child runs and its
    /// task is installed (the pump then sends `exec.opened`); the caller must already have
    /// checked the id is free.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        o: &ExecOpen,
        uid: Option<u32>,
        cwd: &std::path::Path,
        out: &StreamSender,
        report: &mpsc::UnboundedSender<crate::pty::TaskMsg>,
    ) -> Result<(), ExecRefuseReason> {
        if self.live.len() >= EXECS_PER_SANDBOX as usize {
            return Err(ExecRefuseReason::Limit);
        }
        if o.env.keys().any(|k| !env_name_allowed(k)) {
            return Err(ExecRefuseReason::Unsupported);
        }
        // The program per mode; every other shape is refused (`12 §SSH channels`).
        let (program, args, is_shell): (std::path::PathBuf, Vec<String>, bool) = match o.mode {
            ExecMode::Sftp => {
                if o.command.is_some() || o.tty.is_some() || !self.sftp_server.is_file() {
                    return Err(ExecRefuseReason::Unsupported);
                }
                (self.sftp_server.clone(), Vec::new(), false)
            }
            ExecMode::Shell => {
                if o.command.is_some() {
                    return Err(ExecRefuseReason::Unsupported);
                }
                (self.image_shell(), Vec::new(), true)
            }
            ExecMode::Exec => match &o.command {
                Some(c) if c.len() <= EXEC_COMMAND_MAX && !c.contains('\0') => {
                    (self.image_shell(), vec!["-c".to_owned(), c.clone()], true)
                }
                _ => return Err(ExecRefuseReason::Unsupported),
            },
            _ => return Err(ExecRefuseReason::Unsupported),
        };
        let env = with_skills_dir(&o.env, self.skills_dir.as_deref());
        let (data_tx, data_rx) = mpsc::channel(DATA_QUEUE);
        let credit = Arc::new(Credit::default());
        let (abort, resize) = if let Some(tty) = &o.tty {
            if tty.cols == 0 || tty.rows == 0 {
                return Err(ExecRefuseReason::Unsupported);
            }
            let term = if !tty.term.is_empty()
                && tty.term.len() <= 64
                && tty.term.bytes().all(|b| (0x21..=0x7e).contains(&b))
            {
                tty.term.as_str()
            } else {
                DEFAULT_TERM
            };
            let shell = ResolvedShell {
                program,
                args,
                notice: None,
            };
            let pty = match PtySession::spawn(
                &shell,
                uid,
                cwd,
                &self.bootstrap,
                tty.cols,
                tty.rows,
                term,
                &env,
            ) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = e.kind().to_string(), "exec pty spawn failed");
                    return Err(ExecRefuseReason::Error);
                }
            };
            let (resize_tx, resize_rx) = watch::channel((tty.cols, tty.rows));
            let task = tokio::spawn(exec_tty_task(
                o.pty_id,
                pty,
                o.window as usize,
                data_rx,
                credit.clone(),
                resize_rx,
                out.clone(),
                report.clone(),
            ));
            (task.abort_handle(), Some(resize_tx))
        } else {
            let proc = match ExecProc::spawn(&program, &args, is_shell, uid, cwd, &env) {
                Ok(p) => p,
                Err(e) => {
                    // The OS's error kind only, never anything about the session's bytes.
                    tracing::warn!(error = e.kind().to_string(), "exec spawn failed");
                    return Err(ExecRefuseReason::Error);
                }
            };
            let task = tokio::spawn(exec_task(
                o.pty_id,
                proc,
                o.window as usize,
                data_rx,
                credit.clone(),
                out.clone(),
                report.clone(),
            ));
            (task.abort_handle(), None)
        };
        self.live.insert(
            o.pty_id,
            Live {
                data_tx,
                credit,
                abort,
                resize,
            },
        );
        Ok(())
    }
}

/// `LANG` and `LC_*` only (`12 §SSH channels`); `AURORA_*` never.
fn env_name_allowed(k: &str) -> bool {
    (k == "LANG" || k.starts_with("LC_")) && k.len() <= 64 && !k.starts_with("AURORA_")
}

/// `[tag][pty_id BE][payload]`: tag `0x02` stdout, `0x03` stderr.
fn frame_data(tag: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(payload.len() + 5);
    b.push(tag);
    b.extend_from_slice(&id.to_be_bytes());
    b.extend_from_slice(payload);
    b
}

/// The spawned child with its pipe ends. Ending it (drop) hangs it up and kills its process
/// group after the grace, in a detached reaper — nothing on the data path waits for it.
struct ExecProc {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl ExecProc {
    /// `sftp-server` (no argv) or the image shell (`shell`/`exec`, with `SHELL` set) over pipes:
    /// own process group, agent uid when the supervisor is root, cwd `/workspace`, the container
    /// environment minus `AURORA_*` and the boot document (`spawn::child_env`) plus the accepted
    /// `LANG`/`LC_*`. Pipe ends are `CLOEXEC` (std/tokio's pipes), so no other child inherits
    /// them (FIX-059 posture).
    fn spawn(
        path: &std::path::Path,
        args: &[String],
        is_shell: bool,
        uid: Option<u32>,
        cwd: &std::path::Path,
        env: &std::collections::BTreeMap<String, String>,
    ) -> std::io::Result<Self> {
        let shell_var = is_shell.then(|| ("SHELL", path.to_string_lossy().into_owned()));
        let extra = [("PATH", crate::spawn::IMAGE_PATH.to_owned())]
            .into_iter()
            .chain(shell_var)
            .chain(env.iter().map(|(k, v)| (k.as_str(), v.clone())));
        let mut cmd = crate::spawn::tokio_command(path, extra);
        cmd.args(args);
        for (k, _) in std::env::vars_os() {
            if k.to_str().is_some_and(|k| k.starts_with("AURORA_")) {
                cmd.env_remove(&k);
            }
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(cwd)
            .process_group(0);
        if let Some(uid) = uid
            && rustix::process::getuid().is_root()
        {
            cmd.uid(uid).gid(uid);
        }
        let mut child = cmd.spawn()?;
        Ok(Self {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            child: Some(child),
        })
    }
}

impl Drop for ExecProc {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            // The child leads its own group (`process_group(0)`): pgid == pid.
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

/// The name of a terminating signal without `SIG`.
fn signal_name(n: i32) -> String {
    match n {
        1 => "HUP",
        2 => "INT",
        3 => "QUIT",
        4 => "ILL",
        6 => "ABRT",
        7 => "BUS",
        8 => "FPE",
        9 => "KILL",
        10 => "USR1",
        11 => "SEGV",
        12 => "USR2",
        13 => "PIPE",
        14 => "ALRM",
        15 => "TERM",
        other => return other.to_string(),
    }
    .to_owned()
}

fn exit_of(id: u32, st: std::process::ExitStatus) -> ExecExit {
    use std::os::unix::process::ExitStatusExt as _;
    match (st.code(), st.signal()) {
        (Some(c), _) => ExecExit {
            pty_id: id,
            status: Some(c),
            signal: None,
        },
        (None, Some(s)) => ExecExit {
            pty_id: id,
            status: None,
            signal: Some(signal_name(s)),
        },
        (None, None) => ExecExit {
            pty_id: id,
            status: Some(255),
            signal: None,
        },
    }
}

/// One exec's copy loop: runner → stdin (window-bounded by the pump; credit returns after the
/// bytes were written onward), stdout/stderr → runner (only with credit: nothing is read that
/// cannot be sent, so the pipe's own buffer is the backpressure on the child).
async fn exec_task(
    id: u32,
    mut proc: ExecProc,
    send_credit: usize,
    mut data_rx: mpsc::Receiver<ExecIn>,
    credit_cell: Arc<Credit>,
    out: StreamSender,
    report: mpsc::UnboundedSender<crate::pty::TaskMsg>,
) {
    tracing::debug!(pty_id = id, "exec open");
    let mut stdin = proc.stdin.take();
    let mut stdout = proc.stdout.take();
    let mut stderr = proc.stderr.take();
    let mut wbuf: VecDeque<u8> = VecDeque::new();
    let mut eof_pending = false;
    let mut credit = send_credit;
    let mut bytes_in: u64 = 0;
    let mut bytes_out: u64 = 0;
    let mut obuf = vec![0u8; TERMINAL_CHUNK];
    let mut ebuf = vec![0u8; TERMINAL_CHUNK];
    let lost = loop {
        if stdout.is_none() && stderr.is_none() {
            break false;
        }
        let want = credit.min(TERMINAL_CHUNK);
        tokio::select! {
            n = async {
                match stdout.as_mut() {
                    Some(s) => s.read(&mut obuf[..want]).await,
                    None => std::future::pending().await,
                }
            },
                if want > 0 && stdout.is_some() => match n {
                Ok(n) if n > 0 => {
                    if out.send(StreamOut::Binary(frame_data(0x02, id, &obuf[..n]))).await.is_err() {
                        break true;
                    }
                    credit -= n;
                    bytes_out += n as u64;
                }
                _ => stdout = None,
            },
            n = async {
                match stderr.as_mut() {
                    Some(s) => s.read(&mut ebuf[..want]).await,
                    None => std::future::pending().await,
                }
            },
                if want > 0 && stderr.is_some() => match n {
                Ok(n) if n > 0 => {
                    if out.send(StreamOut::Binary(frame_data(0x03, id, &ebuf[..n]))).await.is_err() {
                        break true;
                    }
                    credit -= n;
                    bytes_out += n as u64;
                }
                _ => stderr = None,
            },
            msg = data_rx.recv() => match msg {
                Some(ExecIn::Data(chunk)) => {
                    if stdin.is_some() {
                        wbuf.extend(chunk);
                    } else {
                        // The child closed its stdin: discard, but hand the credit back so the
                        // runner's window does not stall.
                        let n = chunk.len();
                        credit_cell.release(n);
                        if !send_credit_frame(&out, id, n).await {
                            break true;
                        }
                    }
                }
                Some(ExecIn::Eof) => {
                    eof_pending = true;
                    if wbuf.is_empty() {
                        stdin = None;
                    }
                }
                None => break true,
            },
            w = async {
                match stdin.as_mut() {
                    Some(s) => s.write(wbuf.make_contiguous()).await,
                    None => std::future::pending().await,
                }
            },
                if stdin.is_some() && !wbuf.is_empty() => match w {
                Ok(n) if n > 0 => {
                    wbuf.drain(..n);
                    bytes_in += n as u64;
                    credit_cell.release(n);
                    if !send_credit_frame(&out, id, n).await {
                        break true;
                    }
                    if eof_pending && wbuf.is_empty() {
                        stdin = None;
                    }
                }
                _ => {
                    // EPIPE or a closed pipe: the child no longer reads. Free what was queued.
                    let n = wbuf.len();
                    wbuf.clear();
                    stdin = None;
                    if n > 0 {
                        credit_cell.release(n);
                        if !send_credit_frame(&out, id, n).await {
                            break true;
                        }
                    }
                }
            },
            () = credit_cell.granted() => {
                credit = (credit + credit_cell.take()).min(TERMINAL_STREAM_WINDOW);
            }
        }
    };
    if lost {
        tracing::debug!(pty_id = id, bytes_in, bytes_out, "exec closed (channel)");
        let _ = report.send(crate::pty::TaskMsg::ExecFinished(ExecDone {
            id,
            exit: None,
            reason: PtyCloseReason::ChannelLost,
            bytes_in,
            bytes_out,
        }));
        return; // `proc` drops: hang-up then kill
    }
    // Both pipes ended: the output is drained. Close stdin (the child may be waiting on it) and
    // collect the status.
    drop(stdin);
    let status = match proc.child.as_mut() {
        Some(c) => c.wait().await.ok(),
        None => None,
    };
    proc.child = None; // reaped: nothing for `Drop` to kill
    tracing::debug!(pty_id = id, bytes_in, bytes_out, "exec closed");
    let _ = report.send(crate::pty::TaskMsg::ExecFinished(ExecDone {
        id,
        exit: Some(status.map_or(
            ExecExit {
                pty_id: id,
                status: Some(255),
                signal: None,
            },
            |s| exit_of(id, s),
        )),
        reason: PtyCloseReason::Eof,
        bytes_in,
        bytes_out,
    }));
}

/// Ends the adapter task with its exec.
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A `tty` exec: the terminal's PTY copy loop (`crate::pty::pty_io`), then the child's exit
/// status. `exec.eof` is dropped (a PTY has no half-close; the client's `^D` is a byte), and the
/// window size follows `pty.resize`.
#[allow(clippy::too_many_arguments)]
async fn exec_tty_task(
    id: u32,
    mut pty: PtySession,
    send_credit: usize,
    mut data_rx: mpsc::Receiver<ExecIn>,
    credit_cell: Arc<Credit>,
    resize_rx: watch::Receiver<(u16, u16)>,
    out: StreamSender,
    report: mpsc::UnboundedSender<crate::pty::TaskMsg>,
) {
    tracing::debug!(pty_id = id, "exec pty open");
    // The PTY loop takes plain byte chunks; `exec.eof` has no meaning here.
    let (pty_tx, pty_rx) = mpsc::channel::<Vec<u8>>(DATA_QUEUE);
    let adapter = tokio::spawn(async move {
        while let Some(m) = data_rx.recv().await {
            if let ExecIn::Data(d) = m
                && pty_tx.send(d).await.is_err()
            {
                break;
            }
        }
    });
    let _adapter = AbortOnDrop(adapter.abort_handle());
    let (reason, bytes_in, bytes_out) =
        crate::pty::pty_io(id, &pty, send_credit, pty_rx, credit_cell, resize_rx, &out).await;
    if matches!(reason, PtyCloseReason::ChannelLost) {
        tracing::debug!(
            pty_id = id,
            bytes_in,
            bytes_out,
            "exec pty closed (channel)"
        );
        let _ = report.send(crate::pty::TaskMsg::ExecFinished(ExecDone {
            id,
            exit: None,
            reason: PtyCloseReason::ChannelLost,
            bytes_in,
            bytes_out,
        }));
        return; // `pty` drops: hang-up then kill
    }
    // The master read ended: the session is over. Reap the shell for its status; a straggler
    // gets the process-group kill after the grace.
    let status = match pty.take_child() {
        Some(mut child) => {
            let group = child
                .id()
                .and_then(|p| rustix::process::Pid::from_raw(p as i32));
            match tokio::time::timeout(KILL_GRACE, child.wait()).await {
                Ok(s) => s.ok(),
                Err(_) => {
                    if let Some(g) = group {
                        let _ =
                            rustix::process::kill_process_group(g, rustix::process::Signal::KILL);
                    }
                    child.wait().await.ok()
                }
            }
        }
        None => None,
    };
    tracing::debug!(pty_id = id, bytes_in, bytes_out, "exec pty closed");
    let _ = report.send(crate::pty::TaskMsg::ExecFinished(ExecDone {
        id,
        exit: Some(status.map_or(
            ExecExit {
                pty_id: id,
                status: Some(255),
                signal: None,
            },
            |s| exit_of(id, s),
        )),
        reason: PtyCloseReason::Eof,
        bytes_in,
        bytes_out,
    }));
}

async fn send_credit_frame(out: &StreamSender, id: u32, n: usize) -> bool {
    let f = SupervisorToRunner::PtyCredit(PtyCredit {
        pty_id: id,
        bytes: n as u32,
    });
    match channel::encode_pty_frame(f) {
        Some(t) => out.send(StreamOut::Text(t)).await.is_ok(),
        None => true,
    }
}

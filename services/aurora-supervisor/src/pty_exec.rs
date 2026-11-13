//! The PTY bootstrap (`__pty-exec`, FIX-054): a hidden subcommand the supervisor re-executes
//! to give a terminal shell a **controlling terminal** without `unsafe`.
//!
//! `tokio::process` can only run `pre_exec` hooks through an `unsafe fn`, and this crate
//! forbids `unsafe` (`12 §Terminal` needs `setsid` + `TIOCSCTTY` for job control). The safe
//! equivalent is the classic two-step spawn: the supervisor spawns **itself** as a tiny
//! bootstrap (fresh process, therefore single-threaded at that instant) with the pty slave as
//! its stdio, and the bootstrap — safe `rustix` calls only — makes its own session
//! (`setsid`), takes fd 0 as its controlling terminal (`TIOCSCTTY`; explicit, so the
//! acquire does not depend on the open-by-session-leader behaviour), and then execs the
//! shell in-process (`crate::spawn::exec`), so the shell **is** the session leader the
//! terminal belongs to. Bash's `tcsetpgrp` then works and job control lives.
//!
//! Posture: the supervisor computed the environment (`crate::spawn::child_env` plus the
//! `AURORA_*` sweep plus `TERM`/`COLORTERM`) and the uid/gid drop and the cwd **before**
//! spawning this bootstrap — std applies them pre-exec, so the bootstrap starts with
//! exactly the environment and privilege the shell must have; the bootstrap only re-uses
//! its own environ for the exec. A failed step is reported on the pty itself (the
//! bootstrap's stderr is the slave) and exits non-zero; the PTY then ends as `hangup`
//! through the ordinary master-EIO path.

use std::process::ExitCode;

/// The hidden subcommand name (`aurora-supervisor __pty-exec <shell>`).
pub const SUBCOMMAND: &str = "__pty-exec";

/// `setsid()` + `TIOCSCTTY` on fd 0: one fresh process, no runtime to disturb.
fn make_session() -> std::io::Result<()> {
    // The child is not a process-group leader (the supervisor did not spawn it with
    // `process_group(0)` — `setsid` would fail `EPERM`), so this always succeeds and makes
    // this process the session (and group) leader of a new session.
    rustix::process::setsid()?;
    // fd 0/1/2 are the pty slave (the supervisor spawned this bootstrap with the slave as
    // its stdio). As a session leader without a controlling terminal, `TIOCSCTTY` on the
    // slave makes it ours; the slave is not yet any other session's ctty — the supervisor
    // opens it with `O_NOCTTY` (`pty::PtySession::spawn`; as PID 1 it is a session leader
    // without a ctty and would otherwise adopt the slave itself, FIX-091 item 7) — so no
    // privilege is needed (the uid drop, when any, already happened pre-exec).
    rustix::process::ioctl_tiocsctty(std::io::stdin())?;
    Ok(())
}

/// The `__pty-exec` subcommand: session + controlling terminal, then exec the shell.
/// Diverges on success (`exec` replaces this process); a failure prints on the pty and
/// exits non-zero.
pub fn run(shell: &str) -> ExitCode {
    if let Err(e) = make_session() {
        eprintln!("aurora-supervisor {SUBCOMMAND}: {e}");
        return ExitCode::FAILURE;
    }
    // The environment was scrubbed by the supervisor before spawning this bootstrap
    // (`PtySession::spawn`); exec the shell with exactly that environ. On success `exec`
    // never returns.
    let env: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os().collect();
    let e = crate::spawn::exec(shell, env);
    eprintln!("aurora-supervisor {SUBCOMMAND}: exec {shell}: {e}");
    ExitCode::FAILURE
}

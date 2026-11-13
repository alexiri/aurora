//! (FIX-059, SEC-87) The terminal's PTY master never leaks into a child process, closing a
//! terminal ends its shell, and output beyond the credit is held back (never dropped).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use crate::common::TempDir;
use crate::pty_env_probe::{open, rig};
use aurora_protocol::runner::{PtyClose, PtyCloseReason, PtyCredit, PtyOpen};
use aurora_protocol::supervisor::RunnerToSupervisor;
use aurora_supervisor::channel::StreamOut;

/// Targets of every open descriptor of `pid` (`/proc/<pid>/fd`).
fn fd_targets(pid: u32) -> Vec<String> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap_or_else(|e| panic!("/proc/{pid}/fd: {e}"))
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .map(|p| p.display().to_string())
        .collect()
}

fn is_ptmx(target: &str) -> bool {
    target == "/dev/ptmx" || target == "/dev/pts/ptmx"
}

/// Asks the shell on `id` for its pid. The sentinel is spelled in pieces so the PTY's echo
/// of the input cannot match; only the shell's own output can.
async fn shell_pid(r: &mut crate::pty_env_probe::Rig, id: u32) -> u32 {
    r.data(id, b"echo SHPID_\"$$\"_\"END\"\n").await;
    let out = r.until("_END").await;
    let start = out.rfind("SHPID_").expect("pid marker") + "SHPID_".len();
    let digits: String = out[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("no pid in {out:?}"))
}

/// No descriptor of any terminal shell, nor of a child spawned afterwards through the
/// ordinary process manager, points at the PTY multiplexer (the masters are CLOEXEC).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_master_never_leaks_into_children() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    open(&mut r, 2).await;
    let p1 = shell_pid(&mut r, 1).await;
    let p2 = shell_pid(&mut r, 2).await;

    // Sanity: the supervisor (this process) does hold both masters, so the check below can
    // fail — it is not vacuous.
    let mine = fd_targets(std::process::id())
        .iter()
        .filter(|t| is_ptmx(t))
        .count();
    assert!(mine >= 2, "the engine's own masters are missing ({mine})");

    // An ordinary child spawned after both terminals exist (the agent's harness path).
    let mut child = aurora_supervisor::spawn::tokio_command("sleep", [("X", "1")])
        .arg("30")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pc = child.id().unwrap();

    for (name, pid) in [("shell 1", p1), ("shell 2", p2), ("later child", pc)] {
        let leaked: Vec<String> = fd_targets(pid).into_iter().filter(|t| is_ptmx(t)).collect();
        assert!(
            leaked.is_empty(),
            "{name} (pid {pid}) holds a PTY master: {leaked:?}"
        );
    }
    let _ = child.kill().await;
}

/// Closing a terminal leg closes the master, so the shell gets SIGHUP and exits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_terminal_ends_its_shell() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    let pid = shell_pid(&mut r, 1).await;
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "shell not running"
    );
    r.frame(RunnerToSupervisor::PtyClose(PtyClose {
        pty_id: 1,
        reason: PtyCloseReason::Eof,
        bytes_in: None,
        bytes_out: None,
    }))
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while std::path::Path::new(&format!("/proc/{pid}")).exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "shell {pid} still alive 2 s after the terminal closed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// With a 4 KiB window and a shell producing 1 MiB, every byte arrives (none dropped) and
/// the runner returns credit piecewise as it drains.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_beyond_the_credit_is_held_not_dropped() {
    const TOTAL: usize = 1 << 20;
    const WINDOW: u32 = 4096;
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    r.frame(RunnerToSupervisor::PtyOpen(PtyOpen {
        pty_id: 1,
        window: WINDOW,
        cols: 80,
        rows: 24,
    }))
    .await;
    assert_eq!(r.next_frame().await["kind"], "pty.opened");

    // `stty -opost` keeps the line discipline from rewriting `\n`; the payload is 'a'
    // bytes between two markers so the echoed command line cannot be confused with it.
    r.data(
        1,
        format!(
            "stty -echo -opost; head -c {TOTAL} /dev/zero | tr '\\0' a; echo; echo DONE_\"MARK\"\n"
        )
        .as_bytes(),
    )
    .await;

    let mut got = 0usize;
    let mut pieces = 0usize;
    let mut tail = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {got} of {TOTAL} bytes arrived"
        );
        let unit = tokio::time::timeout(Duration::from_secs(5), r.next_out())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "output stalled after {got} of {TOTAL} bytes ({pieces} chunks), tail {:?}",
                    String::from_utf8_lossy(&tail)
                )
            });
        match unit {
            StreamOut::Binary(b) => {
                let payload = &b[5..];
                assert!(
                    payload.len() <= WINDOW as usize,
                    "a chunk exceeded the window"
                );
                got += payload.len();
                tail.extend_from_slice(payload);
                pieces += 1;
                // Return credit piecewise, exactly what was drained.
                r.frame(RunnerToSupervisor::PtyCredit(PtyCredit {
                    pty_id: 1,
                    bytes: payload.len() as u32,
                }))
                .await;
                if String::from_utf8_lossy(&tail).contains("DONE_MARK") {
                    break;
                }
            }
            StreamOut::Text(_) | StreamOut::Flush => {}
            StreamOut::Abort => panic!("engine aborted the channel"),
        }
    }
    // The longest unbroken run of 'a' is exactly the produced payload: nothing dropped
    // (the run would be shorter), nothing duplicated (longer).
    let (mut run, mut longest) = (0usize, 0usize);
    for c in &tail {
        run = if *c == b'a' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    assert_eq!(longest, TOTAL, "bytes lost under backpressure (got {got})");
    assert!(
        pieces >= TOTAL / WINDOW as usize,
        "credit was not returned piecewise ({pieces} chunks)"
    );
}

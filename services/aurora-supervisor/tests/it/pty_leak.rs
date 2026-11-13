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

/// (FIX-073, `docs/18 §Web terminal`) The supervisor's `shutdown` frame ends every PTY: each
/// shell exits (its master closed) and the runner is told `pty.close { channel_lost }` for
/// every id, before `run` returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_closes_every_pty() {
    use aurora_supervisor::channel::ChannelSender;
    use aurora_supervisor::fake::FakeHarness;
    use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig};

    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    open(&mut r, 2).await;
    let p1 = shell_pid(&mut r, 1).await;
    let p2 = shell_pid(&mut r, 2).await;

    let (out_tx, _out_rx) = tokio::sync::mpsc::channel(256);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(8);
    let mut cfg = SupervisorConfig::new(crate::common::dead_upstream(), "aub_x");
    cfg.working_dir = home.path().to_path_buf();
    cfg.process = aurora_supervisor::process::ProcessConfig::new(home.path().to_path_buf());
    cfg.process.uid = None;
    let sup = Supervisor::new(
        cfg,
        ChannelSender::from_mpsc(out_tx),
        std::sync::Arc::new(FakeHarness),
        aurora_supervisor::streams::StreamsHandle::noop(),
        r.handle(),
    );
    let run = tokio::spawn(sup.run(in_rx));
    in_tx
        .send(
            serde_json::from_value(
                serde_json::json!({"kind": "shutdown", "body": {"grace_secs": 1}}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .expect("the supervisor never finished its shutdown")
        .unwrap();

    // The runner was told for each PTY.
    let mut closed = std::collections::BTreeSet::new();
    while closed.len() < 2 {
        let f = r.next_frame().await;
        if f["kind"] == "pty.close" {
            assert_eq!(f["body"]["reason"], "channel_lost", "{f}");
            closed.insert(f["body"]["pty_id"].as_u64().unwrap());
        }
    }
    assert_eq!(closed, [1u64, 2].into_iter().collect());
    // And both shells are gone.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    for pid in [p1, p2] {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "shell {pid} survived the supervisor's shutdown"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// Waits for `pid` to disappear from `/proc` (reaped), bounded.
async fn gone_within(pid: u32, d: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + d;
    while std::path::Path::new(&format!("/proc/{pid}")).exists() {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

/// FIX-091 item 5: a shell that ignores SIGHUP does not survive its terminal: the engine
/// escalates to SIGKILL on the session's process group after a short grace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_ignores_hup_is_killed_when_its_terminal_closes() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    let pid = shell_pid(&mut r, 1).await;
    // Ignores SIGHUP and keeps running (an idle interactive shell would exit on the master's
    // EOF anyway; a busy one is what the escalation is for).
    r.data(
        1,
        b"trap '' HUP; echo TRAP_\"SET\"; while :; do sleep 1; done\n",
    )
    .await;
    r.until("TRAP_SET").await;
    r.frame(RunnerToSupervisor::PtyClose(PtyClose {
        pty_id: 1,
        reason: PtyCloseReason::Eof,
        bytes_in: None,
        bytes_out: None,
    }))
    .await;
    assert!(
        gone_within(pid, Duration::from_secs(8)).await,
        "a shell ignoring SIGHUP outlived its closed terminal"
    );
}

/// FIX-091 item 2: a program that never reads its stdin stalls only its own terminal. The fake
/// runner pushes input until the kernel stops accepting it (credit stops coming back), then
/// closes the terminal and opens two more — before the fix the pump awaited the terminal's
/// 16-slot queue, so `pty.close` was never read and the slots stayed taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_never_reads_stalls_no_other_terminal() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    // `exec sleep`: nothing reads the terminal's input from here on. Raw mode: a canonical
    // line discipline silently drops input beyond its 4 KiB line buffer, raw mode makes the
    // writer wait (as for a real program that has stopped reading).
    r.data(1, b"stty raw -echo; echo STALL_\"READY\"; exec sleep 600\n")
        .await;
    r.until("STALL_READY").await;

    const WINDOW: usize = 131_072;
    const FRAME: usize = 1000;
    let mut unacked = 0usize;
    let mut sent = 0usize;
    loop {
        while unacked + FRAME <= WINDOW {
            r.data(1, &[b'x'; FRAME]).await;
            unacked += FRAME;
            sent += 1;
        }
        // A quiet second means the terminal stopped accepting input.
        let mut got = false;
        while let Some(out) = r.next_out_within(Duration::from_secs(1)).await {
            if let StreamOut::Text(t) = out {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                if v["kind"] == "pty.credit" {
                    unacked -= v["body"]["bytes"].as_u64().unwrap() as usize;
                    got = true;
                    break;
                }
            }
        }
        if !got {
            break;
        }
        assert!(sent < 1_000_000, "the terminal never stalled");
    }
    assert!(sent > 17, "stalled below the old queue depth ({sent})");

    // The engine still reads control frames: close the stalled terminal, open two new ones.
    r.frame(RunnerToSupervisor::PtyClose(PtyClose {
        pty_id: 1,
        reason: PtyCloseReason::Eof,
        bytes_in: None,
        bytes_out: None,
    }))
    .await;
    open(&mut r, 2).await;
    open(&mut r, 3).await;
}

/// FIX-091 item 3: a wedged engine cannot hold shutdown past its grace. The engine's outbound
/// queue is full and nobody drains it (a stuck channel), so telling the runner `pty.close`
/// blocks; `shutdown` still finishes within the grace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_with_a_wedged_engine_finishes_within_the_grace() {
    use aurora_supervisor::channel::{ChannelSender, PtyEnds, StreamSender};
    use aurora_supervisor::fake::FakeHarness;
    use aurora_supervisor::pty::PtyEngine;
    use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig};

    let home = TempDir::new();
    let (pty_out_tx, mut pty_out_rx) = tokio::sync::mpsc::channel(4);
    let (pty_in_tx, pty_in_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_epoch_tx, epoch_rx) = tokio::sync::watch::channel(0u64);
    let handle = PtyEngine::spawn(
        PtyEnds {
            sender: StreamSender::from_mpsc(pty_out_tx.clone()),
            inbound: pty_in_rx,
            epoch: epoch_rx,
        },
        true,
        None,
        home.path().to_path_buf(),
        crate::pty_env_probe::bootstrap_exe(),
    );
    pty_in_tx
        .send(aurora_supervisor::channel::PtyIn::Frame(
            RunnerToSupervisor::PtyOpen(PtyOpen {
                pty_id: 1,
                window: 131_072,
                cols: 80,
                rows: 24,
            }),
        ))
        .unwrap();
    // Wait for `pty.opened`, then wedge the queue: nothing drains it from here on.
    loop {
        let out = tokio::time::timeout(Duration::from_secs(10), pty_out_rx.recv())
            .await
            .expect("no pty.opened")
            .expect("engine ended");
        if let StreamOut::Text(t) = out
            && t.contains("pty.opened")
        {
            break;
        }
    }
    while pty_out_tx.try_send(StreamOut::Flush).is_ok() {}

    let (out_tx, _out_rx) = tokio::sync::mpsc::channel(256);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(8);
    let mut cfg = SupervisorConfig::new(crate::common::dead_upstream(), "aub_x");
    cfg.working_dir = home.path().to_path_buf();
    cfg.process = aurora_supervisor::process::ProcessConfig::new(home.path().to_path_buf());
    cfg.process.uid = None;
    let sup = Supervisor::new(
        cfg,
        ChannelSender::from_mpsc(out_tx),
        std::sync::Arc::new(FakeHarness),
        aurora_supervisor::streams::StreamsHandle::noop(),
        (*handle).clone(),
    );
    let run = tokio::spawn(sup.run(in_rx));
    in_tx
        .send(
            serde_json::from_value(
                serde_json::json!({"kind": "shutdown", "body": {"grace_secs": 1}}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), run)
        .await
        .expect("a wedged PTY engine held the supervisor's shutdown past its grace")
        .unwrap();
}

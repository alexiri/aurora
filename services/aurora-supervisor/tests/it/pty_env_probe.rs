//! (P4-TERM, `16 §W19` on `12 §Terminal`) The supervisor's PTY engine against the real
//! pump: `pty.open` spawns the image shell in a pseudo-terminal (cwd parameter, own session
//! and controlling terminal via the `pty_exec` bootstrap — job control works, FIX-054;
//! env minus every `AURORA_*` — the SEC-34 probe reads the PTY's own `environ`),
//! `pty.resize` applies `TIOCSWINSZ`, bytes round-trip through the credit machinery under
//! the window bounds, `pty.refused` answers `disabled`/`limit`, and a connection change
//! (`PtyIn::Reset`) ends every PTY with `channel_lost` — nothing is resumed. Everything
//! rides the real channel ends (`PtyEnds` built by hand, like the production wiring), so
//! the tag `0x02` framing and the unsequenced `pty.*` frame shape are exercised as wired.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use crate::common::TempDir;
use aurora_protocol::common::Validate as _;
use aurora_protocol::runner::{PtyCloseReason, PtyRefuseReason, PtyResize};
use aurora_protocol::supervisor::{Frame, RunnerToSupervisor, SupervisorToRunner};
use aurora_supervisor::channel::{self, PtyIn, StreamOut};
use aurora_supervisor::pty::PtyEngine;
use serde_json::Value;
use tokio::sync::mpsc;

const T: Duration = Duration::from_secs(10);

/// The bootstrap executable for the rig: the built `aurora-supervisor` binary (the test
/// process is a libtest binary, not the supervisor — the bootstrap re-exec must be the
/// real one, which understands `pty_exec::SUBCOMMAND`).
fn bootstrap_exe() -> std::path::PathBuf {
    // The path comes from nextest's runtime variable when running from an archive built
    // elsewhere (CI's shard runners never compile), from the re-exec parent's explicit
    // handoff (`spawn::command` scrubs the environment, so the `sh` in
    // `pty_environment_has_no_aurora_or_boot_trace` cannot inherit nextest's variables),
    // and from the compile-time path as the fallback for plain `cargo test`.
    if let Some(p) = std::env::var_os("PTY_PROBE_BOOTSTRAP") {
        return std::path::PathBuf::from(p);
    }
    std::env::var_os("NEXTEST_BIN_EXE_aurora-supervisor")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_aurora-supervisor")))
}

/// The channel ends a PTY engine runs on, built like `channel::spawn` builds them, plus the
/// receiving half the test reads the engine's frames/data from.
struct Rig {
    handle: std::sync::Arc<aurora_supervisor::pty::PtyHandle>,
    pty_tx: mpsc::UnboundedSender<PtyIn>,
    out_rx: mpsc::Receiver<StreamOut>,
    /// Kept alive: a dropped sender would resolve the engine's `epoch.changed()` and end
    /// every PTY with `channel_lost` immediately.
    _epoch_tx: tokio::sync::watch::Sender<u64>,
    /// `pty.credit` byte counts seen while draining output (`until` intercepts the text
    /// frames; each written input chunk yields exactly one credit frame).
    credits: Vec<u64>,
}

/// A PTY engine over hand-built ends, rooted at `home` (the cwd parameter replaces the
/// image's `/workspace`).
fn rig(home: &std::path::Path, enabled: bool) -> Rig {
    let (pty_out_tx, pty_out_rx) = mpsc::channel(1024);
    let (pty_in_tx, pty_in_rx) = mpsc::unbounded_channel();
    let (epoch_tx, epoch_rx) = tokio::sync::watch::channel(0u64);
    let ends = channel::PtyEnds {
        sender: channel::StreamSender::from_mpsc(pty_out_tx),
        inbound: pty_in_rx,
        epoch: epoch_rx,
    };
    let handle = PtyEngine::spawn(ends, enabled, None, home.to_path_buf(), bootstrap_exe());
    Rig {
        handle,
        pty_tx: pty_in_tx,
        out_rx: pty_out_rx,
        _epoch_tx: epoch_tx,
        credits: Vec::new(),
    }
}

impl Rig {
    /// Drives a `pty.*` control frame in (as the runner's channel pump would).
    async fn frame(&self, msg: RunnerToSupervisor) {
        let _ = self.pty_tx.send(PtyIn::Frame(msg));
    }

    /// Drives binary terminal data in (`[0x02][pty_id BE][payload]` decoded by the channel).
    async fn data(&self, pty_id: u32, payload: &[u8]) {
        let _ = self.pty_tx.send(PtyIn::Data {
            pty_id,
            payload: payload.to_vec(),
        });
    }

    /// The engine's next outgoing unit (a text `pty.*` frame, binary data or a flush).
    async fn next_out(&mut self) -> StreamOut {
        tokio::time::timeout(T, self.out_rx.recv())
            .await
            .expect("engine output timed out")
            .expect("engine output ended")
    }

    /// The next `pty.*` text frame (skips binary data and flushes).
    async fn next_frame(&mut self) -> Value {
        loop {
            match self.next_out().await {
                StreamOut::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["kind"].as_str().unwrap_or("").starts_with("pty.") {
                        return v;
                    }
                }
                StreamOut::Binary(_) | StreamOut::Flush => {}
                StreamOut::Abort => panic!("engine aborted the channel"),
            }
        }
    }

    /// The next binary data chunk: `(pty_id, payload)` without the `[0x02][id]` header.
    #[allow(dead_code)]
    async fn next_data(&mut self) -> (u32, Vec<u8>) {
        loop {
            match self.next_out().await {
                StreamOut::Binary(b) => {
                    assert_eq!(b[0], 0x02, "terminal data is tag 0x02");
                    let id = u32::from_be_bytes([b[1], b[2], b[3], b[4]]);
                    return (id, b[5..].to_vec());
                }
                StreamOut::Text(_) | StreamOut::Flush => {}
                StreamOut::Abort => panic!("engine aborted the channel"),
            }
        }
    }

    /// Shell output until `needle` appears (as text, CRLF-line-discipline included). Credit
    /// frames seen on the way are recorded (each written input chunk yields one).
    async fn until(&mut self, needle: &str) -> String {
        let mut out = String::new();
        let deadline = tokio::time::Instant::now() + T;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "\"{needle}\" never arrived: {out}");
            match tokio::time::timeout(remaining, self.next_out())
                .await
                .unwrap()
            {
                StreamOut::Binary(b) => {
                    assert_eq!(b[0], 0x02, "shell output is tag 0x02, {b:?}");
                    out.push_str(&String::from_utf8_lossy(&b[5..]));
                }
                StreamOut::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["kind"].as_str() == Some("pty.credit") {
                        self.credits.push(v["body"]["bytes"].as_u64().unwrap());
                    }
                    continue;
                }
                StreamOut::Flush => continue,
                StreamOut::Abort => panic!("engine aborted the channel"),
            }
            if out.contains(needle) {
                return out;
            }
        }
    }
}

/// `pty.open` with the given id; asserts `pty.opened` and returns the announced window.
/// A `pty.refused{reason: "error"}` is the OS refusing the spawn (fork/pty pressure —
/// nextest runs every test in its own process and CI runners get resource-starved
/// mid-shard); that is transient pressure, not a posture failure, so the open is retried
/// with exponential backoff (~25 s total). Any other refusal is asserted through — only
/// `error` can be transient (`limit`/`disabled` are deterministic). If the window
/// exhausts, two direct probes (a raw `openpt` and a bare fork of the bootstrap) name the
/// real errno in the panic, so one failing CI run identifies the resource.
async fn open(rig: &mut Rig, pty_id: u32) -> u64 {
    const BACKOFF_MS: [u64; 8] = [100, 200, 400, 800, 1600, 3200, 6400, 12800];
    for wait in std::iter::once(0u64).chain(BACKOFF_MS) {
        if wait > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        rig.frame(RunnerToSupervisor::PtyOpen(
            aurora_protocol::runner::PtyOpen {
                pty_id,
                window: 131_072,
                cols: 80,
                rows: 24,
            },
        ))
        .await;
        let opened = rig.next_frame().await;
        if opened["kind"] == "pty.refused" {
            assert_eq!(
                opened["body"]["reason"], "error",
                "pty {pty_id}: non-transient refusal: {opened}"
            );
            continue;
        }
        assert_eq!(opened["kind"], "pty.opened", "{opened}");
        assert_eq!(opened["body"]["pty_id"], pty_id);
        return opened["body"]["window"].as_u64().unwrap();
    }
    // Diagnostics: reproduce the spawn's steps directly and surface their exact errors —
    // the engine's `pty.refused{error}` deliberately carries no errno. Probe 1: ptmx
    // allocation. Probe 2: a full `PtySession` spawn against the test's own engine-free
    // path, printing the io::Error of whichever step actually fails.
    let openpt = match rustix::pty::openpt(
        rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY,
    ) {
        Ok(_) => "openpt ok".to_owned(),
        Err(e) => format!("openpt FAILED: {e}"),
    };
    let direct = match aurora_supervisor::pty::debug_spawn_session(
        "/bin/sh",
        None,
        &std::env::temp_dir(),
        &bootstrap_exe(),
        80,
        24,
    ) {
        Ok(_) => "direct spawn ok".to_owned(),
        Err(e) => format!("direct spawn FAILED: {e}"),
    };
    let fork = match aurora_supervisor::spawn::command(
        bootstrap_exe(),
        std::iter::empty::<(&str, &str)>(),
    )
    .arg("no-such-subcommand")
    .output()
    {
        Ok(_) => "fork ok".to_owned(),
        Err(e) => format!("fork FAILED: {e}"),
    };
    panic!(
        "pty {pty_id}: spawn refused after 8 attempts (~25 s). Probes: {openpt}; {direct}; {fork}"
    );
}

/// The echo round trip (W19 on W18's credit machinery): runner bytes reach the shell, the
/// shell's output comes back framed `0x02`, and credit returns after the write onward.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_echo_round_trip_with_credit() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    let window = open(&mut r, 1).await;
    assert_eq!(window, 131_072);

    // Runner → shell: a typed command (the PTY echoes it back first — line discipline).
    r.data(1, b"echo aurora-terminal-$((40+2))\n").await;
    let out = r.until("aurora-terminal-42").await;
    assert!(
        out.contains("echo aurora-terminal-"),
        "no echo of input: {out}"
    );

    // The engine returned credit for the bytes it wrote onward (`pty.credit`).
    assert_eq!(
        r.credits.len(),
        1,
        "one credit per written chunk: {:?}",
        r.credits
    );
    assert!(r.credits[0] > 0);

    // The runner ends the PTY (`pty.close`): the engine drops its side without another
    // close. (The slot release is asserted by the cap test, where the close and the next
    // open ride the same in-order queue — here the handle call would race the frame.)
    r.frame(RunnerToSupervisor::PtyClose(
        aurora_protocol::runner::PtyClose {
            pty_id: 1,
            reason: PtyCloseReason::Eof,
            bytes_in: None,
            bytes_out: None,
        },
    ))
    .await;
}

/// The SEC-34 probe: the PTY's own environment has no `AURORA_*` variable and no boot-doc
/// trace, even though the supervisor itself runs with `AURORA_BOOT` set. Like
/// `spawn_env::children_never_receive_boot`, the outer half re-executes the test binary with
/// the variables set (no `unsafe set_var`); `inner_probe` runs the real assertions there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_environment_has_no_aurora_or_boot_trace() {
    if std::env::var_os("AURORA_BOOT").is_some() {
        probe_env_assertions().await; // direct run with the variables set (debugging)
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let out = aurora_supervisor::spawn::command("sh", std::iter::empty::<(&str, &str)>())
        .args([
            "-c",
            "AURORA_BOOT=leak-me FOO=aub_leak-me PTY_PROBE_BOOTSTRAP=\"$1\" exec \"$0\" --exact pty_env_probe::inner_probe --nocapture",
        ])
        .arg(exe)
        .arg(bootstrap_exe())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The inner half of the SEC-34 probe (only meaningful when re-executed with the boot
/// variables set; the outer half above drives the re-execution and asserts success).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inner_probe() {
    if std::env::var_os("AURORA_BOOT").is_some() {
        probe_env_assertions().await;
    }
}

/// The probe itself: open a PTY, read its own `/proc/self/environ`, assert the posture.
/// `cat` (not `tr < file`): a shell-opened redirect + exec invalidates the mm the open
/// captured, so reads of `/proc/self/environ` come back empty after the exec. The sentinel
/// is spelled `PROBE_"OK"` in the command so the PTY's echo of the input cannot match
/// `PROBE_OK` — only the shell's own output can — and `until` therefore sees the *whole*
/// environ (a substring stop at `PATH=` would race `LD_LIBRARY_PATH=` ahead of `PATH=`).
async fn probe_env_assertions() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    r.data(1, b"cat /proc/self/environ; echo PROBE_\"OK\"\n")
        .await;
    let env = r.until("PROBE_OK").await;
    assert!(
        !env.contains("AURORA_"),
        "AURORA_* leaked into the PTY: {env}"
    );
    assert!(
        !env.contains("leak-me"),
        "the boot value leaked into the PTY: {env}"
    );
    // TERM is set for a sane shell; ordinary container env still passes (the environ is
    // NUL-separated, so the entry boundary check is exact).
    assert!(env.contains("TERM=xterm-256color"), "{env}");
    assert!(env.split('\0').any(|l| l.starts_with("PATH=")), "{env}");
}

/// The FIX-054 probe: the shell has a **controlling terminal** — the `pty_exec` bootstrap
/// made its own session and took the slave as the ctty, so job control works. `tty` prints
/// a `/dev/pts/N` path; the shell's output carries neither of the owner's two error lines
/// ("Inappropriate ioctl", "no job control"); `kill -0 $$` round-trips (the reaped-pid
/// check); `$-` shows `m` (monitor mode: job control enabled); and `sleep 30 &` + `jobs`
/// lists a real background job (a ctty-less shell cannot run one). The plain echo round
/// trip at the end keeps the input path covered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_shell_has_a_controlling_terminal_and_job_control() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    // The sentinels are spelled in pieces so the PTY's echo of the input cannot match
    // them — only the shell's own output can (the `pty_env_probe` playbook).
    r.data(
        1,
        b"tty; echo CTTY_\"DONE\"; kill -0 $$ && echo ALIVE_\"OK\"; \
          echo flags=$-; sleep 30 & jobs; kill %1; echo JOBS_\"OK\"\n",
    )
    .await;
    let out = r.until("JOBS_OK").await;
    assert!(
        !out.contains("Inappropriate ioctl"),
        "the shell still lacks its terminal: {out}"
    );
    assert!(
        !out.contains("no job control"),
        "job control is dead: {out}"
    );
    // `tty` printed a pts path (a substring: the line discipline interleaves the shell's
    // prompt escape sequences with the answer), not "not a tty".
    assert!(
        out.contains("/dev/pts/"),
        "`tty` did not report a pts: {out}"
    );
    assert!(!out.contains("not a tty"), "`tty` failed: {out}");
    assert!(out.contains("CTTY_DONE"), "the probe never finished: {out}");
    assert!(out.contains("ALIVE_OK"), "kill -0 $$ failed: {out}");
    // Monitor mode (`m` in `$-`) is exactly "job control works": bash enables it only
    // when the ctty is real (`isatty` + successful `tcsetpgrp`).
    assert!(
        out.contains("flags=himBH") || out.contains("flags=imBH"),
        "monitor mode is off, job control is dead: {out}"
    );
    assert!(
        out.contains("JOBS_OK"),
        "the jobs probe never finished: {out}"
    );
    // A plain echo round trip so the input path stays covered in this probe too.
    r.data(1, b"echo ECHO_\"OK\"\n").await;
    r.until("ECHO_OK").await;
}

/// The cwd is the parameter (the image's `/workspace`; here the temp home): `pwd` says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_starts_in_the_given_cwd() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    r.data(1, b"pwd\n").await;
    let home_s = home.path().display().to_string();
    let out = r.until(&home_s).await;
    assert!(out.contains(&home_s), "cwd {home_s} not seen: {out}");
}

/// `pty.refused {disabled}` when the boot document lacks the `terminal` capability (C-623).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_open_without_capability_is_refused_disabled() {
    let home = TempDir::new();
    let mut r = rig(home.path(), false);
    r.frame(RunnerToSupervisor::PtyOpen(
        aurora_protocol::runner::PtyOpen {
            pty_id: 1,
            window: 131_072,
            cols: 80,
            rows: 24,
        },
    ))
    .await;
    let refused = r.next_frame().await;
    assert_eq!(refused["kind"], "pty.refused", "{refused}");
    assert_eq!(
        refused["body"]["reason"],
        PtyRefuseReason::Disabled.as_str()
    );
}

/// `TERMINALS_PER_SANDBOX` (2) holds: the third concurrent open is `pty.refused {limit}`,
/// and a closed PTY frees its slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_cap_per_sandbox_and_slot_release() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    open(&mut r, 2).await;
    r.frame(RunnerToSupervisor::PtyOpen(
        aurora_protocol::runner::PtyOpen {
            pty_id: 3,
            window: 131_072,
            cols: 80,
            rows: 24,
        },
    ))
    .await;
    let refused = r.next_frame().await;
    assert_eq!(refused["kind"], "pty.refused", "{refused}");
    assert_eq!(refused["body"]["reason"], PtyRefuseReason::Limit.as_str());
    assert_eq!(refused["body"]["pty_id"], 3);

    // A `pty.close` from the runner frees the slot; a fourth id opens.
    r.frame(RunnerToSupervisor::PtyClose(
        aurora_protocol::runner::PtyClose {
            pty_id: 1,
            reason: PtyCloseReason::Eof,
            bytes_in: None,
            bytes_out: None,
        },
    ))
    .await;
    // The engine processes frames in order: the close above lands before this open.
    open(&mut r, 4).await;
}

/// `pty.resize` applies `TIOCSWINSZ`: the shell sees the new size (`stty size`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_resize_applies_tiocswinsz() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    r.frame(RunnerToSupervisor::PtyResize(PtyResize {
        pty_id: 1,
        cols: 137,
        rows: 41,
    }))
    .await;
    r.data(1, b"stty size\n").await;
    let out = r.until("41 137").await;
    assert!(out.contains("41 137"), "resize not applied: {out}");
}

/// A connection change (`PtyIn::Reset`, sent on every W6 drop/reconnect): every PTY ends
/// with `channel_lost`; nothing is resumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_reset_ends_every_pty_with_channel_lost() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    open(&mut r, 2).await;

    // The channel drop (the pump sends `PtyIn::Reset` on every connection change).
    let _ = r.pty_tx.send(PtyIn::Reset);

    let mut closed = Vec::new();
    for _ in 0..2 {
        let f = r.next_frame().await;
        assert_eq!(f["kind"], "pty.close", "{f}");
        closed.push((
            f["body"]["pty_id"].as_u64().unwrap(),
            f["body"]["reason"].as_str().unwrap().to_owned(),
        ));
    }
    closed.sort();
    assert_eq!(
        closed,
        vec![
            (1, "channel_lost".to_owned()),
            (2, "channel_lost".to_owned())
        ]
    );
    assert!(!r.handle.pty_closed(1).await, "already freed by the engine");
    assert!(!r.handle.pty_closed(2).await);
}

/// Bulk output through the credit machinery: ~9.5 KiB across many chunks arrives in order
/// (the window never truncates the stream) with credit frames flowing back as it is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_bulk_output_completes_within_the_window() {
    let home = TempDir::new();
    let mut r = rig(home.path(), true);
    open(&mut r, 1).await;
    r.data(1, b"seq 1 512 | awk '{printf \"line-%04d\\n\", $1}'\n")
        .await;
    let out = r.until("line-0512").await;
    assert!(out.contains("line-0001"), "the first line was lost: {out}");
    // The engine credited the input chunk once written onward (the frame rode the same
    // queue as the output `until` drained).
    assert_eq!(
        r.credits,
        vec![b"seq 1 512 | awk '{printf \"line-%04d\\n\", $1}'\n".len() as u64]
    );
}

/// One unsequenced `pty.*` frame encodes with `seq: 0, ack: 0` and validates (the shape the
/// channel pump routes); `frame` round-trips through the test's own decode.
#[test]
fn pty_frame_encoding_is_unsequenced() {
    let f = Frame {
        seq: 0,
        ack: 0,
        msg: SupervisorToRunner::PtyOpened(aurora_protocol::runner::PtyOpened {
            pty_id: 7,
            window: 131_072,
        }),
    };
    let text = serde_json::to_string(&f).unwrap();
    let back: Frame<SupervisorToRunner> = serde_json::from_str(&text).unwrap();
    assert_eq!(back.seq, 0);
    assert_eq!(back.ack, 0);
    assert!(back.msg.validate().is_ok());
}

//! (P5-FILES-1, `16 §W20` leg 2 / `§W21`, `12 §SSH channels`) The supervisor's pipes-only
//! exec engine in mode `sftp`: stdout rides tag `0x02` and stderr tag `0x03`, `exec.eof` closes
//! stdin after the data before it, `exec.exit { status | signal }` precedes `pty.close { eof }`,
//! the child leaks no descriptor to another exec, closing an exec ends its process group even
//! when it ignores HUP, refusals (`unsupported` / `disabled` / `limit`), and the credit bounds
//! under a hostile writer. The child is a stand-in script at the engine's `sftp-server` path
//! parameter; one test also speaks to the host's real `sftp-server` when it has one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::TempDir;
use crate::pty_env_probe::bootstrap_exe;
use aurora_protocol::runner::{ExecMode, ExecOpen, ExecTty, PtyClose, PtyCloseReason};
use aurora_protocol::supervisor::RunnerToSupervisor;
use aurora_supervisor::channel::{self, PtyIn, StreamOut};
use aurora_supervisor::pty::PtyEngine;
use serde_json::Value;
use tokio::sync::mpsc;

const T: Duration = Duration::from_secs(15);

struct Rig {
    tx: mpsc::UnboundedSender<PtyIn>,
    out: mpsc::Receiver<StreamOut>,
    _epoch: tokio::sync::watch::Sender<u64>,
    _handle: std::sync::Arc<aurora_supervisor::pty::PtyHandle>,
}

/// A stand-in `sftp-server`: an executable script `body` in `dir`.
fn script(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join(format!("fake-sftp-{}", body.len()));
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn rig(cwd: &Path, enabled: bool, sftp: PathBuf) -> Rig {
    let (out_tx, out_rx) = mpsc::channel(1024);
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (epoch_tx, epoch_rx) = tokio::sync::watch::channel(0u64);
    let ends = channel::PtyEnds {
        sender: channel::StreamSender::from_mpsc(out_tx),
        inbound: in_rx,
        epoch: epoch_rx,
    };
    let handle = PtyEngine::spawn_with_exec(
        ends,
        enabled,
        None,
        cwd.to_path_buf(),
        bootstrap_exe(),
        aurora_supervisor::pty::ShellPaths::default(),
        None,
        sftp,
    );
    Rig {
        tx: in_tx,
        out: out_rx,
        _epoch: epoch_tx,
        _handle: handle,
    }
}

fn open_frame(id: u32, window: u32, mode: ExecMode) -> ExecOpen {
    ExecOpen {
        pty_id: id,
        window,
        mode,
        command: None,
        tty: None,
        env: BTreeMap::new(),
    }
}

/// What the engine sent, in order.
#[derive(Debug)]
enum Out {
    Frame(Value),
    Stdout(u32, Vec<u8>),
    Stderr(u32, Vec<u8>),
}

impl Rig {
    fn send(&self, m: RunnerToSupervisor) {
        let _ = self.tx.send(PtyIn::Frame(m));
    }

    fn open(&self, o: ExecOpen) {
        self.send(RunnerToSupervisor::ExecOpen(o));
    }

    fn data(&self, id: u32, payload: &[u8]) {
        let _ = self.tx.send(PtyIn::Data {
            pty_id: id,
            payload: payload.to_vec(),
        });
    }

    async fn next(&mut self) -> Out {
        loop {
            let u = tokio::time::timeout(T, self.out.recv())
                .await
                .expect("engine output timed out")
                .expect("engine output ended");
            match u {
                StreamOut::Text(t) => return Out::Frame(serde_json::from_str(&t).unwrap()),
                StreamOut::Binary(b) => {
                    let id = u32::from_be_bytes([b[1], b[2], b[3], b[4]]);
                    return match b[0] {
                        0x02 => Out::Stdout(id, b[5..].to_vec()),
                        0x03 => Out::Stderr(id, b[5..].to_vec()),
                        t => panic!("unexpected tag {t:#x}"),
                    };
                }
                StreamOut::Flush => {}
                StreamOut::Abort => panic!("engine aborted the channel"),
            }
        }
    }

    /// Frames of `kind`, skipping data and credit; the first one found.
    async fn frame_of(&mut self, kind: &str) -> Value {
        loop {
            if let Out::Frame(v) = self.next().await
                && v["kind"] == kind
            {
                return v;
            }
        }
    }

    /// Everything until `pty.close` for `id`: (stdout, stderr, frames excluding credit).
    async fn drain(&mut self, id: u32) -> (Vec<u8>, Vec<u8>, Vec<Value>) {
        let (mut so, mut se, mut frames) = (Vec::new(), Vec::new(), Vec::new());
        loop {
            match self.next().await {
                Out::Stdout(i, b) if i == id => so.extend(b),
                Out::Stderr(i, b) if i == id => se.extend(b),
                Out::Frame(v) if v["kind"] == "pty.credit" => {}
                Out::Frame(v) => {
                    let done = v["kind"] == "pty.close";
                    frames.push(v);
                    if done {
                        return (so, se, frames);
                    }
                }
                _ => {}
            }
        }
    }
}

async fn opened(r: &mut Rig, id: u32, window: u32) -> Value {
    r.open(open_frame(id, window, ExecMode::Sftp));
    let f = r.frame_of("exec.opened").await;
    assert_eq!(f["body"]["pty_id"], id);
    f
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdout_stderr_eof_and_exit_status() {
    let d = TempDir::new();
    let sftp = script(d.path(), "echo diag >&2; cat; exit 7");
    let mut r = rig(d.path(), true, sftp);
    let f = opened(&mut r, 1, 65_536).await;
    assert_eq!(
        f["body"]["window"],
        aurora_core::limits::TERMINAL_STREAM_WINDOW
    );
    r.data(1, b"hello ");
    r.data(1, b"world");
    r.send(RunnerToSupervisor::ExecEof(
        aurora_protocol::runner::ExecEof { pty_id: 1 },
    ));
    let (so, se, frames) = r.drain(1).await;
    assert_eq!(so, b"hello world");
    assert_eq!(se, b"diag\n");
    // exactly one exec.exit, then pty.close { eof } last.
    let kinds: Vec<&str> = frames.iter().map(|v| v["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["exec.exit", "pty.close"], "{frames:?}");
    assert_eq!(frames[0]["body"]["status"], 7);
    assert_eq!(frames[1]["body"]["reason"], "eof");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_death_is_exit_signal() {
    let d = TempDir::new();
    let sftp = script(d.path(), "kill -KILL $$");
    let mut r = rig(d.path(), true, sftp);
    opened(&mut r, 1, 4096).await;
    let (_, _, frames) = r.drain(1).await;
    assert_eq!(frames[0]["kind"], "exec.exit");
    assert_eq!(frames[0]["body"]["signal"], "KILL", "{frames:?}");
    assert!(frames[0]["body"].get("status").is_none());
}

/// The child runs as the supervisor's own user in the configured cwd with the image `PATH`,
/// `LANG` passed, and exactly what `spawn::child_env` allows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cwd_and_environment() {
    let d = TempDir::new();
    let sftp = script(d.path(), "pwd; env; exit 0");
    let mut r = rig(d.path(), true, sftp);
    let mut o = open_frame(1, 65_536, ExecMode::Sftp);
    o.env.insert("LANG".into(), "C.UTF-8".into());
    r.open(o);
    r.frame_of("exec.opened").await;
    let (so, _, _) = r.drain(1).await;
    let text = String::from_utf8_lossy(&so);
    let want = std::fs::canonicalize(d.path()).unwrap();
    assert_eq!(text.lines().next().unwrap(), want.to_str().unwrap());
    assert!(text.contains("LANG=C.UTF-8"), "{text}");
    assert!(
        text.contains(&format!("PATH={}", aurora_supervisor::spawn::IMAGE_PATH)),
        "{text}"
    );
    assert!(!text.contains("AURORA_"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_shapes_are_refused() {
    let d = TempDir::new();
    let sftp = script(d.path(), "cat");
    let mut r = rig(d.path(), true, sftp);
    let mut cases = vec![
        open_frame(1, 4096, ExecMode::Shell),
        open_frame(2, 4096, ExecMode::Exec),
    ];
    let mut c = open_frame(3, 4096, ExecMode::Sftp);
    c.command = Some("ls".into());
    cases.push(c);
    let mut c = open_frame(4, 4096, ExecMode::Sftp);
    c.tty = Some(ExecTty {
        term: "xterm".into(),
        cols: 80,
        rows: 24,
    });
    cases.push(c);
    let mut c = open_frame(5, 4096, ExecMode::Sftp);
    c.env.insert("AURORA_BOOT".into(), "x".into());
    cases.push(c);
    let mut c = open_frame(6, 4096, ExecMode::Sftp);
    c.env.insert("PATH".into(), "/x".into());
    cases.push(c);
    for o in cases {
        let id = o.pty_id;
        r.open(o);
        let f = r.frame_of("exec.refused").await;
        assert_eq!(f["body"]["pty_id"], id);
        assert_eq!(f["body"]["reason"], "unsupported", "id {id}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_sftp_server_is_unsupported_and_disabled_is_disabled() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    r.open(open_frame(1, 4096, ExecMode::Sftp));
    let f = r.frame_of("exec.refused").await;
    assert_eq!(f["body"]["reason"], "unsupported");
    let sftp = script(d.path(), "cat");
    let mut r = rig(d.path(), false, sftp);
    r.open(open_frame(1, 4096, ExecMode::Sftp));
    let f = r.frame_of("exec.refused").await;
    assert_eq!(f["body"]["reason"], "disabled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn more_than_the_exec_cap_is_refused_with_limit() {
    let d = TempDir::new();
    let sftp = script(d.path(), "cat");
    let mut r = rig(d.path(), true, sftp);
    let cap = aurora_core::limits::EXECS_PER_SANDBOX;
    for id in 1..=cap {
        opened(&mut r, id, 4096).await;
    }
    r.open(open_frame(cap + 1, 4096, ExecMode::Sftp));
    let f = r.frame_of("exec.refused").await;
    assert_eq!(f["body"]["reason"], "limit");
    // Freeing one slot admits another.
    r.send(RunnerToSupervisor::PtyClose(PtyClose {
        pty_id: 1,
        reason: PtyCloseReason::Eof,
        bytes_in: None,
        bytes_out: None,
    }));
    opened(&mut r, cap + 2, 4096).await;
}

fn fd_targets(pid: u32) -> Vec<String> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .map(|p| p.display().to_string())
        .collect()
}

/// (FIX-059 posture) Another exec's pipe ends are `CLOEXEC`: a second child holds only its
/// own three pipes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_descriptor_leaks_into_another_exec() {
    let d = TempDir::new();
    let sftp = script(d.path(), "echo $$; cat");
    let mut r = rig(d.path(), true, sftp);
    let mut pids = Vec::new();
    for id in [1, 2] {
        opened(&mut r, id, 65_536).await;
        loop {
            if let Out::Stdout(i, b) = r.next().await
                && i == id
            {
                pids.push(String::from_utf8_lossy(&b).trim().parse::<u32>().unwrap());
                break;
            }
        }
    }
    let pipes = |pid: u32| -> Vec<String> {
        fd_targets(pid)
            .into_iter()
            .filter(|t| t.starts_with("pipe:"))
            .collect()
    };
    let (p1, p2) = (pipes(pids[0]), pipes(pids[1]));
    assert!(!p1.is_empty() && !p2.is_empty(), "{p1:?} {p2:?}");
    assert!(
        p1.iter().all(|t| !p2.contains(t)),
        "exec 2 inherited exec 1's pipes: {p1:?} vs {p2:?}"
    );
}

/// Closing an exec hangs it up and, after the grace, kills its whole process group even when
/// it (and a background child) ignore HUP (FIX-091 posture).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_kills_the_process_group() {
    let d = TempDir::new();
    let sftp = script(d.path(), "trap '' HUP; sleep 300 & echo $! $$; wait");
    let mut r = rig(d.path(), true, sftp);
    opened(&mut r, 1, 4096).await;
    let pids: Vec<u32> = loop {
        if let Out::Stdout(1, b) = r.next().await {
            break String::from_utf8_lossy(&b)
                .split_whitespace()
                .map(|p| p.parse().unwrap())
                .collect();
        }
    };
    for p in &pids {
        assert!(Path::new(&format!("/proc/{p}")).exists());
    }
    r.send(RunnerToSupervisor::PtyClose(PtyClose {
        pty_id: 1,
        reason: PtyCloseReason::Eof,
        bytes_in: None,
        bytes_out: None,
    }));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        // A killed child is a zombie until reaped; `/proc/<pid>/stat` says `Z` or is gone.
        let alive = pids.iter().any(|p| {
            std::fs::read_to_string(format!("/proc/{p}/stat"))
                .map(|s| !s.contains(") Z "))
                .unwrap_or(false)
        });
        if !alive {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "processes {pids:?} outlived the exec"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A child that never reads stdin plus a writer that ignores the window: the over-window
/// chunk ends the exec with `pty.close { error }`, and no byte beyond the window was queued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hostile_writer_beyond_the_window_is_closed() {
    let d = TempDir::new();
    let sftp = script(d.path(), "sleep 300");
    let mut r = rig(d.path(), true, sftp);
    opened(&mut r, 1, 4096).await;
    let chunk = vec![b'x'; aurora_core::limits::TERMINAL_CHUNK];
    // The kernel's pipe buffer (64 KiB) absorbs some of it and credit is released as it is
    // written; 3 × the window is beyond what pipe + window can hold.
    for _ in 0..(3 * aurora_core::limits::TERMINAL_STREAM_WINDOW / chunk.len()) {
        r.data(1, &chunk);
    }
    let f = r.frame_of("pty.close").await;
    assert_eq!(f["body"]["pty_id"], 1);
    assert_eq!(f["body"]["reason"], "error");
}

/// With a 4 KiB window and a child writing 1 MiB, every byte arrives and credit returns
/// piecewise: output is held back, never dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_beyond_the_credit_is_held_not_dropped() {
    const TOTAL: usize = 1 << 20;
    const WINDOW: u32 = 4096;
    let d = TempDir::new();
    let sftp = script(d.path(), &format!("head -c {TOTAL} /dev/zero"));
    let mut r = rig(d.path(), true, sftp);
    opened(&mut r, 1, WINDOW).await;
    let mut got = 0usize;
    let mut grants = 0u32;
    let mut outstanding = 0usize;
    loop {
        match r.next().await {
            Out::Stdout(1, b) => {
                outstanding += b.len();
                assert!(outstanding <= WINDOW as usize, "sent beyond the window");
                got += b.len();
                // Return credit as a draining runner does.
                outstanding -= b.len();
                r.send(RunnerToSupervisor::PtyCredit(
                    aurora_protocol::runner::PtyCredit {
                        pty_id: 1,
                        bytes: b.len() as u32,
                    },
                ));
                grants += 1;
            }
            Out::Frame(v) if v["kind"] == "pty.close" => break,
            _ => {}
        }
    }
    assert_eq!(got, TOTAL);
    assert!(grants > 1);
}

/// The host's real `sftp-server` answers an `INIT` with a `VERSION` that lists the
/// `posix-rename@openssh.com` extension (skipped without one).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_sftp_server_speaks_sftp_over_the_exec() {
    let Some(real) = [
        "/usr/libexec/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file()) else {
        eprintln!("skipped: no sftp-server on this host (covered by the e2e)");
        return;
    };
    let d = TempDir::new();
    let mut r = rig(d.path(), true, real);
    opened(&mut r, 1, 65_536).await;
    r.data(1, &[0, 0, 0, 5, 1, 0, 0, 0, 3]);
    let mut got = Vec::new();
    while got.len() < 5
        || got.len() < 4 + u32::from_be_bytes([got[0], got[1], got[2], got[3]]) as usize
    {
        if let Out::Stdout(1, b) = r.next().await {
            got.extend(b);
        }
    }
    assert_eq!(got[4], 2, "SSH_FXP_VERSION");
    assert!(
        String::from_utf8_lossy(&got).contains("posix-rename@openssh.com"),
        "{got:?}"
    );
}

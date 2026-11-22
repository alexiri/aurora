//! (P4-SSH-2, `16 §W20` leg 2, `12 §SSH channels`) The `shell` and `exec` modes with and without
//! `tty` on the exec engine: stdout and stderr tags over pipes, stdin EOF, exit status and
//! signal, the environment allow-list (`LANG`/`LC_*` in, `TERM` from the tty, `PATH`/`AURORA_*`
//! refused), window size and resize under a PTY, the refusals of malformed shapes, and the
//! descriptor posture of a PTY child. Included from `exec_sftp.rs` (it reuses that file's rig).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use aurora_protocol::runner::PtyResize;

fn exec_frame(id: u32, command: &str) -> ExecOpen {
    let mut o = open_frame(id, 65_536, ExecMode::Exec);
    o.command = Some(command.to_owned());
    o
}

fn tty(term: &str, cols: u16, rows: u16) -> Option<ExecTty> {
    Some(ExecTty {
        term: term.into(),
        cols,
        rows,
    })
}

impl Rig {
    /// Accumulates stdout of `id` until it contains `needle`.
    async fn until_stdout(&mut self, id: u32, needle: &str) -> String {
        let mut got = Vec::new();
        loop {
            if let Out::Stdout(i, b) = self.next().await
                && i == id
            {
                got.extend(b);
                let s = String::from_utf8_lossy(&got).into_owned();
                if s.contains(needle) {
                    return s;
                }
            }
        }
    }
}

fn status_of(frames: &[Value]) -> (Option<i64>, Option<String>) {
    let kinds: Vec<&str> = frames.iter().map(|v| v["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["exec.exit", "pty.close"], "{frames:?}");
    assert_eq!(frames[1]["body"]["reason"], "eof");
    (
        frames[0]["body"]["status"].as_i64(),
        frames[0]["body"]["signal"].as_str().map(str::to_owned),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_mode_splits_stdout_and_stderr_and_reports_the_status() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    r.open(exec_frame(1, "echo out; echo err >&2; exit 3"));
    r.frame_of("exec.opened").await;
    let (so, se, frames) = r.drain(1).await;
    assert_eq!(so, b"out\n");
    assert_eq!(se, b"err\n");
    assert_eq!(status_of(&frames), (Some(3), None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_mode_signal_death_and_stdin_eof() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    r.open(exec_frame(1, "kill -9 $$"));
    r.frame_of("exec.opened").await;
    let (_, _, frames) = r.drain(1).await;
    assert_eq!(status_of(&frames), (None, Some("KILL".into())));
    // `cat` finishes only when EOF closes its stdin, after the bytes before it.
    r.open(exec_frame(2, "cat"));
    r.frame_of("exec.opened").await;
    r.data(2, b"one ");
    r.data(2, b"two");
    r.send(RunnerToSupervisor::ExecEof(
        aurora_protocol::runner::ExecEof { pty_id: 2 },
    ));
    let (so, _, frames) = r.drain(2).await;
    assert_eq!(so, b"one two");
    assert_eq!(status_of(&frames), (Some(0), None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_mode_without_tty_reads_commands_from_stdin() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    r.open(open_frame(1, 65_536, ExecMode::Shell));
    r.frame_of("exec.opened").await;
    r.data(1, b"echo from-stdin; echo e >&2; exit 7\n");
    let (so, se, frames) = r.drain(1).await;
    assert_eq!(so, b"from-stdin\n");
    assert_eq!(se, b"e\n");
    assert_eq!(status_of(&frames), (Some(7), None));
}

/// `LANG` in, `TERM` from the tty, no `AURORA_*`; the shell is the agent's (`SHELL` set), cwd is
/// the configured directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_environment_is_the_allow_list_with_and_without_tty() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    for (id, with_tty) in [(1u32, false), (2, true)] {
        let mut o = exec_frame(id, "pwd; env");
        o.env.insert("LANG".into(), "C.UTF-8".into());
        o.env.insert("LC_ALL".into(), "C.UTF-8".into());
        if with_tty {
            o.tty = tty("xterm-test", 80, 24);
        }
        r.open(o);
        r.frame_of("exec.opened").await;
        let (so, _, _) = r.drain(id).await;
        let text = String::from_utf8_lossy(&so).replace('\r', "");
        let want = std::fs::canonicalize(d.path()).unwrap();
        assert_eq!(text.lines().next().unwrap(), want.to_str().unwrap());
        assert!(text.contains("LANG=C.UTF-8"), "{text}");
        assert!(text.contains("LC_ALL=C.UTF-8"), "{text}");
        assert!(text.contains("SHELL=/"), "{text}");
        assert!(!text.contains("AURORA_"), "{text}");
        // (P5-SKILLDIR, D-454) Aurora's own variable, the adapter's folder.
        assert!(
            text.lines()
                .any(|l| l == format!("SKILLS_DIR={SKILLS_DIR_VALUE}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("PATH={}", aurora_supervisor::spawn::IMAGE_PATH)),
            "{text}"
        );
        if with_tty {
            assert!(text.contains("TERM=xterm-test"), "{text}");
        } else {
            assert!(!text.contains("TERM="), "{text}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tty_exec_has_one_output_stream_a_terminal_and_the_status() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    let mut o = exec_frame(
        1,
        "test -t 0 && test -t 1 && test -t 2 && echo is-a-tty; echo e >&2; exit 5",
    );
    o.tty = tty("a b", 100, 30); // an invalid TERM falls back
    r.open(o);
    r.frame_of("exec.opened").await;
    let (so, se, frames) = r.drain(1).await;
    let text = String::from_utf8_lossy(&so);
    assert!(text.contains("is-a-tty"), "{text}");
    assert!(
        text.contains('e'),
        "stderr joins stdout under a tty: {text}"
    );
    assert!(se.is_empty(), "no stderr tag under a tty");
    assert_eq!(status_of(&frames), (Some(5), None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_term_falls_back() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    let mut o = exec_frame(1, "echo \"[$TERM]\"");
    o.tty = tty("bad term", 80, 24);
    r.open(o);
    r.frame_of("exec.opened").await;
    let (so, _, _) = r.drain(1).await;
    assert!(
        String::from_utf8_lossy(&so).contains("[xterm-256color]"),
        "{so:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tty_shell_has_the_window_size_and_follows_a_resize() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    let mut o = open_frame(1, 65_536, ExecMode::Shell);
    o.tty = tty("xterm", 100, 30);
    r.open(o);
    r.frame_of("exec.opened").await;
    r.data(1, b"stty size\n");
    r.until_stdout(1, "30 100").await;
    r.send(RunnerToSupervisor::PtyResize(PtyResize {
        pty_id: 1,
        cols: 90,
        rows: 20,
    }));
    // The resize and the next bytes travel on different paths: ask until the new size shows.
    let mut got = String::new();
    for _ in 0..50 {
        r.data(1, b"stty size\n");
        got = r.until_stdout(1, "\n").await;
        if got.contains("20 90") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    r.data(1, b"exit 4\n");
    let (so, _, frames) = r.drain(1).await;
    let all = format!("{got}{}", String::from_utf8_lossy(&so));
    assert!(all.contains("20 90"), "{all}");
    assert_eq!(status_of(&frames), (Some(4), None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_shapes_are_refused() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    let mut cases = vec![];
    cases.push(exec_frame(1, "echo a\0b"));
    cases.push(exec_frame(
        2,
        &"x".repeat(aurora_protocol::runner::EXEC_COMMAND_MAX + 1),
    ));
    cases.push(open_frame(3, 4096, ExecMode::Shell).with_command("ls"));
    let mut c = exec_frame(4, "ls");
    c.tty = tty("xterm", 0, 24);
    cases.push(c);
    let mut c = exec_frame(5, "ls");
    c.env.insert("PATH".into(), "/evil".into());
    cases.push(c);
    let mut c = exec_frame(6, "ls");
    c.env.insert("AURORA_X".into(), "1".into());
    cases.push(c);
    // (P5-SKILLDIR) A client cannot set `SKILLS_DIR`: it is not an accepted name.
    let mut c = exec_frame(7, "ls");
    c.env.insert("SKILLS_DIR".into(), "/evil".into());
    cases.push(c);
    for o in cases {
        let id = o.pty_id;
        r.open(o);
        let f = r.frame_of("exec.refused").await;
        assert_eq!(f["body"]["pty_id"], id);
        assert_eq!(f["body"]["reason"], "unsupported", "id {id}");
    }
}

/// (FIX-059 posture) A PTY child holds no `/dev/ptmx` (the master) and only its own slave and
/// the pipes' absence: stdio is the terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tty_child_holds_no_master_descriptor() {
    let d = TempDir::new();
    let mut r = rig(d.path(), true, d.path().join("absent"));
    let mut o = exec_frame(
        1,
        "for f in /proc/$$/fd/*; do readlink $f; done; echo done-listing",
    );
    o.tty = tty("xterm", 80, 24);
    r.open(o);
    r.frame_of("exec.opened").await;
    let (so, _, _) = r.drain(1).await;
    let text = String::from_utf8_lossy(&so);
    assert!(text.contains("done-listing"), "{text}");
    assert!(
        !text.contains("ptmx"),
        "master leaked into the child: {text}"
    );
    assert!(text.contains("/dev/pts/"), "{text}");
}

/// A stand-in shell that prints its argv and `$SHELL` (stdin is not read).
fn arg_standin(dir: &Path, name: &str, tag: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(
        &p,
        format!("#!/bin/sh\necho \"{tag}_ARGS=$*\"\necho \"{tag}_SHELL=$SHELL\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn standin_paths(dir: &Path, bash: bool, zsh: bool) -> aurora_supervisor::pty::ShellPaths {
    aurora_supervisor::pty::ShellPaths {
        bash: if bash {
            arg_standin(dir, "bash", "BASH")
        } else {
            dir.join("no-bash")
        },
        zsh: if zsh {
            arg_standin(dir, "zsh", "ZSH")
        } else {
            dir.join("no-zsh")
        },
        sh: PathBuf::from("/bin/sh"),
    }
}

/// (FIX-218, D-459) `shell` mode runs the chosen shell as a login shell (`-l`), with and without
/// a tty; absent means bash; `SHELL` is the binary run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_mode_runs_the_chosen_shell_as_a_login_shell() {
    use aurora_protocol::runner::PtyShell;
    let d = TempDir::new();
    let mut r = rig_with_shells(
        d.path(),
        true,
        d.path().join("absent"),
        standin_paths(d.path(), true, true),
    );
    for (id, shell, with_tty, tag) in [
        (1u32, Some(PtyShell::Zsh), false, "ZSH"),
        (2, Some(PtyShell::Bash), false, "BASH"),
        (3, None, false, "BASH"),
        (4, Some(PtyShell::Zsh), true, "ZSH"),
        (5, None, true, "BASH"),
    ] {
        let mut o = open_frame(id, 65_536, ExecMode::Shell);
        o.shell = shell;
        if with_tty {
            o.tty = tty("xterm", 80, 24);
        }
        r.open(o);
        r.frame_of("exec.opened").await;
        let (so, _, _) = r.drain(id).await;
        let text = String::from_utf8_lossy(&so).replace('\r', "");
        assert!(
            text.contains(&format!("{tag}_ARGS=-l\n")),
            "id {id}: {text}"
        );
        let want = format!("{tag}_SHELL={}/", d.path().display());
        assert!(text.contains(&want), "id {id}: {text}");
        assert!(!text.contains("not installed"), "no notice: {text}");
    }
}

/// `exec` mode: `<shell> -c <command>` without `-l`, the command one argument.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_mode_runs_the_chosen_shell_with_dash_c_and_no_login() {
    use aurora_protocol::runner::PtyShell;
    let d = TempDir::new();
    let mut r = rig_with_shells(
        d.path(),
        true,
        d.path().join("absent"),
        standin_paths(d.path(), true, true),
    );
    for (id, shell, tag) in [(1u32, Some(PtyShell::Zsh), "ZSH"), (2, None, "BASH")] {
        let mut o = exec_frame(id, "echo a; echo 'b c'");
        o.shell = shell;
        r.open(o);
        r.frame_of("exec.opened").await;
        let (so, _, _) = r.drain(id).await;
        let text = String::from_utf8_lossy(&so);
        assert!(
            text.contains(&format!("{tag}_ARGS=-c echo a; echo 'b c'\n")),
            "{text}"
        );
    }
}

/// Zsh missing falls back to bash; the notice appears on a tty `shell` session only (a pipe
/// session and `exec` keep stdout clean).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fallback_notice_is_for_a_tty_shell_session_only() {
    use aurora_protocol::runner::PtyShell;
    let d = TempDir::new();
    let mut r = rig_with_shells(
        d.path(),
        true,
        d.path().join("absent"),
        standin_paths(d.path(), true, false),
    );
    let notice = "zsh is not installed";
    for (id, mode, with_tty, want_notice) in [
        (1u32, ExecMode::Shell, true, true),
        (2, ExecMode::Shell, false, false),
        (3, ExecMode::Exec, false, false),
        (4, ExecMode::Exec, true, false),
    ] {
        let mut o = if mode == ExecMode::Exec {
            exec_frame(id, "true")
        } else {
            open_frame(id, 65_536, ExecMode::Shell)
        };
        o.shell = Some(PtyShell::Zsh);
        if with_tty {
            o.tty = tty("xterm", 80, 24);
        }
        r.open(o);
        r.frame_of("exec.opened").await;
        let (so, _, _) = r.drain(id).await;
        let text = String::from_utf8_lossy(&so);
        assert!(text.contains("BASH_ARGS="), "id {id}: {text}");
        assert_eq!(text.contains(notice), want_notice, "id {id}: {text}");
    }
}

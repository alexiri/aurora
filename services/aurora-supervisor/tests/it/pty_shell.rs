//! (FIX-138, `12 §Terminal`, `16 §W19`) The terminal's shell choice reaches the PTY spawn: the
//! enumerated `pty.open.shell` is mapped to a **fixed** binary and started as a login shell
//! (`-l`) with the one image `PATH`; a missing zsh falls back to bash and a missing bash to
//! `sh`, each with a one-line notice in the pane. The binaries are stand-in scripts (the host
//! may have no zsh) that print a marker, their arguments and `$PATH`, then become a plain `sh`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use crate::common::TempDir;
use crate::pty_env_probe::{Rig, rig_with_shells};
use aurora_protocol::runner::{PtyOpen, PtyShell};
use aurora_protocol::supervisor::RunnerToSupervisor;
use aurora_supervisor::pty::ShellPaths;

/// A stand-in shell: prints `STANDIN_<tag>_ARGS=<args>` and `STANDIN_<tag>_PATH=<$PATH>`,
/// then replaces itself with `/bin/sh` so the session stays interactive.
fn standin(dir: &Path, name: &str, tag: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(
        &p,
        format!(
            "#!/bin/sh\necho \"STANDIN_{tag}_ARGS=$*\"\necho \"STANDIN_{tag}_PATH=$PATH\"\nexec /bin/sh\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

async fn open_with(r: &mut Rig, shell: PtyShell) {
    r.frame(RunnerToSupervisor::PtyOpen(PtyOpen {
        pty_id: 1,
        window: 131_072,
        cols: 80,
        rows: 24,
        shell,
        models: None,
    }))
    .await;
    let opened = r.next_frame().await;
    assert_eq!(opened["kind"], "pty.opened", "{opened}");
}

fn paths(dir: &Path, bash: bool, zsh: bool) -> ShellPaths {
    ShellPaths {
        bash: if bash {
            standin(dir, "bash", "BASH")
        } else {
            dir.join("no-bash")
        },
        zsh: if zsh {
            standin(dir, "zsh", "ZSH")
        } else {
            dir.join("no-zsh")
        },
        sh: PathBuf::from("/bin/sh"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_zsh_runs_the_zsh_binary_as_a_login_shell() {
    let home = TempDir::new();
    let mut r = rig_with_shells(home.path(), true, paths(home.path(), true, true));
    open_with(&mut r, PtyShell::Zsh).await;
    let out = r.until("/usr/local/bin:/usr/bin:/bin").await;
    assert!(out.contains("STANDIN_ZSH_ARGS=-l"), "{out}");
    assert!(!out.contains("STANDIN_BASH"), "{out}");
    // The one image PATH, set by the supervisor for both shells.
    assert!(
        out.contains(
            "STANDIN_ZSH_PATH=/workspace/.local/bin:/workspace/.local/share/gem/bin:\
             /opt/aurora/bin:/usr/local/bin:/usr/bin:/bin"
        ),
        "{out}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_bash_runs_the_bash_binary_as_a_login_shell() {
    let home = TempDir::new();
    let mut r = rig_with_shells(home.path(), true, paths(home.path(), true, true));
    open_with(&mut r, PtyShell::Bash).await;
    let out = r.until("STANDIN_BASH_PATH=").await;
    assert!(out.contains("STANDIN_BASH_ARGS=-l"), "{out}");
    assert!(!out.contains("STANDIN_ZSH"), "{out}");
    assert!(
        !out.contains("not installed"),
        "no notice without a fallback: {out}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_zsh_missing_falls_back_to_bash_with_a_notice() {
    let home = TempDir::new();
    let mut r = rig_with_shells(home.path(), true, paths(home.path(), true, false));
    open_with(&mut r, PtyShell::Zsh).await;
    let out = r.until("STANDIN_BASH_PATH=").await;
    assert!(out.contains("zsh is not installed"), "{out}");
    assert!(out.contains("STANDIN_BASH_ARGS=-l"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_neither_falls_back_to_sh_with_a_notice() {
    let home = TempDir::new();
    let mut r = rig_with_shells(home.path(), true, paths(home.path(), false, false));
    open_with(&mut r, PtyShell::Zsh).await;
    let out = r.until("starting sh").await;
    assert!(out.contains("bash and zsh are not installed"), "{out}");
    // …and it is a working shell.
    r.data(1, b"echo FALLBACK_\"OK\"\n").await;
    r.until("FALLBACK_OK").await;
}

/// The default choice against the real binaries of this host: bash really is started as a
/// login shell (`shopt -q login_shell`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_bash_is_a_real_login_shell() {
    if !Path::new("/bin/bash").is_file() {
        return;
    }
    let home = TempDir::new();
    let mut r = rig_with_shells(home.path(), true, ShellPaths::default());
    open_with(&mut r, PtyShell::Bash).await;
    r.data(
        1,
        b"shopt -q login_shell && echo IS_\"LOGIN\"; echo DONE_\"1\"\n",
    )
    .await;
    let out = r.until("DONE_1").await;
    assert!(out.contains("IS_LOGIN"), "{out}");
}

//! A real OpenSSH client (the `ssh` binary on the machine) against the real server: a certificate
//! logs in and gets the leg's exit status, a plain key and a bad certificate get the same
//! refusal and cost the stub API nothing (P4-SSH-2 handoff check (d); D-356). Skipped, loudly,
//! when there is no `ssh` binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use russh::keys::ssh_key::LineEnding;
use uuid::Uuid;

use crate::common::*;

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn have_ssh() -> bool {
    Command::new("ssh")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Runs `ssh` as `user@127.0.0.1:port echo canary` with the identity (and certificate) given;
/// returns (exit code, stderr).
async fn run_ssh(port: u16, user: String, dir: &Path, with_cert: bool) -> (i32, String) {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut c = Command::new("ssh");
        c.args(["-F", "/dev/null", "-p", &port.to_string()])
            .arg("-i")
            .arg(dir.join("id"))
            .args([
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "LogLevel=ERROR",
                "-o",
                "ConnectTimeout=10",
            ]);
        if with_cert {
            c.arg("-o").arg(format!(
                "CertificateFile={}",
                dir.join("id-cert.pub").display()
            ));
        }
        let out = c
            .arg(format!("{user}@127.0.0.1"))
            .arg("echo canary")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    })
    .await
    .unwrap()
}

/// `ssh -tt user@127.0.0.1` (a shell with a pty requested) with a piped stdin and, when `term` is `None`, no `TERM`
/// in the environment: what a script or a CI job does. Returns (exit code, stderr).
async fn run_ssh_tt(port: u16, user: String, dir: &Path, term: Option<&str>) -> (i32, String) {
    use std::io::Write;
    let dir = dir.to_owned();
    let term = term.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let mut c = Command::new("ssh");
        c.args(["-F", "/dev/null", "-tt", "-p", &port.to_string()])
            .arg("-i")
            .arg(dir.join("id"))
            .args([
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "LogLevel=ERROR",
                "-o",
                "ConnectTimeout=10",
                "-o",
            ])
            .arg(format!(
                "CertificateFile={}",
                dir.join("id-cert.pub").display()
            ))
            .arg(format!("{user}@127.0.0.1"));
        match term {
            Some(t) => c.env("TERM", t),
            None => c.env_remove("TERM"),
        };
        let mut ch = c
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut si = ch.stdin.take().unwrap();
        si.write_all(b"echo hi\n").unwrap();
        drop(si);
        let out = ch.wait_with_output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    })
    .await
    .unwrap()
}

fn write_identity(r: &Rig, cert: Option<&russh::keys::ssh_key::Certificate>) -> Dir {
    let dir = std::env::temp_dir().join(format!("aurora-term-openssh-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("id");
    std::fs::write(
        &key,
        r.user_key.to_openssh(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    if let Some(c) = cert {
        std::fs::write(dir.join("id-cert.pub"), c.to_openssh().unwrap()).unwrap();
    }
    Dir(dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn openssh_certificate_logs_in_and_gets_the_exit_status() {
    if !have_ssh() {
        eprintln!("SKIPPED: no ssh binary");
        return;
    }
    let r = rig(RigOpts::default()).await;
    let d = write_identity(&r, Some(&r.good()));
    let (code, err) = run_ssh(r.ssh.port(), r.agent.to_string(), &d.0, true).await;
    // The stub runner answers the client's stdin EOF with `bye` on stderr and status 7.
    assert_eq!(code, 7, "stderr: {err}");
    assert!(err.contains("bye"), "stderr: {err}");
    assert_eq!(r.stub.count("sessions"), 1);
    assert_eq!(r.runner.opens.lock().unwrap().len(), 1);
}

/// A script's `ssh -tt host` has a pipe for stdin (the client sends a 0×0 window) and often no `TERM` (it sends an
/// empty one): both are "unspecified", so the pty is granted — 80×24 and a default terminal type — and the shell runs.
/// (Found by the kind SSH e2e: "PTY allocation request failed on channel 0".)
#[tokio::test(flavor = "multi_thread")]
async fn openssh_tt_with_a_piped_stdin_and_no_term_still_gets_a_pty() {
    if !have_ssh() {
        eprintln!("SKIPPED: no ssh binary");
        return;
    }
    for term in [None, Some("xterm")] {
        let r = rig(RigOpts::default()).await;
        let d = write_identity(&r, Some(&r.good()));
        let (code, err) = run_ssh_tt(r.ssh.port(), r.agent.to_string(), &d.0, term).await;
        assert!(
            !err.contains("PTY allocation request failed"),
            "TERM {term:?}: {err}"
        );
        // The stub runner ends the shell with status 7 at the client's EOF.
        assert_eq!(code, 7, "TERM {term:?}: {err}");
        let opens = r.runner.opens.lock().unwrap().clone();
        assert_eq!(opens.len(), 1, "TERM {term:?}");
        assert_eq!(opens[0]["mode"], "shell");
        assert_eq!(opens[0]["tty"]["cols"], 80);
        assert_eq!(opens[0]["tty"]["rows"], 24);
        let t = opens[0]["tty"]["term"].as_str().unwrap();
        assert!(!t.is_empty(), "the terminal type must not be empty: {t:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn openssh_plain_key_and_bad_certificate_get_the_same_refusal() {
    if !have_ssh() {
        eprintln!("SKIPPED: no ssh binary");
        return;
    }
    let r = rig(RigOpts::default()).await;
    let user = r.agent.to_string();

    // A plain registered-looking key, no certificate.
    let plain = write_identity(&r, None);
    let (c1, e1) = run_ssh(r.ssh.port(), user.clone(), &plain.0, false).await;

    // A certificate from a CA the API does not trust.
    let mut spec = r.spec();
    spec.ca = key(0x77);
    let foreign = write_identity(&r, Some(&r.cert(&spec)));
    let (c2, e2) = run_ssh(r.ssh.port(), user.clone(), &foreign.0, true).await;

    // An expired certificate from the right CA.
    let mut spec = r.spec();
    let now = chrono::Utc::now().timestamp();
    (spec.valid_after, spec.valid_before) = (now - 7200, now - 3600);
    let expired = write_identity(&r, Some(&r.cert(&spec)));
    let (c3, e3) = run_ssh(r.ssh.port(), user, &expired.0, true).await;

    for (c, e) in [(&c1, &e1), (&c2, &e2), (&c3, &e3)] {
        assert_eq!(*c, 255, "stderr: {e}");
        assert!(e.contains("Permission denied (publickey)"), "stderr: {e}");
    }
    let norm = |s: &str| s.lines().last().unwrap_or("").to_owned();
    assert_eq!(
        norm(&e1),
        norm(&e2),
        "the client must not learn which rule failed"
    );
    assert_eq!(norm(&e1), norm(&e3));
    assert_eq!(
        r.stub.count("sessions"),
        0,
        "a refusal by the law never reaches the API"
    );
    assert!(r.runner.opens.lock().unwrap().is_empty());
}

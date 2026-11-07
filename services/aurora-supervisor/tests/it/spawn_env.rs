//! SEC-34 / I-23: no child (process manager, gate, harness launcher) inherits `AURORA_BOOT` or
//! credential-shaped values; ordinary container/app env still passes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt as _;
use std::time::Duration;

use crate::common::TempDir;
use aurora_protocol::runner::GateOutcome;
use aurora_supervisor::adapters::claude_code::config::LaunchSpec;
use aurora_supervisor::adapters::claude_code::launcher::{Launcher, ProcessLauncher};
use aurora_supervisor::channel::ChannelSender;
use aurora_supervisor::process::{ProcessConfig, ProcessManager};
use aurora_supervisor::spawn;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

const MARK: &str = "AURORA_TEST_INNER";

fn check(env: &str) {
    assert!(!env.contains("AURORA_BOOT"), "leaked AURORA_BOOT: {env}");
    assert!(!env.contains("leak-me"), "leaked boot value: {env}");
    assert!(env.lines().any(|l| l.starts_with("PATH=")), "{env}");
    assert!(env.contains("HTTPS_PROXY=http://127.0.0.1:3128"), "{env}");
    assert!(!env.contains(MARK), "{env}");
    // snapshot app env (set on the container) still reaches children; credential-shaped
    // names with real values do not
    assert!(
        env.contains("APP_SETTING=keep-me"),
        "app env dropped: {env}"
    );
    assert!(!env.contains("DEPLOY_TOKEN"), "{env}");
}

/// Re-executes this test with `AURORA_BOOT` in its environment (no `unsafe set_var` needed).
#[test]
fn children_never_receive_boot() {
    if std::env::var_os("AURORA_BOOT").is_none() {
        let exe = std::env::current_exe().unwrap();
        let out = spawn::command("sh", std::iter::empty::<(&str, &str)>())
            .args([
                "-c",
                "AURORA_BOOT=leak-me APP_SETTING=keep-me DEPLOY_TOKEN=leak-me FOO=aub_leak-me exec \"$0\" --exact spawn_env::inner_probe --nocapture",
            ])
            .arg(exe)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[tokio::test]
async fn inner_probe() {
    if std::env::var_os("AURORA_BOOT").is_none() {
        return; // only meaningful when re-executed with the boot variable set
    }
    let d = TempDir::new();
    let (tx, _rx) = mpsc::channel(64);
    let mut cfg = ProcessConfig::new(d.path().to_path_buf());
    cfg.uid = None;
    let pm = ProcessManager::new(cfg, ChannelSender::from_mpsc(tx));

    // gate
    let g = pm.run_gate(Uuid::nil(), "env", 10).await;
    assert_eq!(g.outcome, GateOutcome::Wake);
    check(&g.output.unwrap());

    // process manager
    let r = pm
        .call("process_start", &json!({"name": "e", "command": "env"}))
        .unwrap();
    let id = r["id"].as_str().unwrap().to_owned();
    let mut tail = String::new();
    for _ in 0..100 {
        let s = pm.call("process_status", &json!({"id": id})).unwrap();
        tail = s["tail"].as_str().unwrap_or_default().to_owned();
        if tail.contains("PATH=") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    check(&tail);

    // claude-code launcher with a stub binary
    let stub = d.path().join("stub.sh");
    std::fs::write(&stub, "#!/bin/sh\nenv\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let io = ProcessLauncher { program: stub }
        .launch(LaunchSpec {
            args: vec![],
            env: vec![
                ("HTTPS_PROXY".into(), "http://127.0.0.1:3128".into()),
                ("AURORA_BOOT".into(), "leak-me".into()),
                ("GH_TOKEN".into(), "leak-me".into()),
            ],
            cwd: d.path().to_path_buf(),
        })
        .await
        .unwrap();
    let mut lines = io.lines;
    let mut all = String::new();
    while let Some(l) = tokio::time::timeout(Duration::from_secs(10), lines.recv())
        .await
        .unwrap()
    {
        all.push_str(&l);
        all.push('\n');
    }
    check(&all);
    assert!(!all.contains("GH_TOKEN"));
}

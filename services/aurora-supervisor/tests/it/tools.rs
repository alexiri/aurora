#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::common::TempDir;
use aurora_protocol::runner::{GateOutcome, HomeSync};
use aurora_protocol::supervisor::{RunnerToSupervisor, SupervisorToRunner};
use aurora_protocol::tools::{TextContent, ToolContent, ToolResult};
use aurora_supervisor::channel::ChannelSender;
use aurora_supervisor::home::{HomeConfig, HomeProjector, PartOutcome};
use aurora_supervisor::mcp_bridge;
use aurora_supervisor::process::{ProcessConfig, ProcessManager};
use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig};
use aurora_supervisor::tools::{BridgeRequest, Method, ToolHub, ToolsConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::timeout;
use uuid::Uuid;

const T: Duration = Duration::from_secs(10);
const CONV: &str = "00000000-0000-0000-0000-0000000000c1";

type Out = mpsc::Receiver<SupervisorToRunner>;

fn catalogue(dir: &std::path::Path, names: &[&str]) {
    fs::create_dir_all(dir.join(".aurora")).unwrap();
    let v: Vec<Value> = names
        .iter()
        .map(|n| json!({"name": n, "description": format!("{n} tool"), "input_schema": {"type": "object"}}))
        .collect();
    fs::write(
        dir.join(".aurora/tools.json"),
        serde_json::to_vec(&v).unwrap(),
    )
    .unwrap();
}

async fn next_matching<F: Fn(&SupervisorToRunner) -> bool>(
    rx: &mut Out,
    f: F,
) -> SupervisorToRunner {
    loop {
        let m = timeout(T, rx.recv()).await.unwrap().unwrap();
        if f(&m) {
            return m;
        }
    }
}

fn text(r: &ToolResult) -> String {
    match &r.content.as_ref().unwrap()[0] {
        ToolContent::Text(t) => t.text.clone(),
        _ => panic!("not text"),
    }
}

fn procs(dir: &std::path::Path) -> (Arc<ProcessManager>, Out) {
    let (tx, rx) = mpsc::channel(256);
    let mut cfg = ProcessConfig::new(dir.to_path_buf());
    cfg.uid = None;
    cfg.debounce = Duration::from_millis(100);
    cfg.tick = Duration::from_millis(50);
    cfg.stop_grace = Duration::from_millis(300);
    let pm = ProcessManager::new(cfg, ChannelSender::from_mpsc(tx));
    pm.spawn_background();
    (pm, rx)
}

fn hub(dir: &std::path::Path, tools: ToolsConfig) -> (Arc<ToolHub>, Out) {
    let (pm, _rx0) = procs(dir);
    let (tx, rx) = mpsc::channel(256);
    let hub = ToolHub::new(
        tools,
        dir.join(".aurora/tools.json"),
        ChannelSender::from_mpsc(tx),
        pm,
    );
    (hub, rx)
}

#[tokio::test]
async fn bridge_supervisor_runner_round_trip() {
    let d = TempDir::new();
    let root = d.path().to_path_buf();
    catalogue(&root, &["memory_save"]);
    let sock = root.join("tools.sock");
    let (out_tx, mut out_rx) = mpsc::channel(256);
    let (in_tx, in_rx) = mpsc::channel(256);
    let mut cfg = SupervisorConfig::new("127.0.0.1:1", "aub_x");
    cfg.working_dir = root.clone();
    cfg.process = ProcessConfig::new(root.clone());
    cfg.tools.socket = Some(sock.clone());
    let sup = Supervisor::new(
        cfg,
        ChannelSender::from_mpsc(out_tx),
        Arc::new(aurora_supervisor::fake::FakeHarness),
    );
    tokio::spawn(sup.run(in_rx));
    next_matching(&mut out_rx, |m| matches!(m, SupervisorToRunner::Ready(_))).await;
    timeout(T, async {
        while !sock.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .unwrap();

    // The harness side: an MCP client over a duplex pipe.
    let (client, server) = tokio::io::duplex(1 << 16);
    let (sr, sw) = tokio::io::split(server);
    tokio::spawn(mcp_bridge::run(CONV.into(), sock.clone(), sr, sw));
    let (cr, mut cw) = tokio::io::split(client);
    let mut lines = BufReader::new(cr).lines();
    let rpc = |v: Value| {
        let mut b = v.to_string().into_bytes();
        b.push(b'\n');
        b
    };
    cw.write_all(&rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}))).await.unwrap();
    cw.write_all(&rpc(
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ))
    .await
    .unwrap();
    cw.write_all(&rpc(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})))
        .await
        .unwrap();
    let init: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let list: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(list["id"], 2);
    assert_eq!(list["result"]["tools"][0]["name"], "memory_save");
    assert_eq!(
        list["result"]["tools"][0]["inputSchema"],
        json!({"type":"object"})
    );
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "process_start",
        "process_status",
        "process_stop",
        "process_watch",
    ] {
        assert!(names.contains(&n), "tools/list lacks {n}: {names:?}");
    }

    cw.write_all(&rpc(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory_save","arguments":{"key":"k"}}}))).await.unwrap();
    let SupervisorToRunner::ToolCall(call) = next_matching(&mut out_rx, |m| {
        matches!(m, SupervisorToRunner::ToolCall(_))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(call.conversation_id, Uuid::parse_str(CONV).unwrap());
    assert_eq!(call.tool, "memory_save");
    assert_eq!(call.args, json!({"key":"k"}));
    in_tx
        .send(RunnerToSupervisor::ToolResult(ToolResult::success(
            call.call_id,
            vec![ToolContent::Text(TextContent {
                text: "saved".into(),
            })],
        )))
        .await
        .unwrap();
    let res: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(res["id"], 3);
    assert_eq!(res["result"]["content"][0]["text"], "saved");
    assert_eq!(res["result"]["isError"], false);

    // A tool outside the catalogue is explained, not relayed.
    cw.write_all(&rpc(
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"gone"}}),
    ))
    .await
    .unwrap();
    let res: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(res["result"]["isError"], true);
    assert!(
        res["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not available")
    );
}

#[tokio::test]
async fn call_frame_carries_current_turn() {
    let d = TempDir::new();
    catalogue(d.path(), &["memory_get"]);
    let (hub, mut rx) = hub(d.path(), ToolsConfig::default());
    let conv = Uuid::parse_str(CONV).unwrap();
    let turn = Uuid::now_v7();
    hub.set_turn(conv, Some(turn));
    let h = hub.clone();
    let task = tokio::spawn(async move { h.call(CONV, "memory_get", json!({})).await });
    let SupervisorToRunner::ToolCall(c) = next_matching(&mut rx, |_| true).await else {
        panic!()
    };
    assert_eq!(c.turn_id, Some(turn));
    hub.on_result(ToolResult::success(c.call_id, vec![]));
    assert!(task.await.unwrap().ok);
}

#[tokio::test]
async fn breaker_opens_after_two_transport_failures_and_closes_on_success() {
    let d = TempDir::new();
    catalogue(d.path(), &["memory_get"]);
    let (hub, mut rx) = hub(
        d.path(),
        ToolsConfig {
            call_timeout: Duration::from_millis(100),
            probe_interval: Duration::from_millis(300),
            ..ToolsConfig::default()
        },
    );
    for _ in 0..2 {
        let r = hub.call(CONV, "memory_get", json!({})).await;
        assert_eq!(r.error.unwrap().code.as_str(), "tool_timeout");
        next_matching(&mut rx, |_| true).await;
    }
    // Open: immediate answer, nothing sent.
    let r = hub.call(CONV, "memory_get", json!({})).await;
    assert_eq!(r.error.unwrap().code.as_str(), "tools_unavailable");
    assert!(rx.try_recv().is_err());
    // After the probe interval one call goes through; success closes the breaker.
    tokio::time::sleep(Duration::from_millis(350)).await;
    let h = hub.clone();
    let probe = tokio::spawn(async move { h.call(CONV, "memory_get", json!({})).await });
    let SupervisorToRunner::ToolCall(c) = next_matching(&mut rx, |_| true).await else {
        panic!()
    };
    hub.on_result(ToolResult::success(c.call_id, vec![]));
    assert!(probe.await.unwrap().ok);
    let h = hub.clone();
    let again = tokio::spawn(async move { h.call(CONV, "memory_get", json!({})).await });
    let SupervisorToRunner::ToolCall(c) = next_matching(&mut rx, |_| true).await else {
        panic!()
    };
    hub.on_result(ToolResult::success(c.call_id, vec![]));
    assert!(again.await.unwrap().ok);
}

#[tokio::test]
async fn oversized_results_are_truncated_with_a_note() {
    let d = TempDir::new();
    catalogue(d.path(), &["memory_get"]);
    let (hub, mut rx) = hub(d.path(), ToolsConfig::default());
    let h = hub.clone();
    let task = tokio::spawn(async move { h.call(CONV, "memory_get", json!({})).await });
    let SupervisorToRunner::ToolCall(c) = next_matching(&mut rx, |_| true).await else {
        panic!()
    };
    let big = "é\"".repeat(100_000);
    hub.on_result(ToolResult::success(
        c.call_id,
        vec![ToolContent::Text(TextContent { text: big })],
    ));
    let r = task.await.unwrap();
    assert!(r.ok);
    assert!(serde_json::to_vec(&r).unwrap().len() <= 64 * 1024);
    assert!(text(&r).ends_with("[result truncated: exceeded 64 KiB]"));
}

#[tokio::test]
async fn socket_speaks_s09() {
    let d = TempDir::new();
    catalogue(d.path(), &["process_status"]);
    let (hub, _rx) = hub(d.path(), ToolsConfig::default());
    let sock: PathBuf = d.path().join("t.sock");
    let s2 = sock.clone();
    tokio::spawn(hub.serve_owned(s2));
    timeout(T, async {
        while !sock.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .unwrap();
    let mut s = tokio::net::UnixStream::connect(&sock).await.unwrap();
    let req = BridgeRequest {
        id: 7,
        session: CONV.into(),
        method: Method::Call,
        tool: Some("process_status".into()),
        arguments: Some(json!({})),
    };
    let mut line = serde_json::to_vec(&req).unwrap();
    line.push(b'\n');
    s.write_all(&line).await.unwrap();
    let mut r = BufReader::new(s);
    let mut resp = String::new();
    r.read_line(&mut resp).await.unwrap();
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["id"], 7);
    assert_eq!(v["result"]["ok"], true);
}

#[tokio::test]
async fn process_pattern_watch_fires_and_state_is_reported() {
    let d = TempDir::new();
    let (pm, mut rx) = procs(d.path());
    let id = pm
        .call(
            "process_start",
            &json!({"name":"srv","command":"sleep 0.4; echo READY port 80; sleep 30"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    pm.call(
        "process_watch",
        &json!({"id": id, "kind":"pattern","pattern":"READY"}),
    )
    .unwrap();
    let SupervisorToRunner::WatchFired(w) =
        next_matching(&mut rx, |m| matches!(m, SupervisorToRunner::WatchFired(_))).await
    else {
        panic!()
    };
    assert!(w.detail.contains("READY port 80"));
    let SupervisorToRunner::ProcessState(s) = next_matching(&mut rx, |m| {
        matches!(m, SupervisorToRunner::ProcessState(_))
    })
    .await
    else {
        panic!()
    };
    assert_eq!(s.processes[0].process_id, id);
    let st = pm.call("process_status", &json!({"id": id})).unwrap();
    assert_eq!(st["state"], "running");
    assert!(st["tail"].as_str().unwrap().contains("READY"));
    pm.call("process_stop", &json!({"id": id})).unwrap();
    next_matching(&mut rx, |m| matches!(m, SupervisorToRunner::ProcessState(s) if s.processes.iter().any(|p| p.state.as_str() == "exited"))).await;
}

#[tokio::test]
async fn process_silence_and_exit_watches_fire() {
    let d = TempDir::new();
    let (pm, mut rx) = procs(d.path());
    let a = pm
        .call(
            "process_start",
            &json!({"name":"quiet","command":"sleep 30"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let w = pm
        .call(
            "process_watch",
            &json!({"id": a, "kind":"silence","silence_secs":1}),
        )
        .unwrap()["watch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let SupervisorToRunner::WatchFired(f) =
        next_matching(&mut rx, |m| matches!(m, SupervisorToRunner::WatchFired(_))).await
    else {
        panic!()
    };
    assert_eq!(f.watch_id, w);
    pm.call("process_stop", &json!({"id": a})).unwrap();

    let b = pm
        .call(
            "process_start",
            &json!({"name":"short","command":"sleep 0.3; exit 3"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let w = pm
        .call("process_watch", &json!({"id": b, "kind":"exit"}))
        .unwrap()["watch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let SupervisorToRunner::WatchFired(f) = next_matching(
        &mut rx,
        |m| matches!(m, SupervisorToRunner::WatchFired(f) if f.watch_id == w),
    )
    .await
    else {
        panic!()
    };
    assert!(f.detail.contains("code Some(3)"));
}

#[tokio::test]
async fn process_arguments_are_enforced() {
    let d = TempDir::new();
    let (pm, _rx) = procs(d.path());
    assert!(
        pm.call(
            "process_start",
            &json!({"name":"x","command":"true","cwd":"../etc"})
        )
        .is_err()
    );
    assert!(
        pm.call("process_start", &json!({"name":"","command":"true"}))
            .is_err()
    );
    assert!(
        pm.call("process_watch", &json!({"id":"p9","kind":"exit"}))
            .is_err()
    );
    assert!(pm.call("process_stop", &json!({"id":"p9"})).is_err());
}

#[tokio::test]
async fn gates_capture_output_and_time_out() {
    let d = TempDir::new();
    let (pm, _rx) = procs(d.path());
    let t = Uuid::now_v7();
    let r = pm.run_gate(t, "echo changed", 5).await;
    assert_eq!(
        (r.outcome, r.output.as_deref()),
        (GateOutcome::Wake, Some("changed"))
    );
    let r = pm.run_gate(t, "printf ''", 5).await;
    assert_eq!(r.outcome, GateOutcome::Quiet);
    let r = pm.run_gate(t, "echo '{\"wake_agent\": false}'", 5).await;
    assert_eq!(r.outcome, GateOutcome::Quiet);
    let r = pm.run_gate(t, "echo oops; exit 1", 5).await;
    assert_eq!(
        (r.outcome, r.output.as_deref()),
        (GateOutcome::Failed, Some("oops"))
    );
    let started = std::time::Instant::now();
    let r = pm.run_gate(t, "sleep 30", 1).await;
    assert_eq!(r.outcome, GateOutcome::Failed);
    assert!(started.elapsed() < Duration::from_secs(5));
    let r = pm
        .run_gate(t, "head -c 100000 /dev/zero | tr '\\0' a", 5)
        .await;
    assert_eq!(r.outcome, GateOutcome::Wake);
    assert!(r.output.unwrap().len() <= aurora_protocol::runner::MAX_GATE_OUTPUT);
}

#[test]
fn skill_exec_bit_selects_0555_or_0444() {
    let d = TempDir::new();
    let mut p = HomeProjector::new(HomeConfig {
        root: d.path().to_path_buf(),
        skills_dir: Some(".claude/skills".into()),
        instruction_files: vec!["AGENTS.md".into()],
    });
    let part = |i: u32, kind: &str, entries: Value| -> HomeSync {
        serde_json::from_value(json!({"generation":1,"part_index":i,"part_count":5,"part":{"kind":kind,"entries":entries}})).unwrap()
    };
    let parts = [
        part(
            0,
            "skills",
            json!([
                {"path": ".claude/skills/a/SKILL.md", "content": "doc"},
                {"path": ".claude/skills/a/run.sh", "content": "#!/bin/sh\n", "exec": true},
            ]),
        ),
        part(
            1,
            "render_inputs",
            json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}}]),
        ),
        part(2, "tools", json!([])),
        part(
            3,
            "manifest",
            json!([{"path": ".claude/skills/a/SKILL.md"}, {"path": ".claude/skills/a/run.sh"}]),
        ),
        part(4, "final", json!([])),
    ];
    let mut last = PartOutcome::Staged;
    for s in &parts {
        last = p.apply_part(s).unwrap();
    }
    assert_eq!(last, PartOutcome::ReadyToCommit(1));
    p.commit(1).unwrap();
    let mode = |f: &str| fs::metadata(d.path().join(f)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(".claude/skills/a/SKILL.md"), 0o444);
    assert_eq!(mode(".claude/skills/a/run.sh"), 0o555);
}

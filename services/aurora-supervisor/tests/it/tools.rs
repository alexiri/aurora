#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::common::TempDir;
use aurora_protocol::runner::{GateOutcome, HomeSync};
use aurora_protocol::supervisor::SupervisorToRunner;
use aurora_protocol::tools::{TextContent, ToolContent, ToolResult};
use aurora_supervisor::channel::ChannelSender;
use aurora_supervisor::home::{HomeConfig, HomeProjector, PartOutcome};
use aurora_supervisor::mcp_bridge;
use aurora_supervisor::process::{ProcessConfig, ProcessManager};
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
    // (FIX-110) A tool call needs a running turn: the hub serves the socket with one recorded.
    let (hub, mut out_rx) = hub(&root, ToolsConfig::default());
    hub.set_turn(Uuid::parse_str(CONV).unwrap(), Some(Uuid::now_v7()));
    tokio::spawn(hub.clone().serve_owned(sock.clone()));
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
    hub.on_result(ToolResult::success(
        call.call_id,
        vec![ToolContent::Text(TextContent {
            text: "saved".into(),
        })],
    ));
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

/// (FIX-110, SEC-24) With no turn running in the conversation (a process left behind by an
/// earlier turn) a tool call is refused locally: no frame reaches the runner.
#[tokio::test]
async fn no_frame_is_sent_without_a_running_turn() {
    let d = TempDir::new();
    catalogue(d.path(), &["memory_get"]);
    let (hub, mut rx) = hub(d.path(), ToolsConfig::default());
    let conv = Uuid::parse_str(CONV).unwrap();
    let r = hub.call(CONV, "memory_get", json!({})).await;
    assert_eq!(r.error.unwrap().code.as_str(), "no_active_turn");
    assert!(rx.try_recv().is_err(), "nothing was sent");
    // After the turn ended the same applies.
    hub.set_turn(conv, Some(Uuid::now_v7()));
    hub.set_turn(conv, None);
    let r = hub.call(CONV, "memory_get", json!({})).await;
    assert_eq!(r.error.unwrap().code.as_str(), "no_active_turn");
    assert!(rx.try_recv().is_err(), "nothing was sent");
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
    hub.set_turn(Uuid::parse_str(CONV).unwrap(), Some(Uuid::now_v7()));
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
    hub.set_turn(Uuid::parse_str(CONV).unwrap(), Some(Uuid::now_v7()));
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

/// (FIX-114, `12` §Process model) A child nobody waits for (what an orphan reparented to PID 1
/// looks like) is reaped by the supervisor's reaper instead of staying a zombie. The test process
/// is the parent here; the real thing runs as PID 1 (hosted e2e).
#[tokio::test]
async fn the_reaper_collects_zombie_children_nobody_waits_for() {
    use aurora_supervisor::process::{parse_proc_stat, spawn_reaper};
    assert_eq!(parse_proc_stat("12 (a b) c) Z 7 1 1"), Some(('Z', 7)));
    assert_eq!(parse_proc_stat("garbage"), None);
    let child = aurora_supervisor::spawn::command("true", std::iter::empty::<(&str, &str)>())
        .spawn()
        .unwrap();
    let pid = child.id();
    // Leak the handle: nobody calls wait(), so the exited child stays a zombie.
    std::mem::forget(child);
    let state = |pid: u32| {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| parse_proc_stat(&s))
            .map(|(st, _)| st)
    };
    timeout(T, async {
        while state(pid) != Some('Z') {
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .expect("the child became a zombie");
    let reaper = spawn_reaper(Duration::from_millis(100));
    timeout(T, async {
        while state(pid).is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .expect("the zombie was reaped");
    reaper.abort();
}

/// (FIX-114) Shutdown signals the registered process groups.
#[tokio::test]
async fn terminate_all_signals_running_groups() {
    let d = TempDir::new();
    let (pm, _rx) = procs(d.path());
    let id = pm.start("sleeper", "sleep 30", None).unwrap();
    // pid is recorded right after spawn
    timeout(T, async {
        while pm.terminate_all() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .expect("a running group was signalled");
    timeout(T, async {
        while pm.running_ids().contains(&id) {
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .expect("the SIGTERM ended the process");
}

/// (FIX-114) 200 quick starts keep the table bounded (exited records are capped), and N+1
/// concurrent starts yield at most `max_running` (the slot is reserved under the check's lock).
#[tokio::test]
async fn process_table_is_bounded_and_concurrent_starts_respect_max_running() {
    use aurora_supervisor::process::MAX_EXITED_RECORDS;
    let d = TempDir::new();
    let (pm, mut rx) = procs(d.path());
    // The runner side drains the reports (records not yet reported are never pruned).
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    for i in 0..200 {
        // Stay below `max_running`: this part is about exited records, not the running cap.
        while pm.running_ids().len() >= 8 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let id = pm.start(&format!("q{i}"), "true", None);
        assert!(id.is_ok(), "{id:?}");
    }
    // Pruning runs inside `start` and spares records whose exit is not reported yet, so how many
    // survive the loop depends on how far the drain got (a fixed 500 ms sleep was flaky on a
    // loaded CI runner: 88 records). Once the reports drain, the next start prunes to the cap:
    // keep starting quick processes until it does. An unbounded table never gets there and
    // fails at the deadline.
    let bounded = timeout(T, async {
        while pm.record_count() > MAX_EXITED_RECORDS + 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            while pm.running_ids().len() >= 8 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            pm.start("prune-probe", "true", None).unwrap();
        }
    })
    .await;
    assert!(bounded.is_ok(), "{} records", pm.record_count());
    // N+1 concurrent starts of long-running processes: at most max_running (16) succeed.
    let (pm, _rx) = procs(d.path());
    let mut joins = Vec::new();
    for i in 0..24 {
        let pm = pm.clone();
        joins.push(tokio::task::spawn_blocking(move || {
            pm.start(&format!("s{i}"), "sleep 30", None).is_ok()
        }));
    }
    let mut ok = 0;
    for j in joins {
        ok += usize::from(j.await.unwrap());
    }
    assert_eq!(ok, 16, "exactly max_running starts win");
    for id in pm.running_ids() {
        let _ = pm.stop(&id);
    }
}

/// (FIX-114) Above the per-connection in-flight cap a call is refused `tools_unavailable`, and an
/// over-long bridge line is refused instead of being buffered.
#[tokio::test]
async fn over_cap_calls_and_over_long_bridge_lines_are_refused() {
    use aurora_supervisor::mcp_bridge::{Line, next_line_bounded};
    // The reader refuses a line over the bound, resynchronises at the next newline and keeps
    // the line after it.
    let mut data = vec![b'x'; 5000];
    data.extend_from_slice(b"\nshort\n");
    let mut r = BufReader::new(&data[..]);
    let (mut buf, mut skipping) = (Vec::new(), false);
    let first = next_line_bounded(&mut r, &mut buf, &mut skipping, 1000)
        .await
        .unwrap();
    assert!(matches!(first, Some(Line::TooLong)));
    let second = next_line_bounded(&mut r, &mut buf, &mut skipping, 1000)
        .await
        .unwrap();
    assert!(matches!(second, Some(Line::Ok(ref s)) if s == "short"));
    assert!(
        next_line_bounded(&mut r, &mut buf, &mut skipping, 1000)
            .await
            .unwrap()
            .is_none()
    );

    // In-flight cap on the tools socket: with the cap at 1 and one call waiting for its result,
    // the second request is refused.
    let d = TempDir::new();
    catalogue(d.path(), &["memory_get"]);
    let (hub, mut rx) = hub(
        d.path(),
        ToolsConfig {
            max_inflight: 1,
            ..ToolsConfig::default()
        },
    );
    hub.set_turn(Uuid::parse_str(CONV).unwrap(), Some(Uuid::now_v7()));
    let sock: PathBuf = d.path().join("cap.sock");
    tokio::spawn(hub.clone().serve_owned(sock.clone()));
    timeout(T, async {
        while !sock.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .unwrap();
    let mut s = tokio::net::UnixStream::connect(&sock).await.unwrap();
    let req = |id: u64| {
        let r = BridgeRequest {
            id,
            session: CONV.into(),
            method: Method::Call,
            tool: Some("memory_get".into()),
            arguments: Some(json!({})),
        };
        let mut line = serde_json::to_vec(&r).unwrap();
        line.push(b'\n');
        line
    };
    s.write_all(&req(1)).await.unwrap();
    // The first call reaches the runner and waits for its result.
    let SupervisorToRunner::ToolCall(c) = next_matching(&mut rx, |_| true).await else {
        panic!()
    };
    s.write_all(&req(2)).await.unwrap();
    let mut r = BufReader::new(s);
    let mut resp = String::new();
    r.read_line(&mut resp).await.unwrap();
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["id"], 2, "{v}");
    assert_eq!(v["error"]["code"], "tools_unavailable", "{v}");
    hub.on_result(ToolResult::success(c.call_id, vec![]));
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
        pg: None,
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

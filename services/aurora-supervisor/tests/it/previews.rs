//! (P4-PRV-1, D-144/D-145, `18 §Port previews` → Supervisor) The preview share list lives in
//! supervisor memory only (a file planted in the home changes nothing), `preview.start` for an
//! unknown port, a share without a command, an already listening port or an empty list is a
//! no-op, and a started command runs as `preview:<port>` with the scrubbed container environment
//! (`SEC-34`/I-23: no `AURORA_*` boot variable, no platform credential). The frame carries no
//! command text (`16 §W5`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use crate::common::{TempDir, stub_gateway};
use aurora_supervisor::fake::FakeHarness;
use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const T: Duration = Duration::from_secs(10);

struct Conn {
    ws: WebSocketStream<TcpStream>,
    seq: u64,
    last_in: u64,
}

impl Conn {
    async fn send(&mut self, kind: &str, body: Value) {
        self.seq += 1;
        let f = json!({"seq": self.seq, "ack": self.last_in, "kind": kind, "body": body});
        self.ws.send(Message::text(f.to_string())).await.unwrap();
    }

    async fn next(&mut self) -> Value {
        loop {
            let m = timeout(T, self.ws.next())
                .await
                .expect("timeout")
                .expect("closed")
                .unwrap();
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["kind"] == "ack" {
                continue;
            }
            let seq = v["seq"].as_u64().unwrap();
            assert_eq!(
                seq,
                self.last_in + 1,
                "frame out of order or duplicated: {v}"
            );
            self.last_in = seq;
            return v;
        }
    }

    async fn next_of(&mut self, kind: &str) -> Value {
        loop {
            let v = self.next().await;
            if v["kind"] == kind {
                return v["body"].clone();
            }
        }
    }

    /// Applies the whole sync generation (manifest, the part under test, final). The `previews`
    /// part itself never touches the projector; the `final` part commits the generation and
    /// answers `home.synced`.
    async fn sync(&mut self, kind: &str, entries: Value) {
        self.send(
            "home.sync",
            json!({"generation": 7, "part_index": 0, "part_count": 3, "part": {"kind": "manifest", "entries": []}}),
        )
        .await;
        self.send(
            "home.sync",
            json!({"generation": 7, "part_index": 1, "part_count": 3, "part": {"kind": kind, "entries": entries}}),
        )
        .await;
        self.send(
            "home.sync",
            json!({"generation": 7, "part_index": 2, "part_count": 3, "part": {"kind": "final", "entries": []}}),
        )
        .await;
        self.next_of("home.synced").await;
    }

    async fn preview_start(&mut self, port: u16) {
        self.send("preview.start", json!({"port": port})).await;
    }
}

/// A loopback port that is free right now (unique per run, so a leftover listener from an
/// earlier run of this binary cannot turn `preview.start` into an "already listening" no-op).
async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start(home: &TempDir) -> (TcpListener, Conn) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/v1/supervisor", listener.local_addr().unwrap());
    let mut cfg = aurora_supervisor::channel::ChannelConfig::new(
        url,
        aurora_protocol::common::SecretString::new("aup_test"),
    );
    cfg.ack_interval = Duration::from_millis(100);
    cfg.backoff_initial = Duration::from_millis(50);
    let chan = aurora_supervisor::channel::spawn(cfg);
    let mut sc = SupervisorConfig::new(stub_gateway().await.0, "aub_boot");
    sc.working_dir = home.path().to_path_buf();
    sc.process = aurora_supervisor::process::ProcessConfig::new(home.path().to_path_buf());
    sc.process.uid = None;
    sc.process.debounce = Duration::from_millis(100);
    sc.tools.socket = Some(home.path().join("tools.sock"));
    let sup = Supervisor::new(
        sc,
        chan.sender,
        std::sync::Arc::new(FakeHarness),
        aurora_supervisor::streams::StreamsHandle::noop(),
        aurora_supervisor::pty::PtyHandle::noop(),
    );
    tokio::spawn(sup.run(chan.inbound));
    tokio::spawn(async move {
        let _ = chan.task.await;
    });
    let (tcp, _) = timeout(T, listener.accept()).await.unwrap().unwrap();
    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    let hello: Value = match timeout(T, ws.next()).await.unwrap().unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
        m => panic!("unexpected {m:?}"),
    };
    assert_eq!(hello["hello"]["bootstrap_token"], "aup_test");
    let welcome = json!({"welcome": {
        "session": {"sandbox_id": "00000000-0000-0000-0000-000000000001",
                    "boot_id": "00000000-0000-0000-0000-000000000002",
                    "issued_at": 1, "mac": "m"},
        "resume_from": 0 }});
    ws.send(Message::text(welcome.to_string())).await.unwrap();
    let mut conn = Conn {
        ws,
        seq: 0,
        last_in: 0,
    };
    conn.next_of("ready").await;
    (listener, conn)
}

/// The agent-side view of the processes: `process_status` over the tools socket.
async fn process_status(home: &TempDir, id: Option<&str>) -> Value {
    let stream = tokio::net::UnixStream::connect(home.path().join("tools.sock"))
        .await
        .unwrap();
    let (mut r, mut w) = stream.into_split();
    let req = json!({
        "id": 1, "session": "00000000-0000-0000-0000-0000000000c1", "method": "call",
        "tool": "process_status",
        "arguments": match id {
            Some(id) => json!({"id": id}),
            None => json!({}),
        }
    });
    w.write_all(format!("{req}\n").as_bytes()).await.unwrap();
    w.flush().await.unwrap();
    let mut line = String::new();
    timeout(T, tokio::io::BufReader::new(&mut r).read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let resp: Value = serde_json::from_str(line.trim()).unwrap();
    let content = resp["result"]["content"].as_array().unwrap();
    serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap()
}

/// The preview app the command binds: an HTTP server that answers with the variables it sees.
/// The script is staged into the workspace (quotes survive `sh -c` untouched that way).
fn serve(home: &TempDir, port: u16) -> String {
    let path = home.path().join(format!(".aurora/preview-serve-{port}.py"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"
import os, socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', 0)) # replaced below
s.listen(8)
while True:
    c, _ = s.accept()
    c.recv(65536)
    env = b'|'.join(k.encode() + b'=' + v.encode() for k, v in os.environ.items())
    body = b'seen:' + env
    c.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: ' + str(len(body)).encode() + b'\r\nConnection: close\r\n\r\n' + body)
    c.close()
"#.replace("s.bind(('127.0.0.1', 0))", &format!("s.bind(('127.0.0.1', {port}))")),
    )
    .unwrap();
    format!("python3 {}", path.display())
}

#[tokio::test]
async fn share_list_is_memory_only_and_start_runs_agent_env() {
    let home = TempDir::new();
    let (_l, mut c) = start(&home).await;
    // A unique port per run: a leftover listener from another test binary must not make
    // `preview.start` an "already listening" no-op.
    let port = free_port().await;
    let p4 = port.to_string();

    // A file planted in the home is never a share: the list comes from the `previews` part.
    std::fs::write(
        home.path().join("previews.json"),
        r#"{"shares":[{"share_id":"00000000-0000-0000-0000-00000000000f","port":3000,"start_command":"sleep 60"}]}"#,
    )
    .unwrap();

    // A full generation installs the planted file as an ordinary home file (agent-writable);
    // the `previews` part is skipped by the projector, the list stays in memory.
    c.sync("skills", json!([{"path": "previews.json", "content": r#"{"shares":[{"share_id":"00000000-0000-0000-0000-00000000000f","port":3000,"start_command":"sleep 60"}]}"#}])).await;

    // Not in the (empty) share list: a no-op, no process ever appears.
    c.preview_start(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let listed = process_status(&home, None).await;
    assert!(
        listed["processes"].as_array().unwrap().is_empty(),
        "planted home file started a process: {listed}"
    );

    // The real share list (in memory only), then `preview.start`.
    c.sync(
        "previews",
        json!([{"shares": [{"share_id": "00000000-0000-0000-0000-00000000000a", "port": port,
                            "start_command": format!("cd {} && {}", home.path().display(), serve(&home, port))}]}]),
    )
    .await;
    c.preview_start(port).await;

    // `preview:<port>` runs and sees the scrubbed container environment (SEC-34).
    let deadline = tokio::time::Instant::now() + T;
    loop {
        let listed = process_status(&home, None).await;
        let procs = listed["processes"].as_array().unwrap();
        if let Some(p) = procs.iter().find(|p| {
            p["name"]
                .as_str()
                .is_some_and(|n| n == format!("preview:{p4}"))
        }) {
            assert_eq!(p["state"].as_str().unwrap(), "running", "{p}");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no preview process: {listed}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // `running` in the supervisor's process list is not listener readiness: the app can bind
    // its loopback listener a beat later, so a single-shot connect loses that race on CI
    // (FIX-049). Retry the connect until a deadline instead.
    let mut stream = None;
    let mut last = None;
    let bind_deadline = tokio::time::Instant::now() + T;
    while tokio::time::Instant::now() < bind_deadline {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("unexpected error connecting to the preview on {port}: {e}"),
        }
    }
    let mut stream = stream
        .unwrap_or_else(|| panic!("preview on {port} never bound its listener in 10s: {last:?}"));
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut body = Vec::new();
    timeout(T, stream.read_to_end(&mut body))
        .await
        .unwrap()
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("seen:"), "body {body:?}");
    assert!(!body.contains("AURORA_BOOT"), "boot leaked: {body}");
    assert!(!body.contains("aup_"), "platform credential leaked: {body}");
    assert!(body.contains("PATH="), "{body}");

    // Already listening: a second `preview.start` is a no-op (one process stays).
    c.preview_start(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let listed = process_status(&home, None).await;
    let previews = listed["processes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| {
            p["name"]
                .as_str()
                .is_some_and(|n| n == format!("preview:{p4}"))
        })
        .count();
    assert_eq!(previews, 1, "{listed}");
}

#[tokio::test]
async fn start_no_ops_for_unknown_port_commandless_share_and_empty_list() {
    let home = TempDir::new();
    let (_l, mut c) = start(&home).await;
    let p0 = free_port().await;
    let (p1, p2, p3) = (p0 as u16 + 1, p0 as u16 + 2, p0 as u16 + 3);

    // Empty list (the part is always sent when previews run, even with no shares).
    c.sync("previews", json!([{"shares": []}])).await;
    c.preview_start(p0).await;
    // Unknown port with a share on another port.
    c.sync(
        "previews",
        json!([{"shares": [{"share_id": "00000000-0000-0000-0000-00000000000b", "port": p1,
                            "start_command": "sleep 60"}]}]),
    )
    .await;
    c.preview_start(p0).await;
    // Share without a start command.
    c.sync(
        "previews",
        json!([{"shares": [{"share_id": "00000000-0000-0000-0000-00000000000c", "port": 4762}]}]),
    )
    .await;
    c.preview_start(p2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let listed = process_status(&home, None).await;
    assert!(
        listed["processes"].as_array().unwrap().is_empty(),
        "a no-op preview.start ran a process: {listed}"
    );

    // An empty or blank command is also refused.
    c.sync(
        "previews",
        json!([{"shares": [{"share_id": "00000000-0000-0000-0000-00000000000d", "port": p3,
                            "start_command": "   "}]}]),
    )
    .await;
    c.preview_start(p3).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let listed = process_status(&home, None).await;
    assert!(
        listed["processes"].as_array().unwrap().is_empty(),
        "a blank command started a process: {listed}"
    );
}

/// The wire frame carries only the port: `PreviewStartFrame` has no command field and the
/// supervisor looks the command up in its own list (`16 §W5`).
#[test]
fn preview_start_frame_is_port_only() {
    let v =
        serde_json::to_value(aurora_protocol::runner::PreviewStartFrame { port: 8080 }).unwrap();
    assert_eq!(v, json!({"port": 8080}), "{v}");
}

/// (P4-PRV-2) Port discovery: the supervisor scans loopback listeners and reports
/// `ports.state { ports }` on change (`11 §Port discovery`, `16 §W5`) — the first report
/// fires immediately after boot, without waiting a tick, so the API derives a share's
/// `listening` status from the current boot's ports.
#[tokio::test]
async fn ports_state_reports_loopback_listeners_on_change() {
    let home = TempDir::new();
    let (listener, mut c) = start(&home).await;

    // A port the supervisor is not itself listening on becomes part of the first report.
    let app = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = app.local_addr().unwrap().port();
    let mut seen: Vec<u16> = Vec::new();
    let deadline = tokio::time::Instant::now() + T;
    'outer: while tokio::time::Instant::now() < deadline {
        let v = timeout(T, c.next()).await.expect("no frame");
        if v["kind"] == "ports.state"
            && let Some(ports) = v["body"]["ports"].as_array()
        {
            seen = ports
                .iter()
                .filter_map(|p| p.as_u64())
                .map(|p| p as u16)
                .collect();
            break 'outer;
        }
    }
    assert!(
        seen.contains(&port),
        "ports.state {seen:?} misses the bound {port}"
    );
    assert!(!seen.contains(&0), "port 0 is never reported: {seen:?}");

    // No change → no re-send; change → re-send.
    let quiet = std::time::Instant::now();
    while quiet.elapsed() < Duration::from_millis(700) {
        let left = 700_u64.saturating_sub(quiet.elapsed().as_millis() as u64);
        match timeout(Duration::from_millis(left), c.next()).await {
            Err(_) => break,
            Ok(v) => assert_ne!(v["kind"], "ports.state", "re-sent without a change: {v}"),
        }
    }
    drop(app);
    let deadline = tokio::time::Instant::now() + T;
    while tokio::time::Instant::now() < deadline {
        let v = timeout(T, c.next()).await.expect("no frame");
        if v["kind"] == "ports.state" {
            let ports = v["body"]["ports"].as_array().unwrap();
            assert!(
                !ports.iter().any(|p| p.as_u64() == Some(u64::from(port))),
                "closed port still reported: {ports:?}"
            );
            break;
        }
    }
    drop(listener);
}

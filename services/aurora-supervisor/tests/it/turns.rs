#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use crate::common::{decode_basic, stub_gateway};
use aurora_protocol::common::SecretString;
use aurora_supervisor::channel::{self, ChannelConfig};
use aurora_supervisor::fake::FakeHarness;
use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const T: Duration = Duration::from_secs(10);
const CONV: &str = "00000000-0000-0000-0000-0000000000c1";

struct Runner {
    listener: TcpListener,
    /// The supervisor's working directory (its home).
    dir: std::path::PathBuf,
}

struct Conn {
    ws: WebSocketStream<TcpStream>,
    seq: u64,
    last_in: u64,
    hello: Value,
}

impl Runner {
    async fn accept(&self, resume_from: u64, start_seq: u64, last_in: u64) -> Conn {
        let (tcp, _) = timeout(T, self.listener.accept()).await.unwrap().unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let hello: Value = match timeout(T, ws.next()).await.unwrap().unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
            m => panic!("unexpected {m:?}"),
        };
        let welcome = json!({"welcome": {
            "session": {"sandbox_id": "00000000-0000-0000-0000-000000000001",
                        "boot_id": "00000000-0000-0000-0000-000000000002",
                        "issued_at": 1, "mac": "m"},
            "resume_from": resume_from }});
        ws.send(Message::text(welcome.to_string())).await.unwrap();
        Conn {
            ws,
            seq: start_seq,
            last_in,
            hello,
        }
    }
}

impl Conn {
    async fn send(&mut self, kind: &str, body: Value) {
        self.seq += 1;
        let f = json!({"seq": self.seq, "ack": self.last_in, "kind": kind, "body": body});
        self.ws.send(Message::text(f.to_string())).await.unwrap();
    }

    /// Next sequenced frame (acks skipped); asserts strictly contiguous seqs.
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

    async fn deliver(&mut self, turn: &str, text: &str, cred: &str) {
        self.send(
            "turn.deliver",
            json!({
                "conversation_id": CONV, "turn_id": turn, "session": {"epoch": 1},
                "model": {"provider": "p", "endpoint_id": "e", "base_url": "http://x", "dialect": "anthropic_messages",
                          "model": "m", "effort": "low", "client_env": []},
                "context_credential": cred,
                "input": {"text": text, "speaker_label": "Ann", "door": "web", "recall": {"index": "", "snippets": []}, "notices": []},
                "attachments": []
            }),
        )
        .await;
    }

    async fn home_sync(&mut self) {
        self.send(
            "home.sync",
            json!({"generation": 1, "part_index": 0, "part_count": 2, "part": {"kind": "manifest", "entries": []}}),
        )
        .await;
        self.send(
            "home.sync",
            json!({"generation": 1, "part_index": 1, "part_count": 2, "part": {"kind": "final", "entries": []}}),
        )
        .await;
        self.next_of("home.synced").await;
    }
}

async fn start(gateway: aurora_supervisor::proxy::GatewayUpstream) -> (Runner, Conn) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/v1/supervisor", listener.local_addr().unwrap());
    let mut cfg = ChannelConfig::new(url, SecretString::new("aup_test"));
    cfg.ack_interval = Duration::from_millis(100);
    cfg.backoff_initial = Duration::from_millis(50);
    let chan = channel::spawn(cfg);
    let mut sc = SupervisorConfig::new(gateway, "aub_boot");
    sc.progress_interval = Duration::from_millis(100);
    let dir = crate::common::TempDir::new().keep();
    sc.working_dir = dir.clone();
    let sup = Supervisor::new(sc, chan.sender, Arc::new(FakeHarness));
    tokio::spawn(sup.run(chan.inbound));
    tokio::spawn(async move {
        let _ = chan.task.await;
    });
    let runner = Runner { listener, dir };
    let mut conn = runner.accept(0, 0, 0).await;
    assert_eq!(conn.hello["hello"]["bootstrap_token"], "aup_test");
    assert_eq!(conn.hello["hello"]["protocol"], 1);
    let ready = conn.next_of("ready").await;
    assert_eq!(ready["harness"]["id"], "fake");
    assert!(
        ready["harness"]["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("steer"))
    );
    (runner, conn)
}

const TURN: &str = "00000000-0000-0000-0000-0000000000a1";

#[tokio::test]
async fn full_fake_turn_ordered_events_and_context_credential() {
    let (gw, seen) = stub_gateway().await;
    let (_r, mut c) = start(gw.clone()).await;
    c.home_sync().await;
    let text = format!(
        "[[say: hi]] [[tool: memory_save {{}}]] [[http: GET http://{}/ping]] [[say: bye]]",
        gw.addr
    );
    c.deliver(TURN, &text, "auc_turn1").await;
    assert_eq!(c.next_of("turn.started").await["turn_id"], TURN);

    let mut kinds = Vec::new();
    let mut ns = Vec::new();
    let result = loop {
        let v = c.next().await;
        match v["kind"].as_str().unwrap() {
            "turn.event" => {
                ns.push(v["body"]["n"].as_u64().unwrap());
                kinds.push(v["body"]["event"]["kind"].as_str().unwrap().to_owned());
            }
            "turn.result" => break v["body"].clone(),
            _ => {}
        }
    };
    assert_eq!(
        kinds,
        [
            "text.delta",
            "tool.started",
            "tool.finished",
            "activity",
            "text.delta",
            "text.delta"
        ]
    );
    assert_eq!(ns, [1, 2, 3, 4, 5, 6]);
    assert_eq!(result["status"], "done");
    assert!(result["usage"].is_object());

    // The harness request went through its session proxy with the turn's context credential.
    let creds: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|h| decode_basic(&h[0]))
        .collect();
    assert_eq!(creds, ["x:auc_turn1"]);
}

#[tokio::test]
async fn echo_and_not_ready() {
    let (gw, _) = stub_gateway().await;
    let (_r, mut c) = start(gw).await;
    c.deliver(TURN, "hello", "auc_t").await;
    let r = c.next_of("turn.result").await;
    assert_eq!(
        (r["status"].as_str(), r["error_code"].as_str()),
        (Some("failed"), Some("not_ready"))
    );
    c.home_sync().await;
    c.deliver(TURN, "hello big world", "auc_t").await;
    let mut text = String::new();
    loop {
        let v = c.next().await;
        if v["kind"] == "turn.event" {
            text.push_str(v["body"]["event"]["delta"].as_str().unwrap());
        }
        if v["kind"] == "turn.result" {
            break;
        }
    }
    assert_eq!(text, "echo: hello big world");
}

#[tokio::test]
async fn abort_ends_with_aborted_result() {
    let (gw, _) = stub_gateway().await;
    let (_r, mut c) = start(gw).await;
    c.home_sync().await;
    c.deliver(TURN, "[[say: a]] [[sleep: 30s]]", "auc_t").await;
    c.next_of("turn.event").await;
    c.send("turn.abort", json!({"turn_id": TURN})).await;
    let r = c.next_of("turn.result").await;
    assert_eq!(r["status"], "aborted");
}

#[tokio::test]
async fn steer_delivered_missed_and_refused() {
    let (gw, _) = stub_gateway().await;
    let (_r, mut c) = start(gw).await;
    c.home_sync().await;
    let f1 = "00000000-0000-0000-0000-0000000000f1";
    // between turns: refused
    c.send(
        "turn.message",
        json!({"turn_id": TURN, "followup_id": f1, "text": "x", "speaker_label": "Ann"}),
    )
    .await;
    assert_eq!(c.next_of("steer.result").await["outcome"], "refused");

    c.deliver(TURN, "[[steer-accept]]", "auc_t").await;
    c.next_of("turn.started").await;
    c.send(
        "turn.message",
        json!({"turn_id": TURN, "followup_id": f1, "text": "more", "speaker_label": "Ann"}),
    )
    .await;
    let s = c.next_of("steer.result").await;
    assert_eq!(
        (s["followup_id"].as_str(), s["outcome"].as_str()),
        (Some(f1), Some("delivered"))
    );
    assert_eq!(c.next_of("turn.result").await["status"], "done");

    // harness without [[steer-accept]] refuses a live steer
    let t2 = "00000000-0000-0000-0000-0000000000a2";
    c.deliver(t2, "[[sleep: 1s]]", "auc_t").await;
    c.next_of("turn.started").await;
    c.send(
        "turn.message",
        json!({"turn_id": t2, "followup_id": f1, "text": "x", "speaker_label": "Ann"}),
    )
    .await;
    assert_eq!(c.next_of("steer.result").await["outcome"], "refused");
    c.send("turn.send_now", json!({"turn_id": t2})).await;
    assert_eq!(c.next_of("send_now.result").await["outcome"], "moved");
    assert_eq!(c.next_of("turn.result").await["status"], "done");
}

#[tokio::test]
async fn reconnect_replays_without_duplicates() {
    let (gw, _) = stub_gateway().await;
    let (r, mut c) = start(gw).await;
    c.home_sync().await;
    c.deliver(
        TURN,
        "[[say: a]] [[sleep: 400ms]] [[say: b]] [[say: c]]",
        "auc_t",
    )
    .await;
    c.next_of("turn.event").await; // n=1, then the sleep begins
    let (last_in, my_seq) = (c.last_in, c.seq);
    drop(c); // abrupt loss; the supervisor keeps producing frames meanwhile

    // The runner acked everything it saw: resume_from = last_in. Frames b, c, result are replayed
    // (or sent fresh) exactly once, contiguously after `last_in`.
    let mut c2 = r.accept(last_in, my_seq, last_in).await;
    assert!(c2.hello["hello"].get("bootstrap_token").is_none());
    assert_eq!(
        c2.hello["hello"]["session"]["boot_id"],
        "00000000-0000-0000-0000-000000000002"
    );
    let mut ns = vec![1u64];
    loop {
        let v = c2.next().await; // asserts seq == last_in + 1
        if v["kind"] == "turn.event" {
            ns.push(v["body"]["n"].as_u64().unwrap());
        }
        if v["kind"] == "turn.result" {
            assert_eq!(v["body"]["status"], "done");
            break;
        }
    }
    assert_eq!(ns, [1, 2, 3]);
}

#[tokio::test]
async fn replays_frames_unacked_by_runner() {
    let (gw, _) = stub_gateway().await;
    let (r, mut c) = start(gw).await;
    c.home_sync().await;
    let before = c.last_in;
    drop(c);
    // runner claims it only saw up to `before - 1` of the supervisor's frames: the last one is replayed
    let mut c2 = r.accept(before - 1, 2, before - 1).await;
    let v = c2.next().await;
    assert_eq!(v["seq"].as_u64(), Some(before));
    // and the runner->supervisor direction works over the new connection (seq continues)
    c2.deliver(TURN, "hi", "auc_t").await;
    assert_eq!(c2.next_of("turn.result").await["status"], "done");
}

#[tokio::test]
async fn spent_bootstrap_is_fatal() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/v1/supervisor", listener.local_addr().unwrap());
    let chan = channel::spawn(ChannelConfig::new(url, SecretString::new("aup_x")));
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    ws.next().await;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    ws.close(Some(CloseFrame {
        code: 4409.into(),
        reason: "spent".into(),
    }))
    .await
    .unwrap();
    let r = timeout(T, chan.task).await.unwrap().unwrap();
    assert!(matches!(r, Err(channel::ChannelError::Rejected(4409))));
}

fn marker(dir: &std::path::Path, session_ref: &str) -> std::path::PathBuf {
    dir.join(".fake-sessions").join(session_ref)
}

#[tokio::test]
async fn session_purge_deletes_the_stored_session_and_reports() {
    let (gw, _) = stub_gateway().await;
    let (r, mut c) = start(gw).await;
    c.home_sync().await;
    c.deliver(TURN, "hello", "auc_t").await;
    c.next_of("turn.result").await;
    let sref = format!("fake-{CONV}");
    assert!(
        marker(&r.dir, &sref).exists(),
        "the fake harness stored a session"
    );

    // an unsafe reference is refused and nothing outside the session store is touched
    c.send(
        "session.purge",
        json!({"conversation_id": CONV, "session_ref": "../../etc/passwd"}),
    )
    .await;
    let bad = c.next_of("session.purged").await;
    assert_eq!(bad["ok"], false);
    assert!(marker(&r.dir, &sref).exists());

    c.send(
        "session.purge",
        json!({"conversation_id": CONV, "session_ref": sref}),
    )
    .await;
    let done = c.next_of("session.purged").await;
    assert_eq!(
        (done["ok"].as_bool(), done["conversation_id"].as_str()),
        (Some(true), Some(CONV))
    );
    assert_eq!(done["session_ref"], sref);
    assert!(!marker(&r.dir, &sref).exists(), "session files deleted");
    // idempotent
    c.send(
        "session.purge",
        json!({"conversation_id": CONV, "session_ref": sref}),
    )
    .await;
    assert_eq!(c.next_of("session.purged").await["ok"], true);
}

#[tokio::test]
async fn purges_in_the_home_sync_run_before_the_sandbox_accepts_a_turn() {
    let (gw, _) = stub_gateway().await;
    let (r, mut c) = start(gw).await;
    // A parked sandbox still holds the files of a conversation deleted while it was stopped.
    let sref = "fake-deleted";
    std::fs::create_dir_all(r.dir.join(".fake-sessions")).unwrap();
    std::fs::write(marker(&r.dir, sref), b"old").unwrap();
    c.send(
        "home.sync",
        json!({"generation": 1, "part_index": 0, "part_count": 3, "part": {"kind": "render_inputs",
            "entries": [{"purges": [{"conversation_id": CONV, "session_ref": sref}]}]}}),
    )
    .await;
    c.send(
        "home.sync",
        json!({"generation": 1, "part_index": 1, "part_count": 3, "part": {"kind": "manifest", "entries": []}}),
    )
    .await;
    c.send(
        "home.sync",
        json!({"generation": 1, "part_index": 2, "part_count": 3, "part": {"kind": "final", "entries": []}}),
    )
    .await;
    // purged is reported before the sync is acknowledged (so before any turn can be dispatched)
    let purged = c.next().await;
    assert_eq!(purged["kind"], "session.purged", "{purged}");
    assert_eq!(
        (
            purged["body"]["ok"].as_bool(),
            purged["body"]["session_ref"].as_str()
        ),
        (Some(true), Some(sref))
    );
    assert!(!marker(&r.dir, sref).exists());
    c.next_of("home.synced").await;
    c.deliver(TURN, "hello", "auc_t").await;
    assert_eq!(c.next_of("turn.result").await["status"], "done");
}

//! RUN-002: control-channel server against a wiremock API and a tungstenite client.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use aurora_protocol::runner::frame_report_id;
use aurora_runner::api::{ApiClient, RetryPolicy};
use aurora_runner::channel::{ChannelServer, ChannelSettings};
use aurora_runner::hub::{ChannelHub, Frame, HubEvent};
use base64::Engine;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

const KEY: [u8; 32] = [7; 32];

struct Rig {
    hub: ChannelServer,
    addr: std::net::SocketAddr,
    events: mpsc::Receiver<HubEvent>,
    _stop: watch::Sender<bool>,
}

async fn rig(api: &MockServer) -> Rig {
    rig_with(api, ChannelSettings::new(KEY)).await
}

async fn rig_with(api: &MockServer, settings: ChannelSettings) -> Rig {
    let hub = ChannelServer::new(settings);
    let retry = RetryPolicy {
        attempts: 1,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(10),
    };
    hub.attach_api(Arc::new(
        ApiClient::new(&api.uri(), "aur_test", retry).unwrap(),
    ));
    let events = hub.take_events().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = watch::channel(false);
    let h = hub.clone();
    tokio::spawn(async move { h.serve(listener, stop_rx).await });
    Rig {
        hub,
        addr,
        events,
        _stop: stop_tx,
    }
}

async fn mount_hold(api: &MockServer) {
    for p in ["/v1/runner/channel/hold", "/v1/runner/channel/release"] {
        Mock::given(method("POST"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(api)
            .await;
    }
}

async fn connect(addr: std::net::SocketAddr, hello: Value) -> Client {
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/supervisor"))
        .await
        .unwrap();
    ws.send(Message::text(hello.to_string())).await.unwrap();
    ws
}

/// Next JSON text message, or `{"close": code}` when the server closes.
async fn next(ws: &mut Client) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out")
        {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                if v["kind"] != "ack" {
                    return v;
                }
            }
            Some(Ok(Message::Close(c))) => {
                return json!({"close": c.map_or(0, |c| u16::from(c.code))});
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return json!({"close": "dropped"}),
        }
    }
}

fn bootstrap_hello() -> Value {
    json!({"hello": {"protocol": 1, "bootstrap_token": "aup_secret"}})
}

fn frame(seq: u64, kind: &str, body: Value) -> Message {
    Message::text(json!({"seq": seq, "ack": 0, "kind": kind, "body": body}).to_string())
}

async fn mount_claim(api: &MockServer, sandbox: Uuid, boot: Uuid) {
    Mock::given(method("POST"))
        .and(path("/v1/runner/channel/claim"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sandbox_id": sandbox, "boot_id": boot})),
        )
        .up_to_n_times(1)
        .mount(api)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/channel/claim"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error": "bootstrap_spent"})))
        .mount(api)
        .await;
}

fn session_hello(welcome: &Value) -> Value {
    json!({"hello": {"protocol": 1, "session": welcome["welcome"]["session"].clone()}})
}

async fn wait_for<F: Fn() -> bool>(f: F) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached");
}

#[tokio::test]
async fn bootstrap_admits_once_then_4409() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    let r = rig(&api).await;

    let mut ws = connect(r.addr, bootstrap_hello()).await;
    let w = next(&mut ws).await;
    assert_eq!(w["welcome"]["resume_from"], 0);
    assert_eq!(w["welcome"]["session"]["sandbox_id"], sandbox.to_string());
    assert_eq!(w["welcome"]["session"]["boot_id"], boot.to_string());
    wait_for(|| r.hub.is_connected(sandbox)).await;

    let mut again = connect(r.addr, bootstrap_hello()).await;
    assert_eq!(next(&mut again).await, json!({"close": 4409}));

    let mut bad = connect(
        r.addr,
        json!({"hello": {"protocol": 1, "session": {
            "sandbox_id": sandbox, "boot_id": boot, "issued_at": 1, "mac": "00"}}}),
    )
    .await;
    assert_eq!(next(&mut bad).await, json!({"close": 4401}));
}

#[tokio::test]
async fn session_reconnect_is_accepted_by_another_replica() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    let a = rig(&api).await;
    let b = rig(&api).await;

    let mut ws = connect(a.addr, bootstrap_hello()).await;
    let w = next(&mut ws).await;
    let mut ws2 = connect(b.addr, session_hello(&w)).await;
    let w2 = next(&mut ws2).await;
    assert_eq!(w2["welcome"]["session"]["boot_id"], boot.to_string());
    wait_for(|| b.hub.is_connected(sandbox)).await;
}

#[tokio::test]
async fn replay_after_reconnect_is_applied_once_with_stable_report_ids() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    let mut a = rig(&api).await;

    let turn = Uuid::new_v4();
    let mut ws = connect(a.addr, bootstrap_hello()).await;
    let w = next(&mut ws).await;
    ws.send(frame(1, "turn.started", json!({"turn_id": turn})))
        .await
        .unwrap();
    ws.send(frame(2, "turn.progress", json!({"turn_id": turn})))
        .await
        .unwrap();
    let e1 = a.events.recv().await.unwrap();
    let e2 = a.events.recv().await.unwrap();
    assert_eq!((e1.seq, e2.seq), (1, 2));
    drop(ws);

    // Reconnect to the same runner: resume_from reports what was applied; replays are ignored.
    let mut ws = connect(a.addr, session_hello(&w)).await;
    let w2 = next(&mut ws).await;
    assert_eq!(w2["welcome"]["resume_from"], 2);
    ws.send(frame(2, "turn.progress", json!({"turn_id": turn})))
        .await
        .unwrap();
    ws.send(frame(3, "turn.progress", json!({"turn_id": turn})))
        .await
        .unwrap();
    let e3 = a.events.recv().await.unwrap();
    assert_eq!(e3.seq, 3);
    assert!(a.events.try_recv().is_err());

    // A second replica without state sees the replay; the report id is the same.
    let mut b = rig(&api).await;
    let mut wsb = connect(b.addr, session_hello(&w2)).await;
    let wb = next(&mut wsb).await;
    assert_eq!(wb["welcome"]["resume_from"], 0);
    wsb.send(frame(3, "turn.progress", json!({"turn_id": turn})))
        .await
        .unwrap();
    // A stateless replica adopts the replayed seq; the report id is the same as before.
    let eb = b.events.recv().await.unwrap();
    assert_eq!((eb.boot_id, eb.seq), (e3.boot_id, e3.seq));
    assert_eq!(
        frame_report_id(eb.boot_id, eb.seq),
        frame_report_id(boot, 3)
    );
    assert!(b.events.try_recv().is_err());
}

#[tokio::test]
async fn identity_comes_from_the_channel_not_the_frame() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/tool-calls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"call_id": "c1", "ok": true, "content": [{"type": "text", "text": "hi"}]}),
        ))
        .mount(&api)
        .await;
    let mut r = rig(&api).await;
    let mut ws = connect(r.addr, bootstrap_hello()).await;
    next(&mut ws).await;

    let (fake_s, fake_b) = (Uuid::new_v4(), Uuid::new_v4());
    ws.send(frame(
        1,
        "turn.started",
        json!({"turn_id": Uuid::new_v4(), "sandbox_id": fake_s, "boot_id": fake_b}),
    ))
    .await
    .unwrap();
    let ev = r.events.recv().await.unwrap();
    assert_eq!((ev.sandbox_id, ev.boot_id), (sandbox, boot));

    // Tool call round trip; the API sees the channel identity.
    ws.send(frame(
        2,
        "tool.call",
        json!({"call_id": "c1", "conversation_id": Uuid::new_v4(), "tool": "x", "args": {},
               "sandbox_id": fake_s, "boot_id": fake_b}),
    ))
    .await
    .unwrap();
    let res = loop {
        let m = next(&mut ws).await;
        if m["kind"] == "tool.result" {
            break m;
        }
    };
    assert_eq!(res["body"]["call_id"], "c1");
    assert_eq!(res["body"]["ok"], true);
    assert_eq!(res["seq"], 1);
    let reqs = api.received_requests().await.unwrap();
    let call = reqs
        .iter()
        .find(|q| q.url.path() == "/v1/runner/tool-calls")
        .unwrap();
    let body: Value = serde_json::from_slice(&call.body).unwrap();
    assert_eq!(body["sandbox_id"], sandbox.to_string());
    assert_eq!(body["boot_id"], boot.to_string());
}

#[tokio::test]
async fn oversize_frame_is_rejected() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    let mut r = rig(&api).await;
    let mut ws = connect(r.addr, bootstrap_hello()).await;
    next(&mut ws).await;
    let big = "a".repeat(aurora_protocol::supervisor::MAX_FRAME + 1);
    let _ = ws
        .send(frame(1, "turn.started", json!({ "pad": big })))
        .await;
    let m = next(&mut ws).await;
    assert!(m.get("close").is_some(), "expected close, got {m}");
    assert!(r.events.try_recv().is_err());
}

#[tokio::test]
async fn memory_write_and_file_upload_round_trip() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    let attachment = Uuid::new_v4();
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/memory-writes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"revision": 4})))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/attachments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"attachment_id": attachment})),
        )
        .mount(&api)
        .await;
    let r = rig(&api).await;
    let mut ws = connect(r.addr, bootstrap_hello()).await;
    next(&mut ws).await;

    ws.send(frame(
        1,
        "memory.write",
        json!({"key": "k", "content": "c"}),
    ))
    .await
    .unwrap();
    let m = next(&mut ws).await;
    assert_eq!(m["kind"], "memory.write.result");
    assert_eq!(m["body"]["revision"], 4);

    let data = b"hello world".to_vec();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    ws.send(frame(
        2,
        "file.part",
        json!({"upload_id": "u1", "index": 0, "data_b64": b64}),
    ))
    .await
    .unwrap();
    ws.send(frame(
        3,
        "file.commit",
        json!({"upload_id": "u1", "conversation_id": Uuid::new_v4(), "turn_id": Uuid::new_v4(),
               "filename": "a.txt", "sha256": hex::encode(Sha256::digest(&data)), "size": data.len()}),
    ))
    .await
    .unwrap();
    let m = next(&mut ws).await;
    assert_eq!(m["kind"], "file.result");
    assert_eq!(m["body"]["ok"], true);
    assert_eq!(m["body"]["attachment_id"], attachment.to_string());

    // Corrupt checksum is refused without touching the API.
    ws.send(frame(
        4,
        "file.commit",
        json!({"upload_id": "u2", "conversation_id": Uuid::new_v4(), "turn_id": Uuid::new_v4(),
               "filename": "b", "sha256": "0".repeat(64), "size": 0}),
    ))
    .await
    .unwrap();
    let m = next(&mut ws).await;
    assert_eq!(m["body"]["ok"], false);
}

#[tokio::test]
async fn hub_send_reaches_supervisor_and_replays_after_reconnect() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    let r = rig(&api).await;
    let mut ws = connect(r.addr, bootstrap_hello()).await;
    let w = next(&mut ws).await;
    wait_for(|| r.hub.is_connected(sandbox)).await;
    r.hub
        .send(
            sandbox,
            Frame {
                kind: "turn.abort".into(),
                body: json!({"turn_id": Uuid::new_v4()}),
            },
        )
        .await
        .unwrap();
    let m = next(&mut ws).await;
    assert_eq!(
        (m["kind"].as_str(), m["seq"].as_u64()),
        (Some("turn.abort"), Some(1))
    );
    drop(ws);
    wait_for(|| !r.hub.is_connected(sandbox)).await;
    assert!(
        r.hub
            .send(
                sandbox,
                Frame {
                    kind: "turn.abort".into(),
                    body: json!({})
                }
            )
            .await
            .is_err()
    );
    // Unacked frame 1 is replayed on reconnect.
    let mut ws = connect(r.addr, session_hello(&w)).await;
    next(&mut ws).await;
    let m = next(&mut ws).await;
    assert_eq!(m["seq"], 1);
}

#[tokio::test]
async fn fast_reconnect_is_never_followed_by_a_stale_release() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    mount_hold(&api).await;
    let r = rig(&api).await;
    let mut ws = connect(r.addr, bootstrap_hello()).await;
    let w = next(&mut ws).await;
    drop(ws);
    let mut ws2 = connect(r.addr, session_hello(&w)).await;
    next(&mut ws2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let paths: Vec<String> = api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|q| q.url.path().to_owned())
        .filter(|p| p.ends_with("/hold") || p.ends_with("/release"))
        .collect();
    let last_hold = paths.iter().rposition(|p| p.ends_with("/hold")).unwrap();
    assert!(paths[..].len() >= 2, "{paths:?}");
    assert!(
        paths[last_hold..].iter().all(|p| p.ends_with("/hold")),
        "release after the newest hold: {paths:?}"
    );
    assert!(r.hub.is_connected(sandbox));
}

/// FIX-094 (LIFE-4): the claim succeeds and `channel/hold` then fails with a retryable error
/// (503): the supervisor is admitted with a welcome (the hold is retried in the background), not
/// sent away with 1013 to present a token the API now regards as spent.
#[tokio::test]
async fn bootstrap_claim_then_hold_503_still_admits_with_a_welcome() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_claim(&api, sandbox, boot).await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/channel/hold"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&api)
        .await;
    let mut settings = ChannelSettings::new(KEY);
    settings.hold_retry = Duration::from_secs(3600); // no background retry within the test
    let r = rig_with(&api, settings).await;
    let mut ws = connect(r.addr, bootstrap_hello()).await;
    let w = next(&mut ws).await;
    assert_eq!(
        w["welcome"]["session"]["sandbox_id"],
        sandbox.to_string(),
        "welcome, not a 1013 close: {w}"
    );
    wait_for(|| r.hub.is_connected(sandbox)).await;
}

/// FIX-094 (RUN-1/2): the runner claims **once** per connection. When the claim's response is
/// lost (a 503 after the API committed), the connection closes 1013 and the supervisor's
/// re-presentation is answered again (the API's idempotence while no channel was held) and
/// admitted; the claim was never retried inside one connection.
#[tokio::test]
async fn a_claim_whose_response_was_lost_is_answered_again_on_the_next_connection() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    Mock::given(method("POST"))
        .and(path("/v1/runner/channel/claim"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/channel/claim"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sandbox_id": sandbox, "boot_id": boot})),
        )
        .mount(&api)
        .await;
    mount_hold(&api).await;
    // A runner that retries (as the pre-fix `call` did, five attempts) would swallow the 503.
    let hub = ChannelServer::new(ChannelSettings::new(KEY));
    hub.attach_api(Arc::new(
        ApiClient::new(
            &api.uri(),
            "aur_test",
            RetryPolicy {
                attempts: 5,
                initial: Duration::from_millis(5),
                max: Duration::from_millis(10),
            },
        )
        .unwrap(),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (_stop, stop_rx) = watch::channel(false);
    let h = hub.clone();
    tokio::spawn(async move { h.serve(listener, stop_rx).await });

    let mut first = connect(addr, bootstrap_hello()).await;
    assert_eq!(next(&mut first).await, json!({"close": 1013}));
    let claims = |reqs: Vec<wiremock::Request>| {
        reqs.iter()
            .filter(|q| q.url.path().ends_with("/channel/claim"))
            .count()
    };
    assert_eq!(
        claims(api.received_requests().await.unwrap()),
        1,
        "one claim attempt per connection"
    );
    let mut second = connect(addr, bootstrap_hello()).await;
    let w = next(&mut second).await;
    assert_eq!(w["welcome"]["session"]["boot_id"], boot.to_string(), "{w}");
}

/// FIX-094 (RUN-3): silent TCP connections to the supervisor port are capped (here 8; 256 in
/// production): the ones beyond the cap are closed at once, the rest at the handshake deadline.
#[tokio::test]
async fn silent_connections_to_the_channel_port_are_capped_and_time_out() {
    use tokio::io::AsyncReadExt as _;
    let api = MockServer::start().await;
    let mut settings = ChannelSettings::new(KEY);
    settings.preauth_cap = 8;
    settings.hello_timeout = Duration::from_secs(2);
    let r = rig_with(&api, settings).await;
    let mut socks = Vec::new();
    for _ in 0..12 {
        socks.push(TcpStream::connect(r.addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut early = 0;
    for s in &mut socks {
        let mut b = [0u8; 1];
        if let Ok(Ok(0)) = tokio::time::timeout(Duration::from_millis(20), s.read(&mut b)).await {
            early += 1;
        }
    }
    assert_eq!(early, 4, "exactly the connections beyond the cap");
    for s in &mut socks {
        let mut b = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(6), s.read(&mut b))
            .await
            .expect("a silent connection outlived the handshake deadline");
        assert!(matches!(n, Ok(0) | Err(_)));
    }
}

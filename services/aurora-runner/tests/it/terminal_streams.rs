//! (P4-TERM, `16 §W19` leg 1) The runner's stream listener serves `kind: "terminal"` grants:
//! the API's dial (`Bearer auy_…`) opens a PTY on the supervisor channel (tag `0x02` beside
//! the port-stream `0x01`), binary frames are PTY bytes in both directions, `pty.resize` is
//! the one text control (`pty_id: 0` names "this leg's PTY" — the browser cannot know the
//! runner's allocation), bytes stamp the keep-awake at the API (C-620), and the close-code
//! table of `16 §W18` holds (eof → 1000, channel loss → 4503). Refusals before the 101 are
//! plain HTTP statuses (the API maps the dial failure to `terminal_upstream`); a dial without
//! a bearer never reaches the grant redemption (401).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use aurora_runner::api::{ApiClient, RetryPolicy};
use aurora_runner::channel::{ChannelServer, ChannelSettings};
use aurora_runner::hub::ChannelHub as _;
use aurora_runner::stream_listener::{CLOSE_CHANNEL_LOST, CLOSE_NORMAL, StreamListener};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{
    Error as WsError, Message, handshake::client::Request as WsRequest,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

const KEY: [u8; 32] = [23; 32];
/// `TERMINAL_STREAM_WINDOW` (the open the supervisor answers with).
const WINDOW: u64 = 131_072;

/// The runner half: channel server + stream listener, both live, the listener open
/// (`set_previews_enabled(true)` is what a registration with `terminal: true` drives —
/// C-623: the listener opens when previews **or** the terminal is on).
async fn rig(
    api: &MockServer,
) -> (
    std::net::SocketAddr,
    std::net::SocketAddr,
    ChannelServer,
    watch::Sender<bool>,
    watch::Sender<bool>,
) {
    let server = ChannelServer::new(ChannelSettings::new(KEY));
    let retry = RetryPolicy {
        attempts: 1,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(10),
    };
    server.attach_api(Arc::new(
        ApiClient::new(&api.uri(), "aur_test", retry).unwrap(),
    ));
    let streams = StreamListener::new(server.clone());
    server.set_streams(streams.clone());

    let ch = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ch_addr = ch.local_addr().unwrap();
    let (stop_tx, stop_rx) = watch::channel(false);
    let s = server.clone();
    tokio::spawn(async move { s.serve(ch, stop_rx).await });

    let st = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let st_addr = st.local_addr().unwrap();
    let (stop_tx2, stop_rx2) = watch::channel(false);
    let l = streams.clone();
    let api_w = server.api_watch();
    tokio::spawn(async move {
        aurora_runner::stream_listener::serve(st, l, api_w, None, stop_rx2).await
    });

    server.set_previews_enabled(true);
    (ch_addr, st_addr, server, stop_tx, stop_tx2)
}

/// The fake supervisor: the W6 client a boot runs. Admits its channel (the claim mock names
/// the session) and then answers the frames the listener sends (leg 2 under test).
async fn connect_supervisor(addr: std::net::SocketAddr) -> Client {
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/supervisor"))
        .await
        .unwrap();
    ws.send(Message::text(
        json!({"hello": {"protocol": 1, "bootstrap_token": "aup_secret"}}).to_string(),
    ))
    .await
    .unwrap();
    // Welcome — drain frames until it arrives (the pty.open comes after).
    let _ = next_text(&mut ws).await;
    ws
}

/// Next text frame (skips pings and acks).
async fn next_text(ws: &mut Client) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("frame timed out")
        {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(&t).unwrap();
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

/// The API mocks one boot needs: claim (welcome), hold, the terminal-shape redemption
/// (`kind: "terminal"` answer — no `port`, no `share_id`, C-617) and the keep-awake stamp.
async fn mount_runner_api(api: &MockServer, sandbox: Uuid, boot: Uuid) {
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
        .and(path("/v1/runner/channel/hold"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(api)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/preview-streams/open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sandbox_id": sandbox, "boot_id": boot, "window": WINDOW,
            "idle_secs": 600, "max_lifetime_secs": 14400
        })))
        .mount(api)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/terminal/keep-awake"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(api)
        .await;
}

/// The API's dial: `GET /v1/preview-stream` with the single-use `auy_` grant as the bearer
/// (C-617). Returned as a task so the test can interleave the supervisor's answer; the
/// handshake error (an HTTP refusal) is kept for the refusal tests.
fn dial(st_addr: std::net::SocketAddr) -> tokio::task::JoinHandle<Result<Client, WsError>> {
    tokio::spawn(async move {
        let mut req: WsRequest = format!("ws://{st_addr}/v1/preview-stream")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert(AUTHORIZATION, "Bearer auy_testgrant".parse().unwrap());
        match tokio::time::timeout(Duration::from_secs(10), connect_async(req)).await {
            Ok(Ok((ws, _))) => Ok(ws),
            Ok(Err(e)) => Err(e),
            Err(_) => panic!("dial timed out (101 never completed)"),
        }
    })
}

/// The leg's end: skips pings/pongs and buffered data, returns the close frame's code.
async fn close_code(ws: &mut Client) -> u16 {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("close timed out")
            .unwrap()
            .unwrap();
        match &m {
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(c) => return c.as_ref().map_or(0, |c| u16::from(c.code)),
            _ => {}
        }
    }
}

/// Terminal leg end to end (C-617/C-619/C-620): the dial is answered 101 only after the
/// supervisor's `pty.opened`; browser bytes arrive on the channel framed
/// `[0x02][pty_id BE][payload]`; `pty.resize` with `pty_id: 0` is stamped with the leg's
/// real id; PTY output reaches the browser as plain binary; bytes stamp the keep-awake at
/// the API; the supervisor's `pty.close {eof}` closes the browser's socket 1000.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_leg_round_trip_tags_0x02_and_stamps_awake() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch_addr, st_addr, _server, _stop, _stop2) = rig(&api).await;
    let mut sup = connect_supervisor(ch_addr).await;

    let term = dial(st_addr);

    // The listener asks the supervisor for a PTY (`pty.open`, seeded 80×24, terminal window).
    let opened = next_text(&mut sup).await;
    assert_eq!(opened["kind"], "pty.open", "{opened}");
    let pty_id = opened["body"]["pty_id"].as_u64().unwrap() as u32;
    assert_eq!(opened["body"]["cols"], 80);
    assert_eq!(opened["body"]["rows"], 24);
    assert_eq!(opened["body"]["window"], WINDOW);
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "pty.opened",
               "body": {"pty_id": pty_id, "window": WINDOW}})
        .to_string(),
    ))
    .await
    .unwrap();

    let mut term = term.await.unwrap().unwrap();

    // Browser → PTY: binary keystrokes arrive framed `[0x02][pty_id BE][payload]`.
    term.send(Message::binary(b"ls\r".to_vec())).await.unwrap();
    let d = tokio::time::timeout(Duration::from_secs(5), sup.next())
        .await
        .expect("data timed out")
        .unwrap()
        .unwrap();
    let Message::Binary(b) = d else {
        panic!("expected binary data, got {d:?}");
    };
    assert_eq!(&b[0], &0x02, "terminal data is tag 0x02, {b:?}");
    assert_eq!(&b[1..5], &pty_id.to_be_bytes());
    assert_eq!(&b[5..], b"ls\r");

    // The one text control: `pty.resize` with `pty_id: 0` — the browser cannot know the
    // allocation; the runner stamps its own id (C-619).
    term.send(Message::text(
        json!({"kind": "pty.resize", "pty_id": 0, "cols": 120, "rows": 40}).to_string(),
    ))
    .await
    .unwrap();
    let rz = tokio::time::timeout(Duration::from_secs(5), sup.next())
        .await
        .expect("resize timed out")
        .unwrap()
        .unwrap();
    let Message::Text(t) = rz else {
        panic!("expected the resize control, got {rz:?}");
    };
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v["kind"], "pty.resize", "{v}");
    assert_eq!(
        v["body"]["pty_id"],
        u64::from(pty_id),
        "the leg's PTY is stamped"
    );
    assert_eq!(v["body"]["cols"], 120);
    assert_eq!(v["body"]["rows"], 40);

    // PTY → browser: output arrives as plain binary (the tag lives on the W6 leg only).
    let mut out = vec![0x02];
    out.extend_from_slice(&pty_id.to_be_bytes());
    out.extend_from_slice(b"total 0");
    sup.send(Message::binary(out)).await.unwrap();
    let back = loop {
        let m = tokio::time::timeout(Duration::from_secs(5), term.next())
            .await
            .expect("output timed out")
            .unwrap()
            .unwrap();
        // Pings (and pongs) ride the leg while we wait; only binary is PTY data.
        if matches!(m, Message::Binary(_)) {
            break m;
        }
    };
    assert_eq!(back, Message::binary(b"total 0".to_vec()), "{back:?}");

    // Bytes flowed both ways: the keep-awake stamp hit the API (C-620; rate-limited to once
    // per grace, so at least one POST — poll briefly, the stamp is spawned).
    let mut stamped = false;
    for _ in 0..20 {
        let reqs = api.received_requests().await.unwrap_or_default();
        if reqs
            .iter()
            .any(|r| r.url.path().ends_with("/runner/terminal/keep-awake"))
        {
            stamped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(stamped, "terminal bytes must stamp terminal_active_until");

    // The supervisor ends the PTY (`pty.close {eof}`): the browser sees a normal 1000.
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "pty.close",
               "body": {"pty_id": pty_id, "reason": "eof",
                         "bytes_in": 3u64, "bytes_out": 7u64}})
        .to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(close_code(&mut term).await, CLOSE_NORMAL);
}

/// A `pty.refused {limit}` before the 101 is a plain HTTP refusal (the supervisor's
/// `TERMINALS_PER_SANDBOX` gate): the dial never upgrades, and the route is released so the
/// channel stays usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_pty_refused_before_upgrade_is_an_http_refusal() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch_addr, st_addr, _server, _stop, _stop2) = rig(&api).await;
    let mut sup = connect_supervisor(ch_addr).await;

    let term = dial(st_addr);
    let opened = next_text(&mut sup).await;
    assert_eq!(opened["kind"], "pty.open", "{opened}");
    let pty_id = opened["body"]["pty_id"].as_u64().unwrap() as u32;
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "pty.refused",
               "body": {"pty_id": pty_id, "reason": "limit"}})
        .to_string(),
    ))
    .await
    .unwrap();

    // No 101: the client sees the HTTP status (the pre-upgrade refusal form).
    let err = tokio::time::timeout(Duration::from_secs(5), term)
        .await
        .expect("dial timed out")
        .unwrap();
    let WsError::Http(resp) = err.expect_err("dial should fail, not upgrade") else {
        panic!("expected an HTTP refusal");
    };
    assert_eq!(resp.status().as_u16(), 503, "refused pre-101 maps to 503");
}

/// The W6 channel drops mid-session (W6 reconnect): every PTY of that channel ends with
/// `4503` — nothing is resumed (`16 §W19` on `16 §W18`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_leg_channel_loss_closes_4503() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch_addr, st_addr, _server, _stop, _stop2) = rig(&api).await;
    let mut sup = connect_supervisor(ch_addr).await;

    let term = dial(st_addr);
    let opened = next_text(&mut sup).await;
    assert_eq!(opened["kind"], "pty.open", "{opened}");
    let pty_id = opened["body"]["pty_id"].as_u64().unwrap() as u32;
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "pty.opened",
               "body": {"pty_id": pty_id, "window": WINDOW}})
        .to_string(),
    ))
    .await
    .unwrap();
    let mut term = term.await.unwrap().unwrap();

    // The supervisor's channel goes away: the leg closes `4503`.
    sup.close(None).await.unwrap();
    assert_eq!(close_code(&mut term).await, CLOSE_CHANNEL_LOST);
}

/// A dial without a bearer is refused before any grant redemption (401).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_dial_without_bearer_is_refused_401() {
    let api = MockServer::start().await;
    let (_ch_addr, st_addr, _server, _stop, _stop2) = rig(&api).await;

    let req: WsRequest = format!("ws://{st_addr}/v1/preview-stream")
        .into_client_request()
        .unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), connect_async(req))
        .await
        .expect("dial timed out");
    let WsError::Http(resp) = r.expect_err("no bearer, no upgrade") else {
        panic!("expected an HTTP refusal");
    };
    assert_eq!(resp.status().as_u16(), 401);
}

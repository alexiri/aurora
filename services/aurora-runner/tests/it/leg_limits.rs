//! (FIX-073, `docs/18 §Web terminal`) Idle and lifetime closes of a terminal leg and the
//! "older supervisor ignores `pty.*`" case. The runner owns the leg limits
//! (`stream_listener.rs`: `IDLE_TIMEOUT`, `LIFETIME`, both `4504`), so they are proven here
//! with shortened values handed to `StreamListener::with_limits` — the very code path the
//! production listener runs. The supervisor side of `docs/18` ("idle/lifetime closes") is
//! therefore not a supervisor behaviour: `pty.rs` has no timers, and `docs/18` says so now.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use aurora_runner::api::{ApiClient, RetryPolicy};
use aurora_runner::channel::{ChannelServer, ChannelSettings};
use aurora_runner::hub::ChannelHub as _;
use aurora_runner::stream_listener::{CLOSE_TIMEOUT, StreamListener};
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
const WINDOW: u64 = 131_072;
const HOUR: Duration = Duration::from_secs(3600);

struct Rig {
    ch_addr: std::net::SocketAddr,
    st_addr: std::net::SocketAddr,
    _server: ChannelServer,
    _stop: watch::Sender<bool>,
    _stop2: watch::Sender<bool>,
}

/// The runner half with leg limits `idle` / `lifetime`.
async fn rig(api: &MockServer, idle: Duration, lifetime: Duration) -> Rig {
    let server = ChannelServer::new(ChannelSettings::new(KEY));
    let retry = RetryPolicy {
        attempts: 1,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(10),
    };
    server.attach_api(Arc::new(
        ApiClient::new(&api.uri(), "aur_test", retry).unwrap(),
    ));
    let streams = StreamListener::with_limits(server.clone(), idle, lifetime);
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
    Rig {
        ch_addr,
        st_addr,
        _server: server,
        _stop: stop_tx,
        _stop2: stop_tx2,
    }
}

async fn connect_supervisor(addr: std::net::SocketAddr) -> Client {
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/supervisor"))
        .await
        .unwrap();
    ws.send(Message::text(
        json!({"hello": {"protocol": 1, "bootstrap_token": "aup_secret"}}).to_string(),
    ))
    .await
    .unwrap();
    let _ = next_text(&mut ws).await;
    ws
}

/// Next text frame (skips pings, acks and binary frames); bounded.
async fn next_text(ws: &mut Client) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next())
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

fn dial(st_addr: std::net::SocketAddr) -> tokio::task::JoinHandle<Result<Client, WsError>> {
    tokio::spawn(async move {
        let mut req: WsRequest = format!("ws://{st_addr}/v1/preview-stream")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert(AUTHORIZATION, "Bearer auy_testgrant".parse().unwrap());
        match tokio::time::timeout(Duration::from_secs(15), connect_async(req)).await {
            Ok(Ok((ws, _))) => Ok(ws),
            Ok(Err(e)) => Err(e),
            Err(_) => panic!("dial timed out (101 never completed)"),
        }
    })
}

/// Opens a terminal leg: dial, the supervisor answers `pty.opened`, returns both ends.
async fn open_terminal(r: &Rig) -> (Client, Client, u32) {
    let mut sup = connect_supervisor(r.ch_addr).await;
    let term = dial(r.st_addr);
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
    (term.await.unwrap().unwrap(), sup, pty_id)
}

/// The leg's end: skips pings/pongs and data, returns the close frame's code (bounded).
async fn close_code(ws: &mut Client) -> u16 {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("the leg was never closed")
            .unwrap()
            .unwrap();
        if let Message::Close(c) = &m {
            return c.as_ref().map_or(0, |c| u16::from(c.code));
        }
    }
}

/// A terminal leg with no traffic closes `4504` at the idle limit (600 s in production,
/// 600 ms here) and the supervisor is told to close the PTY.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_leg_closes_4504_when_idle() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let r = rig(&api, Duration::from_millis(600), HOUR).await;
    let (mut term, mut sup, pty_id) = open_terminal(&r).await;
    let t = Instant::now();
    assert_eq!(close_code(&mut term).await, CLOSE_TIMEOUT);
    let idle = t.elapsed();
    assert!(
        idle >= Duration::from_millis(450),
        "closed before the idle limit: {idle:?}"
    );
    // The runner releases the PTY at the supervisor: `pty.close` for this id.
    let close = next_text(&mut sup).await;
    assert_eq!(close["kind"], "pty.close", "{close}");
    assert_eq!(close["body"]["pty_id"], u64::from(pty_id), "{close}");
}

/// A busy terminal leg is still closed `4504` at its lifetime (4 h in production, 1.2 s here):
/// keystrokes keep resetting the idle timer, the lifetime timer never resets.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_leg_closes_4504_at_its_lifetime() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let r = rig(
        &api,
        Duration::from_millis(500),
        Duration::from_millis(1200),
    )
    .await;
    let (mut term, mut sup, _pty) = open_terminal(&r).await;
    let t = Instant::now();
    let mut code = None;
    while t.elapsed() < Duration::from_secs(10) {
        if term.send(Message::binary(b"x".to_vec())).await.is_err() {
            break;
        }
        // Drain the supervisor side so the channel never backs up.
        while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_millis(5), sup.next()).await
        {
        }
        match tokio::time::timeout(Duration::from_millis(150), term.next()).await {
            Ok(Some(Ok(Message::Close(c)))) => {
                code = Some(c.map_or(0, |c| u16::from(c.code)));
                break;
            }
            Ok(None | Some(Err(_))) => break,
            _ => {}
        }
    }
    let lived = t.elapsed();
    assert_eq!(code, Some(CLOSE_TIMEOUT), "closed after {lived:?}");
    assert!(
        lived >= Duration::from_millis(1000),
        "the lifetime was cut short by the idle limit: {lived:?}"
    );
}

/// An older supervisor does not know `pty.*` and never answers `pty.open` (its channel
/// ignores the unknown kind harmlessly): the runner gives up after `TERMINAL_PTY_OPEN_TIMEOUT`,
/// the dial is refused with a plain HTTP status before any 101, and the route is released so
/// the channel stays usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn older_supervisor_ignoring_pty_open_ends_in_a_clean_refusal() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let r = rig(&api, HOUR, HOUR).await;
    let mut sup = connect_supervisor(r.ch_addr).await;
    let term = dial(r.st_addr);
    let opened = next_text(&mut sup).await;
    assert_eq!(opened["kind"], "pty.open", "{opened}");
    // ... and the "supervisor" says nothing at all.
    let err = term.await.unwrap().expect_err("no pty.opened, no upgrade");
    let WsError::Http(resp) = err else {
        panic!("expected an HTTP refusal, got {err:?}");
    };
    assert!(
        matches!(resp.status().as_u16(), 502..=504),
        "pre-101 refusal status: {}",
        resp.status()
    );
    // The channel is still usable: a second dial reaches the supervisor again.
    let term2 = dial(r.st_addr);
    // The runner tells the supervisor to drop the abandoned PTY (`pty.close`), then asks again.
    let mut released = false;
    let again = loop {
        let f = next_text(&mut sup).await;
        match f["kind"].as_str() {
            Some("pty.close") => released = true,
            Some("pty.open") => break f,
            _ => panic!("unexpected frame {f}"),
        }
    };
    assert!(released, "the abandoned PTY was released at the supervisor");
    let pty_id = again["body"]["pty_id"].as_u64().unwrap();
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "pty.refused",
               "body": {"pty_id": pty_id, "reason": "limit"}})
        .to_string(),
    ))
    .await
    .unwrap();
    assert!(term2.await.unwrap().is_err());
}

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
use aurora_runner::stream_listener::{Bounds, CLOSE_TIMEOUT, StreamListener};
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
    rig_with(
        api,
        idle,
        lifetime,
        ChannelSettings::new(KEY),
        Bounds::default(),
    )
    .await
}

/// [`rig`] with explicit channel settings and listener abuse bounds (FIX-092).
async fn rig_with(
    api: &MockServer,
    idle: Duration,
    lifetime: Duration,
    settings: ChannelSettings,
    bounds: Bounds,
) -> Rig {
    let server = ChannelServer::new(settings);
    let retry = RetryPolicy {
        attempts: 1,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(10),
    };
    server.attach_api(Arc::new(
        ApiClient::new(&api.uri(), "aur_test", retry).unwrap(),
    ));
    let streams = StreamListener::with_bounds(server.clone(), idle, lifetime, bounds);
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

/// Drains the supervisor side for `window` and returns every `pty.resize` it received.
async fn collect_resizes(sup: &mut Client, window: Duration) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let end = tokio::time::Instant::now() + window;
    while let Ok(Some(Ok(m))) = tokio::time::timeout_at(end, sup.next()).await {
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["kind"] == "pty.resize" {
                out.push((
                    v["body"]["cols"].as_u64().unwrap(),
                    v["body"]["rows"].as_u64().unwrap(),
                ));
            }
        }
    }
    out
}

/// FIX-092 (STRM-5): the browser's `pty.resize` messages are coalesced to the latest size —
/// 100,000 of them reach the supervisor as a handful of frames, the last one being the final
/// size. Before the fix every message was a queued frame on the runner's outbound queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resize_flood_is_coalesced_to_the_latest_size() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let r = rig(&api, HOUR, HOUR).await;
    let (mut term, mut sup, _pty) = open_terminal(&r).await;
    for i in 0..100_000u32 {
        let cols = if i == 99_999 { 999 } else { 20 + i % 100 };
        term.feed(Message::text(
            json!({"kind": "pty.resize", "pty_id": 0, "cols": cols, "rows": 40}).to_string(),
        ))
        .await
        .unwrap();
    }
    term.flush().await.unwrap();
    let got = collect_resizes(&mut sup, Duration::from_secs(3)).await;
    assert!(
        got.len() < 1_000,
        "100,000 resizes must be coalesced, the supervisor got {}",
        got.len()
    );
    assert_eq!(got.last(), Some(&(999, 40)), "the latest size is delivered");
}

/// FIX-092 (STRM-5): the runner's per-sandbox live-leg cap (here 1 PTY) answers 429 before any
/// supervisor traffic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legs_beyond_the_per_sandbox_cap_are_refused_429() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let r = rig_with(
        &api,
        HOUR,
        HOUR,
        ChannelSettings::new(KEY),
        Bounds {
            max_ptys: 1,
            ..Bounds::default()
        },
    )
    .await;
    let (_term, _sup, _pty) = open_terminal(&r).await;
    let err = dial(r.st_addr).await.unwrap().expect_err("over the cap");
    let WsError::Http(resp) = err else {
        panic!("expected an HTTP refusal, got {err:?}");
    };
    assert_eq!(resp.status().as_u16(), 429);
}

/// FIX-092 (STRM-6): unauthenticated connections are capped (32 here, 256 in production):
/// the sockets beyond the cap are closed at once, the others at the head deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_sockets_are_capped_and_closed_at_the_head_deadline() {
    use tokio::io::AsyncReadExt as _;
    let api = MockServer::start().await;
    let r = rig_with(
        &api,
        HOUR,
        HOUR,
        ChannelSettings::new(KEY),
        Bounds {
            pre_auth_cap: 32,
            head_deadline: Duration::from_secs(2),
            ..Bounds::default()
        },
    )
    .await;
    let mut socks = Vec::new();
    for _ in 0..40 {
        socks.push(TcpStream::connect(r.st_addr).await.unwrap());
    }
    // The refused ones see EOF well before the head deadline.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut closed_early = 0;
    for s in &mut socks {
        let mut b = [0u8; 1];
        if let Ok(Ok(0)) = tokio::time::timeout(Duration::from_millis(20), s.read(&mut b)).await {
            closed_early += 1;
        }
    }
    assert_eq!(closed_early, 8, "exactly the sockets beyond the cap");
    // The rest are closed at the deadline (2 s; the bound below is generous).
    for s in &mut socks {
        let mut b = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(6), s.read(&mut b))
            .await
            .expect("a silent socket outlived the head deadline");
        assert!(matches!(n, Ok(0) | Err(_)));
    }
}

/// FIX-092 (STRM-6): a request head that trickles in one byte at a time is cut at the total
/// deadline, not held by a per-read timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_loris_head_is_closed_at_the_total_deadline() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let api = MockServer::start().await;
    let r = rig_with(
        &api,
        HOUR,
        HOUR,
        ChannelSettings::new(KEY),
        Bounds {
            head_deadline: Duration::from_secs(1),
            ..Bounds::default()
        },
    )
    .await;
    let mut s = TcpStream::connect(r.st_addr).await.unwrap();
    let t = Instant::now();
    let mut closed = false;
    while t.elapsed() < Duration::from_secs(6) {
        if s.write_all(b"G").await.is_err() {
            closed = true;
            break;
        }
        let mut b = [0u8; 1];
        if let Ok(Ok(0) | Err(_)) =
            tokio::time::timeout(Duration::from_millis(200), s.read(&mut b)).await
        {
            closed = true;
            break;
        }
    }
    assert!(closed, "the trickling socket was never closed");
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
}

/// FIX-092 (CONV-5, C-617): a runner that does not hold the sandbox's channel answers 404 with
/// the standard error body `not_here`, not a bare status line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_here_carries_the_standard_error_body() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let r = rig(&api, HOUR, HOUR).await; // no supervisor ever connects
    let err = dial(r.st_addr).await.unwrap().expect_err("not here");
    let WsError::Http(resp) = err else {
        panic!("expected an HTTP refusal, got {err:?}");
    };
    assert_eq!(resp.status().as_u16(), 404);
    let body: Value = serde_json::from_slice(resp.body().as_deref().unwrap_or_default()).unwrap();
    assert_eq!(body["error"]["code"], "not_here", "{body}");
}

/// FIX-092 (STRM-7): `Pong`s are liveness. A leg that receives only pongs stays open past the
/// idle limit (600 ms here); with nothing at all it closes `4504`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pongs_keep_a_terminal_leg_open_and_silence_closes_it() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let r = rig(&api, Duration::from_millis(600), HOUR).await;
    let (mut term, _sup, _pty) = open_terminal(&r).await;
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(2000) {
        term.send(Message::Pong(Vec::new().into())).await.unwrap();
        // A close in this window would be the idle rule ignoring the pongs.
        if let Ok(Some(Ok(Message::Close(c)))) =
            tokio::time::timeout(Duration::from_millis(150), term.next()).await
        {
            panic!("closed while pongs were flowing: {c:?}");
        }
    }
    assert_eq!(close_code(&mut term).await, CLOSE_TIMEOUT);
}

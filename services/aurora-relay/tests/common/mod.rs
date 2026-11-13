//! Shared rig of the relay's behaviour tests (FIX-073): a loopback app with the routes the
//! `docs/18` cases need, a fake runner (leg 1), a fake API with a revocation switch, and the
//! relay edge itself on shortened `Timeouts`. Everything binds `127.0.0.1:0`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::body::Frame;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

pub use aurora_relay::limits::Timeouts;

pub const LABEL: &str = "paaaaaaaaaaaaaaaaaaaaaaaa";
pub const COOKIE: &str = "__Host-aurora_preview=auv_one";
pub const HOST: &str = "paaaaaaaaaaaaaaaaaaaaaaaa.previews.example.org";
pub const SSE_GAP: Duration = Duration::from_millis(800);
pub const GIB: u64 = 1 << 30;

pub fn share() -> Uuid {
    Uuid::from_slice(&LABEL.as_bytes()[1..17]).unwrap()
}

type AppBody = UnsyncBoxBody<Bytes, Infallible>;

fn full(status: u16, body: &'static str) -> hyper::Response<AppBody> {
    hyper::Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from_static(body.as_bytes())).boxed_unsync())
        .unwrap()
}

/// A body made of the frames `f` yields, with a declared length when `len` is set.
fn streamed(
    len: Option<u64>,
    content_type: &str,
    s: impl futures::Stream<Item = Bytes> + Send + 'static,
) -> hyper::Response<AppBody> {
    let mut b = hyper::Response::builder()
        .status(200)
        .header("content-type", content_type);
    if let Some(n) = len {
        b = b.header("content-length", n);
    }
    b.body(StreamBody::new(s.map(|d| Ok::<_, Infallible>(Frame::data(d)))).boxed_unsync())
        .unwrap()
}

async fn app_route(
    mut req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<AppBody>, Infallible> {
    let path = req.uri().path().to_owned();
    let is_ws = req
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    if path == "/ws" && is_ws {
        let key = req
            .headers()
            .get("sec-websocket-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        let up = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            let Ok(up) = up.await else { return };
            let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
                hyper_util::rt::TokioIo::new(up),
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            let (mut sink, mut source) = ws.split();
            while let Some(Ok(msg)) = source.next().await {
                match msg {
                    Message::Text(_) | Message::Binary(_) => {
                        if sink.send(msg).await.is_err() {
                            return;
                        }
                    }
                    Message::Close(_) => {
                        let _ = sink.send(Message::Close(None)).await;
                        return;
                    }
                    _ => {}
                }
            }
        });
        return Ok(hyper::Response::builder()
            .status(101)
            .header("upgrade", "websocket")
            .header("connection", "Upgrade")
            .header("sec-websocket-accept", accept)
            .body(Full::new(Bytes::new()).boxed_unsync())
            .unwrap());
    }
    Ok(match path.as_str() {
        "/echo" => full(200, "echo"),
        // What the app saw of the request's cookies and connection handling (FIX-088).
        "/who" => {
            let h = |n: &str| {
                req.headers()
                    .get(n)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("none")
                    .to_owned()
            };
            let body = format!(
                "cookie={} connection={} xrealip={} via={}",
                h("cookie"),
                h("connection"),
                h("x-real-ip"),
                h("via")
            );
            hyper::Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .body(Full::new(Bytes::from(body)).boxed_unsync())
                .unwrap()
        }
        // Counts the request body's bytes (a chunked upload has no length up front).
        "/upload" => {
            let n = req
                .into_body()
                .collect()
                .await
                .map_or(0, |b| b.to_bytes().len());
            hyper::Response::builder()
                .status(200)
                .body(Full::new(Bytes::from(format!("got={n}"))).boxed_unsync())
                .unwrap()
        }
        // Headers the relay must not pass to the browser (SEC-85).
        "/hdrs" => hyper::Response::builder()
            .status(200)
            .header("clear-site-data", "\"cache\"")
            .header("alt-svc", "h3=\":443\"")
            .header("service-worker-allowed", "/")
            .header("strict-transport-security", "max-age=1")
            .header("content-security-policy", "default-src 'self'")
            .body(Full::new(Bytes::from_static(b"h")).boxed_unsync())
            .unwrap(),
        // Range requests pass through untouched: the app answers 206 itself.
        "/range" => match req.headers().get("range").and_then(|v| v.to_str().ok()) {
            Some("bytes=0-3") => hyper::Response::builder()
                .status(206)
                .header("content-range", "bytes 0-3/10")
                .header("accept-ranges", "bytes")
                .body(Full::new(Bytes::from_static(b"0123")).boxed_unsync())
                .unwrap(),
            _ => hyper::Response::builder()
                .status(200)
                .header("accept-ranges", "bytes")
                .body(Full::new(Bytes::from_static(b"0123456789")).boxed_unsync())
                .unwrap(),
        },
        // Server-sent events: `chunked`, no content-length, one event per gap.
        "/sse" => streamed(
            None,
            "text/event-stream",
            futures::stream::unfold(0u8, |i| async move {
                if i >= 3 {
                    return None;
                }
                if i > 0 {
                    tokio::time::sleep(SSE_GAP).await;
                }
                let ev = ["data: one\n\n", "data: two\n\n", "data: three\n\n"][i as usize];
                Some((Bytes::from_static(ev.as_bytes()), i + 1))
            }),
        ),
        // A head and five bytes of a hundred, then silence (idle timeout).
        "/stall" => streamed(
            Some(100),
            "text/plain",
            futures::stream::once(async { Bytes::from_static(b"stall") })
                .chain(futures::stream::pending()),
        ),
        // One GiB in 64 KiB pieces.
        "/gig" => streamed(
            Some(GIB),
            "application/octet-stream",
            futures::stream::iter(0..(GIB / 65_536)).map(|_| Bytes::from(vec![0x5au8; 65_536])),
        ),
        _ => full(404, "nope"),
    })
}

pub async fn fake_app(listener: TcpListener) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    hyper_util::rt::TokioIo::new(tcp),
                    hyper::service::service_fn(app_route),
                )
                .with_upgrades()
                .await;
        });
    }
}

/// Serves leg 1 and pipes each stream to the loopback app.
pub async fn fake_runner(listener: TcpListener, app_port: u16) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let (mut sink, mut source) = ws.split();
            let Ok(tcp) = TcpStream::connect(("127.0.0.1", app_port)).await else {
                let _ = sink.send(Message::Close(None)).await;
                return;
            };
            let (mut app_r, mut app_w) = tcp.into_split();
            let to_app = async move {
                while let Some(Ok(msg)) = source.next().await {
                    match msg {
                        Message::Binary(b) => {
                            if app_w.write_all(&b).await.is_err() {
                                return;
                            }
                        }
                        Message::Close(_) => return,
                        _ => {}
                    }
                }
            };
            let to_ws = async move {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match app_r.read(&mut buf).await {
                        Ok(0) | Err(_) => {
                            let _ = sink.send(Message::Close(None)).await;
                            return;
                        }
                        Ok(n) => {
                            if sink.send(Message::binary(buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            };
            tokio::join!(to_app, to_ws);
        });
    }
}

/// What the fake API answers.
#[derive(Default)]
pub struct Script {
    /// Session hashes `validate` reports as revoked.
    pub revoked: Mutex<HashSet<String>>,
    /// While true `streams` answers 409 `not_running` (a parked or booting sandbox).
    pub not_running: std::sync::atomic::AtomicBool,
    /// The `route` the next `resolve` answers carry (a wake or start scenario).
    pub route: Mutex<Option<serde_json::Value>>,
    /// A `resolve` that long-polls (`wait_secs > 0`) makes the sandbox run: it flips
    /// `not_running` off and answers a listening route (the app came up in time).
    pub up_after_wait: std::sync::atomic::AtomicBool,
    /// How many long-polling `resolve` calls the API saw, and the `wait_secs` of the last.
    pub waits: std::sync::atomic::AtomicU32,
    pub last_wait: std::sync::atomic::AtomicU32,
    /// When set, every `resolve` answers this body instead (a `deny`, say).
    pub resolve_override: Mutex<Option<serde_json::Value>>,
    /// The status `grants/redeem` answers (0 = 404, the default of the fake).
    pub redeem_status: std::sync::atomic::AtomicU16,
}

pub async fn fake_api(listener: TcpListener, runner_endpoint: String, script: Arc<Script>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let runner_endpoint = runner_endpoint.clone();
        let script = script.clone();
        tokio::spawn(async move {
            let svc =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let runner_endpoint = runner_endpoint.clone();
                    let script = script.clone();
                    async move {
                        let path = req.uri().path().to_owned();
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        let json: serde_json::Value =
                            serde_json::from_slice(&body).unwrap_or_default();
                        let reply = |v: serde_json::Value| {
                            hyper::Response::builder()
                                .status(200)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(v.to_string())))
                                .unwrap()
                        };
                        let resp = match path.as_str() {
                            "/v1/relay/register" => reply(serde_json::json!({
                                "replica_id": "00000000-0000-0000-0000-000000000001",
                                "protocol": 1,
                                "config": {
                                    "previews_domain": "previews.example.org",
                                    "external_url": "https://aurora.example.org",
                                    "auth_cache_secs": 15,
                                    "wake_wait_secs": 45,
                                }
                            })),
                            "/v1/relay/grants/redeem" => {
                                let st = script
                                    .redeem_status
                                    .load(std::sync::atomic::Ordering::SeqCst);
                                hyper::Response::builder()
                                    .status(if st == 0 { 404 } else { st })
                                    .body(Full::new(Bytes::new()))
                                    .unwrap()
                            }
                            "/v1/relay/resolve" => {
                                use std::sync::atomic::Ordering::SeqCst;
                                if let Some(v) = script.resolve_override.lock().unwrap().clone() {
                                    return Ok::<_, Infallible>(reply(v));
                                }
                                let waited = json["wait_secs"].as_u64().unwrap_or(0) as u32;
                                let mut route = script.route.lock().unwrap().clone();
                                if waited > 0 {
                                    script.waits.fetch_add(1, SeqCst);
                                    script.last_wait.store(waited, SeqCst);
                                    if script.up_after_wait.load(SeqCst) {
                                        script.not_running.store(false, SeqCst);
                                        route = Some(serde_json::json!({
                                            "sandbox": "running", "listening": true,
                                            "start": "none"
                                        }));
                                    }
                                }
                                let mut v = serde_json::json!({
                                    "decision": "allow",
                                    "share": { "id": share(), "port": 3000 },
                                    "user": "00000000-0000-0000-0000-0000000000aa",
                                });
                                if let Some(r) = route {
                                    v["route"] = r;
                                }
                                reply(v)
                            }
                            "/v1/relay/streams" => {
                                if script.not_running.load(std::sync::atomic::Ordering::SeqCst) {
                                    hyper::Response::builder()
                                        .status(409)
                                        .body(Full::new(Bytes::new()))
                                        .unwrap()
                                } else {
                                    reply(serde_json::json!({
                                        "grant": "aux_test",
                                        "expires_in": 30,
                                        "runner_endpoint": runner_endpoint,
                                    }))
                                }
                            }
                            "/v1/relay/validate" => {
                                let revoked = script.revoked.lock().unwrap().clone();
                                let out: Vec<_> = json["sessions"]
                                    .as_array()
                                    .cloned()
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter(|e| {
                                        e["session_hash"]
                                            .as_str()
                                            .is_some_and(|h| revoked.contains(h))
                                    })
                                    .collect();
                                reply(serde_json::json!({ "revoked": out }))
                            }
                            "/v1/relay/report" => reply(serde_json::json!({ "ok": true })),
                            _ => hyper::Response::builder()
                                .status(404)
                                .body(Full::new(Bytes::new()))
                                .unwrap(),
                        };
                        Ok::<_, Infallible>(resp)
                    }
                });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
                .await;
        });
    }
}

pub struct Rig {
    pub edge_port: u16,
    pub streams: Arc<aurora_relay::stream::Streams>,
    pub script: Arc<Script>,
    _stop: tokio::sync::watch::Sender<bool>,
}

pub async fn bind_any() -> (TcpListener, u16) {
    let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

/// A raw TCP "app": the test writes exactly the bytes it wants the relay to see (framing
/// violations hyper would never emit).
pub type RawApp = Arc<
    dyn Fn(TcpStream) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// A raw app from an async closure.
pub fn raw_app<F, Fut>(f: F) -> RawApp
where
    F: Fn(TcpStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    Arc::new(move |tcp| Box::pin(f(tcp)))
}

/// Reads one request head from a raw app's socket (up to the blank line).
pub async fn raw_read_head(tcp: &mut TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = tcp.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    raw
}

/// Rig options beyond the timeouts.
#[derive(Clone)]
pub struct Opts {
    pub heartbeat: bool,
    /// `relay.request_body_max_bytes`.
    pub body_max: usize,
    /// Replaces the hyper app (the runner dials it for every stream).
    pub app: Option<RawApp>,
    /// The runner accepts the leg-1 WebSocket and then never reads or writes again.
    pub deaf_runner: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            heartbeat: false,
            body_max: 1 << 20,
            app: None,
            deaf_runner: false,
        }
    }
}

/// A runner that completes the handshake and then holds the socket without reading.
async fn deaf_runner(listener: TcpListener) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let _hold = ws;
            std::future::pending::<()>().await;
        });
    }
}

/// The relay on `timeouts`; `heartbeat` starts the validate + report loop (every
/// `timeouts.validate_every`).
pub async fn rig(timeouts: Timeouts, heartbeat: bool) -> Rig {
    rig_with(
        timeouts,
        Opts {
            heartbeat,
            ..Opts::default()
        },
    )
    .await
}

pub async fn rig_with(timeouts: Timeouts, opts: Opts) -> Rig {
    let heartbeat = opts.heartbeat;
    let (app_l, app_port) = bind_any().await;
    match opts.app {
        Some(app) => {
            tokio::spawn(async move {
                loop {
                    let Ok((tcp, _)) = app_l.accept().await else {
                        return;
                    };
                    tokio::spawn(app(tcp));
                }
            });
        }
        None => {
            tokio::spawn(fake_app(app_l));
        }
    }
    let (run_l, run_port) = bind_any().await;
    if opts.deaf_runner {
        tokio::spawn(deaf_runner(run_l));
    } else {
        tokio::spawn(fake_runner(run_l, app_port));
    }
    let (api_l, api_port) = bind_any().await;
    let script = Arc::new(Script::default());
    tokio::spawn(fake_api(
        api_l,
        format!("ws://127.0.0.1:{run_port}/v1/preview-stream"),
        script.clone(),
    ));
    let (edge_l, edge_port) = bind_any().await;
    let api = Arc::new(
        aurora_relay::api::RelayApi::new(&format!("http://127.0.0.1:{api_port}"), "aul_test")
            .unwrap(),
    );
    let state = aurora_relay::state::State::new(
        api.clone(),
        "previews.example.org".into(),
        "https://aurora.example.org".into(),
        45,
    );
    let streams = aurora_relay::stream::Streams::with_timeouts(32, 64, timeouts);
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    if heartbeat {
        let hooks = aurora_relay::state::ValidateHooks {
            live: {
                let s = streams.clone();
                Arc::new(move || s.live_pairs())
            },
            on_revoked: {
                let s = streams.clone();
                Arc::new(move |r| s.revoke(&r))
            },
            report: {
                let s = streams.clone();
                Arc::new(move || s.drain_report())
            },
            restore: {
                let s = streams.clone();
                Arc::new(move |r| s.restore_report(r))
            },
        };
        tokio::spawn(aurora_relay::state::heartbeat_loop(
            api,
            hooks,
            timeouts.validate_every,
            stop_rx.clone(),
        ));
    }
    tokio::spawn(aurora_relay::edge::serve_listener(
        edge_l,
        state,
        streams.clone(),
        2048,
        opts.body_max,
        stop_rx,
    ));
    Rig {
        edge_port,
        streams,
        script,
        _stop: stop_tx,
    }
}

// ------------------------------------------------------------------ client side

pub async fn connect(r: &Rig) -> TcpStream {
    TcpStream::connect(("127.0.0.1", r.edge_port))
        .await
        .unwrap()
}

/// The text of a GET request head (no terminating blank line is added when `complete` is
/// false: the slow-loris case).
pub fn request(path: &str, extra: &[(&str, &str)], close: bool) -> String {
    let mut s = format!("GET {path} HTTP/1.1\r\nhost: {HOST}\r\ncookie: {COOKIE}\r\n");
    if close {
        s.push_str("connection: close\r\n");
    }
    for (k, v) in extra {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s
}

/// Reads until the blank line ending a head; returns (status, headers, bytes read past it).
pub async fn read_head(tcp: &mut TcpStream) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..i]).into_owned();
            let rest = raw[i + 4..].to_vec();
            let mut lines = head.split("\r\n");
            let status = lines
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .nth(1)
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
            let headers = lines
                .filter_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
                })
                .collect();
            return (status, headers, rest);
        }
        let n = tokio::time::timeout(Duration::from_secs(20), tcp.read(&mut buf))
            .await
            .expect("no response head within 20 s")
            .unwrap();
        assert!(n > 0, "connection closed before a full head: {raw:?}");
        raw.extend_from_slice(&buf[..n]);
    }
}

pub fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Reads exactly `n` body bytes after a head (`rest` is what the head read already took).
pub async fn read_exact_body(tcp: &mut TcpStream, mut rest: Vec<u8>, n: usize) -> Vec<u8> {
    while rest.len() < n {
        let mut buf = [0u8; 4096];
        let k = tokio::time::timeout(Duration::from_secs(20), tcp.read(&mut buf))
            .await
            .expect("body stalled")
            .unwrap();
        assert!(k > 0, "connection closed mid-body");
        rest.extend_from_slice(&buf[..k]);
    }
    rest.truncate(n);
    rest
}

/// Opens a WebSocket through the relay. `Ok(ws)` after the 101, `Err(status)` otherwise.
pub async fn ws_connect(
    r: &Rig,
    origin: Option<&str>,
) -> Result<tokio_tungstenite::WebSocketStream<TcpStream>, (u16, Vec<(String, String)>)> {
    let mut tcp = connect(r).await;
    let mut req = format!(
        "GET /ws HTTP/1.1\r\nhost: {HOST}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
         sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\
         cookie: {COOKIE}\r\n"
    );
    if let Some(o) = origin {
        req.push_str(&format!("origin: {o}\r\n"));
    }
    req.push_str("\r\n");
    tcp.write_all(req.as_bytes()).await.unwrap();
    let (status, headers, rest) = read_head(&mut tcp).await;
    if status != 101 {
        return Err((status, headers));
    }
    assert!(rest.is_empty(), "no frame precedes the first send");
    Ok(tokio_tungstenite::WebSocketStream::from_raw_socket(
        tcp,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await)
}

/// One echo round trip on an open WebSocket (bounded).
pub async fn echo(ws: &mut tokio_tungstenite::WebSocketStream<TcpStream>, text: &str) {
    ws.send(Message::text(text.to_owned())).await.unwrap();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("no echo within 10 s")
            .expect("stream ended")
            .expect("ws error");
        match m {
            Message::Text(t) => {
                assert_eq!(t.as_str(), text);
                return;
            }
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("expected the echo, got {other:?}"),
        }
    }
}

/// Waits (bounded) until the WebSocket ends; returns how long it took.
pub async fn until_closed(
    ws: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    within: Duration,
) -> Duration {
    let t = Instant::now();
    loop {
        match tokio::time::timeout(within, ws.next()).await {
            Err(_) => panic!("the stream was still open after {within:?}"),
            Ok(None | Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return t.elapsed(),
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// Polls until `f` holds (deadline 20 s).
pub async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Resident set size of this process in KiB.
pub fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

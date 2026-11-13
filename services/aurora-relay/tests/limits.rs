//! Relay resource limits end to end (FIX-072, SEC-82, SEC-85; `docs/16 §W18` Limits): the
//! concurrent-stream caps per share and per user (429 / close `4409`, slots freed on every
//! exit), the daily-quota refusal (`preview_quota`, 429 + `Retry-After`), the grant-redemption
//! rate, and the per-share byte report that feeds the shared daily counter.
//!
//! The rig is the one of `edge.rs` (fake API, fake runner, loopback app) with three additions:
//! the app has a `/hold` route that answers only when the test releases it (a stream that
//! stays open), the fake API answers `deny`/`preview_quota` for chosen labels, and the share
//! and user ids it hands out derive from the label and the session hash.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const A: &str = "paaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "pbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "pcccccccccccccccccccccccc";
const D: &str = "pdddddddddddddddddddddddd";
const COOKIE: &str = "__Host-aurora_preview=auv_one";

/// The share id the fake API hands out for `label`.
fn share_of(label: &str) -> Uuid {
    Uuid::from_slice(&label.as_bytes()[1..17]).unwrap()
}

// ------------------------------------------------------------------ fakes

/// Serves leg 1 and pipes each stream to the loopback app.
async fn fake_runner(listener: TcpListener, app_port: u16) {
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
            tokio::select! { () = to_app => {}, () = to_ws => {} }
        });
    }
}

/// The loopback app: `/echo`, `/big` (100 kB) and `/hold` (answers once the test adds a
/// permit to `release`).
async fn fake_app(listener: TcpListener, release: Arc<Semaphore>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let release = release.clone();
        tokio::spawn(async move {
            let svc =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let release = release.clone();
                    async move {
                        let body = match req.uri().path() {
                            "/hold" => {
                                release.acquire().await.unwrap().forget();
                                "released".to_owned()
                            }
                            "/big" => "x".repeat(100_000),
                            _ => "echo".to_owned(),
                        };
                        Ok::<_, std::convert::Infallible>(
                            hyper::Response::builder()
                                .status(200)
                                .header("content-type", "text/plain")
                                .body(http_body_util::Full::new(bytes::Bytes::from(body)))
                                .unwrap(),
                        )
                    }
                });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
                .await;
        });
    }
}

#[derive(Default)]
struct Script {
    /// Labels the API answers `deny` / `preview_quota` for.
    over_quota: Mutex<HashSet<String>>,
    /// `(path)` of every relay-plane call, in order (report bodies are kept apart).
    reports: Mutex<Vec<serde_json::Value>>,
}

async fn fake_api(listener: TcpListener, runner_endpoint: String, script: Arc<Script>) {
    use http_body_util::BodyExt;
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
                                .body(http_body_util::Full::new(bytes::Bytes::from(v.to_string())))
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
                            "/v1/relay/resolve" => {
                                let label = json["label"].as_str().unwrap_or_default().to_owned();
                                if script.over_quota.lock().unwrap().contains(&label) {
                                    reply(serde_json::json!({
                                        "decision": "deny", "reason": "preview_quota"
                                    }))
                                } else {
                                    let hash = json["session_hash"].as_str().unwrap_or_default();
                                    let user = Uuid::from_slice(&hash.as_bytes()[..16]).unwrap();
                                    reply(serde_json::json!({
                                        "decision": "allow",
                                        "share": { "id": share_of(&label), "port": 3000 },
                                        "user": user,
                                    }))
                                }
                            }
                            "/v1/relay/streams" => reply(serde_json::json!({
                                "grant": "aux_test",
                                "expires_in": 30,
                                "runner_endpoint": runner_endpoint,
                            })),
                            "/v1/relay/grants/redeem" => reply(serde_json::json!({
                                "token": "auv_test",
                                "expires_at": "2030-01-01T00:00:00Z",
                                "idle_secs": 3600
                            })),
                            "/v1/relay/report" => {
                                script.reports.lock().unwrap().push(json);
                                reply(serde_json::json!({ "ok": true }))
                            }
                            _ => hyper::Response::builder()
                                .status(404)
                                .body(http_body_util::Full::new(bytes::Bytes::new()))
                                .unwrap(),
                        };
                        Ok::<_, std::convert::Infallible>(resp)
                    }
                });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
                .await;
        });
    }
}

// ------------------------------------------------------------------ rig

struct Rig {
    edge_port: u16,
    streams: Arc<aurora_relay::stream::Streams>,
    release: Arc<Semaphore>,
    script: Arc<Script>,
    _stop: tokio::sync::watch::Sender<bool>,
}

async fn bind_any() -> (TcpListener, u16) {
    let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

async fn rig() -> Rig {
    let release = Arc::new(Semaphore::new(0));
    let (app_l, app_port) = bind_any().await;
    tokio::spawn(fake_app(app_l, release.clone()));
    let (run_l, run_port) = bind_any().await;
    tokio::spawn(fake_runner(run_l, app_port));
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
        api,
        "previews.example.org".into(),
        "https://aurora.example.org".into(),
        45,
    );
    let streams = aurora_relay::stream::Streams::new();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(aurora_relay::edge::serve_listener(
        edge_l,
        state,
        streams.clone(),
        2048,
        1 << 20,
        stop_rx,
    ));
    Rig {
        edge_port,
        streams,
        release,
        script,
        _stop: stop_tx,
    }
}

/// Reads one response head + body (`connection: close`), bounded.
async fn read_response(tcp: &mut TcpStream) -> (u16, String, Vec<(String, String)>) {
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), tcp.read_to_end(&mut raw))
        .await
        .expect("no response within 20 s")
        .ok();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
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
    (status, body.to_owned(), headers)
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Sends a GET without reading the answer (the stream stays open while the app holds it).
async fn send_get(r: &Rig, label: &str, path: &str, cookie: &str) -> TcpStream {
    let mut tcp = TcpStream::connect(("127.0.0.1", r.edge_port))
        .await
        .unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nhost: {label}.previews.example.org\r\nconnection: close\r\n\
         cookie: {cookie}\r\n\r\n"
    );
    tcp.write_all(req.as_bytes()).await.unwrap();
    tcp
}

async fn get(
    r: &Rig,
    label: &str,
    path: &str,
    cookie: &str,
) -> (u16, String, Vec<(String, String)>) {
    let mut tcp = send_get(r, label, path, cookie).await;
    read_response(&mut tcp).await
}

/// Polls until `f` holds (deadline 20 s): the open-stream table settles asynchronously.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Opens `n` held streams on `label` for `cookie` and waits until the relay counts them.
async fn hold(r: &Rig, label: &str, cookie: &str, n: u32, expect_total: u32) -> Vec<TcpStream> {
    let mut v = Vec::new();
    for _ in 0..n {
        v.push(send_get(r, label, "/hold", cookie).await);
    }
    let streams = r.streams.clone();
    let share = share_of(label);
    until("held streams are counted", move || {
        streams.share_count(share) >= expect_total
    })
    .await;
    v
}

// ------------------------------------------------------------------ tests

/// The 33rd concurrent stream on one share is refused (429 `preview_streams_limit` with
/// `Retry-After`), another share is unaffected, and the slots free on every exit — a client
/// that goes away and an app that finishes — so a reopen succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thirty_third_stream_on_a_share_is_refused_and_slots_free() {
    let r = rig().await;
    let mut held = hold(&r, A, COOKIE, 32, 32).await;
    let (status, body, headers) = get(&r, A, "/echo", COOKIE).await;
    assert_eq!(status, 429, "body: {body}");
    assert_eq!(body, "preview_streams_limit\n");
    assert!(header(&headers, "retry-after").is_some(), "{headers:?}");
    // Another share of the same user is unaffected.
    let (status, body, _) = get(&r, B, "/echo", COOKIE).await;
    assert_eq!((status, body.as_str()), (200, "echo"));
    // Client abort: dropping one held connection frees its slot.
    drop(held.pop());
    let (streams, share) = (r.streams.clone(), share_of(A));
    until("an aborted client frees its slot", move || {
        streams.share_count(share) == 31
    })
    .await;
    let (status, _, _) = get(&r, A, "/echo", COOKIE).await;
    assert_eq!(status, 200, "the freed slot is reusable");
    // The app finishes the held requests: every slot frees, the share reopens fully.
    // (One more permit than live requests: the aborted client's app handler is still
    // waiting on the semaphore and may take one.)
    r.release.add_permits(32);
    for mut t in held {
        let (status, body, _) = read_response(&mut t).await;
        assert_eq!((status, body.as_str()), (200, "released"));
    }
    let (streams, share) = (r.streams.clone(), share_of(A));
    until("all slots free", move || streams.share_count(share) == 0).await;
    assert_eq!(get(&r, A, "/echo", COOKIE).await.0, 200);
}

/// The 65th concurrent stream of one user (across shares) is refused; another user is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixty_fifth_stream_of_a_user_is_refused() {
    let r = rig().await;
    let mut held = hold(&r, A, COOKIE, 32, 32).await;
    held.extend(hold(&r, B, COOKIE, 32, 32).await);
    // A third share (0 streams of its own): the user cap is what refuses.
    let (status, body, _) = get(&r, C, "/echo", COOKIE).await;
    assert_eq!(status, 429, "body: {body}");
    assert_eq!(body, "preview_streams_limit\n");
    assert_eq!(r.streams.share_count(share_of(C)), 0);
    // A different user (another credential) on the same third share is served.
    let (status, body, _) = get(&r, C, "/echo", "__Host-aurora_preview=auv_two").await;
    assert_eq!((status, body.as_str()), (200, "echo"));
    r.release.add_permits(64);
    drop(held);
}

/// Over the per-share cap a WebSocket upgrade is accepted and closed with `4409`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_over_the_cap_closes_4409() {
    let r = rig().await;
    let _held = hold(&r, A, COOKIE, 32, 32).await;
    let mut tcp = TcpStream::connect(("127.0.0.1", r.edge_port))
        .await
        .unwrap();
    let host = format!("{A}.previews.example.org");
    let req = format!(
        "GET /ws HTTP/1.1\r\nhost: {host}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
         sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\
         origin: https://{host}\r\ncookie: {COOKIE}\r\n\r\n"
    );
    tcp.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(Duration::from_secs(10), tcp.read_exact(&mut byte))
            .await
            .expect("no handshake answer")
            .unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 101"), "{head}");
    assert!(
        head.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "{head}"
    );
    let mut ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
        tcp,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let m = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("no close within 10 s")
        .expect("stream ended without a close")
        .expect("ws error");
    match m {
        Message::Close(Some(f)) => assert_eq!(u16::from(f.code), 4409, "{f:?}"),
        other => panic!("expected close 4409, got {other:?}"),
    }
    r.release.add_permits(32);
}

/// A share the API reports past its daily quota answers 429 `preview_quota` with a
/// `Retry-After` to the next UTC midnight; another share is unaffected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn share_past_its_daily_quota_answers_429_preview_quota() {
    let r = rig().await;
    r.script.over_quota.lock().unwrap().insert(A.to_owned());
    let (status, body, headers) = get(&r, A, "/echo", COOKIE).await;
    assert_eq!(status, 429, "body: {body}");
    assert!(body.contains("preview_quota"), "{body}");
    let secs: u64 = header(&headers, "retry-after")
        .expect("Retry-After")
        .parse()
        .unwrap();
    assert!((1..=86_400).contains(&secs), "{secs}");
    // The Retry-After is the time to the next UTC midnight (give the clock a few seconds).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let want = 86_400 - now % 86_400;
    assert!(
        want.abs_diff(secs) <= 5,
        "retry-after {secs}, midnight in {want}"
    );
    let (status, body, _) = get(&r, B, "/echo", COOKIE).await;
    assert_eq!((status, body.as_str()), (200, "echo"));
}

/// The grant endpoint refuses the 11th redemption of a share within a minute and the 31st
/// from one address (30 per minute), each with `Retry-After`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grant_redemption_rate_per_share_and_per_address() {
    let r = rig().await;
    let redeem = |label: &'static str| {
        let port = r.edge_port;
        async move {
            let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let req = format!(
                "GET /_aurora/grant?g=agt_x HTTP/1.1\r\nhost: {label}.previews.example.org\r\n\
                 connection: close\r\ncookie: __Host-aurora_pstate=n1.Lw\r\n\r\n"
            );
            tcp.write_all(req.as_bytes()).await.unwrap();
            read_response(&mut tcp).await
        }
    };
    for _ in 0..10 {
        assert_eq!(redeem(A).await.0, 302);
    }
    let (status, body, headers) = redeem(A).await;
    assert_eq!(status, 429, "per-share cap: {body}");
    assert_eq!(body, "grant_rate_limited\n");
    let secs: u64 = header(&headers, "retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&secs), "{secs}");
    // 20 more from the same address on two other shares: the address is at 30.
    for label in [B, C] {
        for _ in 0..10 {
            assert_eq!(redeem(label).await.0, 302);
        }
    }
    let (status, body, _) = redeem(D).await;
    assert_eq!(status, 429, "per-address cap: {body}");
}

/// Relayed bytes are counted per share and handed to the heartbeat report (the API adds them
/// to the share's daily counter), in both directions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relayed_bytes_are_reported_per_share() {
    let r = rig().await;
    let (status, body, _) = get(&r, A, "/big", COOKIE).await;
    assert_eq!((status, body.len()), (200, 100_000));
    let mut seen = 0u64;
    let mut sent = 0u64;
    let end = Instant::now() + Duration::from_secs(20);
    while seen < 100_000 {
        assert!(Instant::now() < end, "the report never carried the bytes");
        for e in r.streams.drain_report() {
            assert_eq!(e.share_id, share_of(A), "only share A moved bytes");
            seen += e.bytes_from_app;
            sent += e.bytes_to_app;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The body plus the response head came back; the rebuilt request head went out.
    assert!(sent > 0, "the request head is counted too");
    // A restored report is counted again with the next drain (delivery failed).
    r.streams.restore_report(&[aurora_relay::api::ReportShare {
        share_id: share_of(B),
        streams_open: 0,
        streams_total: 0,
        bytes_to_app: 7,
        bytes_from_app: 9,
        last_request_at: None,
    }]);
    let again = r.streams.drain_report();
    assert_eq!(again.len(), 1);
    assert_eq!((again[0].bytes_to_app, again[0].bytes_from_app), (7, 9));
}

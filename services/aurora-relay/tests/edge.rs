//! Relay end-to-end tests with a fake runner (`docs/18` §Port previews, "Relay"): a loopback
//! HTTP app stands in for the sandbox, the fake runner accepts the leg-1 WebSocket
//! (`/v1/preview-stream`, bearer `aux_`), writes the tunneled bytes to the app and streams
//! the answer back — the same raw-byte framing as `16 §W18` leg 1.
//!
//! The relay's API stub answers `resolve`, `streams` and `grants/redeem` enough for the
//! edge to proxy one request and to exercise the header hygiene rules of SEC-85.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

// ------------------------------------------------------------------ fake runner

/// Serves the leg-1 endpoint and pipes each stream to the given loopback app port.
async fn fake_runner(listener: TcpListener, app_port: u16) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let app = app_port;
        tokio::spawn(async move {
            let ws = tokio_tungstenite::accept_async(tcp).await;
            let Ok(ws) = ws else { return };
            let (mut sink, mut source) = ws.split();
            // One app connection per stream (one request/response each).
            let Ok(tcp) = TcpStream::connect(("127.0.0.1", app)).await else {
                eprintln!("[fake-runner] app dial failed");
                let _ = sink.send(Message::Close(None)).await;
                return;
            };
            let (mut app_r, mut app_w) = tcp.into_split();
            // Browser → app: raw bytes.
            let to_app = async move {
                while let Some(Ok(msg)) = source.next().await {
                    match msg {
                        Message::Binary(b) => {
                            if app_w.write_all(&b).await.is_err() {
                                eprintln!("[fake-runner] app write failed");
                                return;
                            }
                            let _ = app_w.flush().await;
                        }
                        Message::Close(_) => {
                            eprintln!("[fake-runner] ws closed by relay");
                            return;
                        }
                        _ => {}
                    }
                }
            };
            // App → browser: raw bytes.
            let to_ws = async move {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match app_r.read(&mut buf).await {
                        Ok(0) | Err(_) => {
                            eprintln!("[fake-runner] app EOF, closing ws");
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

// ------------------------------------------------------------------ fake API

/// The minimal relay-plane API: `POST /v1/relay/{register,resolve,streams,grants/redeem}`.
async fn fake_api(
    listener: TcpListener,
    runner_endpoint: String,
    hits: Arc<Mutex<HashMap<String, u32>>>,
) {
    use http_body_util::BodyExt;
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hits = hits.clone();
        let runner_endpoint = runner_endpoint.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(
                move |req: hyper::Request<hyper::body::Incoming>| {
                    let hits = hits.clone();
                    let runner_endpoint = runner_endpoint.clone();
                    async move {
                        let path = req.uri().path().to_owned();
                        {
                            let mut h = hits.lock().unwrap();
                            let e = h.entry(path.clone()).or_insert(0);
                            *e += 1;
                        }
                        // Authorization must be present (a Bearer aul_…).
                        let auth = req
                            .headers()
                            .get("authorization")
                            .map(|v| v.to_str().unwrap_or_default().to_owned())
                            .unwrap_or_default();
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        let json: serde_json::Value =
                            serde_json::from_slice(&body).unwrap_or_default();
                        let reply = |status: u16, v: serde_json::Value| {
                            hyper::Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(http_body_util::Full::new(bytes::Bytes::from(v.to_string())))
                                .unwrap()
                        };
                        let resp = match path.as_str() {
                            "/v1/relay/register" => reply(
                                200,
                                serde_json::json!({
                                    "replica_id": "00000000-0000-0000-0000-000000000001",
                                    "protocol": 1,
                                    "config": {
                                        "previews_domain": "previews.example.org",
                                        "external_url": "https://aurora.example.org",
                                        "auth_cache_secs": 15,
                                        "wake_wait_secs": 45,
                                    }
                                }),
                            ),
                            "/v1/relay/resolve" => {
                                if auth.is_empty() {
                                    reply(401, serde_json::json!({"error": "unauthorized"}))
                                } else {
                                    reply(
                                        200,
                                        serde_json::json!({
                                            "decision": "allow",
                                            "share": { "id": "00000000-0000-0000-0000-000000000002", "port": 3000 }
                                        }),
                                    )
                                }
                            }
                            "/v1/relay/streams" => reply(
                                200,
                                serde_json::json!({
                                    "grant": "aux_test",
                                    "expires_in": 30,
                                    "runner_endpoint": runner_endpoint,
                                }),
                            ),
                            "/v1/relay/grants/redeem" => reply(
                                200,
                                serde_json::json!({
                                    "token": "auv_test",
                                    "expires_at": "2030-01-01T00:00:00Z",
                                    "idle_secs": 3600
                                }),
                            ),
                            _ => reply(404, serde_json::json!({ "error": "not_found" })),
                        };
                        let _ = json;
                        Ok::<_, std::convert::Infallible>(resp)
                    }
                },
            );
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
                .await;
        });
    }
}

// ------------------------------------------------------------------ helpers

/// Starts the loopback app: a tiny HTTP server answering fixed responses per path.
async fn fake_app(listener: TcpListener) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(
                |mut req: hyper::Request<hyper::body::Incoming>| async move {
                    // `/ws` is a WebSocket endpoint: complete the upgrade with hyper, then
                    // echo every frame (the relay must carry the handshake and the raw
                    // frames both ways — `16 §W18`).
                    let path = req.uri().path().to_owned();
                    let is_ws = req
                        .headers()
                        .get("upgrade")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
                        && req.headers().get("sec-websocket-key").is_some();
                    if path == "/ws" && is_ws {
                        let key = req
                            .headers()
                            .get("sec-websocket-key")
                            .map(|v| v.to_str().unwrap_or_default().to_owned())
                            .unwrap_or_default();
                        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(
                            key.as_bytes(),
                        );
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
                        let res: hyper::Response<http_body_util::Full<bytes::Bytes>> =
                            hyper::Response::builder()
                                .status(101)
                                .header("upgrade", "websocket")
                                .header("connection", "Upgrade")
                                .header("sec-websocket-accept", accept)
                                .body(http_body_util::Full::new(bytes::Bytes::new()))
                                .unwrap();
                        return Ok::<_, std::convert::Infallible>(res);
                    }
                    Ok(route(req).await)
                },
            );
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
                .with_upgrades()
                .await;
        });
    }
}

/// The plain-HTTP routes of [`fake_app`].
async fn route(
    req: hyper::Request<hyper::body::Incoming>,
) -> hyper::Response<http_body_util::Full<bytes::Bytes>> {
    let p = req.uri().path().to_owned();
    let cookie = req
        .headers()
        .get("cookie")
        .map(|v| v.to_str().unwrap_or_default().to_owned())
        .unwrap_or_default();
    let xff = req
        .headers()
        .get("x-forwarded-for")
        .map(|v| v.to_str().unwrap_or_default().to_owned())
        .unwrap_or_default();
    let host = req
        .headers()
        .get("host")
        .map(|v| v.to_str().unwrap_or_default().to_owned())
        .unwrap_or_default();
    let aurora = req.headers().get_all("x-aurora-replica").iter().count();
    let (status, body) = match p.as_str() {
        "/echo" => (
            200,
            format!(
                "path={} host={} xff={} cookie_set={} aurora={}",
                p,
                host,
                xff,
                !cookie.is_empty(),
                aurora
            ),
        ),
        "/big" => (200, "x".repeat(100_000)),
        "/chunked" => (200, "chunked-body".to_owned()),
        _ => (404, "nope".to_owned()),
    };
    let mut b = hyper::Response::builder()
        .status(status)
        .header("content-type", "text/plain");
    if !cookie.is_empty() {
        b = b.header("set-cookie", "app_side=1; Domain=elsewhere.example; Path=/");
    }
    b.body(http_body_util::Full::new(bytes::Bytes::from(body)))
        .unwrap()
}

async fn bind_any() -> (TcpListener, u16) {
    let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

struct Rig {
    edge_port: u16,
    api_hits: Arc<Mutex<HashMap<String, u32>>>,
    cookie: String,
}

async fn init_log() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

/// Full rig: app + runner + api + relay edge; `cookie` is the preview credential the
/// browser sends (the relay only hashes it; the stub always allows).
async fn rig(cookie: &str) -> Rig {
    init_log().await;
    let (app_l, app_port) = bind_any().await;
    tokio::spawn(fake_app(app_l));
    let (run_l, run_port) = bind_any().await;
    tokio::spawn(fake_runner(run_l, app_port));
    let (api_l, api_port) = bind_any().await;
    let hits = Arc::new(Mutex::new(HashMap::new()));
    tokio::spawn(fake_api(
        api_l,
        format!("ws://127.0.0.1:{run_port}/v1/preview-stream"),
        hits.clone(),
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
    let edge_task = tokio::spawn(aurora_relay::edge::serve_listener(
        edge_l,
        state,
        streams,
        64,
        1 << 20,
        stop_rx,
    ));
    tokio::spawn(async move {
        let _keep_alive = stop_tx; // a named binding: `let _ =` would drop it immediately
        std::future::pending::<()>().await;
    });
    tokio::spawn(async move {
        if let Err(e) = edge_task.await {
            eprintln!("[rig] EDGE TASK DIED: {e}");
        }
    });
    // Wait until the edge listener accepts (all rig tasks come up asynchronously).
    let mut up = false;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", edge_port))
            .await
            .is_ok()
        {
            // The first success may be the kernel backlog before the task polls; confirm
            // with a second connect after a beat.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if tokio::net::TcpStream::connect(("127.0.0.1", edge_port))
                .await
                .is_ok()
            {
                up = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(up, "the edge listener never came up (port {edge_port})");
    eprintln!("[rig] edge on {edge_port}");
    Rig {
        edge_port,
        api_hits: hits,
        cookie: cookie.to_owned(),
    }
}

/// One proxied request through the whole rig: a raw HTTP/1.1 GET over TCP, response
/// parsed by hand. Returns (status, body, response headers).
async fn get(
    r: &Rig,
    label: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> (u16, String, Vec<(String, String)>) {
    // Retry a few times: the rig's listeners come up asynchronously.
    let mut tcp = None;
    let mut last_err = None;
    for _ in 0..50 {
        match TcpStream::connect(("127.0.0.1", r.edge_port)).await {
            Ok(t) => {
                tcp = Some(t);
                break;
            }
            Err(e) => {
                last_err = Some(e.to_string());
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
    let mut tcp =
        tcp.unwrap_or_else(|| panic!("edge listener never came up in get(): {last_err:?}"));
    eprintln!("[get] connected to edge port {}", r.edge_port);
    let host = format!("{label}.previews.example.org");
    let mut req = format!("GET {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n");
    if !r.cookie.is_empty() {
        req.push_str(&format!("cookie: {}\r\n", r.cookie));
    }
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    tcp.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tcp.read_to_end(&mut raw),
    )
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect();
    (status, body.to_owned(), headers)
}

// ------------------------------------------------------------------ tests

/// A plain request round-trips through the whole rig (edge → API grant → leg-1 WS → app).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_trip_reaches_the_app() {
    let r = rig("__Host-aurora_preview=auv_test").await;
    let (status, body, _) = get(&r, "paaaaaaaaaaaaaaaaaaaaaaaa", "/echo", &[]).await;
    assert_eq!(status, 200, "body: {body}");
    assert!(body.contains("path=/echo"), "body: {body}");
}

/// The app sees a clean header set: no cookies, no Aurora headers, XFF added, Host =
/// the runner endpoint's authority; its own Set-Cookie is made host-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_hygiene_both_ways() {
    let r = rig("__Host-aurora_preview=auv_test").await;
    let (status, body, headers) = get(
        &r,
        "paaaaaaaaaaaaaaaaaaaaaaaa",
        "/echo",
        &[
            ("x-aurora-replica", "evil"),
            ("authorization", "Bearer auv_x"),
        ],
    )
    .await;
    assert_eq!(status, 200, "body: {body}");
    assert!(
        body.contains("cookie_set=false"),
        "the app must not see the preview cookie: {body}"
    );
    assert!(
        body.contains("aurora=0"),
        "the app must not see X-Aurora-*: {body}"
    );
    assert!(body.contains("xff="), "XFF must be added: {body}");
    assert!(!body.contains("host=undefined"), "host must be set: {body}");
    // The app's cookie came back rewritten: no Domain, Aurora names dropped.
    let setc = headers
        .iter()
        .filter(|(k, _)| k == "set-cookie")
        .map(|(_, v)| v.clone())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(!setc.contains("Domain="), "Domain must be stripped: {setc}");
}

/// A large response streams through without corruption (and the connection closes).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn big_body_streams() {
    let r = rig("__Host-aurora_preview=auv_test").await;
    let (status, body, _) = get(&r, "paaaaaaaaaaaaaaaaaaaaaaaa", "/big", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body.len(), 100_000);
    assert!(body.bytes().all(|b| b == b'x'));
}

/// Security headers are forced on the way out (cache/robots/HSTS); hop-by-hop is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_security_headers_present() {
    let r = rig("__Host-aurora_preview=auv_test").await;
    let (_, _, headers) = get(&r, "paaaaaaaaaaaaaaaaaaaaaaaa", "/echo", &[]).await;
    let has = |k: &str| headers.iter().any(|(n, _)| n == k);
    assert!(has("strict-transport-security"));
    assert!(has("x-robots-tag"));
    assert!(has("cache-control"));
    assert!(!has("transfer-encoding"));
}

/// No credential → the grant exchange: 302 to the dashboard's authorize with the state
/// cookie (and never to the app).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_redirect_without_cookie() {
    let r = rig("").await;
    let (status, body, headers) = get(&r, "paaaaaaaaaaaaaaaaaaaaaaaa", "/echo", &[]).await;
    assert_eq!(status, 302, "body: {body}");
    let loc = headers
        .iter()
        .find(|(k, _)| k == "location")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        loc.starts_with(
            "https://aurora.example.org/v1/previews/authorize?host=paaaaaaaaaaaaaaaaaaaaaaaa&state="
        ),
        "loc: {loc}"
    );
    let setc = headers
        .iter()
        .find(|(k, _)| k == "set-cookie")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(setc.starts_with("__Host-aurora_pstate="), "setc: {setc}");
    assert!(setc.contains("Secure"), "setc: {setc}");
    assert!(setc.contains("HttpOnly"), "setc: {setc}");
    assert!(setc.contains("SameSite=Lax"), "setc: {setc}");
    assert!(!setc.contains("Domain="), "setc: {setc}");
}

/// Fetch Metadata: cross-site subrequests are refused before anything else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_site_refused() {
    let r = rig("__Host-aurora_preview=auv_test").await;
    let (status, body, _) = get(
        &r,
        "paaaaaaaaaaaaaaaaaaaaaaaa",
        "/echo",
        &[("sec-fetch-site", "cross-site"), ("sec-fetch-mode", "cors")],
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(body, "preview_cross_site\n");
}

/// A WebSocket round trip through the whole relay (`docs/18` §Port previews "Relay":
/// WebSocket echo): the upgrade request crosses the leg with its handshake headers, the
/// app's 101 is answered to the browser (its `Upgrade`/`Connection` restored), and after
/// the switch the two raw byte pipes echo — a masked client text frame comes back as the
/// app's unmasked echo.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_echo_round_trip() {
    let r = rig("__Host-aurora_preview=auv_test").await;
    let label = "paaaaaaaaaaaaaaaaaaaaaaaa";
    let host = format!("{label}.previews.example.org");
    // Retry a few times: the rig's listeners come up asynchronously.
    let mut tcp = None;
    for _ in 0..50 {
        if let Ok(t) = TcpStream::connect(("127.0.0.1", r.edge_port)).await {
            tcp = Some(t);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let mut tcp = tcp.expect("edge listener never came up");
    let req = format!(
        "GET /ws HTTP/1.1\r\nhost: {host}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
         sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\
         origin: https://{host}\r\ncookie: {}\r\n\r\n",
        r.cookie
    );
    tcp.write_all(req.as_bytes()).await.unwrap();
    // The answer head: must be the app's 101 with the handshake headers restored.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        tcp.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head).to_string();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("upgrade: websocket"),
        "{head}"
    );
    assert!(
        head.to_ascii_lowercase()
            .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "{head}"
    );
    // After the switch: a masked client frame, echoed unmasked by the app.
    let mut ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
        tcp,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    ws.send(Message::text("ws-echo")).await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
        .await
        .expect("no echo within 10s")
        .map(|m| m.expect("ws error"));
    // Either the app's echo or (if the bridge tore down first) a reset: only the echo
    // proves the round trip, so it must arrive.
    match reply {
        Some(m) => assert_eq!(m, Message::text("ws-echo"), "{m:?}"),
        None => panic!("stream ended without an echo"),
    }
    let _ = ws.send(Message::Close(None)).await;
    let _ = ws.next().await; // the close answer or the reset, either way
}

/// A non-preview host gets the uniform 404, never a proxy attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_host_uniform_404() {
    let r = rig("__Host-aurora_preview=auv_test").await;
    let (status, body, _) = get(&r, "other", "/echo", &[]).await;
    assert_eq!(status, 404);
    assert!(body.contains("Preview not found"), "body: {body}");
    let hits = r.api_hits.lock().unwrap();
    assert!(
        !hits.contains_key("/v1/relay/resolve"),
        "no resolve for a non-preview host"
    );
}

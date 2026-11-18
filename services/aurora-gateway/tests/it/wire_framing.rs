#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (FIX-166, D-368) A request body the gateway read to its end is forwarded with the framing it
//! arrived with. The fake HTTPS upstream here records the wire: the HTTP version, `:authority`,
//! every header, and how the body was framed — on h2 the raw frame sequence (END_STREAM on
//! HEADERS, DATA frame sizes), on h1 the `Content-Length`/`Transfer-Encoding` headers and the
//! raw body bytes. Shares the `apps.rs` harness (snapshot, policy handler, grants).

use super::*;
use aurora_gateway::ca::Ca;
use rustls::pki_types::ServerName;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// Records every byte read from the wrapped stream.
struct Tee<S> {
    inner: S,
    rec: Arc<Mutex<Vec<u8>>>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Tee<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            let new = buf.filled()[before..].to_vec();
            self.rec.lock().unwrap().extend_from_slice(&new);
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Tee<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// What reached the upstream.
#[derive(Debug, Default, Clone)]
struct Wire {
    version: &'static str,
    authority: String,
    path_and_query: String,
    /// Regular headers, lower-case names.
    headers: Vec<(String, String)>,
    /// h2: END_STREAM was set on the HEADERS frame.
    headers_end_stream: bool,
    /// h2: `(payload length, END_STREAM)` of each DATA frame.
    data_frames: Vec<(usize, bool)>,
    /// h1: the raw bytes after the head (chunk framing included).
    h1_raw_body: Vec<u8>,
}

impl Wire {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    /// The request has no body framing at all.
    fn bodyless(&self) -> bool {
        if self.version == "h2" {
            self.headers_end_stream && self.data_frames.is_empty()
        } else {
            self.get("content-length").is_none_or(|v| v == "0")
                && self.get("transfer-encoding").is_none()
                && self.h1_raw_body.is_empty()
        }
    }
}

/// The h2 frames of the recorded client bytes (after the 24-byte preface).
fn h2_frames(raw: &[u8], w: &mut Wire) {
    let mut i = 24.min(raw.len());
    while i + 9 <= raw.len() {
        let len =
            (usize::from(raw[i]) << 16) | (usize::from(raw[i + 1]) << 8) | usize::from(raw[i + 2]);
        let (ty, flags) = (raw[i + 3], raw[i + 4]);
        match ty {
            0 => w.data_frames.push((len, flags & 1 == 1)),
            1 => w.headers_end_stream = flags & 1 == 1,
            _ => {}
        }
        i += 9 + len;
    }
}

type WireRx = tokio::sync::oneshot::Receiver<Result<Wire, String>>;

/// A raw HTTPS upstream answering the first request `200 ok`. `h2`: offer h2 (and h1);
/// otherwise h1 only.
async fn oracle(cfg: Arc<rustls::ServerConfig>, h2: bool) -> (SocketAddr, WireRx) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut cfg = (*cfg).clone();
    cfg.alpn_protocols = if h2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    let cfg = Arc::new(cfg);
    tokio::spawn(async move {
        let out = tokio::time::timeout(Duration::from_secs(10), async {
            let (s, _) = l.accept().await.map_err(|e| format!("accept: {e}"))?;
            let tls = TlsAcceptor::from(cfg)
                .accept(s)
                .await
                .map_err(|e| format!("tls: {e}"))?;
            let is_h2 = tls.get_ref().1.alpn_protocol() == Some(&b"h2"[..]);
            let rec = Arc::new(Mutex::new(Vec::new()));
            let io = Tee {
                inner: tls,
                rec: rec.clone(),
            };
            if is_h2 {
                serve_h2(io, rec).await
            } else {
                serve_h1(io, rec).await
            }
        })
        .await
        .unwrap_or_else(|_| Err("oracle timed out".to_owned()));
        let _ = tx.send(out);
    });
    (addr, rx)
}

async fn serve_h2<S>(io: S, rec: Arc<Mutex<Vec<u8>>>) -> Result<Wire, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut conn = h2::server::handshake(io)
        .await
        .map_err(|e| format!("h2 handshake: {e}"))?;
    let (req, mut respond) = match conn.accept().await {
        Some(Ok(x)) => x,
        _ => return Err("h2: no request".into()),
    };
    // Something has to poll the connection while the body is read and the answer flushed.
    tokio::spawn(async move { while let Some(Ok(_)) = conn.accept().await {} });
    let (parts, mut body) = req.into_parts();
    let mut w = Wire {
        version: "h2",
        authority: parts
            .uri
            .authority()
            .map(|a| a.to_string())
            .unwrap_or_default(),
        path_and_query: parts
            .uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default(),
        ..Wire::default()
    };
    for (k, v) in &parts.headers {
        w.headers
            .push((k.as_str().to_owned(), v.to_str().unwrap_or("?").to_owned()));
    }
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|e| format!("body: {e}"))?;
        let _ = body.flow_control().release_capacity(chunk.len());
    }
    let resp = Response::builder().status(200).body(()).unwrap();
    let mut send = respond
        .send_response(resp, false)
        .map_err(|e| format!("send_response: {e}"))?;
    let _ = send.send_data(Bytes::from_static(b"ok"), true);
    h2_frames(&rec.lock().unwrap(), &mut w);
    Ok(w)
}

async fn serve_h1<S>(mut io: S, rec: Arc<Mutex<Vec<u8>>>) -> Result<Wire, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = io.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Err("h1: eof in head".into());
        }
        raw.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut w = Wire {
        version: "h1",
        path_and_query: first.split(' ').nth(1).unwrap_or_default().to_owned(),
        ..Wire::default()
    };
    for l in lines.filter(|l| !l.is_empty()) {
        if let Some((k, v)) = l.split_once(':') {
            w.headers
                .push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    w.authority = w.get("host").unwrap_or_default().to_owned();
    let mut body = raw[head_end..].to_vec();
    let chunked = w
        .get("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let cl: usize = w
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // A bodyless request has nothing after the head: read what is framed, nothing more.
    while (chunked && !body.ends_with(b"0\r\n\r\n")) || (!chunked && body.len() < cl) {
        let n = io.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    w.h1_raw_body = body;
    let _ = rec;
    io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        .await
        .map_err(|e| format!("write: {e}"))?;
    let _ = io.flush().await;
    Ok(w)
}

enum Client {
    H1(hyper::client::conn::http1::SendRequest<Full<Bytes>>),
    H2(hyper::client::conn::http2::SendRequest<Full<Bytes>>),
}

/// CONNECT through the gateway, inner TLS against the gateway CA, then an h1 or h2 client.
async fn mitm(proxy: SocketAddr, target: SocketAddr, h2: bool) -> Client {
    let mut s = crate::common::connect(proxy).await;
    let t = target.to_string();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    s.write_all(
        format!("CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert!(s.read(&mut b).await.unwrap() > 0, "eof in CONNECT head");
        head.push(b[0]);
    }
    assert!(
        head.starts_with(b"HTTP/1.1 200"),
        "CONNECT refused: {}",
        String::from_utf8_lossy(&head)
    );
    let alpn: &[&[u8]] = if h2 { &[b"h2"] } else { &[b"http/1.1"] };
    let cfg = crate::common::client_config(&crate::common::trust_pem(proxy), alpn);
    let tls = TlsConnector::from(cfg)
        .connect(ServerName::IpAddress(target.ip().into()), s)
        .await
        .unwrap();
    let io = TokioIo::new(tls);
    if h2 {
        let (s, c) = hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
            .await
            .unwrap();
        tokio::spawn(c);
        Client::H2(s)
    } else {
        let (s, c) = hyper::client::conn::http1::handshake(io).await.unwrap();
        tokio::spawn(c.with_upgrades());
        Client::H1(s)
    }
}

impl Client {
    async fn send(&mut self, req: Request<Full<Bytes>>) -> (http::StatusCode, String) {
        let r = match self {
            Self::H1(s) => s.send_request(req).await.unwrap(),
            Self::H2(s) => s.send_request(req).await.unwrap(),
        };
        let (p, b) = r.into_parts();
        let b = b.collect().await.unwrap().to_bytes();
        (p.status, String::from_utf8_lossy(&b).into_owned())
    }
}

/// The cern-maps 0.1.3 shape (FIX-166): a credential-free app, `block_untooled`, one host entry
/// with query injections on `/arcgis`, one with none on `/api`, a catalogue tool on each.
fn cern_maps_like() -> Value {
    let mut app = app_json(
        "maps",
        json!({"read": {"items": [
            {"id": "geocode", "name": "g", "host": "127.0.0.1", "path": "/arcgis/**", "methods": ["GET", "POST"]},
            {"id": "search", "name": "s", "host": "127.0.0.1", "path": "/api/**", "methods": ["GET", "POST"]}
        ]}, "write": {"items": []}}),
        false,
    );
    app["credentials"] = json!([]);
    app["metadata"] = json!([]);
    app["connect"] = json!([{"id": "anonymous", "kind": "fields", "fields": []}]);
    app["grants"] = json!({"custom_only": false, "block_untooled": true});
    snapshot(
        vec![app],
        vec![
            host_json(
                "maps",
                Some("/arcgis"),
                json!({"inject": [
                    {"query": {"name": "f", "value": "json"}},
                    {"query": {"name": "outSR", "value": "2056"}}
                ]}),
            ),
            host_json("maps", Some("/api"), json!({"inject": []})),
        ],
    )
}

fn granted_all(c: AppConnectionRow) -> impl FnOnce(&World) -> Rows {
    granted(c)
}

/// One gateway with the cern-maps-like package and a fake upstream trusted by it.
struct Rig {
    proxy: SocketAddr,
    cfg: Arc<rustls::ServerConfig>,
    _env: Env,
}

async fn rig(mk: impl FnOnce(&World) -> Rows + 'static) -> Rig {
    let g = ca::generate("oracle").unwrap();
    let up = Ca::from_pem(&g.cert_pem, &g.key_pem).unwrap();
    let cfg = up.server_config("127.0.0.1").unwrap();
    let (env, ..) = start_with_ca(cern_maps_like(), &[("none", "{}")], mk, Some(g.cert_pem)).await;
    Rig {
        proxy: env.proxy,
        cfg,
        _env: env,
    }
}

fn free_grant(w: &World) -> Rows {
    let c = conn(w, "maps", "none", None);
    granted_all(c)(w)
}

impl Rig {
    /// One request through the gateway; returns what the upstream saw.
    async fn wire(
        &self,
        client_h2: bool,
        upstream_h2: bool,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Wire {
        let (addr, rx) = oracle(self.cfg.clone(), upstream_h2).await;
        let mut c = mitm(self.proxy, addr, client_h2).await;
        let mut b = Request::builder()
            .method(method)
            .uri(format!("https://{addr}{path}"));
        if body.is_some() {
            b = b.header("content-type", "text/plain");
        }
        let req = b
            .body(Full::new(Bytes::from(body.unwrap_or("").to_owned())))
            .unwrap();
        let (status, text) = c.send(req).await;
        assert_eq!(status, 200, "{method} {path}: {text}");
        match rx.await {
            Ok(Ok(w)) => w,
            Ok(Err(e)) => panic!("oracle failed: {e}"),
            Err(e) => panic!("oracle dropped: {e}"),
        }
    }
}

fn assert_bodyless(w: &Wire, what: &str) {
    assert!(
        w.bodyless(),
        "{what}: a GET without a body must reach the upstream without body framing: {w:#?}"
    );
    assert!(w.get("transfer-encoding").is_none(), "{what}: {w:#?}");
    assert!(w.get("content-length").is_none(), "{what}: {w:#?}");
}

/// The Part A matrix: an injected and a non-injected entry, from an h1 and an h2 client, to an
/// h2 and an h1 upstream, a GET with no body arrives bodyless at every one.
#[tokio::test]
async fn get_without_body_arrives_bodyless_on_injected_and_plain_entries() {
    let r = rig(free_grant).await;
    for client_h2 in [false, true] {
        for upstream_h2 in [true, false] {
            for (entry, path) in [
                ("injected", "/arcgis/rest/find?SingleLine=31"),
                ("plain", "/api/search?q=31"),
            ] {
                let w = r.wire(client_h2, upstream_h2, "GET", path, None).await;
                let what = format!("{entry} client_h2={client_h2} upstream_h2={upstream_h2}");
                assert_eq!(w.version, if upstream_h2 { "h2" } else { "h1" }, "{what}");
                assert_bodyless(&w, &what);
                if entry == "injected" {
                    let q = w.path_and_query.clone();
                    assert!(
                        q.contains("f=json") && q.contains("outSR=2056"),
                        "{what}: {q}"
                    );
                }
            }
        }
    }
}

/// The grant plus a block rule with a body condition: the gateway reads the body prefix of every
/// request (`buffer_prefix`) before it decides.
fn with_body_condition(w: &World) -> Rows {
    let c = conn(w, "maps", "none", None);
    let mut r = granted_all(c.clone())(w);
    r.ws.rules.push(rule(
        "no-zzz",
        "block",
        json!([{"type": "connection", "id": c.id}]),
        false,
    ));
    r.ws.rules.last_mut().unwrap().conditions =
        json!([{"on": "body", "op": "contains", "value": "zzz"}]);
    r
}

/// A rule with a condition makes the gateway read the body to its end before forwarding: the
/// request must still reach the upstream with the framing it arrived with.
#[tokio::test]
async fn body_read_for_a_condition_keeps_its_framing() {
    let r = rig(with_body_condition).await;
    let mut bad = Vec::new();
    for client_h2 in [false, true] {
        for upstream_h2 in [true, false] {
            for (entry, path) in [
                ("injected", "/arcgis/rest/find?SingleLine=31"),
                ("plain", "/api/search?q=31"),
            ] {
                let w = r.wire(client_h2, upstream_h2, "GET", path, None).await;
                if !w.bodyless() || w.get("content-length").is_some() {
                    bad.push(format!(
                        "GET {entry} client_h2={client_h2} upstream_h2={upstream_h2}: eos_on_headers={} data={:?} te={:?} cl={:?}",
                        w.headers_end_stream,
                        w.data_frames,
                        w.get("transfer-encoding"),
                        w.get("content-length")
                    ));
                }
            }
            let w = r
                .wire(
                    client_h2,
                    upstream_h2,
                    "POST",
                    "/arcgis/rest/solve",
                    Some("hello"),
                )
                .await;
            if w.get("content-length") != Some("5") || w.get("transfer-encoding").is_some() {
                bad.push(format!(
                    "POST client_h2={client_h2} upstream_h2={upstream_h2}: cl={:?} te={:?}",
                    w.get("content-length"),
                    w.get("transfer-encoding")
                ));
            }
        }
    }
    assert!(bad.is_empty(), "re-framed bodies:\n{}", bad.join("\n"));
}

/// A POST with a small known-length body keeps its bytes and arrives with `Content-Length`.
#[tokio::test]
async fn small_post_keeps_content_length_on_injected_entry() {
    let r = rig(free_grant).await;
    for client_h2 in [false, true] {
        for upstream_h2 in [true, false] {
            let w = r
                .wire(
                    client_h2,
                    upstream_h2,
                    "POST",
                    "/arcgis/rest/solve",
                    Some("hello"),
                )
                .await;
            let what = format!("client_h2={client_h2} upstream_h2={upstream_h2}");
            assert_eq!(w.get("content-length"), Some("5"), "{what}: {w:#?}");
            assert!(w.get("transfer-encoding").is_none(), "{what}: {w:#?}");
            if upstream_h2 {
                let total: usize = w.data_frames.iter().map(|(n, _)| n).sum();
                assert_eq!(total, 5, "{what}: {w:#?}");
            } else {
                assert_eq!(w.h1_raw_body, b"hello", "{what}: {w:#?}");
            }
        }
    }
}

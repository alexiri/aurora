#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (TEST-021, tier 1) RFC-conformance oracles for the upstream hop.
//!
//! The ordinary upstreams in `proxy.rs` are hyper servers, which *accept* wire
//! shapes a strict front door (nginx, haproxy) rejects — that is exactly how
//! FIX-052 (`host` beside `:authority` on h2) shipped. These tests therefore
//! talk to **strict, non-hyper oracles**: a raw `h2` server and raw h1 socket
//! servers that report the *complete* set of what reached the wire — an
//! unexpected extra header fails the test, not only a missing one.
//!
//! Everything here is in-process on loopback (no containers): tier 1 runs
//! under the ordinary `cargo test -p aurora-gateway --test it` everywhere,
//! local and CI (VEL-002), within the < 5 s budget.

use aurora_gateway::ca::{self, Ca};
use aurora_gateway::guard::Guard;
use aurora_gateway::pipeline::PassThrough;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::proxy_tls::ProxyTls;
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{TlsAcceptor, TlsConnector};

// ---------------------------------------------------------------------------
// helpers: gateway, client legs, oracles
// ---------------------------------------------------------------------------

struct StaticResolver;

#[async_trait::async_trait]
impl aurora_gateway::auth::CredentialResolver for StaticResolver {
    async fn resolve(
        &self,
        token: &str,
    ) -> Result<Option<aurora_gateway::auth::RequestIdentity>, aurora_gateway::auth::ResolveError>
    {
        if token != "aug_good" {
            return Ok(None);
        }
        let id = uuid::Uuid::now_v7();
        Ok(Some(aurora_gateway::auth::RequestIdentity {
            credential_id: id,
            kind: aurora_gateway::auth::CredentialKind::Agent,
            agent_id: id,
            workspace_id: id,
            org_id: id,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            speaker_user_id: None,
            speaker_group_ids: Vec::new(),
            capabilities: vec![],
            byo: None,
        }))
    }
}

/// A gateway whose `extra_ca_pem` trusts the oracle upstreams; loopback allowed.
async fn gw(extra_ca_pem: &str) -> (SocketAddr, String) {
    let gw = ca::generate("gw-wire").unwrap();
    let gw_ca = Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = l.local_addr().unwrap();
    let allow = vec!["127.0.0.0/8".to_owned()];
    let guard = Arc::new(Guard::new(&allow, vec![proxy]).unwrap());
    let tls = ProxyTls::from_ca_with(
        gw_ca.clone(),
        vec!["127.0.0.1".into(), "localhost".into()],
        Duration::from_secs(3600),
        Duration::from_secs(7200),
    )
    .unwrap();
    crate::common::trust_anchor(proxy, &gw.cert_pem);
    let p = Proxy::new(ProxyParts {
        ca: gw_ca,
        tls: tls.clone(),
        resolver: Arc::new(StaticResolver),
        handler: Arc::new(PassThrough),
        guard,
        header_timeout: Duration::from_secs(10),
        extra_ca_pem: Some(extra_ca_pem.as_bytes().to_vec()),
        egress_proxy: None,
    })
    .unwrap();
    tokio::spawn(p.run(l, std::future::pending()));
    (proxy, gw.cert_pem)
}

fn basic(token: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(token)
    )
}

enum Sender {
    H1(hyper::client::conn::http1::SendRequest<Full<Bytes>>),
    H2(hyper::client::conn::http2::SendRequest<Full<Bytes>>),
}

impl Sender {
    async fn send(&mut self, req: Request<Full<Bytes>>) -> Response<Incoming> {
        match self {
            Self::H1(s) => s.send_request(req).await.unwrap(),
            Self::H2(s) => s.send_request(req).await.unwrap(),
        }
    }
}

/// CONNECT + inner TLS with the gateway CA, then an h1 or h2 hyper client leg.
async fn mitm(gw_ca_pem: &str, proxy: SocketAddr, target: SocketAddr, h2: bool) -> Sender {
    let mut s = crate::common::connect(proxy).await;
    let t = target.to_string();
    s.write_all(
        format!(
            "CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: {}\r\n\r\n",
            basic("agent:aug_good")
        )
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
    let tls = tls_client(gw_ca_pem, alpn)
        .connect(ServerName::IpAddress(target.ip().into()), s)
        .await
        .unwrap();
    let io = TokioIo::new(tls);
    if h2 {
        let (s, c) = hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
            .await
            .unwrap();
        tokio::spawn(c);
        Sender::H2(s)
    } else {
        let (s, c) = hyper::client::conn::http1::handshake(io).await.unwrap();
        tokio::spawn(c.with_upgrades());
        Sender::H1(s)
    }
}

use hyper_util::rt::{TokioExecutor, TokioIo};

fn tls_client(trust_pem: &str, alpn: &[&[u8]]) -> TlsConnector {
    let mut roots = RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(trust_pem.as_bytes()) {
        roots.add(c.unwrap()).unwrap();
    }
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    TlsConnector::from(Arc::new(cfg))
}

async fn text(r: Response<Incoming>) -> (http::StatusCode, http::HeaderMap, String) {
    let (p, b) = r.into_parts();
    let bytes = b.collect().await.unwrap().to_bytes();
    (
        p.status,
        p.headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// An oracle's own CA + TLS server config (SAN `127.0.0.1`, both ALPNs).
fn oracle_tls(name: &str) -> (String, Arc<rustls::ServerConfig>) {
    let g = ca::generate(name).unwrap();
    let up = Ca::from_pem(&g.cert_pem, &g.key_pem).unwrap();
    let cfg = up.server_config("127.0.0.1").unwrap();
    (g.cert_pem, cfg)
}

/// The observed wire, as the oracles report it.
#[derive(Debug, Default, Clone)]
struct Seen {
    method: String,
    path: String,
    authority: String,
    version: String,
    /// Regular headers, lower-case name, in wire order (duplicates kept).
    headers: Vec<(String, String)>,
    body: String,
    /// Request-body chunk sizes as framed (h1 chunks / h2 DATA frames).
    chunks: Vec<usize>,
    /// Trailers, lower-case name.
    trailers: Vec<(String, String)>,
}

impl Seen {
    fn get(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }
    fn count(&self, name: &str) -> usize {
        let n = name.to_ascii_lowercase();
        self.headers.iter().filter(|(k, _)| *k == n).count()
    }
}

/// Unwraps the oracle's report; a panicked/errored oracle fails the test with
/// its message (no panics in spawned tasks).
fn seen_err(r: Result<Seen, String>) -> Seen {
    match r {
        Ok(s) => s,
        Err(e) => panic!("oracle failed: {e}"),
    }
}

/// Asserts the header set is *exactly* `expected` (order-insensitive): an
/// unexpected extra header fails, not only a missing one — the lenient-oracle
/// trap FIX-052 exploited.
fn assert_exact_headers(seen: &Seen, expected: &[(&str, &str)]) {
    let mut got: Vec<(String, String)> = seen.headers.clone();
    got.sort();
    let mut want: Vec<(String, String)> = expected
        .iter()
        .map(|(k, v)| ((*k).to_ascii_lowercase(), (*v).to_owned()))
        .collect();
    want.sort();
    assert_eq!(
        got, want,
        "exact relayed header set mismatch (method={} path={} authority={} seen={seen:?})",
        seen.method, seen.path, seen.authority
    );
}

/// The RFC 9113 §8.2.2 connection-specific header names; none may appear on
/// an h2 hop (`te` only with value `trailers`).
const H2_FORBIDDEN: [&str; 7] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
    "proxy-authenticate",
    "proxy-authorization",
];

fn assert_h2_clean(seen: &Seen) {
    for n in H2_FORBIDDEN {
        assert!(
            seen.get(n).is_none(),
            "{n} must never reach an h2 upstream (RFC 9113 §8.2.2): {seen:?}"
        );
    }
    if let Some(te) = seen.get("te") {
        assert_eq!(
            te.trim(),
            "trailers",
            "te on h2 is only allowed with the value `trailers`: {seen:?}"
        );
    }
    // The FIX-052 class: `host` beside `:authority` is forbidden (§8.3.1);
    // nginx answers the pair with a bare 400 before the app sees it.
    assert!(
        seen.get("host").is_none(),
        "no host header may reach an h2 upstream (RFC 9113 §8.3.1): {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// strict oracles
// ---------------------------------------------------------------------------

type OracleRx = tokio::sync::oneshot::Receiver<Result<Seen, String>>;

/// A raw HTTP/2 upstream (the `h2` crate behind TLS — **not** hyper): answers
/// the first request `200` with body "ok" (plus `extra` response headers) and
/// reports everything that reached the wire on the receiver. `events`, when
/// set, streams that many `data: N\n\n` DATA frames 60 ms apart (SSE shape)
/// instead of one body.
async fn raw_h2_oracle(
    cfg: Arc<rustls::ServerConfig>,
    extra_resp: Vec<(String, String)>,
    events: Option<usize>,
) -> (SocketAddr, OracleRx) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Seen, String>>();
    tokio::spawn(async move {
        let out = async {
            let acc = TlsAcceptor::from(cfg);
            let (s, _) = l.accept().await.map_err(|e| format!("accept: {e}"))?;
            let tls = acc.accept(s).await.map_err(|e| format!("tls: {e}"))?;
            assert_eq!(
                tls.get_ref().1.alpn_protocol(),
                Some(&b"h2"[..]),
                "the upstream hop must negotiate h2"
            );
            let mut conn = h2::server::handshake(tls)
                .await
                .map_err(|e| format!("h2 handshake: {e}"))?;
            let (req, mut respond) = match conn.accept().await {
                Some(Ok(x)) => x,
                Some(Err(e)) => return Err(format!("h2 accept: {e}")),
                None => return Err("h2 accept: connection ended".to_owned()),
            };
            let (parts, mut body) = req.into_parts();
            let mut seen = Seen {
                method: parts.method.to_string(),
                path: parts.uri.path().to_owned(),
                authority: parts
                    .uri
                    .authority()
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                ..Seen::default()
            };
            for (k, v) in &parts.headers {
                seen.headers
                    .push((k.as_str().to_owned(), v.to_str().unwrap_or("?").to_owned()));
            }
            while let Some(chunk) = body.data().await {
                let chunk = chunk.map_err(|e| format!("body: {e}"))?;
                let _ = body.flow_control().release_capacity(chunk.len());
                seen.chunks.push(chunk.len());
                seen.body.push_str(&String::from_utf8_lossy(&chunk));
            }
            if let Ok(Some(t)) = body.trailers().await {
                for (k, v) in &t {
                    seen.trailers
                        .push((k.as_str().to_owned(), v.to_str().unwrap_or("?").to_owned()));
                }
            }
            let mut resp = Response::builder().status(200);
            for (k, v) in &extra_resp {
                resp = resp.header(k.as_str(), v.as_str());
            }
            let resp = resp.body(()).unwrap();
            let mut send = respond
                .send_response(resp, false)
                .map_err(|e| format!("send_response: {e}"))?;
            match events {
                Some(n) => {
                    for i in 0..n {
                        tokio::time::sleep(Duration::from_millis(60)).await;
                        send.send_data(Bytes::from(format!("data: {i}\n\n")), i + 1 == n)
                            .map_err(|e| format!("send_data: {e}"))?;
                    }
                }
                None => {
                    let _ = send.send_data(Bytes::from_static(b"ok\n"), true);
                }
            }
            // Keep the connection polled so the response frames flush (until
            // the peer goes away or the guard expires).
            let guard = tokio::time::sleep(Duration::from_secs(5));
            tokio::pin!(guard);
            loop {
                tokio::select! {
                    _ = &mut guard => break,
                    r = conn.accept() => match r {
                        Some(Ok(_)) => {}
                        _ => break,
                    },
                }
            }
            Ok::<Seen, String>(seen)
        }
        .await;
        let _ = tx.send(out);
    });
    (addr, rx)
}

/// The same exchange on a **plain** socket (the absolute-form `http://` lane).
async fn raw_h1_plain_oracle(extra_resp: Vec<(String, String)>) -> (SocketAddr, OracleRx) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Seen, String>>();
    tokio::spawn(async move {
        let out = async {
            let (s, _) = l.accept().await.map_err(|e| format!("accept: {e}"))?;
            read_h1_exchange(s, &extra_resp).await
        }
        .await;
        let _ = tx.send(out);
    });
    (addr, rx)
}

/// Shared h1 wire logic: parse the request head, read the body per its own
/// framing (recording chunk boundaries), answer, close.
async fn read_h1_exchange<S>(mut io: S, extra_resp: &[(String, String)]) -> Result<Seen, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut b = [0u8; 1];
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if io.read(&mut b).await.map_err(|e| format!("read: {e}"))? == 0 {
            return Err("eof in request head".into());
        }
        head.push(b[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut seen = Seen::default();
    let mut parts = request_line.split(' ');
    seen.method = parts.next().unwrap_or_default().to_owned();
    seen.path = parts.next().unwrap_or_default().to_owned();
    seen.version = parts.next().unwrap_or_default().to_owned();
    for l in lines {
        if l.is_empty() {
            break;
        }
        let (k, v) = l
            .split_once(':')
            .ok_or_else(|| format!("bad header line {l:?}"))?;
        seen.headers
            .push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
    }
    // Body, per the request's own framing.
    if let Some(cl) = seen
        .get("content-length")
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        let mut body = vec![0u8; cl];
        io.read_exact(&mut body)
            .await
            .map_err(|e| format!("body: {e}"))?;
        seen.chunks.push(cl);
        seen.body = String::from_utf8_lossy(&body).into_owned();
    } else if seen
        .get("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let mut line = Vec::new();
            while !line.ends_with(b"\r\n") {
                if io.read(&mut b).await.map_err(|e| format!("chunk: {e}"))? == 0 {
                    return Err("eof in chunk size".into());
                }
                line.push(b[0]);
            }
            let size = usize::from_str_radix(
                String::from_utf8_lossy(&line[..line.len() - 2])
                    .split(';')
                    .next()
                    .unwrap_or("0")
                    .trim(),
                16,
            )
            .map_err(|e| format!("chunk size: {e}"))?;
            if size == 0 {
                // Trailer section: read to the blank line (which may already
                // be complete after the 0 chunk).
                let mut t = Vec::new();
                loop {
                    if t == b"\r\n" || t.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    if io
                        .read(&mut b)
                        .await
                        .map_err(|e| format!("trailers: {e}"))?
                        == 0
                    {
                        break;
                    }
                    t.push(b[0]);
                }
                let ts = String::from_utf8_lossy(&t);
                for l in ts.split("\r\n").filter(|l| !l.is_empty()) {
                    if let Some((k, v)) = l.split_once(':') {
                        seen.trailers
                            .push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
                    }
                }
                break;
            }
            let mut chunk = vec![0u8; size + 2];
            io.read_exact(&mut chunk)
                .await
                .map_err(|e| format!("chunk data: {e}"))?;
            seen.chunks.push(size);
            seen.body.push_str(&String::from_utf8_lossy(&chunk[..size]));
        }
    }
    // Answer; then close (the response's hop-by-hop headers are only
    // meaningful on h1 — this is where stripping is observable).
    let mut resp = String::from("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n");
    for (k, v) in extra_resp {
        resp.push_str(&format!("{k}: {v}\r\n"));
    }
    resp.push_str("\r\nok");
    io.write_all(resp.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;
    io.shutdown().await.map_err(|e| format!("shutdown: {e}"))?;
    Ok(seen)
}

/// Reads a request head (no body expected — an upgrade exchange); the caller
/// continues the raw pipe on the same stream afterwards.
async fn read_h1_head<S>(io: &mut S) -> Result<Seen, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut b = [0u8; 1];
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if io.read(&mut b).await.map_err(|e| format!("read: {e}"))? == 0 {
            return Err("eof in request head".into());
        }
        head.push(b[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut seen = Seen::default();
    let mut parts = request_line.split(' ');
    seen.method = parts.next().unwrap_or_default().to_owned();
    seen.path = parts.next().unwrap_or_default().to_owned();
    seen.version = parts.next().unwrap_or_default().to_owned();
    for l in lines {
        if l.is_empty() {
            break;
        }
        let (k, v) = l
            .split_once(':')
            .ok_or_else(|| format!("bad header line {l:?}"))?;
        seen.headers
            .push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
    }
    Ok(seen)
}

// ---------------------------------------------------------------------------
// tier-1 oracles
// ---------------------------------------------------------------------------

/// POST relayed on an h2 upstream: the exact pseudo-header + regular-header
/// set, RFC 9113 §8.2.2/§8.3.1 clean, body intact (the FIX-052 regression,
/// generalized to exact sets). Also pins that a client-forged `host` header
/// (which together with `:authority` is the forbidden pair) never survives
/// hygiene: the wire shows `:authority` only.
#[tokio::test]
async fn h2_upstream_relayed_header_set_is_exact_and_rfc_clean() {
    const REQ_BODY: &str = r#"{"model":"m"}"#;
    let (up_ca, cfg) = oracle_tls("up-h2-exact");
    let (target, rx) = raw_h2_oracle(cfg, Vec::new(), None).await;
    let (proxy, gw_ca) = gw(&up_ca).await;
    let mut c = mitm(&gw_ca, proxy, target, true).await;
    let (st, _, resp) = text(
        c.send(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("https://{target}/v1/messages"))
                .header("host", target.to_string()) // the forbidden pair, forged by the client
                .header("content-type", "application/json")
                .header("x-app-header", "v1")
                .body(Full::new(Bytes::from_static(REQ_BODY.as_bytes())))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(st, 200, "{resp}");
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path, "/v1/messages");
    assert_eq!(
        seen.authority,
        target.to_string(),
        ":authority must carry the endpoint address"
    );
    assert_h2_clean(&seen);
    // hyper's h2 client materializes content-length on the wire (13 = the
    // body length); pinned so a change in that behavior is visible.
    assert_exact_headers(
        &seen,
        &[
            ("content-length", "13"),
            ("content-type", "application/json"),
            ("x-app-header", "v1"),
        ],
    );
    assert_eq!(seen.body, REQ_BODY, "body intact");
    assert_eq!(seen.chunks, vec![13], "single DATA frame");
}

/// A no-body POST on h2 is forwarded as `end_stream` on HEADERS — never as a
/// zero-length DATA frame (pinned: the end-stream form).
#[tokio::test]
async fn h2_no_body_post_is_end_stream_not_a_zero_length_data_frame() {
    let (up_ca, cfg) = oracle_tls("up-h2-nobody");
    let (target, rx) = raw_h2_oracle(cfg, Vec::new(), None).await;
    let (proxy, gw_ca) = gw(&up_ca).await;
    let mut c = mitm(&gw_ca, proxy, target, true).await;
    let (st, _, resp) = text(
        c.send(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("https://{target}/empty"))
                .header("content-type", "application/json")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(st, 200, "{resp}");
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path, "/empty");
    assert_h2_clean(&seen);
    assert!(
        seen.chunks.is_empty(),
        "end_stream must be signalled on HEADERS; got DATA frames {:?}: {seen:?}",
        seen.chunks
    );
    assert!(seen.body.is_empty());
}

/// SSE (`text/event-stream`) from an h2 upstream reaches the client as
/// separate events as they are produced — never buffered into one chunk.
#[tokio::test]
async fn h2_sse_events_stream_through_unbuffered() {
    let (up_ca, cfg) = oracle_tls("up-h2-sse");
    let (target, rx) = raw_h2_oracle(
        cfg,
        vec![("content-type".into(), "text/event-stream".into())],
        Some(3),
    )
    .await;
    let (proxy, gw_ca) = gw(&up_ca).await;
    let mut c = mitm(&gw_ca, proxy, target, true).await;
    let r = c
        .send(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("https://{target}/sse"))
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status(), 200);
    let mut frames = Vec::new();
    let mut body = r.into_body();
    while let Some(f) = body.frame().await {
        let f = f.unwrap();
        if f.is_data() {
            frames.push(String::from_utf8_lossy(&f.into_data().unwrap()).into_owned());
        }
    }
    let _ = rx.await;
    assert_eq!(
        frames,
        vec!["data: 0\n\n", "data: 1\n\n", "data: 2\n\n"],
        "each SSE event must arrive as its own chunk (unbuffered relay)"
    );
}

/// Content-length is preserved on buffered h2 relay; `host` never rides along.
#[tokio::test]
async fn h2_content_length_is_preserved_on_buffered_relay() {
    let (up_ca, cfg) = oracle_tls("up-h2-cl");
    let (target, rx) = raw_h2_oracle(cfg, Vec::new(), None).await;
    let (proxy, gw_ca) = gw(&up_ca).await;
    let mut c = mitm(&gw_ca, proxy, target, true).await;
    let (st, _, resp) = text(
        c.send(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("https://{target}/cl"))
                .header("content-type", "text/plain")
                .body(Full::new(Bytes::from_static(b"hello world")))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(st, 200, "{resp}");
    let seen = seen_err(rx.await.unwrap());
    assert_h2_clean(&seen);
    assert_eq!(seen.body, "hello world");
    assert_eq!(
        seen.get("content-length"),
        Some("11"),
        "content-length must be preserved on buffered bodies: {seen:?}"
    );
}

/// A chunked h1 request relayed to an h2 upstream: no `transfer-encoding` on
/// the h2 wire (§8.2.2), body re-framed intact (chunk boundaries may
/// coalesce; the bytes may not change).
#[tokio::test]
async fn h2_chunked_h1_request_is_re_framed_without_transfer_encoding() {
    let (up_ca, cfg) = oracle_tls("up-h2-chunked");
    let (target, rx) = raw_h2_oracle(cfg, Vec::new(), None).await;
    let (proxy, gw_ca) = gw(&up_ca).await;
    // Raw h1 client leg (hyper cannot emit chunked bodies of a known size):
    // CONNECT, TLS, then a chunked POST by hand.
    let mut s = crate::common::connect(proxy).await;
    let t = target.to_string();
    s.write_all(
        format!(
            "CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: {}\r\n\r\n",
            basic("agent:aug_good")
        )
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
    let mut tls = tls_client(&gw_ca, &[b"http/1.1"])
        .connect(ServerName::IpAddress(target.ip().into()), s)
        .await
        .unwrap();
    tls.write_all(
        format!(
            "POST /chunked HTTP/1.1\r\nHost: {t}\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n3\r\nabc\r\n0\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut resp)).await;
    let resp = String::from_utf8_lossy(&resp);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path, "/chunked");
    assert_h2_clean(&seen);
    assert_eq!(
        seen.body, "helloabc",
        "chunked h1 body must be re-framed intact on h2: {seen:?}"
    );
    assert!(
        !seen.chunks.is_empty(),
        "the body must reach the upstream as DATA: {seen:?}"
    );
}

/// Request trailers on h2 (`te: trailers` preserved by hygiene, then the
/// trailer field itself) reach the upstream relayed, not dropped.
#[tokio::test]
async fn h2_te_trailers_request_is_relayed_with_trailers() {
    let (up_ca, cfg) = oracle_tls("up-h2-trailers");
    let (target, rx) = raw_h2_oracle(cfg, Vec::new(), None).await;
    let (proxy, gw_ca) = gw(&up_ca).await;
    // Raw h2 client leg: hyper's SendRequest cannot send trailers.
    let mut s = crate::common::connect(proxy).await;
    let t = target.to_string();
    s.write_all(
        format!(
            "CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: {}\r\n\r\n",
            basic("agent:aug_good")
        )
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
    let tls = tls_client(&gw_ca, &[b"h2"])
        .connect(ServerName::IpAddress(target.ip().into()), s)
        .await
        .unwrap();
    let (send_req, conn) = h2::client::handshake(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut ready = send_req.ready().await.unwrap();
    let req = Request::builder()
        .method(http::Method::POST)
        .uri(format!("https://{t}/trailered"))
        .header("te", "trailers")
        .header("content-type", "text/plain")
        .body(())
        .unwrap();
    let (resp_fut, mut stream) = ready.send_request(req, false).unwrap();
    stream
        .send_data(Bytes::from_static(b"payload"), false)
        .unwrap();
    let mut trailers = http::HeaderMap::new();
    trailers.insert(
        http::HeaderName::from_static("x-check"),
        http::HeaderValue::from_static("sum"),
    );
    stream.send_trailers(trailers).unwrap();
    let resp = resp_fut.await.unwrap();
    assert_eq!(resp.status(), 200);
    let seen = seen_err(rx.await.unwrap());
    assert_h2_clean(&seen); // `te: trailers` is the one allowed value
    assert_eq!(seen.body, "payload");
    assert_eq!(
        seen.get("te"),
        Some("trailers"),
        "te: trailers must be preserved (hygiene keeps it for gRPC): {seen:?}"
    );
    assert_eq!(
        seen.trailers,
        vec![("x-check".to_owned(), "sum".to_owned())],
        "request trailers must be relayed on h2: {seen:?}"
    );
}

/// Response-path conformance on h2: the relayed *request* stays clean when
/// the client sends headers that would be hop-by-hop on h1 (`keep-alive`,
/// `proxy-authenticate`, `connection`) — hygiene strips them at the inbound
/// edge, so an h2 upstream observes none (RFC 9113 §8.2.2). The response
/// direction has nothing h1-specific to strip on an h2 hop: hyper answers
/// h2-forbidden response headers with a protocol error, which the oracle
/// itself would trip (that direction is covered by the raw-h1 oracles).
#[tokio::test]
async fn h2_response_hop_by_hop_headers_do_not_reach_the_client() {
    let (up_ca, cfg) = oracle_tls("up-h2-resp");
    let (target, rx) = raw_h2_oracle(cfg, Vec::new(), None).await;
    let (proxy, gw_ca) = gw(&up_ca).await;
    let mut c = mitm(&gw_ca, proxy, target, true).await;
    let (st, headers, resp) = text(
        c.send(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("https://{target}/resp"))
                .header("keep-alive", "timeout=5")
                .header("proxy-authenticate", "Basic")
                .header("connection", "keep-alive")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(st, 200, "{resp}");
    let seen = seen_err(rx.await.unwrap());
    for banned in [
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "connection",
    ] {
        assert!(
            seen.get(banned).is_none(),
            "{banned} must never reach an h2 upstream (RFC 9113 §8.2.2): {seen:?}"
        );
    }
    for banned in [
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "connection",
    ] {
        assert!(
            !headers.contains_key(banned),
            "{banned} must be stripped from the client response (status {st})"
        );
    }
}

/// WS upgrade relay shape on an h1 upstream: the request arrives with
/// `connection: upgrade` + `upgrade: websocket` re-added by the gateway,
/// `sec-websocket-*` intact, `host` synthesized — and the 101 + raw bytes
/// come back through the tunnel. Raw h1 on both sides (no hyper), so the
/// exact shapes are what a strict front door would judge.
#[tokio::test]
async fn h1_ws_upgrade_relay_shape_and_byte_pipe() {
    // Raw h1 TLS oracle that completes a 101 upgrade and echoes bytes.
    let (up_ca, cfg) = oracle_tls("up-h1-ws");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = l.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Seen, String>>();
    tokio::spawn(async move {
        let out = async {
            let acc = TlsAcceptor::from(cfg);
            let (s, _) = l.accept().await.map_err(|e| format!("accept: {e}"))?;
            let mut tls = acc.accept(s).await.map_err(|e| format!("tls: {e}"))?;
            assert_eq!(
                tls.get_ref().1.alpn_protocol(),
                Some(&b"http/1.1"[..]),
                "upgrade hops are h1 (client_h1)"
            );
            let seen = read_h1_head(&mut tls).await?;
            // 101 + upgrade headers, then echo raw bytes (the byte pipe).
            tls.write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\n\r\n",
            )
            .await
            .map_err(|e| format!("write 101: {e}"))?;
            let mut buf = [0u8; 5];
            tls.read_exact(&mut buf)
                .await
                .map_err(|e| format!("pipe: {e}"))?;
            tls.write_all(&buf).await.map_err(|e| format!("echo: {e}"))?;
            Ok::<_, String>(seen)
        }
        .await;
        let _ = tx.send(out);
    });
    let (proxy, gw_ca) = gw(&up_ca).await;
    // Raw h1 client through the MITM tunnel.
    let mut s = crate::common::connect(proxy).await;
    let t = target.to_string();
    s.write_all(
        format!(
            "CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: {}\r\n\r\n",
            basic("agent:aug_good")
        )
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
    let mut tls = tls_client(&gw_ca, &[b"http/1.1"])
        .connect(ServerName::IpAddress(target.ip().into()), s)
        .await
        .unwrap();
    tls.write_all(
        format!(
            "GET /ws HTTP/1.1\r\nhost: {t}\r\nconnection: Upgrade\r\nupgrade: websocket\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    // The 101 head from (via) the gateway.
    let mut resp_head = Vec::new();
    while !resp_head.ends_with(b"\r\n\r\n") {
        assert!(tls.read(&mut b).await.unwrap() > 0, "eof in 101 head");
        resp_head.push(b[0]);
    }
    let resp_head = String::from_utf8_lossy(&resp_head).to_ascii_lowercase();
    assert!(
        resp_head.starts_with("http/1.1 101"),
        "upgrade must relay as 101: {resp_head}"
    );
    assert!(
        resp_head.contains("connection: upgrade") && resp_head.contains("upgrade: websocket"),
        "101 must carry the upgrade headers: {resp_head}"
    );
    // The byte pipe both ways.
    tls.write_all(b"hello").await.unwrap();
    let mut echo = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), tls.read_exact(&mut echo))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        &echo, b"hello",
        "raw bytes must traverse the relayed tunnel"
    );
    // What the upstream actually received.
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(seen.method, "GET");
    assert_eq!(seen.path, "/ws");
    assert_eq!(seen.version, "HTTP/1.1", "origin-form request line");
    assert!(
        !seen.path.contains("http"),
        "origin-form, not absolute-form"
    );
    assert_eq!(
        seen.get("host"),
        Some(t.as_str()),
        "h1 hop must carry the synthesized Host"
    );
    assert_eq!(
        seen.get("upgrade"),
        Some("websocket"),
        "upgrade header re-added for the h1 hop: {seen:?}"
    );
    assert_eq!(
        seen.get("connection").map(str::to_ascii_lowercase),
        Some("upgrade".to_owned()),
        "connection: upgrade re-added for the h1 hop: {seen:?}"
    );
    assert_eq!(
        seen.get("sec-websocket-key"),
        Some("dGhlIHNhbXBsZSBub25jZQ=="),
        "sec-websocket-* must be relayed: {seen:?}"
    );
    assert!(seen.get("keep-alive").is_none());
    assert!(seen.get("proxy-connection").is_none());
}

/// Plain-HTTP (h1) relay, exact request shape: origin-form request line,
/// exactly one synthesized `Host` (with the non-default port — the
/// hyper-util assumption FIX-052 leans on, pinned so a vendored-stack
/// upgrade that changes it fails here), no proxy-* remnants, app headers
/// intact, nothing else. The plain lane runs absolute-form inbound; the
/// upstream hop is still origin-form.
#[tokio::test]
async fn h1_upstream_request_shape_and_host_synthesis_are_exact() {
    let (target, rx) = raw_h1_plain_oracle(Vec::new()).await;
    let (proxy, _gw_ca) = gw("").await;
    let h = target.to_string();
    let (st, resp) = absolute_form_raw(&proxy, &h, "/plain?q=1", &[("x-app-header", "v1")]).await;
    assert_eq!(st, 200, "{resp}");
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(seen.method, "GET");
    assert_eq!(seen.path, "/plain?q=1", "origin-form request line");
    assert_eq!(seen.version, "HTTP/1.1");
    assert_eq!(
        seen.get("host"),
        Some(h.as_str()),
        "hyper-util synthesizes Host from the URI; a non-default port \
         must appear as host:port"
    );
    assert_eq!(seen.count("host"), 1, "exactly one Host header: {seen:?}");
    assert_exact_headers(&seen, &[("host", &h), ("x-app-header", "v1")]);
}

/// One raw absolute-form GET through the proxy (plaintext h1 client — the
/// absolute-form lane's client is plain h1); returns (status, response text).
async fn absolute_form_raw(
    proxy: &SocketAddr,
    target: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut s = crate::common::connect(*proxy).await;
    let mut req = format!(
        "GET http://{target}{path} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n",
        basic("agent:aug_good")
    );
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|x| x.parse::<u16>().ok())
        .unwrap_or(0);
    (status, text)
}

/// hop-by-hop response stripping on an h1 hop: `connection`/`keep-alive`
/// name-lists and `proxy-authenticate` are stripped toward the client; app
/// headers and the body survive.
#[tokio::test]
async fn h1_response_hop_by_hop_headers_are_stripped() {
    let (target, rx) = raw_h1_plain_oracle(vec![
        ("connection".into(), "keep-alive".into()),
        ("keep-alive".into(), "timeout=5, max=100".into()),
        (
            "proxy-authenticate".into(),
            "Basic realm=\"upstream\"".into(),
        ),
        ("x-keep".into(), "yes".into()),
    ])
    .await;
    let (proxy, _gw_ca) = gw("").await;
    let h = target.to_string();
    let (st, resp) = absolute_form_raw(&proxy, &h, "/resp", &[]).await;
    assert_eq!(st, 200, "{resp}");
    let _ = rx.await;
    let headers = parse_response_headers(&resp);
    // `connection: close` here is the gateway's own edge header (the client
    // asked to close); what must never appear is the *upstream's* hop-by-hop
    // content: its `connection: keep-alive`, its `keep-alive` params and its
    // `proxy-authenticate` challenge.
    assert!(
        !headers
            .iter()
            .any(|(k, v)| k == "connection" && v == "keep-alive"),
        "the upstream's connection header must be stripped: {resp}"
    );
    for banned in ["keep-alive:", "proxy-authenticate:", "proxy-authorization:"] {
        assert!(
            !resp.to_ascii_lowercase().contains(banned),
            "{banned} must be stripped from the relayed response: {resp}"
        );
    }
    assert_eq!(
        headers
            .iter()
            .find(|(k, _)| *k == "x-keep")
            .map(|(_, v)| v.as_str()),
        Some("yes"),
        "end-to-end headers survive: {resp}"
    );
}

/// Lower-cased headers of a raw response text (name, value) in order.
fn parse_response_headers(resp: &str) -> Vec<(String, String)> {
    resp.split("\r\n\r\n")
        .next()
        .unwrap_or(resp)
        .lines()
        .skip(1)
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect()
}

/// A chunked h1 request relayed to an h1 upstream: the pinned re-framing is
/// chunked again (`transfer-encoding: chunked`, no content-length — the body
/// length is unknowable while streaming), boundaries preserved by the stream,
/// body byte-identical. No `te` remnant may reach the upstream.
#[tokio::test]
async fn h1_chunked_request_re_framing_is_pinned() {
    let (target, rx) = raw_h1_plain_oracle(Vec::new()).await;
    let (proxy, _gw_ca) = gw("").await;
    // Raw h1 client on the absolute-form lane (hyper cannot emit chunked).
    let mut s = crate::common::connect(proxy).await;
    let h = target.to_string();
    s.write_all(
        format!(
            "POST http://{h}/chunked HTTP/1.1\r\nHost: {h}\r\nProxy-Authorization: {}\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n3\r\nabc\r\n0\r\n\r\n",
            basic("agent:aug_good")
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut resp)).await;
    let resp = String::from_utf8_lossy(&resp);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path, "/chunked");
    assert_eq!(
        seen.body, "helloabc",
        "chunk boundaries may be re-framed, the body may not change: {seen:?}"
    );
    // Pinned re-framing: chunked again (streamed), no content-length.
    assert_eq!(
        seen.get("transfer-encoding"),
        Some("chunked"),
        "a streamed chunked body is relayed chunked: {seen:?}"
    );
    assert!(
        seen.get("content-length").is_none(),
        "no content-length may be invented for an unknown-length body: {seen:?}"
    );
    assert_eq!(
        seen.chunks,
        vec![5, 3],
        "chunk boundaries preserved by the streaming relay: {seen:?}"
    );
    assert!(
        seen.get("te").is_none(),
        "no te remnant (only `te: trailers` is ever kept): {seen:?}"
    );
}

/// A request that is both content-length'd and chunked is a framing conflict.
/// hyper (the inbound edge) resolves it by `Transfer-Encoding` precedence and
/// strips `content-length`; the pinned safe behavior: the body is relayed
/// **once, chunked, no `content-length`** — never a smuggling-shaped mix of
/// both framings toward one upstream.
#[tokio::test]
async fn h1_content_length_and_chunked_conflict_is_never_relayed_with_both_framings() {
    let (target, rx) = raw_h1_plain_oracle(Vec::new()).await;
    let (proxy, _gw_ca) = gw("").await;
    // Raw h1 client (hyper would refuse to *emit* this shape itself).
    let mut s = crate::common::connect(proxy).await;
    let h = target.to_string();
    s.write_all(
        format!(
            "POST http://{h}/conflict HTTP/1.1\r\nHost: {h}\r\nProxy-Authorization: {}\r\ncontent-type: text/plain\r\ncontent-length: 5\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            basic("agent:aug_good")
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut resp)).await;
    let resp = String::from_utf8_lossy(&resp);
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "the request is answered (TE precedence at the inbound edge): {resp}"
    );
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(
        seen.get("transfer-encoding"),
        Some("chunked"),
        "the relayed framing is chunked: {seen:?}"
    );
    assert!(
        seen.get("content-length").is_none(),
        "content-length must be gone — never both framings (smuggling class): {seen:?}"
    );
    assert_eq!(
        seen.body, "hello",
        "exactly one body interpretation is forwarded: {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// the assumption tests (load-bearing since FIX-052)
// ---------------------------------------------------------------------------

/// The hyper-util Host-synthesis assumption, pinned where production exercises
/// it: on an h1 hop the `Host` header is synthesized by the vendored stack
/// (`hyper_util::client::legacy` `set_host`: value = URI host, non-default
/// ports kept as `host:port`). A future upgrade that changes the byte on the
/// wire fails here. (Default-port dropping is covered by the same `set_host`
/// code path; the gateway always dials explicit ports, so the non-default
/// form is the production-reachable case.)
#[tokio::test]
async fn h1_host_synthesis_value_and_default_port_dropping_are_pinned() {
    let (target, rx) = raw_h1_plain_oracle(Vec::new()).await;
    let (proxy, _gw_ca) = gw("").await;
    let h = target.to_string();
    let (st, resp) = absolute_form_raw(&proxy, &h, "/synth", &[]).await;
    assert_eq!(st, 200, "{resp}");
    let seen = seen_err(rx.await.unwrap());
    assert_eq!(
        seen.get("host"),
        Some(h.as_str()),
        "expected `host: {h}` (synthesized from the URI authority, \
         non-default port kept): {seen:?}"
    );
    assert_eq!(seen.count("host"), 1);
}

/// The RFC-forbidden `host`+`:authority` pair (the FIX-052 bug class): the
/// snapshot-level guard refuses a `host`-targeting injection header at load,
/// so no mechanism can re-create the pair that nginx rejects.
#[test]
fn host_targeting_injection_is_refused_at_snapshot_load() {
    use aurora_gateway::snapshot;
    use serde_json::{Value, json};
    let mk = |inject: Value| {
        json!({
            "format": 1, "contract": 1,
            "packages": [{"package_id": "p", "version": "1.0.0", "digest": "d", "publisher": "x",
                          "hooks_digest": null, "egress": {}, "context": []}],
            "oauth_clients": [],
            "apps": [{"app_id": "a", "package_id": "p", "about": {"id": "a", "name": "A"},
                      "credentials": [], "metadata": [], "connect": [], "token": null,
                      "tools": null, "approvals": {}, "agent": null, "database": null}],
            "hosts": [{
                "host": "api.example.com", "path_prefix": null, "app_id": "a",
                "claim_status": "active", "tenant": null, "rewrite_host": null,
                "inject": [inject], "finalize": null, "transform": null,
                "intercept": null, "passthrough": []
            }],
            "reserved_hosts": []
        })
    };
    for name in ["host", "Host", "HOST"] {
        let snap = mk(json!({"header": {"name": name, "value": "evil"}}));
        let err = snapshot::parse("h", snap).expect_err("host override must be refused");
        assert!(
            err.contains("host override is not supported"),
            "refusal must name the rule: {err}"
        );
    }
    // An unrelated injection header still loads.
    let ok = mk(json!({"header": {"name": "x-api-key", "value": "k"}}));
    assert!(snapshot::parse("h", ok).is_ok());
}

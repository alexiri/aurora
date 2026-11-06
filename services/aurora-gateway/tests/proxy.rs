#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End-to-end proxy tests on ephemeral ports.

use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::guard::Guard;
use aurora_gateway::pipeline::PassThrough;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Empty, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use uuid::Uuid;

struct StaticResolver;

#[async_trait::async_trait]
impl CredentialResolver for StaticResolver {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        if token != "aug_good" {
            return Ok(None);
        }
        let id = Uuid::now_v7();
        Ok(Some(RequestIdentity {
            credential_id: id,
            kind: CredentialKind::Agent,
            agent_id: id,
            workspace_id: id,
            org_id: id,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            principals: vec![id],
        }))
    }
}

type RespBody = BoxBody<Bytes, Infallible>;

async fn upstream_handler(mut req: Request<Incoming>) -> Result<Response<RespBody>, Infallible> {
    let path = req.uri().path().to_owned();
    if req.headers().contains_key("upgrade") {
        let up = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            let Ok(up) = up.await else { return };
            let mut io = TokioIo::new(up);
            let mut buf = [0u8; 1024];
            while let Ok(n) = io.read(&mut buf).await {
                if n == 0 || io.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
        let mut r = Response::new(Empty::new().boxed());
        *r.status_mut() = http::StatusCode::SWITCHING_PROTOCOLS;
        r.headers_mut()
            .insert("connection", "upgrade".parse().unwrap());
        r.headers_mut()
            .insert("upgrade", "websocket".parse().unwrap());
        return Ok(r);
    }
    match path.as_str() {
        "/sse" => {
            let s = futures::stream::unfold(0u8, |i| async move {
                if i == 3 {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
                Some((
                    Ok::<_, Infallible>(Frame::data(Bytes::from(format!("data: {i}\n\n")))),
                    i + 1,
                ))
            });
            Ok(Response::new(StreamBody::new(s).boxed()))
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(Response::new(Full::new(Bytes::from("late")).boxed()))
        }
        _ => {
            let mut out = format!(
                "method={}\npath={}\nversion={:?}\n",
                req.method(),
                req.uri().path(),
                req.version()
            );
            for (k, v) in req.headers() {
                out.push_str(&format!("{}: {}\n", k, v.to_str().unwrap_or("?")));
            }
            Ok(Response::new(Full::new(Bytes::from(out)).boxed()))
        }
    }
}

struct Env {
    proxy: SocketAddr,
    /// TLS upstream (127.0.0.1:port)
    https: SocketAddr,
    http: SocketAddr,
    gw_ca_pem: String,
    up_ca_pem: String,
}

async fn start(allow: &[&str], header_timeout: Duration) -> Env {
    let up_ca = ca::generate("up").unwrap();
    let up = Ca::from_pem(&up_ca.cert_pem, &up_ca.key_pem).unwrap();
    let cfg = up.server_config("127.0.0.1").unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let https = l.local_addr().unwrap();
    tokio::spawn(async move {
        let acc = TlsAcceptor::from(cfg);
        loop {
            let (s, _) = l.accept().await.unwrap();
            let acc = acc.clone();
            tokio::spawn(async move {
                let Ok(tls) = acc.accept(s).await else { return };
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(TokioIo::new(tls), service_fn(upstream_handler))
                    .await;
            });
        }
    });
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(TokioIo::new(s), service_fn(upstream_handler))
                    .await;
            });
        }
    });
    let gw = ca::generate("gw").unwrap();
    let gw_ca = Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = l.local_addr().unwrap();
    let allow: Vec<String> = allow.iter().map(|s| (*s).to_owned()).collect();
    let guard = Arc::new(Guard::new(&allow, vec![proxy]).unwrap());
    let p = Proxy::new(ProxyParts {
        ca: gw_ca,
        resolver: Arc::new(StaticResolver),
        handler: Arc::new(PassThrough),
        guard,
        header_timeout,
        extra_ca_pem: Some(up_ca.cert_pem.clone().into_bytes()),
        egress_proxy: None,
    })
    .unwrap();
    tokio::spawn(p.run(l, std::future::pending()));
    Env {
        proxy,
        https,
        http,
        gw_ca_pem: gw.cert_pem,
        up_ca_pem: up_ca.cert_pem,
    }
}

fn basic(user_token: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(user_token)
    )
}

async fn read_head(s: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        assert!(s.read(&mut b).await.unwrap() > 0, "eof in head");
        buf.push(b[0]);
    }
    String::from_utf8(buf).unwrap()
}

/// One raw request on the proxy port, read to EOF.
async fn raw(proxy: SocketAddr, req: &str) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

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

async fn connect_raw(proxy: SocketAddr, auth: Option<&str>, target: &str) -> (String, TcpStream) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    let a = auth
        .map(|a| format!("Proxy-Authorization: {}\r\n", basic(a)))
        .unwrap_or_default();
    s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{a}\r\n").as_bytes())
        .await
        .unwrap();
    let head = read_head(&mut s).await;
    (head, s)
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

/// MITM tunnel: CONNECT, TLS with the gateway CA, then HTTP/1.1 or HTTP/2.
async fn mitm(env: &Env, target: SocketAddr, h2: bool) -> Sender {
    let (head, s) = connect_raw(env.proxy, Some("agent:aug_good"), &target.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let alpn: &[&[u8]] = if h2 { &[b"h2"] } else { &[b"http/1.1"] };
    let tls = tls_client(&env.gw_ca_pem, alpn)
        .connect(ServerName::IpAddress(target.ip().into()), s)
        .await
        .unwrap();
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(if h2 { &b"h2"[..] } else { &b"http/1.1"[..] })
    );
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

fn get(target: SocketAddr, path: &str, extra: &[(&str, &str)]) -> Request<Full<Bytes>> {
    let mut b = Request::builder()
        .uri(format!("https://{target}{path}"))
        .header("host", target.to_string());
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    b.body(Full::new(Bytes::new())).unwrap()
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

#[tokio::test]
async fn https_connect_h1_and_h2_with_header_hygiene() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    for h2 in [false, true] {
        let mut c = mitm(&env, env.https, h2).await;
        let (st, _, body) = text(
            c.send(get(
                env.https,
                "//a/b?q=1",
                &[
                    ("x-aurora-connection-id", "c1"),
                    ("x-aurora-secret", "s"),
                    ("proxy-foo", "1"),
                    ("keep-alive", "timeout=5"),
                    ("x-keep", "yes"),
                ],
            ))
            .await,
        )
        .await;
        assert_eq!(st, 200, "{body}");
        assert!(body.contains("path=/a/b\n"), "{body}");
        assert!(body.contains("x-keep: yes"), "{body}");
        assert!(
            body.contains("version=HTTP/2.0"),
            "upstream should speak h2: {body}"
        );
        let lower = body.to_lowercase();
        for banned in ["x-aurora", "proxy-", "keep-alive", "proxy-authorization"] {
            assert!(!lower.contains(banned), "{banned} leaked: {body}");
        }
    }
}

#[tokio::test]
async fn sse_streams_intact() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    let mut c = mitm(&env, env.https, false).await;
    let (st, _, body) = text(c.send(get(env.https, "/sse", &[])).await).await;
    assert_eq!(st, 200);
    assert_eq!(body, "data: 0\n\ndata: 1\n\ndata: 2\n\n");
}

#[tokio::test]
async fn auth_required() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    let t = env.https.to_string();
    let missing = raw(
        env.proxy,
        &format!("CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 407"), "{missing}");
    assert!(
        missing
            .to_lowercase()
            .contains("proxy-authenticate: basic realm=\"aurora\"")
    );
    assert!(missing.contains("proxy_auth_required"));
    assert!(missing.to_lowercase().contains("x-should-retry: false"));
    let bad = raw(
        env.proxy,
        &format!(
            "CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n",
            basic("agent:aug_wrong")
        ),
    )
    .await;
    assert!(bad.starts_with("HTTP/1.1 407"), "{bad}");
}

#[tokio::test]
async fn origin_form_and_absolute_form() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    let r = raw(
        env.proxy,
        "GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 400"), "{r}");
    assert!(r.contains("not_a_proxy_request"));
    let h = env.http.to_string();
    let r = raw(
        env.proxy,
        &format!(
            "GET http://{h}/plain HTTP/1.1\r\nHost: {h}\r\nProxy-Authorization: {}\r\nProxy-Connection: keep-alive\r\nX-Aurora-Batch-Id: b\r\nConnection: close\r\n\r\n",
            basic("agent:aug_good")
        ),
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.contains("path=/plain"));
    assert!(!r.to_lowercase().contains("x-aurora"), "{r}");
    assert!(!r.to_lowercase().contains("proxy-connection"), "{r}");
    let r = raw(
        env.proxy,
        &format!("GET http://{h}/plain HTTP/1.1\r\nHost: {h}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 407"), "{r}");
}

#[tokio::test]
async fn method_override_and_ambiguous_paths() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    let mut c = mitm(&env, env.https, false).await;
    let (st, _, body) = text(
        c.send(get(
            env.https,
            "/a",
            &[("x-http-method-override", "DELETE")],
        ))
        .await,
    )
    .await;
    assert_eq!(st, 400);
    assert!(body.contains("method_override_refused"));
    let (st, _, body) = text(c.send(get(env.https, "/a?_method=DELETE", &[])).await).await;
    assert_eq!(st, 400);
    assert!(body.contains("method_override_refused"));
    for p in ["/a/../b", "/a/%2Fb", "/a%5Cb", "/a%00b", "/a/./b"] {
        let (st, _, body) = text(c.send(get(env.https, p, &[])).await).await;
        assert_eq!(st, 400, "{p}");
        assert!(body.contains("ambiguous_path"), "{p}: {body}");
    }
}

#[tokio::test]
async fn destination_guard() {
    // No allow-list: loopback refused (name-checked inside the tunnel).
    let env = start(&[], Duration::from_secs(10)).await;
    let mut c = mitm(&env, env.https, false).await;
    let (st, h, body) = text(c.send(get(env.https, "/", &[])).await).await;
    assert_eq!(st, 403, "{body}");
    assert_eq!(h["x-should-retry"], "false");
    assert!(body.contains("destination_refused"));
    assert!(body.contains("\"host\":\"127.0.0.1\""), "{body}");

    // Hostname resolving to a private IP.
    let (head, s) = connect_raw(
        env.proxy,
        Some("agent:aug_good"),
        &format!("localhost:{}", env.https.port()),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"));
    let tls = tls_client(&env.gw_ca_pem, &[b"http/1.1"])
        .connect(ServerName::try_from("localhost").unwrap(), s)
        .await
        .unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    let req = Request::builder()
        .uri("/")
        .header("host", "localhost")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let (st, _, body) = text(sender.send_request(req).await.unwrap()).await;
    assert_eq!(st, 403, "{body}");
    assert!(body.contains("destination_refused"));

    // Metadata address refused even with a covering CIDR.
    let env = start(&["169.254.0.0/16", "127.0.0.0/8"], Duration::from_secs(10)).await;
    let md: SocketAddr = "169.254.169.254:80".parse().unwrap();
    let mut c = mitm(&env, md, false).await;
    let (st, _, body) = text(c.send(get(md, "/latest/meta-data", &[])).await).await;
    assert_eq!(st, 403, "{body}");
    assert!(body.contains("destination_refused"));
}

#[tokio::test]
async fn open_lane_does_not_terminate_tls() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    let (head, s) = connect_raw(env.proxy, Some("open:aug_good"), &env.https.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    // Trusting only the upstream CA proves the upstream's own cert is shown.
    let tls = tls_client(&env.up_ca_pem, &[b"http/1.1"])
        .connect(ServerName::IpAddress(env.https.ip().into()), s)
        .await
        .unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    let req = Request::builder()
        .uri("/open")
        .header("host", env.https.to_string())
        .header("x-aurora-connection-id", "kept-in-tunnel")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let (st, _, body) = text(sender.send_request(req).await.unwrap()).await;
    assert_eq!(st, 200);
    assert!(body.contains("path=/open"));
    // Open lane still hits the guard on CONNECT.
    let env = start(&[], Duration::from_secs(10)).await;
    let (head, _) = connect_raw(env.proxy, Some("open:aug_good"), &env.https.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
}

#[tokio::test]
async fn websocket_upgrade_through_mitm() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    let mut c = mitm(&env, env.https, false).await;
    let Sender::H1(s) = &mut c else { panic!() };
    let req = Request::builder()
        .uri("/ws")
        .header("host", env.https.to_string())
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let mut resp = s.send_request(req).await.unwrap();
    assert_eq!(resp.status(), 101);
    let up = hyper::upgrade::on(&mut resp).await.unwrap();
    let mut io = TokioIo::new(up);
    io.write_all(b"hello").await.unwrap();
    let mut buf = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), io.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"hello");
}

#[tokio::test]
async fn upstream_header_timeout() {
    let env = start(&["127.0.0.0/8"], Duration::from_millis(300)).await;
    let mut c = mitm(&env, env.https, false).await;
    let (st, _, body) = text(c.send(get(env.https, "/slow", &[])).await).await;
    assert_eq!(st, 504, "{body}");
    assert!(body.contains("upstream_timeout"));
}

#[tokio::test]
async fn connect_failure_is_bad_gateway() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(5)).await;
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = free.local_addr().unwrap();
    drop(free);
    let mut c = mitm(&env, dead, false).await;
    let (st, _, body) = text(c.send(get(dead, "/", &[])).await).await;
    assert_eq!(st, 502, "{body}");
    assert!(body.contains("bad_gateway"));
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End-to-end proxy tests on ephemeral ports.

use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::guard::Guard;
use aurora_gateway::pipeline::PassThrough;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::proxy_tls::ProxyTls;
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
use tokio_rustls::client::TlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use uuid::Uuid;

struct StaticResolver;

#[async_trait::async_trait]
impl CredentialResolver for StaticResolver {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        // `aub_browser` is a boot credential minted with the `browser` capability (RFE-003).
        if token != "aug_good" && token != "aub_browser" {
            return Ok(None);
        }
        let boot = token == "aub_browser";
        let id = Uuid::now_v7();
        Ok(Some(RequestIdentity {
            credential_id: id,
            kind: if boot {
                CredentialKind::Boot
            } else {
                CredentialKind::Agent
            },
            agent_id: id,
            workspace_id: id,
            org_id: id,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            speaker_user_id: None,
            speaker_group_ids: Vec::new(),
            capabilities: if boot { vec!["browser".into()] } else { vec![] },
            byo: None,
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
    tls: Arc<ProxyTls>,
    /// The running proxy (FIX-086: refusal-log statistics).
    handle: Arc<Proxy>,
}

async fn start(allow: &[&str], header_timeout: Duration) -> Env {
    start_with(Arc::new(StaticResolver), allow, header_timeout).await
}

async fn start_with(
    resolver: Arc<dyn CredentialResolver>,
    allow: &[&str],
    header_timeout: Duration,
) -> Env {
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
    // 1 h leaves, renewed below 2 h: `tls.refresh()` always swaps (renewal tests).
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
        resolver,
        handler: Arc::new(PassThrough),
        guard,
        header_timeout,
        extra_ca_pem: Some(up_ca.cert_pem.clone().into_bytes()),
        egress_proxy: None,
    })
    .unwrap();
    tokio::spawn(p.clone().run(l, std::future::pending()));
    Env {
        proxy,
        https,
        http,
        gw_ca_pem: gw.cert_pem,
        up_ca_pem: up_ca.cert_pem,
        tls,
        handle: p,
    }
}

fn basic(user_token: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(user_token)
    )
}

async fn read_head(s: &mut TlsStream<TcpStream>) -> String {
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
    let mut s = crate::common::connect(proxy).await;
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

async fn connect_raw(
    proxy: SocketAddr,
    auth: Option<&str>,
    target: &str,
) -> (String, TlsStream<TcpStream>) {
    let mut s = crate::common::connect(proxy).await;
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
    mitm_after_connect(&env.gw_ca_pem, s, target, h2).await
}

/// MITM tunnel with an explicit trust anchor (a gateway other than `env`'s).
async fn mitm_tls(gw_ca_pem: &str, proxy: SocketAddr, target: SocketAddr, h2: bool) -> Sender {
    let (head, s) = connect_raw(proxy, Some("agent:aug_good"), &target.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    mitm_after_connect(gw_ca_pem, s, target, h2).await
}

async fn mitm_after_connect(
    gw_ca_pem: &str,
    s: TlsStream<TcpStream>,
    target: SocketAddr,
    h2: bool,
) -> Sender {
    let alpn: &[&[u8]] = if h2 { &[b"h2"] } else { &[b"http/1.1"] };
    let tls = tls_client(gw_ca_pem, alpn)
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

/// (FIX-086, CNT-8) Every refused CONNECT authentication is counted, but a flood from one address
/// writes a bounded number of warnings (one per source per interval).
#[tokio::test]
async fn refused_connect_authentications_are_logged_at_a_bounded_rate() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    let t = env.https.to_string();
    for _ in 0..50 {
        let r = raw(
            env.proxy,
            &format!("CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 407"), "{r}");
    }
    let (counted, logged) = env.handle.connect_refusal_stats();
    assert_eq!(counted, 50, "every refusal is counted");
    assert_eq!(
        logged, 1,
        "one log line for one source and reason within the interval"
    );
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
    let (head, s) = connect_raw(env.proxy, Some("open:aub_browser"), &env.https.to_string()).await;
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
    let (head, _) = connect_raw(env.proxy, Some("open:aub_browser"), &env.https.to_string()).await;
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

#[tokio::test]
async fn db_credentials_forwarded_and_revoked_gets_407() {
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool().clone();
    let f = seed(&pool).await;
    let resolver = Arc::new(aurora_gateway::resolver::DbCredentialResolver::new(
        pool.clone(),
    ));
    let hub = aurora_gateway::invalidation::Hub::new();
    hub.register(resolver.cache());
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(hub.clone().run(pool.clone(), rx));
    while !hub.is_connected() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let env = start_with(resolver, &["127.0.0.0/8"], Duration::from_secs(10)).await;
    let h = env.http.to_string();
    let req = format!(
        "GET http://{h}/plain HTTP/1.1\r\nHost: {h}\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n",
        basic(&format!("agent:{}", f.token))
    );
    assert!(raw(env.proxy, &req).await.starts_with("HTTP/1.1 200"));
    sqlx::query("UPDATE gateway_credentials SET revoked_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    aurora_db::bus::notify(
        &pool,
        aurora_db::bus::Channel::GatewayInvalidate,
        &serde_json::json!({"scope":"credential","id":f.credential_id}),
    )
    .await
    .unwrap();
    let mut got = String::new();
    for _ in 0..50 {
        got = raw(env.proxy, &req).await;
        if got.starts_with("HTTP/1.1 407") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(got.starts_with("HTTP/1.1 407"), "{got}");
    let _ = tx.send(true);
}

struct Seed {
    token: String,
    credential_id: Uuid,
}

async fn seed(pool: &sqlx::PgPool) -> Seed {
    let org = Uuid::now_v7();
    let ws = Uuid::now_v7();
    let agent = Uuid::now_v7();
    let user = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1,'u@x.io','U')")
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1,$2,'owner')")
        .bind(org)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','A','byo')")
        .bind(agent)
        .bind(org)
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    let token = "aug_e2etoken".to_owned();
    let cid = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id) VALUES ($1,'agent',$2,'aug_e2etoken',$3)")
        .bind(cid)
        .bind(aurora_core::token::hash(&token).to_vec())
        .bind(agent)
        .execute(pool)
        .await
        .unwrap();
    Seed {
        token,
        credential_id: cid,
    }
}

// ---- (phase 1, RFE-002, SEC-40) the proxy listener is TLS-only ----

struct CountingResolver(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl CredentialResolver for CountingResolver {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        StaticResolver.resolve(token).await
    }
}

/// A plaintext connection is closed at once: no response bytes, no credential read.
#[tokio::test]
async fn plaintext_connection_is_closed_without_reading_a_credential() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let env = start_with(
        Arc::new(CountingResolver(calls.clone())),
        &["127.0.0.0/8"],
        Duration::from_secs(5),
    )
    .await;
    for req in [
        format!(
            "CONNECT {}:443 HTTP/1.1\r\nHost: x:443\r\nProxy-Authorization: {}\r\n\r\n",
            env.https.ip(),
            basic("agent:aug_good")
        ),
        format!(
            "GET http://{}/ HTTP/1.1\r\nHost: x\r\nProxy-Authorization: {}\r\n\r\n",
            env.http,
            basic("agent:aug_good")
        ),
        "GET / HTTP/1.1\r\nHost: x\r\n\r\n".to_owned(),
        "X\u{3}\u{1}".to_owned(),
    ] {
        let mut s = TcpStream::connect(env.proxy).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let r = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        assert!(r.is_ok(), "the gateway must close a plaintext connection");
        assert!(out.is_empty(), "no response to plaintext: {out:?}");
    }
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no credential may be resolved from cleartext"
    );
}

#[tokio::test]
async fn tls_client_with_wrong_name_or_untrusted_ca_is_refused() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(5)).await;
    // Wrong name: the certificate does not cover it.
    let r = crate::common::connect_with(env.proxy, &env.gw_ca_pem, "not-the-gateway.example").await;
    assert!(r.is_err(), "wrong server name must fail verification");
    // Untrusted CA.
    let other = ca::generate("other").unwrap();
    let r = crate::common::connect_with(env.proxy, &other.cert_pem, "127.0.0.1").await;
    assert!(r.is_err(), "an untrusted CA must fail verification");
    // The right name still works, and `localhost` (a configured SAN) too.
    assert!(
        crate::common::connect_with(env.proxy, &env.gw_ca_pem, "127.0.0.1")
            .await
            .is_ok()
    );
    assert!(
        crate::common::connect_with(env.proxy, &env.gw_ca_pem, "localhost")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn alpn_is_http11_only_and_tls13_is_preferred() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(5)).await;
    let tcp = TcpStream::connect(env.proxy).await.unwrap();
    let c = tls_client(&env.gw_ca_pem, &[b"h2", b"http/1.1"]);
    let tls = c
        .connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
        .await
        .unwrap();
    let (_, conn) = tls.get_ref();
    assert_eq!(conn.alpn_protocol(), Some(&b"http/1.1"[..]));
    assert_eq!(
        conn.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    // h2-only clients cannot use this listener.
    let tcp = TcpStream::connect(env.proxy).await.unwrap();
    let c = tls_client(&env.gw_ca_pem, &[b"h2"]);
    assert!(
        c.connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
            .await
            .is_err()
    );
}

/// The CA-issued leaf is replaced in memory; tunnels already established keep working and new
/// connections verify against the same CA with the new certificate.
#[tokio::test]
async fn hot_renewal_does_not_drop_established_tunnels() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(5)).await;
    let before = {
        let s = crate::common::connect(env.proxy).await;
        s.get_ref().1.peer_certificates().unwrap()[0]
            .as_ref()
            .to_vec()
    };
    // An established CONNECT tunnel (inner TLS over the outer session), kept alive across the swap.
    let mut live = mitm(&env, env.https, false).await;
    let get = |env: &Env| {
        Request::get("/ok")
            .header("host", env.https.to_string())
            .body(Full::new(Bytes::new()))
            .unwrap()
    };
    assert_eq!(live.send(get(&env)).await.status(), 200);
    let generation = env.tls.generation();
    assert!(
        env.tls.refresh().unwrap(),
        "leaf below the renew threshold is replaced"
    );
    assert_eq!(env.tls.generation(), generation + 1);
    // The established session is unaffected.
    assert_eq!(live.send(get(&env)).await.status(), 200);
    // New connections get the new certificate and still verify.
    let s = crate::common::connect(env.proxy).await;
    let after = s.get_ref().1.peer_certificates().unwrap()[0]
        .as_ref()
        .to_vec();
    assert_ne!(before, after);
}

/// The inner MITM leg is unchanged: CONNECT through the TLS listener, then inner TLS.
#[tokio::test]
async fn connect_tunnel_carries_inner_tls_over_the_outer_session() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(5)).await;
    let mut sender = mitm(&env, env.https, false).await;
    let resp = sender
        .send(
            Request::get("/ok")
                .header("host", env.https.to_string())
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

// ---- (FIX-052, D-166, C-633) the upstream hop is RFC 9113-clean ------------
//
// The ordinary test upstreams above are hyper servers, which *accept* a `host`
// header beside `:authority` — that is why the forbidden pair shipped unnoticed.
// These tests use wire-strict oracles: a raw `h2::server::handshake` upstream
// (asserting exactly what reached the wire) and, for h1, the reflected header
// set of the existing handler.

/// A raw HTTP/2 server (rust `h2` behind the same TlsAcceptor, NOT hyper):
/// answers the first request with `200` "ok" and reports what reached the wire
/// (method, path, `:authority`, every regular header, body) on the returned
/// receiver once the request has been fully consumed. Errors are reported on
/// the same channel (no panics in the spawned task).
async fn raw_h2_oracle(
    cfg: Arc<rustls::ServerConfig>,
) -> (SocketAddr, tokio::sync::oneshot::Receiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<String>();
    tokio::spawn(async move {
        let mut seen_tx = Some(seen_tx);
        let out = async {
            let acc = TlsAcceptor::from(cfg);
            let (s, _) = l.accept().await.map_err(|e| format!("accept: {e}"))?;
            let tls = acc.accept(s).await.map_err(|e| format!("tls: {e}"))?;
            assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
            // The raw h2 server takes the TLS stream directly (tokio IO traits).
            let mut conn = h2::server::handshake(tls)
                .await
                .map_err(|e| format!("h2 handshake: {e}"))?;
            let (req, mut respond) = match conn.accept().await {
                Some(Ok(x)) => x,
                Some(Err(e)) => return Err(format!("h2 accept: {e}")),
                None => return Err("h2 accept: connection ended".to_owned()),
            };
            let (parts, mut body) = req.into_parts();
            let mut seen = format!(
                "method={}\npath={}\nauthority={}\n",
                parts.method,
                parts.uri.path(),
                parts
                    .uri
                    .authority()
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
            );
            for (k, v) in &parts.headers {
                seen.push_str(&format!("{}: {}\n", k, v.to_str().unwrap_or("?")));
            }
            while let Some(chunk) = body.data().await {
                let chunk = chunk.map_err(|e| format!("body: {e}"))?;
                let _ = body.flow_control().release_capacity(chunk.len());
                seen.push_str(&format!("body={}\n", String::from_utf8_lossy(&chunk)));
            }
            let resp = Response::builder().status(200).body(()).unwrap();
            let mut send = respond
                .send_response(resp, false)
                .map_err(|e| format!("send_response: {e}"))?;
            let _ = send.send_data(Bytes::from_static(b"ok\n"), true);
            let _ = seen_tx.take().unwrap().send(seen);
            // h2 is driven by polling the connection: keep it polled so the
            // response frames flush and the client reads them (until it goes
            // away or the guard expires).
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
            Ok::<(), String>(())
        }
        .await;
        if let Err(e) = out
            && let Some(tx) = seen_tx.take()
        {
            let _ = tx.send(format!("ORACLE-ERROR: {e}"));
        }
    });
    (addr, seen_rx)
}

/// POST relayed over TLS+ALPN h2 must reach the upstream as RFC 9113 §8.3.1
/// requires: `:authority` present, **no `host` header**, method/path/body intact.
#[tokio::test]
async fn h2_upstream_sees_authority_and_never_a_host_header() {
    // A dedicated raw-h2 upstream with its own CA; the gateway must trust it.
    let up_ca = ca::generate("up-h2").unwrap();
    let up = Ca::from_pem(&up_ca.cert_pem, &up_ca.key_pem).unwrap();
    let cfg = up.server_config("127.0.0.1").unwrap();
    let (target, seen) = raw_h2_oracle(cfg).await;
    // A gateway whose `extra_ca_pem` covers the h2 oracle's CA.
    let gw = ca::generate("gw-h2").unwrap();
    let gw_ca = Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        extra_ca_pem: Some(up_ca.cert_pem.clone().into_bytes()),
        egress_proxy: None,
    })
    .unwrap();
    tokio::spawn(p.run(l, std::future::pending()));

    // POST through the gateway; the inner leg negotiates h2 (h2-only ALPN).
    let mut c = mitm_tls(&gw.cert_pem, proxy, target, true).await;
    let (st, _, resp_body) = text(
        c.send(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("https://{target}/v1/messages"))
                .header("host", target.to_string())
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from_static(br#"{"model":"m"}"#)))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(st, 200, "{resp_body}");

    let seen = seen.await.unwrap();
    // `:authority` is the endpoint address (with the explicit default port, as
    // the gateway builds its URIs).
    assert!(
        seen.contains(&format!("authority={target}\n")),
        ":authority must carry the endpoint address: {seen}"
    );
    // No `host` header beside it (RFC 9113 §8.3.1) — the FIX-052 regression.
    assert!(
        !seen.to_lowercase().contains("\nhost:"),
        "no host header may reach an h2 upstream: {seen}"
    );
    assert!(seen.starts_with("method=POST\n"), "{seen}");
    assert!(seen.contains("path=/v1/messages\n"), "{seen}");
    assert!(
        seen.contains(&format!(
            "body={}\n",
            String::from_utf8_lossy(br#"{"model":"m"}"#)
        )),
        "body intact: {seen}"
    );
    assert!(
        seen.to_lowercase()
            .contains("content-type: application/json\n"),
        "regular headers relayed: {seen}"
    );
}

/// Plain-HTTP (h1) relay: `Host` **is** present (synthesized by hyper-util from
/// the request URI), including the non-default-port case (`host:port`). Uses an
/// absolute-form `http://` request on the proxy listener: the upstream hop is
/// then genuinely HTTP/1.1 (the TLS upstreams above negotiate h2 — they
/// advertise `h2` in ALPN).
#[tokio::test]
async fn h1_upstream_sees_synthesized_host_with_non_default_port() {
    let env = start(&["127.0.0.0/8"], Duration::from_secs(10)).await;
    // The plain-HTTP upstream listens on an ephemeral port (never 80), so the
    // synthesized Host must carry `host:port`; the handler reflects headers.
    let h = env.http.to_string();
    let r = raw(
        env.proxy,
        &format!(
            "GET http://{h}/host HTTP/1.1\r\nHost: {h}\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n",
            basic("agent:aug_good")
        ),
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(
        r.contains("version=HTTP/1.1"),
        "the upstream hop is h1: {r}"
    );
    let expected = format!("host: {h}");
    assert!(
        r.to_lowercase().contains(&expected),
        "synthesized Host with explicit port expected, got: {r}"
    );
    // Exactly one Host header.
    assert_eq!(
        r.to_lowercase().matches("\nhost:").count(),
        1,
        "exactly one Host header: {r}"
    );
}

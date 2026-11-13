#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (FIX-106, GWT-1/GWT-2) Inbound tunnel bounds: the inner handshake, the header read, the idle
//! limit of every tunnel kind and the concurrent-tunnel caps. The limits are shortened so each
//! test waits for the bound itself, never for a sleep.

use async_trait::async_trait;
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::guard::Guard;
use aurora_gateway::pipeline::PassThrough;
use aurora_gateway::proxy::{ByoParts, Proxy, ProxyParts, TunnelLimits};
use aurora_gateway::proxy_tls::ProxyTls;
use base64::Engine;
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsConnector;
use uuid::Uuid;

/// `aub_a` and `aub_b`: boot credentials with the `browser` capability, distinct credential ids.
struct Creds;

#[async_trait]
impl CredentialResolver for Creds {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        let n = match token {
            "aub_a" => 1,
            "aub_b" => 2,
            _ => return Ok(None),
        };
        let id = Uuid::from_u128(n);
        Ok(Some(RequestIdentity {
            credential_id: id,
            kind: CredentialKind::Boot,
            agent_id: id,
            workspace_id: id,
            org_id: id,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            speaker_user_id: None,
            speaker_group_ids: Vec::new(),
            capabilities: vec!["browser".to_owned()],
            byo: None,
        }))
    }
}

struct Env {
    proxy: SocketAddr,
    /// An upstream that accepts and then stays silent.
    up: SocketAddr,
    gw_pem: String,
}

async fn start(limits: TunnelLimits) -> Env {
    start_with(limits, None).await
}

async fn start_with(limits: TunnelLimits, egress_proxy: Option<String>) -> Env {
    let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = silent.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = silent.accept().await {
            // Reads and discards until the peer closes, then closes too.
            tokio::spawn(async move {
                let mut b = [0u8; 256];
                while matches!(s.read(&mut b).await, Ok(n) if n > 0) {}
            });
        }
    });
    let gw = ca::generate("gw-tunnel").unwrap();
    let gw_ca = Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = l.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let tls = ProxyTls::from_ca(gw_ca.clone(), vec!["127.0.0.1".into()]).unwrap();
    crate::common::trust_anchor(proxy, &gw.cert_pem);
    let p = Proxy::with_byo(
        ProxyParts {
            ca: gw_ca,
            tls,
            resolver: Arc::new(Creds),
            handler: Arc::new(PassThrough),
            guard,
            header_timeout: Duration::from_secs(5),
            extra_ca_pem: None,
            egress_proxy,
        },
        ByoParts {
            tunnel_limits: limits,
            ..ByoParts::default()
        },
    )
    .unwrap();
    tokio::spawn(p.run(l, std::future::pending()));
    Env {
        proxy,
        up,
        gw_pem: gw.cert_pem,
    }
}

fn limits() -> TunnelLimits {
    TunnelLimits {
        handshake: Duration::from_millis(300),
        header: Duration::from_millis(300),
        idle: Duration::from_millis(300),
        per_credential: 64,
        total: 1024,
    }
}

/// `CONNECT` to the silent upstream; returns the status and the live stream.
async fn connect(
    env: &Env,
    lane: &str,
    token: &str,
) -> (u16, tokio_rustls::client::TlsStream<tokio::net::TcpStream>) {
    let mut s = crate::common::connect(env.proxy).await;
    let t = env.up.to_string();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{lane}:{token}"));
    s.write_all(
        format!("CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert!(s.read(&mut b).await.unwrap() > 0, "eof in the CONNECT head");
        head.push(b[0]);
    }
    let status = String::from_utf8_lossy(&head)
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, s)
}

/// Waits (bounded) for the peer to close: EOF or a reset, never data.
async fn closed_within<S: tokio::io::AsyncRead + Unpin>(s: &mut S, what: &str) {
    let mut b = [0u8; 64];
    match tokio::time::timeout(Duration::from_secs(10), s.read(&mut b)).await {
        Err(_) => panic!("{what}: still open after 10 s"),
        Ok(Ok(n)) => assert_eq!(n, 0, "{what}: unexpected data"),
        Ok(Err(_)) => {}
    }
}

fn client(pem: &str) -> TlsConnector {
    let mut roots = RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(pem.as_bytes()) {
        roots.add(c.unwrap()).unwrap();
    }
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    TlsConnector::from(Arc::new(cfg))
}

#[tokio::test]
async fn a_tunnel_that_sends_no_client_hello_is_closed_at_the_handshake_bound() {
    let env = start(limits()).await;
    let (st, mut s) = connect(&env, "agent", "aub_a").await;
    assert_eq!(st, 200);
    closed_within(&mut s, "silent MITM tunnel").await;
}

#[tokio::test]
async fn a_partial_inner_request_head_is_closed_at_the_header_bound() {
    // Only the header bound can fire: the idle limit is long.
    let env = start(TunnelLimits {
        idle: Duration::from_secs(3600),
        ..limits()
    })
    .await;
    let (st, s) = connect(&env, "agent", "aub_a").await;
    assert_eq!(st, 200);
    let mut tls = client(&env.gw_pem)
        .connect(ServerName::IpAddress(env.up.ip().into()), s)
        .await
        .unwrap();
    tls.write_all(b"GET / HT").await.unwrap();
    closed_within(&mut tls, "partial header").await;
}

#[tokio::test]
async fn an_idle_mitm_tunnel_is_closed_at_the_idle_limit() {
    // The header bound is long, so only the idle limit can end this connection.
    let env = start(TunnelLimits {
        header: Duration::from_secs(3600),
        ..limits()
    })
    .await;
    let (st, s) = connect(&env, "agent", "aub_a").await;
    assert_eq!(st, 200);
    let mut tls = client(&env.gw_pem)
        .connect(ServerName::IpAddress(env.up.ip().into()), s)
        .await
        .unwrap();
    closed_within(&mut tls, "idle MITM tunnel").await;
}

#[tokio::test]
async fn an_idle_open_lane_tunnel_is_closed_at_the_idle_limit() {
    let env = start(limits()).await;
    let (st, mut s) = connect(&env, "open", "aub_a").await;
    assert_eq!(st, 200);
    closed_within(&mut s, "idle open-lane tunnel").await;
}

#[tokio::test]
async fn the_concurrent_tunnel_cap_is_per_credential_and_released_on_close() {
    let env = start(TunnelLimits {
        idle: Duration::from_secs(3600),
        handshake: Duration::from_secs(3600),
        header: Duration::from_secs(3600),
        per_credential: 2,
        ..limits()
    })
    .await;
    let (a, mut s1) = connect(&env, "open", "aub_a").await;
    let (b, _s2) = connect(&env, "open", "aub_a").await;
    assert_eq!((a, b), (200, 200));
    let (st, mut refused) = connect(&env, "open", "aub_a").await;
    assert_eq!(st, 429, "the third tunnel of one credential");
    let mut body = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), refused.read_to_end(&mut body)).await;
    assert!(
        String::from_utf8_lossy(&body).contains("concurrent tunnels"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    // Another credential has its own budget.
    let (st, _other) = connect(&env, "open", "aub_b").await;
    assert_eq!(st, 200);
    // Closing a tunnel frees its slot.
    s1.shutdown().await.unwrap();
    drop(s1);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (st, _s) = connect(&env, "open", "aub_a").await;
        if st == 200 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "slot never released"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn the_total_tunnel_cap_applies_across_credentials() {
    let env = start(TunnelLimits {
        idle: Duration::from_secs(3600),
        total: 1,
        ..limits()
    })
    .await;
    let (st, _s) = connect(&env, "open", "aub_a").await;
    assert_eq!(st, 200);
    let (st, _s) = connect(&env, "open", "aub_b").await;
    assert_eq!(st, 429);
}

/// (FIX-106, GWT-5) With `gateway.egress_proxy` set, a name is judged locally before the proxy is
/// contacted: a decimal spelling of the metadata address, and a name resolving to loopback, are
/// refused and the egress proxy sees no connection.
#[tokio::test]
async fn egress_proxy_mode_judges_names_before_contacting_the_proxy() {
    let egress = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let egress_addr = egress.local_addr().unwrap();
    let contacted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = contacted.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = egress.accept().await {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
        }
    });
    let env = start_with(limits(), Some(egress_addr.to_string())).await;
    for target in ["2852039166:443", "localhost:443", "10.1.2.3:443"] {
        let mut s = crate::common::connect(env.proxy).await;
        let auth = base64::engine::general_purpose::STANDARD.encode("open:aub_a");
        s.write_all(
            format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let mut buf = vec![0u8; 2048];
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let text = String::from_utf8_lossy(&buf[..n]).into_owned();
        assert!(text.starts_with("HTTP/1.1 403"), "{target}: {text}");
        assert!(text.contains("destination_refused"), "{target}: {text}");
    }
    assert_eq!(
        contacted.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the egress proxy was contacted for a refused name"
    );
    // Control: a public destination does reach the egress proxy.
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode("open:aub_a");
    s.write_all(
        format!(
            "CONNECT 93.184.216.34:443 HTTP/1.1\r\nHost: 93.184.216.34:443\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    assert_eq!(contacted.load(std::sync::atomic::Ordering::SeqCst), 1);
}

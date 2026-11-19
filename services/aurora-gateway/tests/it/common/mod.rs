#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Shared helpers: the proxy listener is TLS-only (docs/06 §Proxy listener TLS, RFE-002), so
//! every test client reaches it through a TLS session verified against the test gateway CA.

use aurora_gateway::ca::{Ca, GeneratedCa};
use aurora_gateway::proxy_tls::ProxyTls;
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

fn trust() -> &'static Mutex<HashMap<SocketAddr, String>> {
    static T: OnceLock<Mutex<HashMap<SocketAddr, String>>> = OnceLock::new();
    T.get_or_init(Mutex::default)
}

/// Records `ca_pem` as the trust anchor for clients of `proxy`.
pub fn trust_anchor(proxy: SocketAddr, ca_pem: &str) {
    trust().lock().unwrap().insert(proxy, ca_pem.to_owned());
}

/// The trust anchor recorded for `proxy` (the gateway CA certificate).
pub fn trust_pem(proxy: SocketAddr) -> String {
    trust().lock().unwrap().get(&proxy).cloned().unwrap()
}

/// The gateway CA and its proxy listener TLS (leaf for `127.0.0.1` and `localhost`); records the
/// CA as the trust anchor for clients of `proxy`.
pub fn gateway_tls(gw: &GeneratedCa, proxy: SocketAddr) -> (Arc<Ca>, Arc<ProxyTls>) {
    let ca = Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap());
    let tls = ProxyTls::from_ca(ca.clone(), vec!["127.0.0.1".into(), "localhost".into()]).unwrap();
    trust().lock().unwrap().insert(proxy, gw.cert_pem.clone());
    (ca, tls)
}

/// The boot every proxy test repeats (P5-DEDUP-1 T6): binds a loopback listener, a guard that
/// allows `127.0.0.0/8`, the gateway TLS ([`gateway_tls`]) and a 5 s header timeout. Returns the
/// listener, its address and the parts to hand to `Proxy::new`/`with_byo`.
pub async fn proxy_parts(
    gw: &GeneratedCa,
    resolver: Arc<dyn aurora_gateway::auth::CredentialResolver>,
    handler: Arc<dyn aurora_gateway::pipeline::RequestHandler>,
    extra_ca_pem: Option<Vec<u8>>,
) -> (
    tokio::net::TcpListener,
    SocketAddr,
    aurora_gateway::proxy::ProxyParts,
) {
    let pl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(
        aurora_gateway::guard::Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap(),
    );
    let (ca, tls) = gateway_tls(gw, proxy);
    let parts = aurora_gateway::proxy::ProxyParts {
        ca,
        tls,
        resolver,
        handler,
        guard,
        header_timeout: std::time::Duration::from_secs(5),
        extra_ca_pem,
        egress_proxy: None,
    };
    (pl, proxy, parts)
}

/// The identity most test resolvers answer (P5-DEDUP-1 T7): a nil credential id, no conversation,
/// turn, speaker, capabilities or BYO controls.
pub fn identity(
    kind: aurora_gateway::auth::CredentialKind,
    agent: uuid::Uuid,
    ws: uuid::Uuid,
    org: uuid::Uuid,
) -> aurora_gateway::auth::RequestIdentity {
    aurora_gateway::auth::RequestIdentity {
        credential_id: uuid::Uuid::nil(),
        kind,
        agent_id: agent,
        workspace_id: ws,
        org_id: org,
        conversation_id: None,
        turn_id: None,
        trace_context: None,
        context_label: None,
        speaker_user_id: None,
        speaker_group_ids: Vec::new(),
        capabilities: vec![],
        byo: None,
    }
}

/// [`proxy_parts`] +`Proxy::new` + a spawned `run`; returns the address and the proxy handle.
pub async fn spawn_proxy(
    gw: &GeneratedCa,
    resolver: Arc<dyn aurora_gateway::auth::CredentialResolver>,
    handler: Arc<dyn aurora_gateway::pipeline::RequestHandler>,
    extra_ca_pem: Option<Vec<u8>>,
) -> (SocketAddr, Arc<aurora_gateway::proxy::Proxy>) {
    let (pl, addr, parts) = proxy_parts(gw, resolver, handler, extra_ca_pem).await;
    let p = aurora_gateway::proxy::Proxy::new(parts).unwrap();
    tokio::spawn(p.clone().run(pl, std::future::pending()));
    (addr, p)
}

pub fn client_config(ca_pem: &str, alpn: &[&[u8]]) -> Arc<rustls::ClientConfig> {
    let mut roots = RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
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
    Arc::new(cfg)
}

/// TLS connection to the proxy listener at `proxy` (verified against its gateway CA).
pub async fn connect(proxy: SocketAddr) -> TlsStream<TcpStream> {
    let ca = trust().lock().unwrap().get(&proxy).cloned().unwrap();
    connect_with(proxy, &ca, "127.0.0.1").await.unwrap()
}

pub async fn connect_with(
    proxy: SocketAddr,
    ca_pem: &str,
    name: &str,
) -> std::io::Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(proxy).await?;
    TlsConnector::from(client_config(ca_pem, &[b"http/1.1"]))
        .connect(ServerName::try_from(name.to_owned()).unwrap(), tcp)
        .await
}

/// (FIX-177) Like [`connect`], with the client socket's send buffer set to `bytes`: a tight
/// buffer, as on a CI runner, makes the last write of a large body find the socket full.
pub async fn connect_sndbuf(proxy: SocketAddr, bytes: u32) -> TlsStream<TcpStream> {
    let ca = trust().lock().unwrap().get(&proxy).cloned().unwrap();
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.set_send_buffer_size(bytes).unwrap();
    let tcp = sock.connect(proxy).await.unwrap();
    TlsConnector::from(client_config(&ca, &[b"http/1.1"]))
        .connect(ServerName::try_from("127.0.0.1".to_owned()).unwrap(), tcp)
        .await
        .unwrap()
}

/// (FIX-177) Writes `bytes` to a TLS client stream **and flushes it**. tokio-rustls's
/// `write_all` can return while the last TLS records are still queued in the session (the socket
/// was full), and reading never sends them: a request written without a flush can stop short of
/// its `Content-Length`, and the gateway then rightly waits for the rest of the body while the
/// client waits for the answer. Returns whether records were still queued before the flush (a
/// diagnostic: the regression test needs to see that case).
pub async fn write_flushed<IO>(s: &mut TlsStream<IO>, bytes: &[u8]) -> bool
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    s.write_all(bytes).await.unwrap();
    let queued = s.get_ref().1.wants_write();
    s.flush().await.unwrap();
    queued
}

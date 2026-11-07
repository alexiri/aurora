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

/// The gateway CA and its proxy listener TLS (leaf for `127.0.0.1` and `localhost`); records the
/// CA as the trust anchor for clients of `proxy`.
pub fn gateway_tls(gw: &GeneratedCa, proxy: SocketAddr) -> (Arc<Ca>, Arc<ProxyTls>) {
    let ca = Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap());
    let tls = ProxyTls::from_ca(ca.clone(), vec!["127.0.0.1".into(), "localhost".into()]).unwrap();
    trust().lock().unwrap().insert(proxy, gw.cert_pem.clone());
    (ca, tls)
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

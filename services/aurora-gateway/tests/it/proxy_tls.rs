#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Proxy listener certificate sources (docs/06 §Proxy listener TLS, RFE-002): the CA-issued leaf
//! and the operator-supplied certificate (valid, expired, missing key, reload).

use aurora_gateway::ca::{self, Ca};
use aurora_gateway::proxy_tls::{ProxyTls, cert_names};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("aurora-proxy-tls-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self, f: &str) -> PathBuf {
        self.0.join(f)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An operator CA and a leaf for `names` valid `days` from now (negative: already expired);
/// writes `cert.pem` (leaf only) and `key.pem`, returns the CA PEM.
fn write_operator_cert(dir: &Dir, names: &[&str], days: i64) -> String {
    let ca_key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::default();
    p.distinguished_name = DistinguishedName::new();
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = p.self_signed(&ca_key).unwrap();
    let key = KeyPair::generate().unwrap();
    let mut lp =
        CertificateParams::new(names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>()).unwrap();
    lp.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let now = time::OffsetDateTime::now_utc();
    if days >= 0 {
        lp.not_before = now - time::Duration::hours(1);
        lp.not_after = now + time::Duration::days(days);
    } else {
        lp.not_before = now + time::Duration::days(days - 2);
        lp.not_after = now + time::Duration::days(days);
    }
    let leaf = lp.signed_by(&key, &ca, &ca_key).unwrap();
    std::fs::write(dir.path("cert.pem"), leaf.pem()).unwrap();
    std::fs::write(dir.path("key.pem"), key.serialize_pem()).unwrap();
    ca.pem()
}

/// Serves handshakes of `tls` on an ephemeral port that answer one byte and close.
async fn serve(tls: Arc<ProxyTls>) -> std::net::SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let acc = tls.acceptor();
            tokio::spawn(async move {
                if let Ok(mut t) = acc.accept(s).await {
                    let _ = t.write_all(b"k").await;
                    let _ = t.shutdown().await;
                }
            });
        }
    });
    addr
}

async fn handshake(addr: std::net::SocketAddr, ca: &str, name: &str) -> bool {
    match crate::common::connect_with(addr, ca, name).await {
        Ok(mut s) => {
            let mut b = [0u8; 1];
            s.read(&mut b).await.is_ok()
        }
        Err(_) => false,
    }
}

#[tokio::test]
async fn operator_certificate_replaces_the_ca_issued_leaf() {
    let d = Dir::new();
    let op_ca = write_operator_cert(&d, &["gw.public.example", "127.0.0.1"], 30);
    let tls = ProxyTls::operator(&d.path("cert.pem"), &d.path("key.pem")).unwrap();
    let addr = serve(tls).await;
    assert!(handshake(addr, &op_ca, "gw.public.example").await);
    assert!(handshake(addr, &op_ca, "127.0.0.1").await);
    // The gateway CA is not what signed it.
    let gw = ca::generate("gw").unwrap();
    assert!(!handshake(addr, &gw.cert_pem, "gw.public.example").await);
    // A name outside the certificate fails.
    assert!(!handshake(addr, &op_ca, "other.example").await);
}

#[test]
fn operator_certificate_startup_refusals_name_the_key() {
    let d = Dir::new();
    // expired
    write_operator_cert(&d, &["gw.example"], -5);
    let e = ProxyTls::operator(&d.path("cert.pem"), &d.path("key.pem"))
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains("gateway.proxy_tls.cert_file") && e.contains("expired"),
        "{e}"
    );
    // missing key file
    write_operator_cert(&d, &["gw.example"], 30);
    let e = ProxyTls::operator(&d.path("cert.pem"), &d.path("absent.pem"))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("gateway.proxy_tls.key_file"), "{e}");
    // missing certificate file
    let e = ProxyTls::operator(&d.path("absent.pem"), &d.path("key.pem"))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("gateway.proxy_tls.cert_file"), "{e}");
    // key that does not belong to the certificate
    let other = Dir::new();
    write_operator_cert(&other, &["gw.example"], 30);
    let e = ProxyTls::operator(&d.path("cert.pem"), &other.path("key.pem"))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("gateway.proxy_tls.key_file"), "{e}");
    // not PEM
    std::fs::write(d.path("junk.pem"), "junk").unwrap();
    assert!(ProxyTls::operator(&d.path("junk.pem"), &d.path("key.pem")).is_err());
}

#[tokio::test]
async fn operator_certificate_is_reloaded_when_the_files_change() {
    let d = Dir::new();
    let ca1 = write_operator_cert(&d, &["gw.example"], 30);
    let tls = ProxyTls::operator(&d.path("cert.pem"), &d.path("key.pem")).unwrap();
    let addr = serve(tls.clone()).await;
    assert!(handshake(addr, &ca1, "gw.example").await);
    assert!(!tls.refresh().unwrap(), "unchanged files are not reloaded");
    // mtime granularity: make the change observable
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let ca2 = write_operator_cert(&d, &["gw.example"], 60);
    assert!(tls.refresh().unwrap());
    assert!(handshake(addr, &ca2, "gw.example").await);
    assert!(!handshake(addr, &ca1, "gw.example").await);
    // A broken replacement keeps the previous certificate.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(d.path("cert.pem"), "garbage").unwrap();
    assert!(tls.refresh().is_err());
    assert!(handshake(addr, &ca2, "gw.example").await);
}

#[test]
fn ca_issued_leaf_is_an_unconstrained_leaf_of_the_gateway_ca_with_serverauth() {
    let g = ca::generate("t").unwrap();
    let ca = Arc::new(Ca::from_pem(&g.cert_pem, &g.key_pem).unwrap());
    let tls = ProxyTls::from_ca(
        ca,
        vec![
            "aurora-gateway".into(),
            "aurora-gateway.ns.svc".into(),
            "10.89.250.2".into(),
        ],
    )
    .unwrap();
    let chain = tls.current_chain();
    assert_eq!(chain.len(), 1, "a leaf of the CA, no intermediate");
    let (_, c) = x509_parser::parse_x509_certificate(&chain[0]).unwrap();
    assert!(!c.is_ca());
    let eku = c.extended_key_usage().unwrap().unwrap().value;
    assert!(eku.server_auth);
    let mut names = cert_names(&chain[0]);
    names.sort();
    assert_eq!(
        names,
        ["10.89.250.2", "aurora-gateway", "aurora-gateway.ns.svc"]
    );
    let _ = Path::new("");
}

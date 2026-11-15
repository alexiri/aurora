#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (FIX-106, D-178 Q-R5-7) The gateway's own TLS connection to a database server: `sslmode=require`
//! verifies the server certificate by default (system roots, or the stored `ca_pem`), and the
//! connection's explicit opt-out is the only way around it. The server here is a fake that
//! negotiates TLS with a certificate of a private CA and then refuses the login with a text of its
//! own, so "the handshake succeeded" is observable as a `Server` error and "verification failed"
//! as a `Tls` one — no password is ever sent in the failing cases.

use aurora_gateway::ca::{self, Ca};
use aurora_gateway::db::pg::server::{ConnectError, connect};
use aurora_gateway::db::pg::wire;
use aurora_gateway::db::{DbEndpoint, SslMode};
use aurora_gateway::guard::Guard;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use zeroize::Zeroizing;

/// A fake database server with a certificate of its own private CA; returns the port and the
/// CA certificate (PEM).
async fn fake_server() -> (u16, String) {
    let gen_ca = ca::generate("db-private-ca").unwrap();
    let ca = Ca::from_pem(&gen_ca.cert_pem, &gen_ca.key_pem).unwrap();
    let cfg = ca.server_config("127.0.0.1").unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = l.accept().await {
            let cfg = cfg.clone();
            tokio::spawn(async move {
                let mut req = [0u8; 8];
                if tcp.read_exact(&mut req).await.is_err() {
                    return;
                }
                tcp.write_all(b"S").await.unwrap();
                let Ok(mut tls) = TlsAcceptor::from(cfg).accept(tcp).await else {
                    return;
                };
                let mut startup = [0u8; 256];
                let _ = tls.read(&mut startup).await;
                let _ = tls
                    .write_all(&wire::error_response(
                        "FATAL",
                        "28P01",
                        "SERVER-TEXT password authentication failed",
                        None,
                    ))
                    .await;
            });
        }
    });
    (port, gen_ca.cert_pem)
}

fn endpoint(
    port: u16,
    sslmode: SslMode,
    ca_pem: Option<String>,
    verify_server: bool,
) -> DbEndpoint {
    DbEndpoint {
        host: "127.0.0.1".into(),
        port,
        dbname: "d".into(),
        user: "u".into(),
        password: Zeroizing::new("pw".into()),
        sslmode,
        ca_pem,
        verify_server,
    }
}

async fn outcome(ep: &DbEndpoint) -> Result<(), ConnectError> {
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![]).unwrap());
    connect(&guard, None, ep, None).await.map(|_| ())
}

fn handshake_completed(r: &Result<(), ConnectError>) -> bool {
    matches!(r, Err(ConnectError::Server(m)) if m.contains("SERVER-TEXT"))
}

fn verification_failed(r: &Result<(), ConnectError>) -> bool {
    matches!(r, Err(ConnectError::Tls(_)))
}

#[tokio::test]
async fn require_verifies_against_the_system_roots_by_default() {
    let (port, _ca) = fake_server().await;
    // A private CA is not a system root: refused before any password is sent.
    let r = outcome(&endpoint(port, SslMode::Require, None, true)).await;
    assert!(verification_failed(&r), "{r:?}");
}

#[tokio::test]
async fn require_with_a_stored_ca_verifies_against_it() {
    let (port, ca_pem) = fake_server().await;
    let r = outcome(&endpoint(port, SslMode::Require, Some(ca_pem), true)).await;
    assert!(handshake_completed(&r), "verify-ok: {r:?}");
    // The wrong CA stored: verify-fail, even with the opt-out set (a stored CA always wins).
    let other = ca::generate("some-other-ca").unwrap().cert_pem;
    let r = outcome(&endpoint(port, SslMode::Require, Some(other.clone()), true)).await;
    assert!(verification_failed(&r), "verify-fail: {r:?}");
    let r = outcome(&endpoint(port, SslMode::Require, Some(other), false)).await;
    assert!(
        verification_failed(&r),
        "a stored CA wins over the opt-out: {r:?}"
    );
}

#[tokio::test]
async fn the_explicit_opt_out_connects_without_verification() {
    let (port, _ca) = fake_server().await;
    let r = outcome(&endpoint(port, SslMode::Require, None, false)).await;
    assert!(handshake_completed(&r), "opt-out: {r:?}");
}

#[tokio::test]
async fn verify_full_and_verify_ca_ignore_the_opt_out() {
    let (port, ca_pem) = fake_server().await;
    for mode in [SslMode::VerifyFull, SslMode::VerifyCa] {
        let r = outcome(&endpoint(port, mode, None, false)).await;
        assert!(verification_failed(&r), "{mode:?} without a CA: {r:?}");
        let r = outcome(&endpoint(port, mode, Some(ca_pem.clone()), false)).await;
        assert!(handshake_completed(&r), "{mode:?} with the CA: {r:?}");
    }
}

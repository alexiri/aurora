#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (TEST-021, tier 2 — CI-only) The production topology that escaped FIX-052: a
//! strict front door between the gateway and its upstream. nginx (and, as a
//! second strict peer, haproxy) terminates TLS with `http2` on in front of a
//! mock upstream; the gateway relays an https request to the front door like it
//! would to the owner's real endpoint — and the bare-400 class of failure (an
//! RFC-forbidden header pair on the h2 hop) can no longer ship silently.
//!
//! Started only by `scripts/e2e/gateway.sh`, which launches the containers and
//! exports `AURORA_E2E_FRONTDOOR` (nginx) and `AURORA_E2E_FRONTDOOR_HAPROXY`
//! (haproxy). Without those variables — e.g. a plain local
//! `cargo test -p aurora-e2e-gateway` — the tests print `SKIPPED` and pass,
//! exactly like `AURORA_E2E_DB_TARGET` (`database.rs`). No container is ever
//! started from this file.
//!
//! The oracle is the wire truth of the whole chain: client → gateway → front
//! door (h2, ALPN) → mock upstream must answer 2xx, the app headers and the
//! injected credential must survive end to end, and the mock upstream must
//! never see `proxy-*` remnants.

mod common;

use common::*;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn frontdoor() -> Option<String> {
    match std::env::var("AURORA_E2E_FRONTDOOR") {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_owned()),
        _ => None,
    }
}

fn frontdoor_haproxy() -> Option<String> {
    match std::env::var("AURORA_E2E_FRONTDOOR_HAPROXY") {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_owned()),
        _ => None,
    }
}

/// The mock HTTPS upstream behind the front doors: a self-signed leaf for
/// `fd-mock.e2e.aurora.dev` + `127.0.0.2` (the name the front-door configs verify the upstream
/// leg by, and the address the gateway dials), reflecting the request it received (the app's
/// view of the wire) and recording every raw head it sees. Behind the doors
/// (`scripts/e2e/gateway.sh`) it serves the certificate the script generated (one truth for
/// both front-door legs: the doors verify it by name, the gateway by address) on the fixed
/// address the door configs proxy to; standalone it generates its own and binds an arbitrary
/// loopback port nobody dials. Returns (addr, trusted-CA PEM, observed raw heads).
async fn echo_upstream() -> (SocketAddr, String, Arc<Mutex<Vec<String>>>) {
    let (cert_der, key_der, ca_pem) = match (
        std::env::var("AURORA_E2E_FRONTDOOR_MOCK_CERT"),
        std::env::var("AURORA_E2E_FRONTDOOR_MOCK_KEY"),
    ) {
        (Ok(c), Ok(k)) if !c.trim().is_empty() && !k.trim().is_empty() => {
            use rustls::pki_types::pem::PemObject;
            let pem = std::fs::read_to_string(c.trim()).unwrap();
            let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(
                std::fs::read(k.trim()).unwrap().as_slice(),
            )
            .expect("the front-door mock key must be PEM (PKCS8/SEC1/PKCS1)");
            (
                rustls::pki_types::CertificateDer::pem_slice_iter(pem.as_bytes())
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                key,
                pem,
            )
        }
        _ => {
            let leaf = rcgen::generate_simple_self_signed(vec![
                "fd-mock.e2e.aurora.dev".to_owned(),
                "127.0.0.2".to_owned(),
            ])
            .unwrap();
            (
                vec![leaf.cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::try_from(leaf.key_pair.serialize_der())
                    .expect("rcgen serializes a PKCS8 key"),
                leaf.cert.pem(),
            )
        }
    };
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(cert_der, key_der)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let bind = match std::env::var("AURORA_E2E_FRONTDOOR_MOCK") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_owned(),
        _ => "127.0.0.1:0".to_owned(),
    };
    let l = tokio::net::TcpListener::bind(&bind).await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else {
                return;
            };
            let (acceptor, seen) = (acceptor.clone(), seen2.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let mut n = 0;
                while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => n += k,
                    }
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let echo = format!("frontdoor-echo\n{}", String::from_utf8_lossy(&buf[..n]));
                let body = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{echo}",
                    echo.len()
                );
                let _ = tls.write_all(body.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, ca_pem, seen)
}

/// The full-chain smoke: client → gateway (MITM) → front door (nginx/haproxy,
/// TLS + h2) → mock upstream. The gateway's relayed wire must satisfy a strict
/// front door (2xx, never the bare 400 class), and the upstream must observe a
/// clean, credential-injected request.
async fn frontdoor_smoke(fd: &str, label: &str) {
    let (_up, up_ca, seen) = echo_upstream().await;
    let st = Stack::start(Opts {
        extra_ca_bundle: Some(up_ca),
        ..Opts::default()
    })
    .await;
    let api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("e2e-frontdoor").await;
    let sid = api
        .ws_secret(json!({
            "name": "frontdoor-api", "kind": "generic", "host_pattern": fd,
            "injection": {"kind": "bearer"}, "value": "frontdoor-secret"
        }))
        .await;
    api.grant(&agent, &sid).await;
    api.set_policy("block", json!([])).await;

    let r = via_proxy(&st.gw_a, &token, &st.ca_pem)
        .get(format!("https://{fd}/fd-ok"))
        .header("x-frontdoor-probe", label)
        .send()
        .await
        .unwrap_or_else(|e| {
            panic!(
                "[{label}] the full chain must answer: {e}\n{}",
                st.log_tails()
            )
        });
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    assert!(
        status.is_success(),
        "[{label}] {status} through the front-door chain (a strict front door 4xx/5xx is the \
         FIX-052 failure class):\n{body}\n{}",
        st.log_tails()
    );
    assert!(
        body.contains("frontdoor-echo"),
        "[{label}] the mock upstream must have answered: {body}"
    );
    // Wire truth at the last hop (the mock echoes the request it received).
    let h = body.to_ascii_lowercase();
    assert!(
        h.contains("x-frontdoor-probe:"),
        "[{label}] end-to-end headers survive the chain:\n{body}"
    );
    assert!(
        h.contains("authorization: bearer frontdoor-secret"),
        "[{label}] the gateway must have injected the credential:\n{body}"
    );
    assert!(
        !h.contains("proxy-authorization") && !h.contains("proxy-connection"),
        "[{label}] proxy credentials must not reach the upstream:\n{body}"
    );
    assert!(
        h.contains("host:"),
        "[{label}] the h1 upstream leg always carries Host:\n{body}"
    );
    // The front door dialled the mock at most once for this request.
    let heads = seen.lock().unwrap().clone();
    assert!(
        heads
            .iter()
            .all(|r| !r.to_ascii_lowercase().contains("proxy-authorization")),
        "[{label}] raw upstream view must carry no proxy credentials: {heads:?}"
    );
    st.finish().await;
}

#[tokio::test]
async fn nginx_front_door_relays_the_gateway_wire_to_the_upstream() {
    let Some(addr) = frontdoor() else {
        eprintln!(
            "SKIPPED: AURORA_E2E_FRONTDOOR is not set; run scripts/e2e/gateway.sh (starts the front-door containers)"
        );
        return;
    };
    frontdoor_smoke(&addr, "nginx").await;
}

#[tokio::test]
async fn haproxy_front_door_relays_the_gateway_wire_to_the_upstream() {
    let Some(addr) = frontdoor_haproxy() else {
        eprintln!(
            "SKIPPED: AURORA_E2E_FRONTDOOR_HAPROXY is not set; run scripts/e2e/gateway.sh with FRONTDOOR=1"
        );
        return;
    };
    frontdoor_smoke(&addr, "haproxy").await;
}

/// The snapshot JSON shape the smoke relies on stays compile-checked even when
/// the scenario is skipped (no unused-import churn).
#[allow(dead_code)]
fn unused_shape_probe(v: &serde_json::Value) -> bool {
    v["error"]["code"].is_string()
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The loopback PostgreSQL relay (D-89): it answers the gateway's password request itself, the
//! client never sees an authentication request, TLS is verified (with the connection's own name)
//! before any credential is written, and a cancel request is forwarded over TLS.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::common::tls_fixture;
use aurora_supervisor::pgrelay::{DbTurns, spawn_db_relay};
use aurora_supervisor::proxy::GatewayUpstream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

const LO: &str = "127.0.0.1:0";

fn startup(user: &str) -> Vec<u8> {
    let mut b = 196_608u32.to_be_bytes().to_vec();
    b.extend_from_slice(format!("user\0{user}\0database\0x\0\0").as_bytes());
    let mut m = ((b.len() + 4) as u32).to_be_bytes().to_vec();
    m.extend_from_slice(&b);
    m
}

#[derive(Default, Debug)]
struct Seen {
    /// Frames the fake gateway read after TLS: (type or 0 for untyped, payload).
    startup: Vec<u8>,
    password: Option<String>,
    cancel: Option<Vec<u8>>,
    plaintext_after_ssl_request: usize,
}

/// A fake gateway: `SSLRequest` -> `S` -> TLS (`acceptor`), then either the login exchange
/// (cleartext password request, `AuthenticationOk`, `ReadyForQuery`) or a cancel packet.
async fn fake_gateway(acceptor: tokio_rustls::TlsAcceptor) -> (SocketAddr, Arc<Mutex<Seen>>) {
    let l = TcpListener::bind(LO).await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut tcp, _) = l.accept().await.unwrap();
            let (acceptor, seen) = (acceptor.clone(), s2.clone());
            tokio::spawn(async move {
                let mut ssl = [0u8; 8];
                if tcp.read_exact(&mut ssl).await.is_err() || ssl[4..] != [4, 210, 22, 47] {
                    return;
                }
                tcp.write_all(b"S").await.unwrap();
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    // Verification failed on the client side: it must not have sent anything.
                    return;
                };
                let mut len = [0u8; 4];
                if tls.read_exact(&mut len).await.is_err() {
                    return;
                }
                let n = u32::from_be_bytes(len) as usize;
                let mut body = vec![0u8; n - 4];
                tls.read_exact(&mut body).await.unwrap();
                if body[..4] == 80_877_102u32.to_be_bytes() {
                    seen.lock().unwrap().cancel = Some(body);
                    return;
                }
                seen.lock().unwrap().startup = body;
                tls.write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 3])
                    .await
                    .unwrap();
                let mut h = [0u8; 5];
                tls.read_exact(&mut h).await.unwrap();
                assert_eq!(h[0], b'p');
                let pn = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
                let mut pw = vec![0u8; pn - 4];
                tls.read_exact(&mut pw).await.unwrap();
                seen.lock().unwrap().password =
                    Some(String::from_utf8(pw[..pw.len() - 1].to_vec()).unwrap());
                tls.write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0])
                    .await
                    .unwrap();
                tls.write_all(&[b'Z', 0, 0, 0, 5, b'I']).await.unwrap();
                // Echo whatever the client sends next (proves the transparent phase).
                let mut buf = [0u8; 64];
                if let Ok(k) = tls.read(&mut buf).await {
                    let _ = tls.write_all(&buf[..k]).await;
                }
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, seen)
}

async fn read_n(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    tokio::time::timeout(std::time::Duration::from_secs(10), s.read_exact(&mut b))
        .await
        .unwrap()
        .unwrap();
    b
}

#[tokio::test]
async fn relay_answers_the_password_itself_and_the_client_never_sees_an_auth_request() {
    let conn = Uuid::now_v7();
    let name = format!("{conn}.db.aurora.internal");
    // The leaf holds ONLY the connection's name: the relay must verify exactly that name.
    let (ca, acceptor) = tls_fixture(&[&name]);
    let (gw, seen) = fake_gateway(acceptor).await;
    let up = GatewayUpstream::new(gw.to_string(), &ca, "gateway.invalid").unwrap();
    let turns = DbTurns::new("aub_boot");
    let relay = spawn_db_relay(LO.parse().unwrap(), up, turns.clone())
        .await
        .unwrap();

    let mut c = TcpStream::connect(relay.addr).await.unwrap();
    // psql tries GSS and SSL first; both are declined on the loopback leg.
    c.write_all(&[0, 0, 0, 8, 4, 210, 22, 48]).await.unwrap();
    assert_eq!(read_n(&mut c, 1).await, b"N");
    c.write_all(&[0, 0, 0, 8, 4, 210, 22, 47]).await.unwrap();
    assert_eq!(read_n(&mut c, 1).await, b"N");
    c.write_all(&startup(&conn.to_string())).await.unwrap();
    // The first thing the client sees is AuthenticationOk (code 0), never code 3.
    let r = read_n(&mut c, 9).await;
    assert_eq!(r, [b'R', 0, 0, 0, 8, 0, 0, 0, 0], "{r:?}");
    assert_eq!(read_n(&mut c, 6).await, [b'Z', 0, 0, 0, 5, b'I']);
    c.write_all(b"ping").await.unwrap();
    assert_eq!(read_n(&mut c, 4).await, b"ping");
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.password.as_deref(), Some("aub_boot"));
        assert!(
            String::from_utf8_lossy(&s.startup).contains(&format!("user\0{conn}\0")),
            "startup forwarded with user = connection id"
        );
        assert_eq!(s.plaintext_after_ssl_request, 0);
    }

    // During exactly one turn the turn context credential is used (audit attribution).
    turns.begin(Uuid::now_v7(), "auc_turn".into());
    let mut c = TcpStream::connect(relay.addr).await.unwrap();
    c.write_all(&startup(&conn.to_string())).await.unwrap();
    let _ = read_n(&mut c, 15).await;
    assert_eq!(seen.lock().unwrap().password.as_deref(), Some("auc_turn"));
}

#[tokio::test]
async fn tls_verification_failure_sends_no_credential() {
    let conn = Uuid::now_v7();
    // A certificate for the wrong name, signed by a CA the relay does not trust.
    let (_other_ca, acceptor) = tls_fixture(&[&format!("{conn}.db.aurora.internal")]);
    let (trusted_ca, _) = tls_fixture(&["localhost"]);
    let (gw, seen) = fake_gateway(acceptor).await;
    let up = GatewayUpstream::new(gw.to_string(), &trusted_ca, "localhost").unwrap();
    let relay = spawn_db_relay(LO.parse().unwrap(), up, DbTurns::new("aub_boot"))
        .await
        .unwrap();
    let mut c = TcpStream::connect(relay.addr).await.unwrap();
    c.write_all(&startup(&conn.to_string())).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), c.read_to_end(&mut out)).await;
    assert_eq!(out[0], b'E', "{out:?}");
    assert!(String::from_utf8_lossy(&out).contains("no credential was sent"));
    let s = seen.lock().unwrap();
    assert!(s.password.is_none() && s.startup.is_empty(), "{s:?}");
}

#[tokio::test]
async fn right_ca_wrong_name_sends_no_credential() {
    let conn = Uuid::now_v7();
    let (ca, acceptor) = tls_fixture(&["some-other-name.example"]);
    let (gw, seen) = fake_gateway(acceptor).await;
    let up = GatewayUpstream::new(gw.to_string(), &ca, "some-other-name.example").unwrap();
    let relay = spawn_db_relay(LO.parse().unwrap(), up, DbTurns::new("aub_boot"))
        .await
        .unwrap();
    let mut c = TcpStream::connect(relay.addr).await.unwrap();
    c.write_all(&startup(&conn.to_string())).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), c.read_to_end(&mut out)).await;
    assert_eq!(out[0], b'E', "{out:?}");
    assert!(seen.lock().unwrap().password.is_none());
}

#[tokio::test]
async fn a_user_that_is_not_a_connection_id_is_refused_locally() {
    let (ca, acceptor) = tls_fixture(&["localhost"]);
    let (gw, seen) = fake_gateway(acceptor).await;
    let up = GatewayUpstream::new(gw.to_string(), &ca, "localhost").unwrap();
    let relay = spawn_db_relay(LO.parse().unwrap(), up, DbTurns::new("aub_boot"))
        .await
        .unwrap();
    let mut c = TcpStream::connect(relay.addr).await.unwrap();
    c.write_all(&startup("postgres")).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), c.read_to_end(&mut out)).await;
    assert_eq!(out[0], b'E');
    assert!(seen.lock().unwrap().startup.is_empty());
}

#[tokio::test]
async fn cancel_request_is_forwarded_over_verified_tls() {
    let (ca, acceptor) = tls_fixture(&["localhost"]);
    let (gw, seen) = fake_gateway(acceptor).await;
    let up = GatewayUpstream::new(gw.to_string(), &ca, "localhost").unwrap();
    let relay = spawn_db_relay(LO.parse().unwrap(), up, DbTurns::new("aub_boot"))
        .await
        .unwrap();
    let mut c = TcpStream::connect(relay.addr).await.unwrap();
    let mut pkt = vec![0, 0, 0, 16];
    pkt.extend_from_slice(&80_877_102u32.to_be_bytes());
    pkt.extend_from_slice(&7u32.to_be_bytes());
    pkt.extend_from_slice(&9u32.to_be_bytes());
    c.write_all(&pkt).await.unwrap();
    for _ in 0..50 {
        if seen.lock().unwrap().cancel.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let got = seen
        .lock()
        .unwrap()
        .cancel
        .clone()
        .expect("cancel forwarded");
    assert_eq!(&got[4..], [0, 0, 0, 7, 0, 0, 0, 9]);
}

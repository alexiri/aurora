#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SEC-09: the internal listener binds only its own address, distinct from the proxy's.
use aurora_core::service;
use std::net::SocketAddr;
use tokio::net::TcpStream;

#[tokio::test]
async fn internal_and_proxy_listeners_are_address_separated() {
    let proxy = service::bind("127.0.0.2", 0).await.unwrap();
    let port = proxy.local_addr().unwrap().port();
    let internal = service::bind("127.0.0.3", port).await.unwrap();
    assert_eq!(proxy.local_addr().unwrap().ip().to_string(), "127.0.0.2");
    assert_eq!(internal.local_addr().unwrap().ip().to_string(), "127.0.0.3");

    let a = SocketAddr::new("127.0.0.2".parse().unwrap(), port);
    let b = SocketAddr::new("127.0.0.3".parse().unwrap(), port);
    let (c1, s1) = tokio::join!(TcpStream::connect(a), proxy.accept());
    let (c2, s2) = tokio::join!(TcpStream::connect(b), internal.accept());
    assert_eq!(c1.unwrap().peer_addr().unwrap(), a);
    assert_eq!(s1.unwrap().0.local_addr().unwrap(), a);
    assert_eq!(c2.unwrap().peer_addr().unwrap(), b);
    assert_eq!(s2.unwrap().0.local_addr().unwrap(), b);
    // A third loopback address reaches neither listener.
    let other = SocketAddr::new("127.0.0.4".parse().unwrap(), port);
    assert!(TcpStream::connect(other).await.is_err());
}

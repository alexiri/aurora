//! Minimal listener sanity tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[tokio::test]
async fn listener_accepts() {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = l.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        aurora_relay::edge::serve_listener(
            l,
            aurora_relay::state::State::new(
                std::sync::Arc::new(
                    aurora_relay::api::RelayApi::new("http://127.0.0.1:1", "aul_x").unwrap(),
                ),
                "previews.example.org".into(),
                "https://aurora.example.org".into(),
                45,
            ),
            aurora_relay::stream::Streams::new(),
            64,
            1 << 20,
            stop_rx,
        )
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let t = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
    assert!(t.is_ok(), "connect failed: {t:?}");
    drop(t);
    let _ = stop_tx.send(true);
}

#[tokio::test]
async fn fake_app_answers() {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(super_app(l));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut t = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    t.write_all(b"GET /echo HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(3), t.read_to_end(&mut buf))
        .await
        .expect("no answer within 3 s")
        .unwrap();
    assert!(!buf.is_empty(), "empty answer");
}

// The same fake app the edge tests use.
async fn super_app(listener: tokio::net::TcpListener) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(
                |req: hyper::Request<hyper::body::Incoming>| async move {
                    let p = req.uri().path().to_owned();
                    let resp: hyper::Response<http_body_util::Full<bytes::Bytes>> =
                        hyper::Response::builder()
                            .status(200)
                            .body(http_body_util::Full::new(bytes::Bytes::from(format!(
                                "path={p}"
                            ))))
                            .unwrap();
                    Ok::<_, std::convert::Infallible>(resp)
                },
            );
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
                .await;
        });
    }
}

#[tokio::test]
async fn listener_survives_probe_and_second_connect() {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = l.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        aurora_relay::edge::serve_listener(
            l,
            aurora_relay::state::State::new(
                std::sync::Arc::new(
                    aurora_relay::api::RelayApi::new("http://127.0.0.1:1", "aul_x").unwrap(),
                ),
                "previews.example.org".into(),
                "https://aurora.example.org".into(),
                45,
            ),
            aurora_relay::stream::Streams::new(),
            64,
            1 << 20,
            stop_rx,
        )
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // Two probe connects (like the rig wait), then a real one after a beat.
    drop(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drop(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let t = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
    assert!(t.is_ok(), "third connect failed: {t:?}");
    let _ = stop_tx.send(true);
}

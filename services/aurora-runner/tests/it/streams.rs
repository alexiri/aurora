//! (P4-PRV-2, `16 §W18` leg 1) The relay-facing stream listener against a real WebSocket
//! client — exactly what the relay does: `connect_async` with `Authorization: Bearer aux_…`.
//! Regression: the listener consumes the upgrade request to read the bearer, so it must
//! complete the server handshake itself; `accept_async` would wait for the request a second
//! time and hang the leg until the relay's dial timeout (the 504 of the kind runs).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use aurora_runner::api::{ApiClient, RetryPolicy};
use aurora_runner::channel::{ChannelServer, ChannelSettings};
use aurora_runner::hub::ChannelHub as _;
use aurora_runner::stream_listener::{CLOSE_NORMAL, StreamListener};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{Message, handshake::client::Request as WsRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

const KEY: [u8; 32] = [11; 32];

/// The runner half: channel server + stream listener, both served on private listeners.
/// The returned guards keep both accept loops alive for the rig's lifetime.
#[allow(clippy::type_complexity)]
async fn rig(
    api: &MockServer,
) -> (
    ChannelServer,
    std::net::SocketAddr,
    std::net::SocketAddr,
    watch::Sender<bool>,
    watch::Sender<bool>,
) {
    let server = ChannelServer::new(ChannelSettings::new(KEY));
    let retry = RetryPolicy {
        attempts: 1,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(10),
    };
    server.attach_api(Arc::new(
        ApiClient::new(&api.uri(), "aur_test", retry).unwrap(),
    ));
    let streams = StreamListener::new(server.clone());
    server.set_streams(streams.clone());

    let ch = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ch_addr = ch.local_addr().unwrap();
    let (stop_tx, stop_rx) = watch::channel(false);
    let s = server.clone();
    tokio::spawn(async move { s.serve(ch, stop_rx).await });

    let st = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let st_addr = st.local_addr().unwrap();
    let (stop_tx2, stop_rx2) = watch::channel(false);
    let l = streams.clone();
    let api_w = server.api_watch();
    tokio::spawn(async move {
        aurora_runner::stream_listener::serve(st, l, api_w, None, stop_rx2).await
    });

    // Previews on: the accept loop opens only after this (`previews.enabled` at registration).
    server.set_previews_enabled(true);
    (server, ch_addr, st_addr, stop_tx, stop_tx2)
}

/// The fake supervisor: the W6 client a boot runs. Admits one channel and answers the
/// stream frames the listener sends (this is leg 2 of the round trip under test).
async fn connect_supervisor(addr: std::net::SocketAddr, sandbox: Uuid, boot: Uuid) -> Client {
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/supervisor"))
        .await
        .unwrap();
    ws.send(Message::text(
        json!({"hello": {"protocol": 1, "bootstrap_token": "aup_secret"}}).to_string(),
    ))
    .await
    .unwrap();
    // Welcome (the claim mock names the session) — drain frames until it arrives.
    let _ = next_text(&mut ws).await;
    let _ = (sandbox, boot);
    ws
}

/// Next text frame (skips pings and acks).
async fn next_text(ws: &mut Client) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("frame timed out")
        {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["kind"] != "ack" {
                    return v;
                }
            }
            Some(Ok(Message::Close(c))) => {
                return json!({"close": c.map_or(0, |c| u16::from(c.code))});
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return json!({"close": "dropped"}),
        }
    }
}

fn mount_runner_api(api: &MockServer, sandbox: Uuid, boot: Uuid) {
    // Wiremock mounts are async; the `mock` feature of tokio makes `mount` sync-blockable
    // through `block_on` — the channel IT mounts before its clients connect, same shape.
    async fn mounts(api: &MockServer, sandbox: Uuid, boot: Uuid) {
        Mock::given(method("POST"))
            .and(path("/v1/runner/channel/claim"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"sandbox_id": sandbox, "boot_id": boot})),
            )
            .up_to_n_times(1)
            .mount(api)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/runner/channel/hold"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(api)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/runner/preview-streams/open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sandbox_id": sandbox, "boot_id": boot, "port": 3000,
                "share_id": Uuid::now_v7(), "window": 65536,
                "idle_secs": 600, "max_lifetime_secs": 14400
            })))
            .mount(api)
            .await;
    }
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(mounts(api, sandbox, boot))
    });
}

/// Leg 1 end to end: the relay's client handshake is answered 101 only after the supervisor
/// opened the stream; binary bytes flow both ways; the leg closes 1000 on EOF.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leg1_handshake_completes_after_stream_open_and_echoes_bytes() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot);
    let (_server, ch_addr, st_addr, _stop, _stop2) = rig(&api).await;

    // The supervisor connects first (the channel must be live for `holds_channel`).
    let mut sup = connect_supervisor(ch_addr, sandbox, boot).await;

    // The relay's dial runs concurrently: the runner only sends `stream.open` (leg 2) once
    // the relay's request arrives, and answers 101 only after `stream.opened` — so the test
    // interleaves: dial in the background, answer the supervisor, then await the dial.
    let dial = tokio::spawn({
        async move {
            let mut req: WsRequest = format!("ws://{st_addr}/v1/preview-stream")
                .into_client_request()
                .unwrap();
            req.headers_mut()
                .insert(AUTHORIZATION, "Bearer aux_testgrant".parse().unwrap());
            tokio::time::timeout(Duration::from_secs(10), connect_async(req))
                .await
                .expect("relay dial timed out (101 never completed)")
                .expect("handshake failed")
        }
    });

    // The listener forwarded `stream.open` on the channel; answer `opened` on the
    // unsequenced side and the dial completes with the 101.
    let opened = next_text(&mut sup).await;
    assert_eq!(opened["kind"], "stream.open", "{opened}");
    let sid = opened["body"]["stream_id"].as_u64().unwrap() as u32;
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "stream.opened",
               "body": {"stream_id": sid, "window": 65536}})
        .to_string(),
    ))
    .await
    .unwrap();

    let (mut relay, resp) = dial.await.unwrap();
    assert_eq!(
        resp.status(),
        101,
        "upgrade must complete after stream.open"
    );

    // Relay → supervisor: bytes arrive as binary data frames. Keepalive Pings (and stray
    // Pongs) are legal anywhere on the channel — the runner's conn loop pings from the
    // moment it is admitted (the interval's first tick is immediate), so one can land
    // right here under load — and tungstenite auto-pongs on read: skip control frames
    // and keep waiting. The 5s timeout bounds the WHOLE wait, not each frame.
    relay
        .send(Message::binary(b"ping-through".to_vec()))
        .await
        .unwrap();
    let d = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match sup.next().await {
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Binary(b))) => break b,
                Some(Ok(other)) => panic!("expected binary data, got {other:?}"),
                Some(Err(e)) => panic!("ws error waiting for data: {e}"),
                None => panic!("channel closed while waiting for data"),
            }
        }
    })
    .await
    .expect("data timed out");
    assert_eq!(&d[0], &0x01);
    assert_eq!(&d[1..5], &sid.to_be_bytes());
    assert_eq!(&d[5..], b"ping-through");

    // Supervisor → relay: echo with credit (the runner only forwards credited bytes).
    let mut echo = vec![0x01];
    echo.extend_from_slice(&sid.to_be_bytes());
    echo.extend_from_slice(b"pong-back");
    sup.send(Message::binary(echo)).await.unwrap();
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "stream.credit",
               "body": {"stream_id": sid, "bytes": 9u64}})
        .to_string(),
    ))
    .await
    .unwrap();
    let back = loop {
        let m = tokio::time::timeout(Duration::from_secs(5), relay.next())
            .await
            .expect("echo timed out")
            .unwrap()
            .unwrap();
        // Pings (and pongs) ride the leg while we wait; only binary is app data.
        if matches!(m, Message::Binary(_)) {
            break m;
        }
    };
    assert_eq!(back, Message::binary(b"pong-back".to_vec()), "{back:?}");

    // A second answer sent *immediately before* the close exercises the data/close race:
    // the W6 delivers the data first, but `serve_leg`'s select may see the close first —
    // the buffered answer must still be forwarded before the leg closes. It arrives as
    // binary before the close (asserted below alongside the close code).
    let mut tail = vec![0x01];
    tail.extend_from_slice(&sid.to_be_bytes());
    tail.extend_from_slice(b"tail-before-close");
    sup.send(Message::binary(tail)).await.unwrap();
    // The supervisor ends the stream (`stream.close` with `eof`): the leg closes normal
    // (1000) — the mapped code the relay's edge turns into the app's answer.
    sup.send(Message::text(
        json!({"seq": 0, "ack": 0, "kind": "stream.close",
               "body": {"stream_id": sid, "reason": "eof",
                         "bytes_in": 12u64, "bytes_out": 9u64}})
        .to_string(),
    ))
    .await
    .unwrap();
    let end = loop {
        let m = tokio::time::timeout(Duration::from_secs(5), relay.next())
            .await
            .expect("close timed out")
            .unwrap()
            .unwrap();
        // The buffered answer forwarded during the close drain (the race under test) and
        // the runner's first-tick pings ride the leg before the close frame.
        match &m {
            Message::Binary(_) => {
                assert_eq!(m, Message::binary(b"tail-before-close".to_vec()), "{m:?}");
                continue;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => {}
        }
        break m;
    };
    let Message::Close(c) = end else {
        panic!("expected close, got {end:?}");
    };
    assert_eq!(c.map_or(0, |c| u16::from(c.code)), CLOSE_NORMAL);
}

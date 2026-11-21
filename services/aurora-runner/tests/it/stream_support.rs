#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Helpers the files and ssh stream tests share (P5-DEDUP-1 T5): the bearer dial of the stream
//! listener, the fake supervisor's message readers and the W6 frame builders.

use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{Error as WsError, Message, handshake::client::Request};

use crate::terminal_streams::Client;

/// The API's keep-awake route, exactly (C-620, C-796, C-1058). The tests compare the whole
/// path: matching on its tail let the runner post `/v1/runner/runner/terminal/keep-awake` (a
/// 404) for a whole phase without a test noticing (P5-REFINE-20 batch 5).
pub(crate) const KEEP_AWAKE: &str = "/v1/runner/terminal/keep-awake";

pub(crate) async fn dial(st_addr: std::net::SocketAddr, token: &str) -> Result<Client, WsError> {
    let mut req: Request = format!("ws://{st_addr}/v1/preview-stream")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    match tokio::time::timeout(Duration::from_secs(10), connect_async(req)).await {
        Ok(r) => r.map(|(ws, _)| ws),
        Err(_) => panic!("dial timed out"),
    }
}

/// The next non-keepalive W6 message the fake supervisor receives (text or binary).
pub(crate) async fn next_msg(sup: &mut Client) -> Message {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), sup.next())
            .await
            .expect("supervisor message timed out")
        {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(Message::Text(t))) if t.contains(r#""kind":"ack""#) => {}
            Some(Ok(m)) => return m,
            other => panic!("supervisor stream ended: {other:?}"),
        }
    }
}

/// `true` when the supervisor receives nothing (but keepalives) for `d`.
pub(crate) async fn silent(sup: &mut Client, d: Duration) -> bool {
    let end = tokio::time::Instant::now() + d;
    loop {
        let left = end.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return true;
        }
        match tokio::time::timeout(left, sup.next()).await {
            Err(_) => return true,
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            Ok(Some(Ok(Message::Text(t)))) if t.contains(r#""kind":"ack""#) => {}
            Ok(_) => return false,
        }
    }
}

/// `close_code` for the timeouts that are themselves 5 s long (the first-message and the
/// `exec.opened` waits): its own bound is 15 s.
pub(crate) async fn close_code_slow(ws: &mut Client) -> u16 {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(15), ws.next())
            .await
            .expect("close timed out")
            .unwrap()
            .unwrap();
        match &m {
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(c) => return c.as_ref().map_or(0, |c| u16::from(c.code)),
            _ => {}
        }
    }
}

pub(crate) fn frame(kind: &str, body: Value) -> Message {
    Message::text(json!({"seq": 0, "ack": 0, "kind": kind, "body": body}).to_string())
}

pub(crate) fn data(tag: u8, id: u32, payload: &[u8]) -> Message {
    let mut b = vec![tag];
    b.extend_from_slice(&id.to_be_bytes());
    b.extend_from_slice(payload);
    Message::binary(b)
}

/// The next non-keepalive message on the API's end of the leg.
pub(crate) async fn next_leg(c: &mut Client) -> Message {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), c.next())
            .await
            .expect("leg message timed out")
            .expect("leg ended")
            .expect("leg error");
        if !matches!(m, Message::Ping(_) | Message::Pong(_)) {
            return m;
        }
    }
}

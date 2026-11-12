//! The HTTP forward hop: after the stream grant, the relay tunnels an ordinary HTTP exchange
//! over the leg-1 WebSocket (one short-lived stream per request/response; the runner writes
//! the raw bytes to the loopback app, `16 §W18`).
//!
//! The request is serialized as an HTTP/1.1 head (rebuilt headers) + body; the answer's
//! status/head arrives the same way and the response streams through chunk by chunk — the
//! relay never buffers a whole body. An upgraded answer (101) bridges the browser's upgrade
//! to the same stream byte-for-byte: the relay never inspects frames after the switch.

use std::sync::Arc;

use crate::api::StreamsAnswer;
use crate::edge::RelayBody;
use crate::state::State;
use crate::stream::{LegWs, LiveStream, StreamKey, Streams, dial, run_leg};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::HeaderMap;
use hyper::body::Frame;
use hyper::header::HeaderMap as HMap;

/// An error answer for the edge: the HTTP status the edge should render.
pub type HttpError = u16;

/// The request body's declared length (how the pump sends it over the leg).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyLen {
    /// No body at all.
    None,
    /// `content-length` framing (the value the rebuilt head carries).
    Exact(usize),
    /// Chunked: re-framed by the relay (the outbound head has no `transfer-encoding`).
    Chunked,
}

/// Sends one chunk with its framing (`{len:x}\r\n{data}\r\n`).
async fn send_chunk(tx: &crate::stream::LiveTx, d: &[u8]) -> bool {
    tx.to_app(chunk_header(d.len()).as_ref()).await
        && tx.to_app(d).await
        && tx.to_app(b"\r\n").await
}

/// Forwards one HTTP request through a fresh leg-1 stream and returns the rewritten
/// response, or the failure status. One request = one grant = one stream (the spec's
/// "one WebSocket per upstream connection"). `head` is the rebuilt request head,
/// `body` the request body, `upgrade` the browser's pending upgrade (present for
/// WebSocket requests only): on a 101 answer the two upgraded pipes are bridged.
#[allow(clippy::too_many_arguments)]
pub async fn forward(
    state: Arc<State>,
    streams: Arc<Streams>,
    key: StreamKey,
    sa: StreamsAnswer,
    head: Vec<u8>,
    body_len: BodyLen,
    mut body: hyper::body::Incoming,
    upgrade: Option<hyper::upgrade::OnUpgrade>,
) -> Result<hyper::Response<RelayBody>, HttpError> {
    let ws = dial(&state, &sa).await?;
    let mut live = run_leg(streams, key, ws);
    let tx = live.tx().clone();
    // The head goes first, then the body (the pump frames everything at leg size).
    if !tx.to_app(&head).await {
        return Err(502);
    }
    // The body follows: with a known `content-length` the raw frames pass through; a
    // chunked request is re-framed (the outbound head dropped `transfer-encoding`, so the
    // app expects exactly `content-length` bytes); no body is a no-op.
    let body_task = tokio::spawn(async move {
        match body_len {
            BodyLen::None => true,
            BodyLen::Chunked => {
                while let Some(res) = body.frame().await {
                    let Ok(f) = res else { return false };
                    let Ok(d) = f.into_data() else { return false };
                    if !send_chunk(&tx, &d).await {
                        return false;
                    }
                }
                tx.to_app(FINAL_CHUNK).await
            }
            BodyLen::Exact(n) => {
                let mut left = n;
                while left > 0 {
                    let Some(res) = body.frame().await else {
                        return false;
                    };
                    let Ok(f) = res else { return false };
                    let Ok(d) = f.into_data() else { return false };
                    let d = if d.len() > left { d.slice(..left) } else { d };
                    left -= d.len();
                    if !tx.to_app(&d).await {
                        return false;
                    }
                }
                true
            }
        }
    });
    // The answer's head: raw bytes from the stream until `\r\n\r\n`.
    let raw = match read_head(&mut live).await {
        Ok(b) => b,
        Err(_) => {
            body_task.abort();
            tracing::debug!("relay: the leg ended before the answer head");
            return Err(502);
        }
    };
    let Some(rh) = parse_response_head(&raw) else {
        body_task.abort();
        tracing::debug!(len = raw.len(), "relay: undecodable answer head");
        return Err(502);
    };
    // The bytes that arrived with the head but belong to the body (or the upgraded stream).
    let rest = raw[rh.head_len..].to_vec();
    if rh.status == 101 {
        // An upgraded answer: hyper discards a 101's body (an informational status can never
        // carry one) and instead resolves the browser's pending upgrade with the raw
        // connection after the head is written. Both directions must flow through that
        // upgraded pipe (`16 §W18`: after 101 the leg's binary frames are raw bytes), so
        // this exchange takes over the whole `LiveStream`: `bridge_upgrade` gets the send
        // half, the receive half and `rest` (the app's first frames, if any).
        let (tx, from_app) = live.into_parts();
        if let Some(up) = upgrade {
            bridge_upgrade(up, tx, from_app, rest);
        } else {
            tracing::debug!("relay: 101 without a browser upgrade; dropping the stream");
        }
        // Return the head with an empty, complete body: hyper finishes the message, sees
        // the pending upgrade and hands the socket over.
        let mut res = hyper::Response::builder()
            .status(101)
            .body(RelayBody::ready(Bytes::new()))
            .map_err(|_| 502u16)?;
        rewrite(&mut res, &rh.headers, rh.status);
        return Ok(res);
    }
    // The response is handed to hyper with a body task that forwards until the stream ends.
    let (body_tx, body_rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, std::io::Error>>(16);
    let _body_task = tokio::spawn(async move {
        if !rest.is_empty()
            && body_tx
                .send(Ok(Frame::data(Bytes::from(rest))))
                .await
                .is_err()
        {
            return;
        }
        // Bridge the rest: the body (chunked framing included) — the same copy.
        while let Some(b) = live.from_app().await {
            if body_tx.send(Ok(Frame::data(b))).await.is_err() {
                return;
            }
        }
        // The stream ended; the body ends with it.
    });
    let mut res = hyper::Response::builder()
        .status(rh.status)
        .body(RelayBody::new(body_rx))
        .map_err(|_| 502u16)?;
    // Apply the response header rewrite (SEC-85) on the parsed head.
    rewrite(&mut res, &rh.headers, rh.status);
    Ok(res)
}

/// Bridges hyper's upgraded browser pipe to the leg-1 stream in both directions: browser →
/// app through the [`LiveTx`], app → browser through `from_app` (the leg's receive half;
/// `first` is whatever the app sent along with the 101 head). `16 §W18`: after the switch
/// the relay never inspects frames; the leg's own pings keep it alive.
fn bridge_upgrade(
    up: hyper::upgrade::OnUpgrade,
    tx: crate::stream::LiveTx,
    mut from_app: tokio::sync::mpsc::Receiver<Bytes>,
    first: Vec<u8>,
) {
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let Ok(up) = up.await else {
            return; // the browser hung up before the switch
        };
        let mut io = hyper_util::rt::TokioIo::new(up);
        // The app's first bytes (e.g. a WebSocket answer frame that raced the 101).
        if !first.is_empty() && AsyncWriteExt::write_all(&mut io, &first).await.is_err() {
            return;
        }
        let (mut rio, mut wio) = tokio::io::split(io);
        // Browser → app: raw reads from the upgraded pipe, leg-framed by `to_app`.
        let reader = tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match AsyncReadExt::read(&mut rio, &mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        if !tx.to_app(&buf[..n]).await {
                            return;
                        }
                    }
                }
            }
        });
        // App → browser: leg frames written raw to the upgraded pipe.
        let writer = tokio::spawn(async move {
            while let Some(b) = from_app.recv().await {
                if AsyncWriteExt::write_all(&mut wio, &b).await.is_err() {
                    return;
                }
            }
            let _ = AsyncWriteExt::shutdown(&mut wio).await;
        });
        // Either end ending tears the bridge down (a WebSocket is full duplex but both
        // directions die with the connection; the leg's close codes surface as EOF here).
        let mut reader = reader;
        let mut writer = writer;
        tokio::select! {
            _ = &mut reader => { writer.abort(); }
            _ = &mut writer => { reader.abort(); }
        }
    });
}

/// Applies [`crate::edge::rewrite_response_headers`] to an already-built response. On an
/// upgraded answer (101) the app's `Upgrade`/`Connection` handshake headers are re-added:
/// they are hop-by-hop and the rewrite drops them, but a `101 Switching Protocols` must
/// carry them (RFC 9110 §7.8) for the browser to complete the switch.
fn rewrite(res: &mut hyper::Response<RelayBody>, headers: &[(String, String)], status: u16) {
    let mut h = HMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            hyper::header::HeaderName::from_bytes(k.as_bytes()),
            hyper::header::HeaderValue::from_str(v),
        ) {
            h.append(name, val);
        }
    }
    let rewritten = crate::edge::rewrite_response_headers(&h);
    *res.headers_mut() = rewritten;
    if status == 101 {
        for name in ["upgrade", "connection"] {
            if let Some((_, v)) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name))
                && let (Ok(n), Ok(val)) = (
                    hyper::header::HeaderName::from_bytes(name.as_bytes()),
                    hyper::header::HeaderValue::from_str(v),
                )
            {
                res.headers_mut().insert(n, val);
            }
        }
    }
}

/// Reads the answer's head from the stream: raw bytes until `\r\n\r\n` (60 s, the spec's
/// response header timeout). The returned buffer keeps any body bytes that arrived with
/// the head (the caller slices at `head_len`).
async fn read_head(live: &mut LiveStream) -> Result<Vec<u8>, ()> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf);
        }
        let Some(chunk) = live.from_app().await else {
            return Err(()); // the stream ended before a full head
        };
        if buf.len() + chunk.len() > 256 * 1024 {
            return Err(()); // no sane head is this large
        }
        buf.extend_from_slice(&chunk);
    }
}

/// Rebuilds the HTTP/1.1 head of the proxied request (tests assert the exact bytes).
pub fn request_head(
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body_len: usize,
) -> Vec<u8> {
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for (k, v) in headers {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    if body_len > 0
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
    {
        out.push_str(&format!("content-length: {body_len}\r\n"));
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// The parsed answer head: status, header list, bytes consumed by the head itself.
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub head_len: usize,
}

/// Parses the head of the app's answer (status line + headers) from the byte stream.
/// `head_len` is the number of bytes the head itself occupies (`\r\n\r\n` included);
/// anything after it in the same buffer is body.
pub fn parse_response_head(buf: &[u8]) -> Option<ResponseHead> {
    let i = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..i]).ok()?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    let mut headers = Vec::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    Some(ResponseHead {
        status,
        headers,
        head_len: i + 4,
    })
}

/// True when the head declares a chunked body (streamed through, never buffered).
pub fn is_chunked(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    })
}

/// The bytes of one chunk header (`{len:x}\r\n`).
pub fn chunk_header(len: usize) -> Bytes {
    Bytes::from(format!("{len:x}\r\n"))
}

pub const FINAL_CHUNK: &[u8] = b"0\r\n\r\n";

/// The head of the request the edge sends over the stream (rebuilt header set, host set
/// to the runner endpoint, XFF added, hop-by-hop removed — `outbound_headers` did the
/// filtering). `len` decides the body framing: `None` sends no length header, `Exact(n)`
/// adds `content-length: n`, `Chunked` adds `transfer-encoding: chunked` (the relay
/// re-frames the browser's chunks itself).
pub fn build_request_head(
    method: &hyper::Method,
    path: &str,
    out: &HeaderMap,
    len: BodyLen,
) -> Vec<u8> {
    let headers: Vec<(String, String)> = out
        .iter()
        .filter(|(k, _)| {
            len != BodyLen::Chunked || !k.as_str().eq_ignore_ascii_case("content-length")
        })
        .map(|(k, v)| {
            (
                k.as_str().to_owned(),
                v.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let headers = match len {
        BodyLen::None | BodyLen::Exact(_) => headers,
        BodyLen::Chunked => {
            let mut h = headers;
            h.push(("transfer-encoding".to_owned(), "chunked".to_owned()));
            h
        }
    };
    let body_len = match len {
        BodyLen::None | BodyLen::Chunked => 0,
        BodyLen::Exact(n) => n,
    };
    let mut m = method.as_str().to_owned();
    m.make_ascii_uppercase();
    request_head(&m, path, &headers, body_len)
}

/// The 101 path needs the leg's raw WebSocket for the bridge after the response head;
/// kept for parity with the runner's framing helpers.
pub type RawLeg = LegWs;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_roundtrip() {
        let head = request_head(
            "GET",
            "/x?y=1",
            &[
                ("host".into(), "runner:4108".into()),
                ("accept".into(), "*/*".into()),
            ],
            0,
        );
        assert!(String::from_utf8_lossy(&head).contains("GET /x?y=1 HTTP/1.1\r\n"));
        assert!(String::from_utf8_lossy(&head).contains("host: runner:4108\r\n"));
        assert!(!String::from_utf8_lossy(&head).contains("content-length"));
        let head = request_head("POST", "/x", &[], 5);
        assert!(String::from_utf8_lossy(&head).contains("content-length: 5\r\n"));
    }

    #[test]
    fn response_head_parse() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 4\r\n\r\nabcd";
        let rh = parse_response_head(raw).unwrap();
        assert_eq!(rh.status, 200);
        assert_eq!(rh.head_len, raw.len() - 4); // the body's `abcd` is not the head
        assert!(
            rh.headers
                .iter()
                .any(|(k, v)| k == "content-type" && v == "text/html")
        );
        assert!(parse_response_head(b"HTTP/1.1 200").is_none());
    }

    #[test]
    fn chunked_detection() {
        assert!(is_chunked(&[(
            "Transfer-Encoding".into(),
            "chunked".into()
        )]));
        assert!(!is_chunked(&[("content-length".into(), "3".into())]));
        assert_eq!(chunk_header(4), Bytes::from_static(b"4\r\n"));
    }

    #[test]
    fn build_head_sets_method_and_len() {
        let mut h = HeaderMap::new();
        h.insert("host", "runner:4108".parse().unwrap());
        let head = build_request_head(&hyper::Method::POST, "/x", &h, BodyLen::None);
        let s = String::from_utf8(head).unwrap();
        assert!(s.starts_with("POST /x HTTP/1.1\r\n"));
        assert!(!s.contains("content-length"));
        assert!(s.ends_with("\r\n\r\n"));
        let head = build_request_head(&hyper::Method::POST, "/x", &h, BodyLen::Exact(5));
        let s = String::from_utf8(head).unwrap();
        assert!(s.contains("content-length: 5\r\n"));
        let head = build_request_head(&hyper::Method::POST, "/x", &h, BodyLen::Chunked);
        let s = String::from_utf8(head).unwrap();
        assert!(s.contains("transfer-encoding: chunked\r\n"));
        assert!(!s.contains("content-length"));
    }
}

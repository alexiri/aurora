//! Relay behaviour cases of `docs/18 §Port previews` "Relay" that no other test covers
//! (FIX-073, AUDIT-004 F-2): slow-loris, the plain and upgraded idle limits, the stream
//! lifetime, server-sent events, range requests, keep-alive over one client connection, the
//! WebSocket `Origin` matrix and revocation closing an open stream within `PREVIEW_AUTH_CACHE`.
//!
//! Time limits are shortened through `Streams::with_timeouts` (the same code path production
//! runs with `Timeouts::spec()`); nothing waits on real minutes, and every wait is bounded.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const QUIET: Duration = Duration::from_secs(3600);

fn tm() -> Timeouts {
    Timeouts {
        header: QUIET,
        idle_http: QUIET,
        idle_ws: QUIET,
        lifetime: QUIET,
        validate_every: QUIET,
        close: Duration::from_secs(2),
    }
}

/// The spec's values are what the shortened ones stand in for.
#[test]
fn the_shortened_limits_stand_in_for_the_spec_values() {
    let s = Timeouts::spec();
    assert_eq!(s.header, Duration::from_secs(60));
    assert_eq!(s.idle_http, Duration::from_secs(120));
    assert_eq!(s.idle_ws, Duration::from_secs(600));
    assert_eq!(s.lifetime, Duration::from_secs(4 * 3600));
    assert_eq!(s.validate_every, Duration::from_secs(15));
}

/// A client that dribbles its request head is cut off at the header timeout; a normal
/// request on another connection is served meanwhile.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_loris_headers_time_out() {
    let r = rig(
        Timeouts {
            header: Duration::from_millis(400),
            ..tm()
        },
        false,
    )
    .await;
    let mut slow = connect(&r).await;
    slow.write_all(format!("GET /echo HTTP/1.1\r\nhost: {HOST}\r\n").as_bytes())
        .await
        .unwrap();
    let t = Instant::now();
    // A well-behaved request is not held up by the stalled one.
    let mut ok = connect(&r).await;
    ok.write_all(request("/echo", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, _, _) = read_head(&mut ok).await;
    assert_eq!(status, 200);
    // The stalled connection ends (EOF, possibly after a 408) well before the bound.
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), slow.read_to_end(&mut buf))
        .await
        .expect("the dribbling connection was never closed")
        .ok();
    let waited = t.elapsed();
    assert!(
        waited >= Duration::from_millis(300),
        "closed too early: {waited:?}"
    );
    assert!(!String::from_utf8_lossy(&buf).contains("200 OK"), "{buf:?}");
}

/// A plain HTTP stream whose app goes quiet is closed at `PREVIEW_IDLE_TIMEOUT` (120 s in
/// production, 400 ms here): the browser sees a truncated answer and the slot frees.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_http_idle_closes_the_stream() {
    let r = rig(
        Timeouts {
            idle_http: Duration::from_millis(400),
            ..tm()
        },
        false,
    )
    .await;
    let mut c = connect(&r).await;
    c.write_all(request("/stall", &[], true).as_bytes())
        .await
        .unwrap();
    let t = Instant::now();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!(status, 200);
    assert_eq!(header(&h, "content-length"), Some("100"));
    let mut body = rest;
    tokio::time::timeout(Duration::from_secs(10), c.read_to_end(&mut body))
        .await
        .expect("the idle stream was never closed")
        .ok();
    assert!(
        body.len() < 100,
        "the app never finished its answer: {}",
        body.len()
    );
    assert!(
        t.elapsed() >= Duration::from_millis(350),
        "{:?}",
        t.elapsed()
    );
    let (streams, share) = (r.streams.clone(), share());
    until("the idle stream frees its slot", move || {
        streams.share_count(share) == 0
    })
    .await;
}

/// An upgraded stream idles on the longer `PREVIEW_WS_IDLE_TIMEOUT` (600 s; 1.5 s here), not
/// the plain one (300 ms): it survives a quiet spell past the plain limit, then closes when
/// it stays quiet for the upgraded one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upgraded_idle_closes_on_the_longer_limit() {
    let r = rig(
        Timeouts {
            idle_http: Duration::from_millis(300),
            idle_ws: Duration::from_millis(1500),
            ..tm()
        },
        false,
    )
    .await;
    let origin = format!("https://{HOST}");
    let mut ws = ws_connect(&r, Some(&origin)).await.unwrap();
    echo(&mut ws, "one").await;
    tokio::time::sleep(Duration::from_millis(700)).await; // past the plain limit
    echo(&mut ws, "two").await; // still open: the upgraded limit applies
    let quiet_since = Instant::now();
    let _ = until_closed(&mut ws, Duration::from_secs(10)).await;
    let quiet = quiet_since.elapsed();
    assert!(
        quiet >= Duration::from_millis(1300),
        "closed on the plain limit after {quiet:?}"
    );
    let (streams, share) = (r.streams.clone(), share());
    until("the closed stream frees its slot", move || {
        streams.share_count(share) == 0
    })
    .await;
}

/// A stream is closed at `PREVIEW_STREAM_LIFETIME` (4 h; 1.2 s here) however busy it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_lifetime_closes_a_busy_stream() {
    let r = rig(
        Timeouts {
            lifetime: Duration::from_millis(1200),
            ..tm()
        },
        false,
    )
    .await;
    let origin = format!("https://{HOST}");
    let mut ws = ws_connect(&r, Some(&origin)).await.unwrap();
    let t = Instant::now();
    // Keep the stream busy (its idle timer never fires) until the lifetime ends it.
    let mut n = 0u32;
    loop {
        use futures::SinkExt;
        if ws
            .send(tokio_tungstenite::tungstenite::Message::text(format!(
                "m{n}"
            )))
            .await
            .is_err()
        {
            break;
        }
        n += 1;
        match tokio::time::timeout(
            Duration::from_millis(150),
            futures::StreamExt::next(&mut ws),
        )
        .await
        {
            Ok(None | Some(Err(_)))
            | Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))) => break,
            _ => {}
        }
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the busy stream outlived its lifetime"
        );
    }
    let lived = t.elapsed();
    assert!(
        (Duration::from_millis(1000)..Duration::from_secs(8)).contains(&lived),
        "lifetime ended it after {lived:?}"
    );
}

/// Server-sent events arrive as the app emits them: the first event reaches the browser
/// before the second exists (no buffering), the app's own chunk framing is not leaked into
/// the payload, and the last chunk ends the stream (the slot frees without an idle wait).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_events_arrive_as_sent() {
    let r = rig(tm(), false).await;
    let mut c = connect(&r).await;
    c.write_all(request("/sse", &[("accept", "text/event-stream")], true).as_bytes())
        .await
        .unwrap();
    let t = Instant::now();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!(status, 200);
    assert_eq!(header(&h, "content-type"), Some("text/event-stream"));
    assert_eq!(header(&h, "transfer-encoding"), Some("chunked"));
    // What the browser receives is hyper's chunked framing of the *payload*: decode it.
    let mut dec = aurora_relay::proxy::ChunkedDecoder::default();
    let mut payload: Vec<u8> = Vec::new();
    let mut first_at = None;
    let mut done = false;
    let mut feed = |bytes: &[u8], payload: &mut Vec<u8>, first_at: &mut Option<Duration>| {
        let mut out = Vec::new();
        let d = dec
            .feed(bytes, &mut out)
            .expect("valid chunked framing toward the browser");
        for b in out {
            payload.extend_from_slice(&b);
        }
        if first_at.is_none() && String::from_utf8_lossy(payload).contains("data: one\n\n") {
            *first_at = Some(t.elapsed());
        }
        d
    };
    done |= feed(&rest, &mut payload, &mut first_at);
    let mut buf = [0u8; 4096];
    while !done {
        let n = tokio::time::timeout(Duration::from_secs(20), c.read(&mut buf))
            .await
            .expect("the event stream stalled")
            .unwrap();
        assert!(
            n > 0,
            "closed before the last chunk: {:?}",
            String::from_utf8_lossy(&payload)
        );
        done |= feed(&buf[..n], &mut payload, &mut first_at);
    }
    assert_eq!(
        String::from_utf8_lossy(&payload),
        "data: one\n\ndata: two\n\ndata: three\n\n"
    );
    let first = first_at.expect("first event seen");
    assert!(
        first < SSE_GAP / 2,
        "the first event waited for later ones: {first:?}"
    );
    assert!(
        t.elapsed() >= SSE_GAP * 2 - Duration::from_millis(100),
        "events were not spaced"
    );
    let (streams, share) = (r.streams.clone(), share());
    until("the finished event stream frees its slot", move || {
        streams.share_count(share) == 0
    })
    .await;
}

/// Range requests pass through: the app's `206`, `Content-Range` and body arrive unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn range_requests_pass_through() {
    let r = rig(tm(), false).await;
    let mut c = connect(&r).await;
    c.write_all(request("/range", &[("range", "bytes=0-3")], true).as_bytes())
        .await
        .unwrap();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!(status, 206);
    assert_eq!(header(&h, "content-range"), Some("bytes 0-3/10"));
    assert_eq!(header(&h, "accept-ranges"), Some("bytes"));
    assert_eq!(read_exact_body(&mut c, rest, 4).await, b"0123");
    // Without a Range header the whole body comes back, `Accept-Ranges` intact.
    let mut c = connect(&r).await;
    c.write_all(request("/range", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!((status, header(&h, "accept-ranges")), (200, Some("bytes")));
    assert_eq!(read_exact_body(&mut c, rest, 10).await, b"0123456789");
}

/// HTTP keep-alive: several requests over one browser connection each get their answer
/// (one stream per request underneath), and every slot is free afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keep_alive_serves_several_requests_on_one_connection() {
    let r = rig(tm(), false).await;
    let mut c = connect(&r).await;
    for i in 0..4 {
        c.write_all(request("/echo", &[], false).as_bytes())
            .await
            .unwrap();
        let (status, h, rest) = read_head(&mut c).await;
        assert_eq!(status, 200, "request {i}");
        let n: usize = header(&h, "content-length").unwrap().parse().unwrap();
        assert_eq!(
            read_exact_body(&mut c, rest, n).await,
            b"echo",
            "request {i}"
        );
    }
    let (streams, share) = (r.streams.clone(), share());
    until("every request's stream is released", move || {
        streams.share_count(share) == 0
    })
    .await;
}

/// The WebSocket `Origin` matrix: only the preview's own origin upgrades; a missing,
/// foreign, scheme-downgraded or look-alike `Origin` is refused 403 before any stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_origin_matrix() {
    let r = rig(tm(), false).await;
    let own = format!("https://{HOST}");
    let mut ws = ws_connect(&r, Some(&own))
        .await
        .expect("same-origin upgrades");
    echo(&mut ws, "hi").await;
    drop(ws);
    let evil = format!("https://{HOST}.evil.example");
    for origin in [
        None,
        Some("https://evil.example"),
        Some("null"),
        Some(format!("http://{HOST}").leak() as &str),
        Some(evil.as_str()),
        Some("https://aurora.example.org"),
    ] {
        let Err((status, _)) = ws_connect(&r, origin).await else {
            panic!("origin {origin:?} must be refused");
        };
        assert_eq!(status, 403, "origin {origin:?}");
    }
}

/// A request for a parked share waits for the sandbox: the relay long-polls the API for
/// `PREVIEW_WAKE_WAIT` (45 s) and, once the port listens, serves the request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wake_wait_serves_the_request_when_the_port_comes_up_in_time() {
    use std::sync::atomic::Ordering::SeqCst;
    let r = rig(tm(), false).await;
    r.script.not_running.store(true, SeqCst);
    r.script.up_after_wait.store(true, SeqCst);
    let mut c = connect(&r).await;
    c.write_all(request("/echo", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!(status, 200);
    let n: usize = header(&h, "content-length").unwrap().parse().unwrap();
    assert_eq!(read_exact_body(&mut c, rest, n).await, b"echo");
    assert_eq!(r.script.waits.load(SeqCst), 1, "one long-poll");
    assert_eq!(r.script.last_wait.load(SeqCst), 45, "PREVIEW_WAKE_WAIT");
}

/// When the port is still not up after the wait the browser gets a 503 page that refreshes
/// itself (`Refresh`, `Retry-After`), not a bare 502.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wake_wait_times_out_into_a_refreshing_503_page() {
    use std::sync::atomic::Ordering::SeqCst;
    let r = rig(tm(), false).await;
    r.script.not_running.store(true, SeqCst);
    *r.script.route.lock().unwrap() = Some(serde_json::json!({
        "sandbox": "waking", "listening": false, "start": "starting"
    }));
    let mut c = connect(&r).await;
    c.write_all(request("/echo", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!(status, 503);
    assert_eq!(header(&h, "refresh"), Some("5"));
    assert_eq!(header(&h, "retry-after"), Some("5"));
    let mut body = rest;
    c.read_to_end(&mut body).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("starting"),
        "{body:?}"
    );
    assert_eq!(r.script.waits.load(SeqCst), 1);
}

/// A start command that exited shows the exit page (502, no refresh); a start throttled by
/// `PREVIEW_START_ATTEMPTS` shows "try later" (429 with `Retry-After`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_command_exit_and_throttle_pages() {
    let r = rig(tm(), false).await;
    r.script
        .not_running
        .store(true, std::sync::atomic::Ordering::SeqCst);
    *r.script.route.lock().unwrap() = Some(serde_json::json!({
        "sandbox": "running", "listening": false, "start": "exited"
    }));
    let mut c = connect(&r).await;
    c.write_all(request("/echo", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!(status, 502);
    assert!(
        header(&h, "refresh").is_none(),
        "an exited command does not auto-refresh"
    );
    let mut body = rest;
    c.read_to_end(&mut body).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("start command"),
        "{body:?}"
    );

    *r.script.route.lock().unwrap() = Some(serde_json::json!({
        "sandbox": "running", "listening": false, "start": "limited", "retry_after_secs": 60
    }));
    let mut c = connect(&r).await;
    c.write_all(request("/echo", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, h, _) = read_head(&mut c).await;
    assert_eq!((status, header(&h, "retry-after")), (429, Some("60")));
}

/// Revoking access closes an open stream within the validation interval — `PREVIEW_AUTH_CACHE`
/// (15 s) in production, 200 ms here — instead of at its idle timeout (SEC-81).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_closes_an_open_stream_within_the_auth_cache() {
    assert!(Timeouts::spec().validate_every <= aurora_core::limits::PREVIEW_AUTH_CACHE);
    let r = rig(
        Timeouts {
            validate_every: Duration::from_millis(200),
            ..tm()
        },
        true,
    )
    .await;
    let origin = format!("https://{HOST}");
    let mut ws = ws_connect(&r, Some(&origin)).await.unwrap();
    echo(&mut ws, "alive").await;
    // Still open across several validation cycles while access holds.
    tokio::time::sleep(Duration::from_millis(700)).await;
    echo(&mut ws, "still alive").await;
    // Access is revoked: the next validation cycle closes the stream.
    let hash = aurora_relay::api::session_hash("auv_one");
    r.script.revoked.lock().unwrap().insert(hash);
    let took = until_closed(&mut ws, Duration::from_secs(10)).await;
    assert!(took < Duration::from_secs(3), "revocation took {took:?}");
    let (streams, share) = (r.streams.clone(), share());
    until("the revoked stream frees its slot", move || {
        streams.share_count(share) == 0
    })
    .await;
}

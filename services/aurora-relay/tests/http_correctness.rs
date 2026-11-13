//! Relay HTTP correctness (FIX-088, REVIEW-5 PRV-2/5/6/7/11/16, EVID-12): the app gets its own
//! cookies, a stalled peer cannot block revocation, the response-head limit and the request
//! body cap are enforced, truncated answers abort, the header lists of SEC-85 hold, and the
//! smuggling corpus is exercised on the wire (raw sockets, not on parsed `Request` values).
//!
//! Every wait is bounded; time limits are the shortened `Timeouts` of the shared rig.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
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
        close: Duration::from_millis(500),
    }
}

/// A request head with the given cookie header value (and nothing else of ours).
fn get_with_cookie(path: &str, cookie: Option<&str>) -> String {
    let mut s = format!("GET {path} HTTP/1.1\r\nhost: {HOST}\r\nconnection: close\r\n");
    if let Some(c) = cookie {
        s.push_str(&format!("cookie: {c}\r\n"));
    }
    s.push_str("\r\n");
    s
}

/// Sends `raw`, reads to EOF (bounded) and returns everything received.
async fn exchange(r: &Rig, raw: &str) -> Vec<u8> {
    let mut c = connect(r).await;
    c.write_all(raw.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), c.read_to_end(&mut out))
        .await
        .expect("no end of answer within 10 s");
    out
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

// ------------------------------------------------------------------ 1. cookies

/// The app receives its own cookies and none of Aurora's; a request with only Aurora cookies
/// reaches the app with no `Cookie` header at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_gets_its_own_cookies_and_never_aurora_s() {
    let r = rig(tm(), false).await;
    let both = format!("app=1; {COOKIE}; __Host-aurora_pstate=x.y; aurora_session=s");
    let out = text(&exchange(&r, &get_with_cookie("/who", Some(&both))).await);
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(
        out.contains("cookie=app=1 "),
        "the app sees exactly app=1: {out}"
    );
    // Several Cookie headers (HTTP/1.1 permits them) are merged, the order kept.
    let raw = format!(
        "GET /who HTTP/1.1\r\nhost: {HOST}\r\nconnection: close\r\ncookie: a=1; {COOKIE}\r\ncookie: b=2\r\n\r\n"
    );
    let out = text(&exchange(&r, &raw).await);
    assert!(out.contains("cookie=a=1; b=2 "), "{out}");
    // Only Aurora cookies: no Cookie header reaches the app.
    let out = text(&exchange(&r, &get_with_cookie("/who", Some(COOKIE))).await);
    assert!(out.contains("cookie=none "), "{out}");
}

// ------------------------------------------------------------------ 2. stalled peers

/// A viewer who stops reading a large response keeps the stream open until revocation: the
/// pump must not wait on the full queue, so revoking closes the leg and frees the slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_closes_a_stream_whose_viewer_stopped_reading() {
    let r = rig_with(
        Timeouts {
            validate_every: Duration::from_millis(200),
            ..tm()
        },
        Opts {
            heartbeat: true,
            ..Opts::default()
        },
    )
    .await;
    let mut c = connect(&r).await;
    c.write_all(request("/gig", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, _, _) = read_head(&mut c).await;
    assert_eq!(status, 200);
    // The viewer reads nothing more: the socket buffers and the queues fill up.
    let (streams, share) = (r.streams.clone(), share());
    until("the stream is open", {
        let s = streams.clone();
        move || s.share_count(share) == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(streams.share_count(share), 1, "still streaming, unread");
    r.script
        .revoked
        .lock()
        .unwrap()
        .insert(aurora_relay::api::session_hash("auv_one"));
    let t = Instant::now();
    while streams.share_count(share) != 0 {
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "the stalled stream was still open 2 s after revocation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(c);
}

/// A runner that stops reading does not hold the slot past the bounded close.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runner_that_stops_reading_does_not_hold_the_slot() {
    let r = rig_with(
        Timeouts {
            validate_every: Duration::from_millis(200),
            ..tm()
        },
        Opts {
            heartbeat: true,
            body_max: 1 << 30,
            deaf_runner: true,
            ..Opts::default()
        },
    )
    .await;
    let mut c = connect(&r).await;
    let head = format!(
        "POST /upload HTTP/1.1\r\nhost: {HOST}\r\ncookie: {COOKIE}\r\ncontent-length: {}\r\n\r\n",
        512 * 1024 * 1024
    );
    c.write_all(head.as_bytes()).await.unwrap();
    // Push body bytes until the relay stops reading them (the deaf runner fills every queue).
    let chunk = vec![7u8; 64 * 1024];
    loop {
        match tokio::time::timeout(Duration::from_millis(700), c.write_all(&chunk)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => break,
            Err(_) => break, // the write blocked: the relay is no longer draining
        }
    }
    let (streams, share) = (r.streams.clone(), share());
    assert_eq!(streams.share_count(share), 1, "the stream is held open");
    r.script
        .revoked
        .lock()
        .unwrap()
        .insert(aurora_relay::api::session_hash("auv_one"));
    let t = Instant::now();
    while streams.share_count(share) != 0 {
        assert!(
            t.elapsed() < Duration::from_secs(4),
            "a stalled runner still holds the slot 4 s after revocation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(c);
}

// ------------------------------------------------------------------ 3. limits

/// An app that never answers: 504 at the (shortened) response-head limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_never_answers_gets_a_504_at_the_head_limit() {
    let app = raw_app(|mut tcp| async move {
        let _ = raw_read_head(&mut tcp).await;
        std::future::pending::<()>().await;
    });
    let r = rig_with(
        Timeouts {
            header: Duration::from_millis(600),
            ..tm()
        },
        Opts {
            app: Some(app),
            ..Opts::default()
        },
    )
    .await;
    let t = Instant::now();
    let out = text(&exchange(&r, &request("/anything", &[], true)).await);
    assert!(out.starts_with("HTTP/1.1 504"), "{out}");
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    let (streams, share) = (r.streams.clone(), share());
    until("the slot is freed", move || streams.share_count(share) == 0).await;
}

/// A chunked upload over `relay.request_body_max_bytes` is refused with 413 and the leg closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chunked_upload_over_the_cap_is_a_413() {
    let r = rig_with(
        tm(),
        Opts {
            body_max: 1000,
            ..Opts::default()
        },
    )
    .await;
    let mut c = connect(&r).await;
    let head = format!(
        "POST /upload HTTP/1.1\r\nhost: {HOST}\r\ncookie: {COOKIE}\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
    );
    c.write_all(head.as_bytes()).await.unwrap();
    // 8 chunks of 400 bytes = 3200 bytes against a cap of 1000.
    for _ in 0..8 {
        let _ = c
            .write_all(format!("190\r\n{}\r\n", "x".repeat(400)).as_bytes())
            .await;
    }
    let _ = c.write_all(b"0\r\n\r\n").await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), c.read_to_end(&mut out))
        .await
        .expect("no answer within 10 s");
    let out = text(&out);
    assert!(out.starts_with("HTTP/1.1 413"), "{out}");
    let (streams, share) = (r.streams.clone(), share());
    until("the slot is freed", move || streams.share_count(share) == 0).await;
    // A chunked upload under the cap still works.
    let mut c = connect(&r).await;
    c.write_all(head.as_bytes()).await.unwrap();
    c.write_all(format!("190\r\n{}\r\n0\r\n\r\n", "x".repeat(400)).as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), c.read_to_end(&mut out))
        .await
        .expect("no answer within 10 s");
    assert!(text(&out).contains("got=400"), "{}", text(&out));
}

// ------------------------------------------------------------------ 4. connection: close

/// The outbound head of an ordinary request tells the app to close after the answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_is_told_to_close_after_one_response() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let app = {
        let seen = seen.clone();
        raw_app(move |mut tcp| {
            let seen = seen.clone();
            async move {
                let head = raw_read_head(&mut tcp).await;
                seen.lock().unwrap().push(text(&head));
                let _ = tcp
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                    .await;
                // A keep-alive app keeps its socket; one that honours `close` closes it.
                let close = text(&head)
                    .to_ascii_lowercase()
                    .contains("connection: close");
                if !close {
                    std::future::pending::<()>().await;
                }
            }
        })
    };
    let r = rig_with(
        tm(),
        Opts {
            app: Some(app),
            ..Opts::default()
        },
    )
    .await;
    // The browser asks for keep-alive; the relay's own hop to the app is still one-shot.
    let raw = format!(
        "GET /x HTTP/1.1\r\nhost: {HOST}\r\ncookie: {COOKIE}\r\nconnection: keep-alive\r\n\r\n"
    );
    let mut c = connect(&r).await;
    c.write_all(raw.as_bytes()).await.unwrap();
    let (status, _, rest) = read_head(&mut c).await;
    assert_eq!(status, 200);
    let _ = read_exact_body(&mut c, rest, 2).await;
    let head = seen.lock().unwrap()[0].to_ascii_lowercase();
    assert!(head.contains("connection: close\r\n"), "{head}");
    // And the slot is released without waiting for the idle limit.
    let (streams, share) = (r.streams.clone(), share());
    until("the slot is freed", move || streams.share_count(share) == 0).await;
}

// ------------------------------------------------------------------ 5. truncated answers

/// Serves `answer` from a raw app, then closes; returns what the browser got and whether it
/// looks like a complete answer.
async fn truncated_case(answer: &'static [u8]) -> (String, bool) {
    let app = raw_app(move |mut tcp| async move {
        let _ = raw_read_head(&mut tcp).await;
        let _ = tcp.write_all(answer).await;
        // Close right after: the leg ends mid-body.
    });
    let r = rig_with(
        tm(),
        Opts {
            app: Some(app),
            ..Opts::default()
        },
    )
    .await;
    let mut c = connect(&r).await;
    c.write_all(request("/x", &[], true).as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    let res = tokio::time::timeout(Duration::from_secs(10), c.read_to_end(&mut out))
        .await
        .expect("the truncated answer never ended");
    let out = text(&out);
    // A complete answer ends with the terminal chunk or the declared length; an aborted one is
    // a transport error (reset) or a connection closed short of it.
    let complete = out.ends_with("0\r\n\r\n") && res.is_ok();
    (out, complete)
}

/// A chunk-size line that is not hex, and a leg that drops mid-chunk, both surface as a
/// transport error: never a response that looks complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_or_cut_chunked_answers_do_not_look_complete() {
    let (out, complete) =
        truncated_case(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\nzz\r\n").await;
    assert!(!complete, "a malformed chunk looked complete: {out:?}");
    let (out, complete) = truncated_case(
        b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\na\r\nabc",
    )
    .await;
    assert!(!complete, "a cut chunk looked complete: {out:?}");
    // A leg that ends before `content-length` bytes arrived.
    let (out, complete) = truncated_case(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nabc").await;
    assert!(!complete);
    assert!(!out.contains("abcdefghij"), "{out:?}");
}

// ------------------------------------------------------------------ 6. headers on the wire

/// The app's `clear-site-data`, `alt-svc`, `service-worker-allowed` and HSTS never reach the
/// browser; its CSP does (the preview origin is the app's own); client forwarding headers
/// never reach the app.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn header_lists_hold_on_the_wire() {
    let r = rig(tm(), false).await;
    let out = text(&exchange(&r, &request("/hdrs", &[], true)).await).to_ascii_lowercase();
    for gone in ["clear-site-data", "alt-svc", "service-worker-allowed"] {
        assert!(!out.contains(gone), "{gone} reached the browser: {out}");
    }
    assert!(
        out.contains("content-security-policy: default-src 'self'"),
        "the app's CSP is passed through: {out}"
    );
    assert!(
        out.contains("max-age=31536000"),
        "only the relay's HSTS: {out}"
    );
    assert!(!out.contains("max-age=1\r\n"), "{out}");
    let out = text(
        &exchange(
            &r,
            &request(
                "/who",
                &[
                    ("x-real-ip", "6.6.6.6"),
                    ("via", "1.1 evil"),
                    ("true-client-ip", "6.6.6.6"),
                    ("x-forwarded-port", "1"),
                ],
                true,
            ),
        )
        .await,
    );
    assert!(out.contains("xrealip=none"), "{out}");
    assert!(out.contains("via=none"), "{out}");
}

// ------------------------------------------------------------------ 7. smuggling on the wire

/// Ambiguous framing is refused with 400 on the wire before anything reaches the app. The app
/// here counts connections it was dialled for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smuggling_corpus_is_refused_before_anything_is_forwarded() {
    let dialled = Arc::new(AtomicU32::new(0));
    let app = {
        let dialled = dialled.clone();
        raw_app(move |mut tcp| {
            let dialled = dialled.clone();
            async move {
                dialled.fetch_add(1, SeqCst);
                let _ = raw_read_head(&mut tcp).await;
                let _ = tcp
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                    .await;
            }
        })
    };
    let r = rig_with(
        tm(),
        Opts {
            app: Some(app),
            ..Opts::default()
        },
    )
    .await;
    let h = format!("host: {HOST}\r\ncookie: {COOKIE}\r\n");
    let cases: Vec<(&str, String)> = vec![
        (
            "CL+TE",
            format!(
                "POST /x HTTP/1.1\r\n{h}content-length: 4\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n0\r\n\r\n"
            ),
        ),
        (
            "TE+CL",
            format!(
                "POST /x HTTP/1.1\r\n{h}transfer-encoding: chunked\r\ncontent-length: 4\r\nconnection: close\r\n\r\n0\r\n\r\n"
            ),
        ),
        (
            "duplicate CL",
            format!(
                "POST /x HTTP/1.1\r\n{h}content-length: 2\r\ncontent-length: 3\r\nconnection: close\r\n\r\nab"
            ),
        ),
        (
            "obsolete folding",
            format!("GET /x HTTP/1.1\r\n{h}x-a: 1\r\n 2\r\nconnection: close\r\n\r\n"),
        ),
        (
            "bare LF",
            format!("GET /x HTTP/1.1\n{h}connection: close\r\n\r\n"),
        ),
        (
            "bare LF between headers",
            format!("GET /x HTTP/1.1\r\n{h}x-a: 1\nx-b: 2\r\nconnection: close\r\n\r\n"),
        ),
        (
            "TE not chunked",
            format!(
                "POST /x HTTP/1.1\r\n{h}transfer-encoding: gzip, chunked\r\nconnection: close\r\n\r\n0\r\n\r\n"
            ),
        ),
    ];
    let mut wrong = Vec::new();
    for (name, raw) in cases {
        let out = text(&exchange(&r, &raw).await);
        if !out.starts_with("HTTP/1.1 400") {
            wrong.push(format!("{name}: expected a 400, got {out:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert_eq!(
        dialled.load(SeqCst),
        0,
        "an ambiguous request was forwarded to the app"
    );
    // The control: a clean request is forwarded.
    let out = text(&exchange(&r, &request("/x", &[], true)).await);
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert_eq!(dialled.load(SeqCst), 1);
}

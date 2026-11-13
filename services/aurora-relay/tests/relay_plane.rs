//! The relay plane's own limits and failure handling (FIX-090, REVIEW-5 PRV-3/9/13/15/18): random
//! cookies cannot spend the API call budget, the visitor's real address keys the grant limiter
//! behind a trusted proxy (and an untrusted peer's `X-Forwarded-For` is ignored), a throttled API
//! is a try-later page and a retried `validate`, and an endpoint the API names that is not a plain
//! host[:port] is never dialled.
//!
//! Every wait is bounded; the shortened limits are the shared rig's.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::HashSet;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

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

/// A GET with the given preview cookie token.
fn get_with(path: &str, token: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nhost: {HOST}\r\nconnection: close\r\n\
         cookie: __Host-aurora_preview={token}\r\n\r\n"
    )
}

async fn status_of(r: &Rig, raw: &str) -> u16 {
    let mut c = connect(r).await;
    c.write_all(raw.as_bytes()).await.unwrap();
    read_head(&mut c).await.0
}

// ------------------------------------------------------------------ negative resolves

/// 200 requests with random cookies from one client spend at most the negative-resolve budget
/// of API calls; a visitor with a good, cached credential is still served afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_cookies_cannot_spend_the_api_budget() {
    let r = rig(tm(), false).await;
    *r.script.known.lock().unwrap() =
        Some(HashSet::from([aurora_relay::api::session_hash("auv_one")]));
    // The legitimate visitor first: resolved once, cached for 15 s.
    assert_eq!(status_of(&r, &get_with("/echo", "auv_one")).await, 200);
    // 200 random cookies: every one is a distinct cache key, every one would be a call.
    for i in 0..200 {
        let s = status_of(&r, &get_with("/echo", &format!("auv_random{i}"))).await;
        assert!(s == 302 || s == 401, "request {i}: {s}");
    }
    let calls = r.script.unknown_resolves.load(SeqCst);
    assert_eq!(
        calls,
        aurora_relay::limits::NEGATIVE_RESOLVES_PER_MIN,
        "the API was asked about {calls} unknown cookies"
    );
    // The good credential is served from its cache entry; a good one that is not cached yet
    // from this flooding address is bounced into the login flow (a redirect), never a 502.
    assert_eq!(status_of(&r, &get_with("/echo", "auv_one")).await, 200);
    r.script
        .known
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .insert(aurora_relay::api::session_hash("auv_two"));
    let s = status_of(&r, &get_with("/echo", "auv_two")).await;
    assert_eq!(s, 302, "the login page, not a 502");
}

// ------------------------------------------------------------------ client address

fn grant_request(label: &str, xff: Option<&str>) -> String {
    let mut s = format!(
        "GET /_aurora/grant?g=x HTTP/1.1\r\nhost: {label}.previews.example.org\r\n\
         connection: close\r\ncookie: __Host-aurora_pstate=n1.Lw\r\n"
    );
    if let Some(x) = xff {
        s.push_str(&format!("x-forwarded-for: {x}\r\n"));
    }
    s.push_str("\r\n");
    s
}

fn label_n(n: usize) -> String {
    format!("p{n:024}")
}

/// Behind a trusted proxy each visitor has their own grant-redemption bucket; an untrusted
/// peer's `X-Forwarded-For` is ignored, so one address is one bucket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grant_limiter_keys_on_the_visitor_behind_a_trusted_proxy_only() {
    // The test client connects from 127.0.0.1: trusted here.
    let trusted = rig_with(
        tm(),
        Opts {
            trusted_proxies: vec!["127.0.0.0/8"],
            ..Opts::default()
        },
    )
    .await;
    for n in 0..40 {
        let raw = grant_request(&label_n(n), Some(&format!("198.51.100.{n}, 127.0.0.1")));
        let s = status_of(&trusted, &raw).await;
        assert_ne!(s, 429, "visitor {n} was refused with the others");
    }
    // The same visitor address is still one bucket (the 31st attempt of one address).
    let mut refused = 0;
    for n in 100..140 {
        let raw = grant_request(&label_n(n), Some("203.0.113.7"));
        if status_of(&trusted, &raw).await == 429 {
            refused += 1;
        }
    }
    assert_eq!(refused, 10, "30 of 40 attempts of one visitor pass");

    // No trusted proxies: whatever the peer says in `X-Forwarded-For` is ignored.
    let plain = rig(tm(), false).await;
    let mut refused = 0;
    for n in 0..40 {
        let raw = grant_request(&label_n(n), Some(&format!("198.51.100.{n}")));
        if status_of(&plain, &raw).await == 429 {
            refused += 1;
        }
    }
    assert_eq!(refused, 10, "one socket address is one bucket");
}

/// The app sees the visitor's address in `X-Forwarded-For`: the rightmost untrusted entry
/// behind a trusted proxy, the socket peer otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_sees_the_visitor_address() {
    let trusted = rig_with(
        tm(),
        Opts {
            trusted_proxies: vec!["127.0.0.0/8"],
            ..Opts::default()
        },
    )
    .await;
    let raw = format!(
        "GET /who HTTP/1.1\r\nhost: {HOST}\r\nconnection: close\r\ncookie: {COOKIE}\r\n\
         x-forwarded-for: 6.6.6.6, 203.0.113.9, 127.0.0.1\r\n\r\n"
    );
    let mut c = connect(&trusted).await;
    c.write_all(raw.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    c.read_to_end(&mut out).await.unwrap();
    let out = String::from_utf8_lossy(&out).into_owned();
    assert!(out.contains("xff=203.0.113.9"), "{out}");

    let plain = rig(tm(), false).await;
    let mut c = connect(&plain).await;
    c.write_all(raw.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    c.read_to_end(&mut out).await.unwrap();
    let out = String::from_utf8_lossy(&out).into_owned();
    assert!(out.contains("xff=127.0.0.1"), "{out}");
}

// ------------------------------------------------------------------ a throttled API

/// A 429/5xx from the API on `resolve` is the try-later page (503, `Retry-After`), not a 502;
/// an answer no retry fixes (401) stays a 502.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_throttled_api_is_a_try_later_page() {
    let r = rig(tm(), false).await;
    for api in [429, 500, 503] {
        r.script.resolve_status.store(api, SeqCst);
        let mut c = connect(&r).await;
        c.write_all(request("/echo", &[], true).as_bytes())
            .await
            .unwrap();
        let (status, h, _) = read_head(&mut c).await;
        assert_eq!(status, 503, "the API answered {api}");
        assert_eq!(header(&h, "retry-after"), Some("5"));
    }
    r.script.resolve_status.store(401, SeqCst);
    let mut c = connect(&r).await;
    c.write_all(request("/echo", &[], true).as_bytes())
        .await
        .unwrap();
    assert_eq!(read_head(&mut c).await.0, 502);
}

/// `validate` retries within its cycle when the API throttles it, so a revocation is not delayed
/// by a whole cycle per refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_retries_with_backoff_inside_one_cycle() {
    let cycle = Duration::from_millis(1500);
    let r = rig(
        Timeouts {
            validate_every: cycle,
            ..tm()
        },
        true,
    )
    .await;
    let origin = format!("https://{HOST}");
    let mut ws = ws_connect(&r, Some(&origin)).await.unwrap();
    echo(&mut ws, "alive").await;
    r.script.validate_fail.store(2, SeqCst);
    r.script
        .revoked
        .lock()
        .unwrap()
        .insert(aurora_relay::api::session_hash("auv_one"));
    until_closed(&mut ws, Duration::from_secs(10)).await;
    let calls = r.script.validate_calls.lock().unwrap().clone();
    assert!(calls.len() >= 3, "two refusals and the answer: {calls:?}");
    let spread = calls[2].duration_since(calls[0]);
    assert!(
        spread < cycle,
        "the three attempts came {spread:?} apart: not retried inside one cycle"
    );
}

// ------------------------------------------------------------------ endpoint

/// An endpoint the API names that carries userinfo (or is otherwise not host[:port]) is refused
/// before the stream grant leaves the relay: nothing connects.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runner_endpoint_with_userinfo_is_never_dialled() {
    let r = rig(tm(), false).await;
    let (spy, port) = bind_any().await;
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    {
        let hits = hits.clone();
        tokio::spawn(async move {
            while spy.accept().await.is_ok() {
                hits.fetch_add(1, SeqCst);
            }
        });
    }
    for bad in [
        format!("ws://user@127.0.0.1:{port}/v1/preview-stream"),
        format!("ws://127.0.0.1:{port}/v1/preview-stream?x=1"),
        format!("ws://127.0.0.1:{port}/elsewhere"),
        format!("http://127.0.0.1:{port}/v1/preview-stream"),
        format!("ws://127.0.0.1:{port}\\@evil/v1/preview-stream"),
    ] {
        *r.script.endpoint_override.lock().unwrap() = Some(bad.clone());
        let mut c = connect(&r).await;
        c.write_all(request("/echo", &[], true).as_bytes())
            .await
            .unwrap();
        let (status, _, _) = read_head(&mut c).await;
        assert_eq!(status, 502, "{bad}");
    }
    assert_eq!(
        hits.load(SeqCst),
        0,
        "a live grant was dialled to a bad endpoint"
    );
    // The control: a plain endpoint is dialled (the spy accepts the TCP connection).
    *r.script.endpoint_override.lock().unwrap() =
        Some(format!("ws://127.0.0.1:{port}/v1/preview-stream"));
    let mut c = connect(&r).await;
    c.write_all(request("/echo", &[], true).as_bytes())
        .await
        .unwrap();
    let _ = read_head(&mut c).await;
    assert_eq!(hits.load(SeqCst), 1);
}

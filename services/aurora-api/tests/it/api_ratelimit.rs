#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RFE-017 / SEC-50: management-API rate limiting (docs/05 §Conventions "Rate limiting").
//!
//! The shared harness scales every bucket by 100 so other suites never trip the limiter;
//! these tests reset the multiplier to 1 (or set explicit overrides) to prove the mechanism.

use crate::common::*;
use aurora_config::{ApiConfig, RateClassOverride};
use axum::body::Body;
use http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

fn real_limits(c: &mut ApiConfig) {
    c.ratelimit.api.multiplier = 1.0;
}

/// A class with `burst` tokens and **no meaningful refill** (1 per hour): the tests that use it
/// assert a refusal right after the burst, and a per-minute refill (60/min = one token per second)
/// let a token come back whenever the burst took over a second on a loaded host — the
/// `general_bucket_applies_to_every_authenticated_request` flake (2026-11-13, load ~20), the same
/// window `real_bursts_no_refill` below closed for the defaults table. Time is not injectable.
fn class(c: &mut ApiConfig, name: &str, burst: u32) {
    c.ratelimit.api.multiplier = 1.0;
    c.ratelimit.api.classes.insert(
        name.into(),
        RateClassOverride {
            per_hour: Some(1),
            burst: Some(burst),
            ..Default::default()
        },
    );
}

struct W {
    h: Harness,
    org: Uuid,
    user: Uuid,
    sess: String,
    other_sess: String,
    key: String,
}

async fn world(mutate: impl FnOnce(&mut ApiConfig)) -> W {
    let h = Harness::with_config(mutate).await;
    let org = h.org().await;
    let user = h.user("u@x.io", false).await;
    h.member(org, user, "owner", "active").await;
    let (_, sess) = h.good_session(user).await;
    let (_, other_sess) = h.good_session(user).await;
    let (_, key) = h.api_key(user, org, None, false).await;
    W {
        h,
        org,
        user,
        sess,
        other_sess,
        key,
    }
}

async fn as_session(h: &Harness, tok: &str, m: Method, uri: &str) -> StatusCode {
    let c = cookie(tok);
    h.send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], None)
        .await
        .0
}

async fn as_key(h: &Harness, tok: &str, m: Method, uri: &str) -> StatusCode {
    let b = bearer(tok);
    h.send(m, uri, &[("authorization", &b)], None).await.0
}

fn msg_uri() -> String {
    format!("/v1/conversations/{}/messages", Uuid::now_v7())
}

#[tokio::test]
async fn message_class_burst_then_429_with_full_envelope() {
    let w = world(real_limits).await;
    let uri = msg_uri();
    // burst 10: the handler answers (404: no such conversation), the limiter lets it through
    for i in 0..10 {
        let s = as_session(&w.h, &w.sess, Method::POST, &uri).await;
        assert_ne!(s, StatusCode::TOO_MANY_REQUESTS, "request {i}");
    }
    let c = cookie(&w.sess);
    let (s, hd, v) =
        w.h.send(
            Method::POST,
            &uri,
            &[("cookie", &c), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["error"]["code"], "rate_limited");
    let d = &v["error"]["details"];
    assert_eq!(d["scope"], "class");
    assert_eq!(d["class"], "message");
    assert_eq!(d["limit"], 30);
    assert_eq!(d["per"], "minute");
    // 30/min refills one token per 2 s: retry-after is exact, not a guess
    let ra: u64 = hd["retry-after"].to_str().unwrap().parse().unwrap();
    assert_eq!(ra, 2);
    assert_eq!(d["retry_after_secs"], 2);
    assert_eq!(hd["ratelimit-limit"], "30");
    assert_eq!(hd["ratelimit-remaining"], "0");
    assert_eq!(hd["ratelimit-reset"], "2");
    assert_eq!(hd["x-should-retry"], "true");
    // other classes of the same principal are untouched
    assert_ne!(
        as_session(&w.h, &w.sess, Method::GET, "/v1/me").await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

/// The real default bursts, with a refill so slow (1 token an hour) that no token can come back
/// while a test runs.
///
/// P2-TEST-1 (the `each_class_has_its_own_limit` flake): the limiter refills on the wall clock
/// (`Instant`), and the default `write` class refills 2 tokens a second (120/min, burst 30). The
/// test spends the whole burst and expects the next request to be refused; on a loaded machine
/// (load ~17) the 31 requests took longer than 0.5 s, one token came back, and the 31st request
/// was answered instead of refused. `upload`/`simulate` (20/min) and `search` (30/min) have the
/// same window, only wider. Time is not injectable into the limiter, so the test removes the
/// refill instead: the burst sizes under test are the defaults, only the refill rate differs.
fn real_bursts_no_refill(c: &mut ApiConfig) {
    c.ratelimit.api.multiplier = 1.0;
    for name in ["write", "message", "upload", "simulate", "search", "export"] {
        c.ratelimit.api.classes.insert(
            name.into(),
            RateClassOverride {
                per_hour: Some(1),
                ..Default::default()
            },
        );
    }
}

#[tokio::test]
async fn each_class_has_its_own_limit() {
    let w = world(real_bursts_no_refill).await;
    let ws = Uuid::now_v7();
    let cases: [(Method, String, u32, &str); 5] = [
        (
            Method::POST,
            format!("/v1/conversations/{ws}/attachments"),
            5,
            "upload",
        ),
        (
            Method::POST,
            format!("/v1/workspaces/{ws}/policy/simulate"),
            5,
            "simulate",
        ),
        (
            Method::GET,
            format!("/v1/agents/{ws}/memories/search?q=x"),
            10,
            "search",
        ),
        (
            Method::GET,
            format!("/v1/conversations/{ws}/export"),
            2,
            "export",
        ),
        (Method::DELETE, format!("/v1/agents/{ws}"), 30, "write"),
    ];
    for (m, uri, burst, name) in cases {
        for i in 0..burst {
            let s = as_key(&w.h, &w.key, m.clone(), &uri).await;
            assert_ne!(s, StatusCode::TOO_MANY_REQUESTS, "{name} #{i}");
        }
        let b = bearer(&w.key);
        let (s, hd, v) =
            w.h.send(m.clone(), &uri, &[("authorization", &b)], None)
                .await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{name}");
        assert_eq!(v["error"]["details"]["class"], name);
        assert!(hd.contains_key("retry-after"));
    }
}

#[tokio::test]
async fn general_bucket_applies_to_every_authenticated_request() {
    let w = world(|c| class(c, "general", 3)).await;
    for _ in 0..3 {
        assert_eq!(
            as_session(&w.h, &w.sess, Method::GET, "/v1/me").await,
            StatusCode::OK
        );
    }
    let c = cookie(&w.sess);
    let (s, _, v) =
        w.h.send(Method::GET, "/v1/me", &[("cookie", &c)], None)
            .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["error"]["details"]["scope"], "general");
    assert_eq!(v["error"]["details"]["class"], "general");
}

#[tokio::test]
async fn principals_are_isolated_and_a_users_sessions_share_one_budget() {
    let w = world(|c| class(c, "general", 2)).await;
    for _ in 0..2 {
        as_session(&w.h, &w.sess, Method::GET, "/v1/me").await;
    }
    // a second session of the same user buys nothing
    assert_eq!(
        as_session(&w.h, &w.other_sess, Method::GET, "/v1/me").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // a key (even of the same user) and another user are separate principals
    assert_eq!(
        as_key(&w.h, &w.key, Method::GET, "/v1/me").await,
        StatusCode::OK
    );
    let (_, key2) = w.h.api_key(w.user, w.org, None, false).await;
    assert_eq!(
        as_key(&w.h, &key2, Method::GET, "/v1/me").await,
        StatusCode::OK
    );
    let u2 = w.h.user("v@x.io", false).await;
    let (_, s2) = w.h.good_session(u2).await;
    assert_eq!(
        as_session(&w.h, &s2, Method::GET, "/v1/me").await,
        StatusCode::OK
    );
    // the hammering key is limited without affecting the others
    for _ in 0..2 {
        as_key(&w.h, &w.key, Method::GET, "/v1/me").await;
    }
    assert_eq!(
        as_key(&w.h, &w.key, Method::GET, "/v1/me").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        as_key(&w.h, &key2, Method::GET, "/v1/me").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn org_bucket_is_shared_by_all_principals_of_the_org() {
    // 10/min => burst 2 for the org
    let w = world(|c| {
        c.ratelimit.api.org_per_minute = 10;
        c.ratelimit.api.multiplier = 1.0;
    })
    .await;
    let (_, k2) = w.h.api_key(w.user, w.org, None, false).await;
    let (_, k3) = w.h.api_key(w.user, w.org, None, false).await;
    assert_eq!(
        as_key(&w.h, &w.key, Method::GET, "/v1/me").await,
        StatusCode::OK
    );
    assert_eq!(
        as_key(&w.h, &k2, Method::GET, "/v1/me").await,
        StatusCode::OK
    );
    let b = bearer(&k3);
    let (s, hd, v) =
        w.h.send(Method::GET, "/v1/me", &[("authorization", &b)], None)
            .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["error"]["details"]["scope"], "org");
    assert!(hd.contains_key("retry-after"));
}

#[tokio::test]
async fn a_non_member_cannot_drain_another_orgs_bucket() {
    let w = world(|c| {
        c.ratelimit.api.org_per_minute = 10; // burst 2
        c.ratelimit.api.multiplier = 1.0;
    })
    .await;
    let outsider = w.h.user("out@x.io", false).await;
    let (_, osess) = w.h.good_session(outsider).await;
    let uri = format!("/v1/orgs/{}", w.org);
    for _ in 0..6 {
        assert_eq!(
            as_session(&w.h, &osess, Method::GET, &uri).await,
            StatusCode::NOT_FOUND
        );
    }
    // the member still has the org's whole burst
    assert_eq!(
        as_session(&w.h, &w.sess, Method::GET, &uri).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn refused_requests_never_reach_the_handler() {
    let w = world(|c| class(c, "message", 1)).await;
    let uri = msg_uri();
    assert_ne!(
        as_key(&w.h, &w.key, Method::POST, &uri).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // a malformed body would be a 400 from the handler; the limiter answers first
    let b = bearer(&w.key);
    let (s, _, _) =
        w.h.send(
            Method::POST,
            &uri,
            &[("authorization", &b)],
            Some(json!({"nonsense": true})),
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn exempt_routes_are_never_limited() {
    let w = world(|c| {
        class(c, "general", 1);
        class(c, "write", 1);
    })
    .await;
    for _ in 0..8 {
        // webhook ingress and machine planes apply their own limits / auth
        let (s, _, _) =
            w.h.send(Method::POST, "/v1/hooks/aug_nope", &[], Some(json!({})))
                .await;
        assert_ne!(s, StatusCode::TOO_MANY_REQUESTS);
        let (s, _, _) =
            w.h.send(
                Method::GET,
                "/v1/runner/work",
                &[("authorization", "Bearer aur_bad")],
                None,
            )
            .await;
        assert_ne!(s, StatusCode::TOO_MANY_REQUESTS);
        let (s, _, _) =
            w.h.send(
                Method::POST,
                "/v1/connector/ops/claim",
                &[("authorization", "Bearer aun_bad")],
                Some(json!({})),
            )
            .await;
        assert_ne!(s, StatusCode::TOO_MANY_REQUESTS);
    }
}

#[tokio::test]
async fn unauthenticated_and_rejected_bearer_requests_are_limited_per_ip() {
    let w = world(real_limits).await;
    // The per-IP buckets refill on the wall clock and their rates are constants (only the
    // multiplier scales them), so on a loaded machine a token can come back while the burst is
    // being spent. Instead of "exactly burst, then 429", assert what the limiter guarantees: at
    // least `burst` requests pass, then a 429 arrives no later than burst + the tokens that could
    // have refilled in the elapsed time (+1 for a partial token).
    let allowed_until_429 = |passed: u32, per_min: f64, burst: u32, started: std::time::Instant| {
        let refill = (started.elapsed().as_secs_f64() * per_min / 60.0).ceil() as u32 + 1;
        assert!(
            passed >= burst,
            "only {passed} passed before 429, burst is {burst}"
        );
        assert!(
            passed <= burst + refill,
            "{passed} passed before 429; burst {burst} + refill {refill} allows at most {}",
            burst + refill
        );
    };

    // public GETs: 120/min/IP
    let started = std::time::Instant::now();
    let mut passed = 0u32;
    let (hd, v) = loop {
        let (s, hd, v) = w.h.send(Method::GET, "/v1/instance", &[], None).await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            break (hd, v);
        }
        passed += 1;
        assert!(passed <= 1000, "no 429 after {passed} public GETs");
    };
    allowed_until_429(passed, 120.0, 120, started);
    assert_eq!(v["error"]["details"]["class"], "unauth");
    assert!(hd.contains_key("retry-after"));

    // rejected bearer: 30/min/IP, tracked separately from the public-GET bucket
    let started = std::time::Instant::now();
    let mut passed = 0u32;
    let v = loop {
        let (s, _, v) =
            w.h.send(
                Method::GET,
                "/v1/me",
                &[("authorization", "Bearer auo_not_a_real_key")],
                None,
            )
            .await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            break v;
        }
        assert_eq!(s, StatusCode::UNAUTHORIZED, "request {passed}");
        passed += 1;
        assert!(
            passed <= 1000,
            "no 429 after {passed} rejected bearer requests"
        );
    };
    allowed_until_429(passed, 30.0, 30, started);
    assert_eq!(v["error"]["code"], "rate_limited");
}

/// FIX-090: a relay replica's `aul_` bearer is charged to its own preview class (100/s, burst
/// 200), never as a rejected bearer to the relay pod's address (30/min): 200 calls in a row with a
/// live token are not refused; the same calls with a dead token hit the rejected-bearer limit.
#[tokio::test]
async fn relay_plane_calls_are_charged_to_the_preview_class_not_as_rejected_bearers() {
    use aurora_core::token::{self, TokenPrefix};
    let w = world(real_limits).await;
    let live = token::generate(TokenPrefix::Relay).expose().to_owned();
    let token_id = Uuid::now_v7();
    sqlx::query("INSERT INTO relay_tokens (id, token_hash) VALUES ($1,$2)")
        .bind(token_id)
        .bind(&token::hash(&live)[..])
        .execute(w.h.pool())
        .await
        .unwrap();
    let replica = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relay_replicas (id, token_id, version, last_seen_at) VALUES ($1,$2,'t',now())",
    )
    .bind(replica)
    .bind(token_id)
    .execute(w.h.pool())
    .await
    .unwrap();
    let call = |tok: String, rep: Option<Uuid>| {
        let h = &w.h;
        async move {
            let b = bearer(&tok);
            let r = rep.map(|r| r.to_string());
            let mut hd = vec![
                ("authorization", b.as_str()),
                ("x-aurora-protocol", "relay/1"),
            ];
            if let Some(r) = &r {
                hd.push(("x-aurora-replica", r.as_str()));
            }
            h.send(
                Method::POST,
                "/v1/relay/resolve",
                &hd,
                Some(json!({"label": "p0000000000000000000000000"})),
            )
            .await
            .0
        }
    };
    // 200 calls with the live token (previews are off in this rig: the handler answers 404, which
    // is not the point — the limiter must let every one of them through).
    for i in 0..200 {
        let s = call(live.clone(), Some(replica)).await;
        assert_ne!(
            s,
            StatusCode::TOO_MANY_REQUESTS,
            "live relay call {i} was throttled"
        );
    }
    // A dead token (rotated away, never issued) is an ordinary rejected bearer: 30 per minute.
    let dead = token::generate(TokenPrefix::Relay).expose().to_owned();
    let mut first_429 = None;
    for i in 0..100 {
        if call(dead.clone(), None).await == StatusCode::TOO_MANY_REQUESTS {
            first_429 = Some(i);
            break;
        }
    }
    let n = first_429.expect("a dead relay token must hit the rejected-bearer limit");
    assert!((29..=40).contains(&n), "refused after {n} dead-token calls");
}

#[tokio::test]
async fn disabled_limiter_lets_everything_through() {
    let w = world(|c| {
        c.ratelimit.api.enabled = false;
        c.ratelimit.api.multiplier = 1.0;
    })
    .await;
    let uri = msg_uri();
    for _ in 0..40 {
        assert_ne!(
            as_key(&w.h, &w.key, Method::POST, &uri).await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}

#[tokio::test]
async fn multiplier_scales_every_bucket() {
    let w = world(|c| c.ratelimit.api.multiplier = 0.1).await;
    // message burst 10 * 0.1 = 1
    let uri = msg_uri();
    assert_ne!(
        as_key(&w.h, &w.key, Method::POST, &uri).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        as_key(&w.h, &w.key, Method::POST, &uri).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

async fn open_stream(h: &Harness, sess: &str, uri: &str) -> http::Response<Body> {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header("cookie", cookie(sess))
        .body(Body::empty())
        .unwrap();
    h.app.clone().oneshot(req).await.unwrap()
}

#[tokio::test]
async fn stream_cap_is_per_principal_and_released_on_drop() {
    let w = world(|c| c.ratelimit.api.multiplier = 100.0).await;
    let u2 = w.h.user("v@x.io", false).await;
    let (_, s2) = w.h.good_session(u2).await;
    let mut held = Vec::new();
    for i in 0..20 {
        let r = open_stream(&w.h, &w.sess, "/v1/approvals/stream").await;
        assert_eq!(r.status(), StatusCode::OK, "stream {i}");
        held.push(r);
    }
    // all of the user's sessions share the cap
    let r = open_stream(&w.h, &w.other_sess, "/v1/approvals/stream").await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = http_body_util::BodyExt::collect(r.into_body())
        .await
        .unwrap()
        .to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "too_many_streams");
    assert_eq!(v["error"]["details"]["scope"], "streams");
    // the cap also covers long-polls of the same principal
    let lp = format!("/v1/workspaces/{}/approvals?wait=1", Uuid::now_v7());
    assert_eq!(
        as_session(&w.h, &w.sess, Method::GET, &lp).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // ordinary requests of that principal still work
    assert_eq!(
        as_session(&w.h, &w.sess, Method::GET, "/v1/me").await,
        StatusCode::OK
    );
    // another principal is unaffected
    let r2 = open_stream(&w.h, &s2, "/v1/approvals/stream").await;
    assert_eq!(r2.status(), StatusCode::OK);
    // dropping one connection frees exactly one slot
    held.pop();
    let r = open_stream(&w.h, &w.sess, "/v1/approvals/stream").await;
    assert_eq!(r.status(), StatusCode::OK);
    held.push(r);
    let r = open_stream(&w.h, &w.sess, "/v1/approvals/stream").await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
}

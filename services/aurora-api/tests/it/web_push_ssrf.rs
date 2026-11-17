#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-011B-3 (SEC-76, D-267(b)): Web Push cannot be used to reach internal networks. Registration
//! refuses every endpoint a push service is not; a send never follows a redirect, is bounded in
//! time and in what it reads, and a name that does not resolve to a public address fails closed.
//! (Startup refusals of an empty or invalid allow-list and of `push.dev_endpoint_overrides` with a
//! non-loopback target or external URL are proven by
//! `aurora-config` `push_validation_follows_the_switch_and_the_override_rules`; the resolver's
//! private-address judgement by the `deliver` unit test `the_resolver_judges_what_a_name_resolves_to_now`.)

use crate::web_push::*;
use aurora_api::domains::push::deliver;
use aurora_testkit::push::Answer;
use http::Method;
use serde_json::json;
use std::time::{Duration, Instant};

async fn failures(w: &W) -> (i32, bool) {
    sqlx::query_as(
        "SELECT failure_count, last_success_at IS NOT NULL FROM push_subscriptions LIMIT 1",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn registration_refuses_what_a_push_service_is_not() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let k = w.mock.subscribe("/x");
    let long = format!("https://{HOST}/{}", "a".repeat(2100));
    for (endpoint, reason) in [
        ("http://fcm.googleapis.com/x".to_owned(), "not_https"),
        (
            "https://user:pw@fcm.googleapis.com/x".to_owned(),
            "userinfo",
        ),
        ("https://127.0.0.1/x".to_owned(), "ip_literal"),
        ("https://10.0.0.5/x".to_owned(), "ip_literal"),
        ("https://169.254.169.254/latest".to_owned(), "ip_literal"),
        ("https://[::1]/x".to_owned(), "ip_literal"),
        (
            "https://fcm.googleapis.com.evil.example/x".to_owned(),
            "host_not_allowed",
        ),
        (
            "https://evilfcm.googleapis.com/x".to_owned(),
            "host_not_allowed",
        ),
        ("https://internal.example/x".to_owned(), "host_not_allowed"),
        (
            "https://fcm.googleapis.com:8443/x".to_owned(),
            "host_not_allowed",
        ),
        (long, "too_long"),
    ] {
        let mut body = reg_body(&w, "/x", &k, false);
        body["endpoint"] = json!(endpoint);
        let (s, _, v) = call(
            &w,
            &a,
            Method::POST,
            "/v1/me/push-subscriptions",
            Some(body),
        )
        .await;
        assert_eq!(s, 422, "{endpoint}: {v}");
        assert_eq!(v["error"]["code"], "push_endpoint_not_allowed");
        assert_eq!(v["error"]["details"]["reason"], reason, "{endpoint}");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing was created");
    assert!(w.mock.received().is_empty());
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    register(&w, &a, "/push/a", false).await;
    w.mock.respond(Answer {
        redirect_to: Some(format!("{}/elsewhere", w.mock.url())),
        ..Answer::status(307)
    });
    approval(&w, None, false).await;
    age(&w).await;
    sweep(&w).await;
    let got = w.mock.received();
    assert_eq!(got.len(), 1, "one request, no second hop: {got:?}");
    assert_eq!(got[0].path, "/push/a");
    assert_eq!(failures(&w).await, (1, false), "a redirect is a failure");
}

#[tokio::test]
async fn a_slow_service_is_cut_off_at_the_timeout() {
    let w = world_with(|c| c.push.timeout_secs = 2).await;
    let a = person(&w, "a@x.io", "approver").await;
    register(&w, &a, "/push/a", false).await;
    w.mock.respond(Answer {
        delay: Duration::from_secs(8),
        ..Answer::created()
    });
    approval(&w, None, false).await;
    age(&w).await;
    let started = Instant::now();
    sweep(&w).await;
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "bounded: {:?}",
        started.elapsed()
    );
    assert_eq!(failures(&w).await, (1, false));
}

#[tokio::test]
async fn an_oversized_answer_is_not_read() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    register(&w, &a, "/push/a", false).await;
    w.mock.respond(Answer {
        body_len: 16 * 1024 * 1024,
        ..Answer::created()
    });
    approval(&w, None, false).await;
    age(&w).await;
    let started = Instant::now();
    sweep(&w).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        failures(&w).await,
        (0, true),
        "the status decides; the body is ignored"
    );
}

#[tokio::test]
async fn a_name_that_is_not_a_public_host_fails_closed_without_the_test_seam() {
    // `*.invalid` never resolves (RFC 6761); no override is configured for it, so the send goes
    // through the real destination-guarded client and fails before any connection.
    let w = world_with(|c| {
        c.push
            .allowed_endpoint_hosts
            .push("push.unresolvable.invalid".into())
    })
    .await;
    let a = person(&w, "a@x.io", "approver").await;
    let k = w.mock.subscribe("/z");
    let mut body = reg_body(&w, "/z", &k, false);
    body["endpoint"] = json!("https://push.unresolvable.invalid/z");
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(body),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    approval(&w, None, false).await;
    age(&w).await;
    assert_eq!(deliver::sweep_once(&w.h.state).await.unwrap(), 1);
    assert_eq!(failures(&w).await, (1, false));
    assert!(
        w.mock.received().is_empty(),
        "nothing reached the loopback service"
    );
}

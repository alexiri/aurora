#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SCIM authentication and the instance switch (SEC-72, docs/18 §SCIM): `/scim/v2` is 404
//! with `scim.enabled = false`, 401 (one identical body) for every bad credential, and an
//! `aud_` token is useless on `/v1/**`.

use crate::common::*;
use crate::scim_common::*;
use aurora_config::SignupMode;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const USERS: &str = "/scim/v2/Users";

#[tokio::test]
async fn instance_switch_off_is_404_everywhere() {
    // `scim.enabled` defaults to false.
    let h = Harness::with_config(|c| {
        c.signup_mode = SignupMode::Open;
        c.org_creation = aurora_config::OrgCreation::Anyone;
    })
    .await;
    let owner = h.sign_in("owner@x.io").await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/orgs",
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"name": "Acme"})),
        )
        .await;
    assert_eq!(s, http::StatusCode::CREATED, "{v}");
    let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
    // The management GET still works and reports `available: false`; PUT is refused
    // (409 `scim_unavailable`, docs/05 §SCIM).
    let (s, v) = h
        .get(
            &format!("/v1/orgs/{org}/scim"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
        )
        .await;
    assert_eq!(s, http::StatusCode::OK, "{v}");
    assert_eq!(v["available"], false, "{v}");
    let (s, _, v) = h
        .send(
            Method::PUT,
            &format!("/v1/orgs/{org}/scim"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"enabled": true, "email_domains": ["x.io"]})),
        )
        .await;
    assert_eq!(s, http::StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "scim_unavailable", "{v}");
    // Token creation is refused too.
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/orgs/{org}/scim/tokens"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"name": "t"})),
        )
        .await;
    assert_eq!(s, http::StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "scim_unavailable", "{v}");
    // The plane itself: 404 with the /v1 error envelope (not a SCIM error — no oracle).
    for uri in [USERS, "/scim/v2/Groups", "/scim/v2/Schemas"] {
        let (s, v) = h
            .get(uri, &[("authorization", "Bearer aud_whatever")])
            .await;
        assert_eq!(s, http::StatusCode::NOT_FOUND, "{uri}: {v}");
        assert_eq!(v["error"]["code"], "not_found", "{uri}: {v}");
    }
}

#[tokio::test]
async fn every_auth_failure_is_one_identical_401() {
    let fx = ScimFx::new().await;
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no token", vec![]),
        ("malformed", vec![("authorization", "Bearer not-a-token")]),
        (
            "other prefix auk",
            vec![("authorization", "Bearer auk_abcdef0123456789")],
        ),
        (
            "other prefix aug",
            vec![("authorization", "Bearer aug_abcdef0123456789")],
        ),
        ("session cookie only", vec![]),
    ];
    let mut bodies: Vec<Value> = Vec::new();
    for (what, headers) in &cases {
        let hdrs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, *v)).collect();
        let (s, hd, v) = fx.h.send(Method::GET, USERS, &hdrs, None).await;
        assert_eq!(s, http::StatusCode::UNAUTHORIZED, "{what}: {v}");
        assert_scim_error(401, &v);
        assert_eq!(
            hd["www-authenticate"].to_str().unwrap(),
            "Bearer",
            "{what}: WWW-Authenticate"
        );
        bodies.push(v);
    }
    // Malformed prefix and unknown token are the one identical body.
    for b in &bodies {
        assert_eq!(b, &bodies[0], "every 401 is the same body");
    }
}

/// The SCIM runbook's "alert on a rising 401 rate" rests on these counters: every failed
/// authentication adds to `aurora_scim_auth_failures_total` and to the `unauthorized`
/// request outcome, a good request to `ok`. Other tests share the process-wide recorder
/// and only ever add, so the assertions are lower bounds on the delta.
#[tokio::test]
async fn failed_auth_increments_the_scim_auth_failure_counter() {
    const FAILS: &str = "aurora_scim_auth_failures_total";
    const R401: &str = "aurora_scim_requests_total{outcome=\"unauthorized\"}";
    const ROK: &str = "aurora_scim_requests_total{outcome=\"ok\"}";
    // Installs the recorder before the first request, so nothing is lost.
    let (f0, u0, ok0) = (
        counter_value(FAILS),
        counter_value(R401),
        counter_value(ROK),
    );
    let fx = ScimFx::new().await;
    for hdrs in [
        vec![],
        vec![("authorization", "Bearer not-a-token")],
        vec![(
            "authorization",
            "Bearer aud_0000000000000000000000000000000000000000",
        )],
    ] {
        let (s, _, _) = fx.h.send(Method::GET, USERS, &hdrs, None).await;
        assert_eq!(s, http::StatusCode::UNAUTHORIZED);
    }
    assert!(counter_value(FAILS) >= f0 + 3.0, "auth failures counted");
    assert!(counter_value(R401) >= u0 + 3.0, "401 outcomes counted");
    let bearer = format!("Bearer {}", fx.token);
    let (s, _, _) =
        fx.h.send(
            Method::GET,
            USERS,
            &[("authorization", bearer.as_str())],
            None,
        )
        .await;
    assert_eq!(s, http::StatusCode::OK);
    assert!(counter_value(ROK) >= ok0 + 1.0, "ok outcome counted");
}

#[tokio::test]
async fn revoked_and_expired_and_org_disabled_are_401() {
    let fx = ScimFx::new().await;
    // Revoked: revoke through the management route, then the plane refuses.
    let (s, v) = fx
        .mgmt(
            Method::GET,
            &format!("/v1/orgs/{}/scim/tokens", fx.org),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let id: Uuid = v["tokens"][0]["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = fx
        .mgmt(Method::DELETE, &format!("/v1/scim/tokens/{id}"), None)
        .await;
    assert_eq!(s, 204);
    let (s, _, v) = fx.scim(Method::GET, USERS, None).await;
    assert_eq!(s, http::StatusCode::UNAUTHORIZED, "{v}");

    // Expired: mint another, then wind its expiry into the past.
    let tok = mint_token(&fx.h, &fx.owner.cookie, fx.org).await;
    sqlx::query("UPDATE scim_tokens SET expires_at = now() - interval '1 min'")
        .execute(fx.h.pool())
        .await
        .unwrap();
    let (s, _, v) = fx.scim_with(Method::GET, USERS, &bearer(&tok), None).await;
    assert_eq!(s, http::StatusCode::UNAUTHORIZED, "{v}");

    // Org-disabled: the token row lives, but the org switched SCIM off.
    let tok2 = mint_token(&fx.h, &fx.owner.cookie, fx.org).await;
    let (s, _) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/orgs/{}/scim", fx.org),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, _, v) = fx.scim_with(Method::GET, USERS, &bearer(&tok2), None).await;
    assert_eq!(s, http::StatusCode::UNAUTHORIZED, "{v}");
    assert_scim_error(401, &v);
}

#[tokio::test]
async fn aud_token_is_useless_on_v1_and_session_ignored_on_scim() {
    let fx = ScimFx::new().await;
    // `aud_` on `/v1`: 401 (the SCIM token is not a /v1 credential).
    let auth = [("authorization", format!("Bearer {}", fx.token))];
    let hdrs: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, v) = fx.h.get("/v1/me", &hdrs).await;
    assert_eq!(s, http::StatusCode::UNAUTHORIZED, "{v}");
    // The org key of a member is equally useless on the plane (wrong prefix).
    let u = fx.h.user("m@x.io", false).await;
    fx.h.member(fx.org, u, "member", "active").await;
    let (_, org_key) = fx.h.api_key(u, fx.org, None, false).await;
    let (s, _, v) = fx
        .scim_with(Method::GET, USERS, &bearer(&org_key), None)
        .await;
    assert_eq!(s, http::StatusCode::UNAUTHORIZED, "{v}");
}

#[tokio::test]
async fn token_lifecycle_rules() {
    let fx = ScimFx::new().await;
    // Owner-only: an admin gets 403 on the token route.
    let admin = fx.h.user("admin@x.io", false).await;
    fx.h.member(fx.org, admin, "admin", "active").await;
    let (_, tok) = fx.h.good_session(admin).await;
    let (s, _, v) =
        fx.h.send(
            Method::POST,
            &format!("/v1/orgs/{}/scim/tokens", fx.org),
            &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
            Some(json!({"name": "t"})),
        )
        .await;
    assert_eq!(s, http::StatusCode::FORBIDDEN, "{v}");

    // Hash only: no cleartext in the database.
    let raw: String =
        sqlx::query_scalar("SELECT count(*)::text FROM scim_tokens WHERE token_hash = $1")
            .bind(fx.token.as_bytes())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(raw, "0", "the cleartext token is never stored");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM scim_tokens")
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1, "exactly the hashed row");

    // Expiry cap: the instance maximum (730 d) clamps a bigger request.
    let tok = mint_token(&fx.h, &fx.owner.cookie, fx.org).await;
    let _ = tok;
    let (s, v) = fx
        .mgmt(
            Method::POST,
            &format!("/v1/orgs/{}/scim/tokens", fx.org),
            Some(json!({"name": "long", "expires_in_days": 100_000})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let days: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(epoch FROM (expires_at - created_at))::float8 / 86400 FROM scim_tokens WHERE name = 'long'",
    )
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert!(
        (days - 730.0).abs() < 1.0,
        "expiry capped at 730 d, got {days}"
    );

    // Five-token limit: the org already holds 3 (the fixture's + it + long); go to five,
    // the sixth is 422.
    for i in 0..2 {
        let (s, v) = fx
            .mgmt(
                Method::POST,
                &format!("/v1/orgs/{}/scim/tokens", fx.org),
                Some(json!({"name": format!("t{i}")})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
    let (s, v) = fx
        .mgmt(
            Method::POST,
            &format!("/v1/orgs/{}/scim/tokens", fx.org),
            Some(json!({"name": "sixth"})),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "scim_token_limit_reached", "{v}");
}

/// FIX-062: the token cap holds under concurrency — creates serialize on the org lock
/// before counting, so parallel requests never push the org past `SCIM_TOKENS_PER_ORG`.
#[tokio::test]
async fn concurrent_token_creates_respect_the_cap() {
    const N: usize = 8;
    let fx = ScimFx::new().await; // one live token already
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(N));
    let rs = futures::future::join_all((0..N).map(|i| {
        let barrier = barrier.clone();
        let fx = &fx;
        async move {
            barrier.wait().await;
            fx.mgmt(
                Method::POST,
                &format!("/v1/orgs/{}/scim/tokens", fx.org),
                Some(json!({"name": format!("race-{i}")})),
            )
            .await
        }
    }))
    .await;
    let created = rs.iter().filter(|(s, _)| *s == 201).count();
    for (s, v) in &rs {
        assert!(
            *s == 201 || (*s == 422 && v["error"]["code"] == "scim_token_limit_reached"),
            "{s}: {v}"
        );
    }
    assert_eq!(
        created,
        aurora_core::limits::SCIM_TOKENS_PER_ORG - 1,
        "{rs:?}"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM scim_tokens WHERE org_id = $1 AND revoked_at IS NULL",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, aurora_core::limits::SCIM_TOKENS_PER_ORG as i64);
}

#[tokio::test]
async fn token_expiring_notices_14d_and_1d() {
    let fx = ScimFx::new().await;
    // Wind the token to 13 days: the 14-day notice is due once.
    sqlx::query("UPDATE scim_tokens SET expires_at = now() + interval '13 days'")
        .execute(fx.h.pool())
        .await
        .unwrap();
    aurora_api::jobs::sweep_scim_token_expiry(fx.h.pool(), None)
        .await
        .unwrap();
    let n14 = count(
        &fx.h,
        "SELECT count(*) FROM audit_log WHERE action = 'scim_token.expiry_notice'",
        fx.org,
    )
    .await;
    assert_eq!(n14, 1, "the 14-day notice fires once");
    // Re-running the same day does not duplicate it.
    aurora_api::jobs::sweep_scim_token_expiry(fx.h.pool(), None)
        .await
        .unwrap();
    let n = count(
        &fx.h,
        "SELECT count(*) FROM audit_log WHERE action = 'scim_token.expiry_notice'",
        fx.org,
    )
    .await;
    assert_eq!(n, 1);
    // Past the 1-day threshold the second notice is due.
    sqlx::query("UPDATE scim_tokens SET expires_at = now() + interval '6 hours'")
        .execute(fx.h.pool())
        .await
        .unwrap();
    aurora_api::jobs::sweep_scim_token_expiry(fx.h.pool(), None)
        .await
        .unwrap();
    let n = count(
        &fx.h,
        "SELECT count(*) FROM audit_log WHERE action = 'scim_token.expiry_notice'",
        fx.org,
    )
    .await;
    assert_eq!(n, 2, "the 1-day notice fires");
}

/// FIX-081: the expiry notice reaches the org's owners and admins by e-mail (the channel quota
/// notices use), once per recipient and threshold, and never carries token material.
#[tokio::test]
async fn token_expiry_notice_is_mailed_once_per_owner_and_threshold() {
    let fx = ScimFx::new().await;
    let admin = fx.h.user("admin-notice@x.io", false).await;
    fx.h.member(fx.org, admin, "admin", "active").await;
    let mail = aurora_api::domains::attachments::sweep::QuotaMail {
        mailer: fx.h.mail.clone(),
        external_url: "https://aurora.example".to_owned(),
    };
    fx.h.mail.clear();
    let sweep = || aurora_api::jobs::sweep_scim_token_expiry(fx.h.pool(), Some(&mail));
    sqlx::query("UPDATE scim_tokens SET expires_at = now() + interval '13 days'")
        .execute(fx.h.pool())
        .await
        .unwrap();
    sweep().await.unwrap();
    let recipients = |n: usize| {
        let mut to: Vec<String> = fx.h.mail.sent().into_iter().map(|m| m.to).collect();
        to.sort();
        assert_eq!(to.len(), n, "{to:?}");
        to
    };
    assert_eq!(
        recipients(2),
        vec!["admin-notice@x.io".to_owned(), "owner@x.io".to_owned()]
    );
    // The next pass of the same day mails nobody again.
    sweep().await.unwrap();
    recipients(2);
    // The 1-day threshold is a second notice per recipient.
    sqlx::query("UPDATE scim_tokens SET expires_at = now() + interval '6 hours'")
        .execute(fx.h.pool())
        .await
        .unwrap();
    sweep().await.unwrap();
    recipients(4);
    for m in fx.h.mail.sent() {
        assert!(m.link.ends_with("/settings/directory"), "{}", m.link);
        assert!(
            !m.body.contains(&fx.token) && !m.subject.contains(&fx.token),
            "token material in a notice"
        );
    }
}

#[tokio::test]
async fn token_never_in_any_response_or_log_or_audit() {
    let fx = ScimFx::new().await;
    // List shows the display prefix only.
    let (s, v) = fx
        .mgmt(
            Method::GET,
            &format!("/v1/orgs/{}/scim/tokens", fx.org),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(!v.to_string().contains(&fx.token), "raw token leaked");
    assert!(v["tokens"][0]["display_prefix"].as_str().is_some(), "{v}");
    // Audit rows and responses never carry it.
    let (s, _, v) = fx.scim(Method::GET, USERS, None).await;
    assert_eq!(s, http::StatusCode::OK, "{v}");
    let audit: String = sqlx::query_scalar(
        "SELECT coalesce(string_agg(metadata::text, ' '), '') FROM audit_log WHERE org_id = $1",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert!(!audit.contains(&fx.token), "token in audit rows");
}

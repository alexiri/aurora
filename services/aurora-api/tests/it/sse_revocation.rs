#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-005 (FUP-M2): SSE responses end promptly when the authenticating session / API key is
//! revoked, the session expires or the user is suspended (docs/04 §Sessions).

use crate::common::*;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::json;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct Fx {
    h: Harness,
    app: axum::Router,
    user: Uuid,
    sess: Uuid,
    cookie: String,
    key_id: Uuid,
    key: String,
    conv: Uuid,
}

/// `recheck` is the periodic re-validation interval of the streams under test.
async fn fx(recheck: Duration) -> Fx {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let user = h.user("m@x.io", false).await;
    h.member(org, user, "member", "active").await;
    h.bind_ws(ws, user, "member").await;
    let (sess, tok) = h.good_session(user).await;
    let (key_id, key) = h.api_key(user, org, Some(ws), false).await;
    let app = test_router(h.state.clone().with_stream_recheck(recheck));
    let cookie = cookie(&tok);
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/agents/{agent}/conversations/direct"),
            &[("cookie", &cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let conv = v["id"].as_str().unwrap().parse().unwrap();
    Fx {
        h,
        app,
        user,
        sess,
        cookie,
        key_id,
        key: bearer(&key),
        conv,
    }
}

async fn open(f: &Fx, uri: &str, hdr: (&str, &str)) -> Body {
    let res = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(hdr.0, hdr.1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    Body::new(res.into_body())
}

/// Drain until the body ends; false when it is still open after `wait`.
async fn ends_within(body: &mut Body, wait: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Err(_) => return false,
            Ok(None) | Ok(Some(Err(_))) => return true,
            Ok(Some(Ok(_))) => {}
        }
    }
}

fn conv_uri(f: &Fx) -> String {
    format!("/v1/conversations/{}/stream", f.conv)
}

#[tokio::test]
async fn stream_stays_open_while_the_session_is_live() {
    let f = fx(Duration::from_millis(100)).await;
    let mut b = open(&f, &conv_uri(&f), ("cookie", &f.cookie)).await;
    assert!(!ends_within(&mut b, Duration::from_millis(800)).await);
}

#[tokio::test]
async fn conversation_stream_ends_on_session_revocation_via_notify_path() {
    // Periodic re-check effectively disabled: only the revocation ping can end it.
    let f = fx(Duration::from_secs(3600)).await;
    let mut b = open(&f, &conv_uri(&f), ("cookie", &f.cookie)).await;
    assert!(!ends_within(&mut b, Duration::from_millis(300)).await);
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1")
        .bind(f.sess)
        .execute(f.h.pool())
        .await
        .unwrap();
    // What the aurora_auth_revoked listener does on NOTIFY.
    f.h.state
        .session_cache
        .invalidate(&json!({"kind": "session", "id": f.sess}));
    assert!(ends_within(&mut b, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn conversation_stream_ends_on_session_revocation_via_periodic_check() {
    // No NOTIFY at all (lost / listener down): the periodic re-check ends it.
    let f = fx(Duration::from_millis(200)).await;
    let mut b = open(&f, &conv_uri(&f), ("cookie", &f.cookie)).await;
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1")
        .bind(f.sess)
        .execute(f.h.pool())
        .await
        .unwrap();
    assert!(ends_within(&mut b, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn approvals_stream_ends_on_sign_out_expiry_and_suspension() {
    for what in ["revoke", "expire", "suspend"] {
        let f = fx(Duration::from_millis(200)).await;
        let mut b = open(&f, "/v1/approvals/stream", ("cookie", &f.cookie)).await;
        assert!(
            !ends_within(&mut b, Duration::from_millis(500)).await,
            "{what}"
        );
        let q = match what {
            "revoke" => "UPDATE sessions SET revoked_at = now() WHERE id = $1",
            "expire" => {
                "UPDATE sessions SET expires_at = now() - interval '1 second' WHERE id = $1"
            }
            _ => {
                "UPDATE users SET disabled_at = now() WHERE id = (SELECT user_id FROM sessions WHERE id = $1)"
            }
        };
        sqlx::query(q)
            .bind(f.sess)
            .execute(f.h.pool())
            .await
            .unwrap();
        assert!(ends_within(&mut b, Duration::from_secs(5)).await, "{what}");
    }
}

#[tokio::test]
async fn conversation_stream_ends_on_api_key_revocation_and_suspension() {
    // revoked key (NOTIFY path)
    let f = fx(Duration::from_secs(3600)).await;
    let mut b = open(&f, &conv_uri(&f), ("authorization", &f.key)).await;
    assert!(!ends_within(&mut b, Duration::from_millis(300)).await);
    sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1")
        .bind(f.key_id)
        .execute(f.h.pool())
        .await
        .unwrap();
    f.h.state
        .session_cache
        .invalidate(&json!({"kind": "api_key", "id": f.key_id}));
    assert!(ends_within(&mut b, Duration::from_secs(5)).await);

    // suspended owner (periodic path)
    let f = fx(Duration::from_millis(200)).await;
    let mut b = open(&f, &conv_uri(&f), ("authorization", &f.key)).await;
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
        .bind(f.user)
        .execute(f.h.pool())
        .await
        .unwrap();
    assert!(ends_within(&mut b, Duration::from_secs(5)).await);
}

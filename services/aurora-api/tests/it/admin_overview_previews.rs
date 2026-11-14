#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-148 (AUDIT-004 F-4, C-841): `GET /v1/admin/overview` carries the Previews line — the
//! relay replicas the API has seen — only while `previews.enabled`.

use crate::common::*;
use aurora_core::token::{self, TokenPrefix};
use uuid::Uuid;

async fn admin_cookie(h: &Harness) -> String {
    let root = h.user("root@x.io", true).await;
    cookie(&h.good_session(root).await.1)
}

async fn admin_overview(h: &Harness, c: &str) -> serde_json::Value {
    let (st, v) = h.get("/v1/admin/overview", &[("cookie", c)]).await;
    assert_eq!(st, 200, "{v}");
    v
}

#[tokio::test]
async fn overview_previews_line_counts_the_relay_replicas() {
    let h = Harness::with_config(|c| {
        c.previews.enabled = true;
        c.previews.domain = Some("preview.example.test".into());
    })
    .await;
    let c = admin_cookie(&h).await;
    // No replica yet: the line is there, empty.
    let v = admin_overview(&h, &c).await;
    assert_eq!(v["previews"]["relay_replicas"], 0, "{v}");
    assert!(v["previews"].get("last_seen_at").is_none(), "{v}");

    let tok = token::generate(TokenPrefix::Relay).expose().to_owned();
    let token_id = Uuid::now_v7();
    sqlx::query("INSERT INTO relay_tokens (id, token_hash) VALUES ($1,$2)")
        .bind(token_id)
        .bind(&token::hash(&tok)[..])
        .execute(h.pool())
        .await
        .unwrap();
    for (version, age) in [
        ("1.0.0", "1 minute"),
        ("1.0.0", "2 minutes"),
        ("old", "2 hours"),
    ] {
        sqlx::query(
            "INSERT INTO relay_replicas (id, token_id, version, last_seen_at) \
             VALUES ($1,$2,$3, now() - $4::interval)",
        )
        .bind(Uuid::now_v7())
        .bind(token_id)
        .bind(version)
        .bind(age)
        .execute(h.pool())
        .await
        .unwrap();
    }
    let v = admin_overview(&h, &c).await;
    // The stale replica (not seen within 15 minutes) is not "seen".
    assert_eq!(v["previews"]["relay_replicas"], 2, "{v}");
    assert!(v["previews"]["last_seen_at"].is_string(), "{v}");
}

#[tokio::test]
async fn overview_has_no_previews_line_when_previews_are_off() {
    let h = Harness::new().await;
    let c = admin_cookie(&h).await;
    let v = admin_overview(&h, &c).await;
    assert!(v.get("previews").is_none(), "{v}");
}

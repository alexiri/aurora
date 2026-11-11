#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Linking (SEC-73, docs/18 §SCIM): SCIM pre-provisions an unverified person; only the OIDC
//! sign-in with a verified e-mail claim binds the link; a signed-in user's global row is
//! never mutated by SCIM; existing users are linked without touching their row.

use crate::scim_common::*;
use http::Method;
use serde_json::json;
use uuid::Uuid;

const USERS: &str = "/scim/v2/Users";

#[tokio::test]
async fn preprovisioned_user_cannot_act_before_signin() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("new@x.io").await;
    let aurora = fx.aurora_user(u["id"].as_str().unwrap()).await;
    // The global user exists, but has no identity and no session: nothing to sign in with.
    let has_idp: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_identities WHERE user_id = $1")
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(has_idp, 0, "pre-provisioned: no identity yet");
    let has_session: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = $1")
        .bind(aurora)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(has_session, 0);
    // The membership exists and is active — the *link* is what is unverified.
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "active", "{st}");
}

#[tokio::test]
async fn oidc_signin_binds_by_verified_email() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("bind@x.io").await;
    let scim_id: Uuid = u["id"].as_str().unwrap().parse().unwrap();
    let aurora = fx.aurora_user(u["id"].as_str().unwrap()).await;
    // Sign in through the real dev IdP with the same e-mail (verified by the provider).
    let signed = fx.h.sign_in("bind@x.io").await;
    assert_eq!(signed.user_id, aurora, "the link binds to the existing row");
    // The identity now exists (the sign-in created it) and the SCIM link is intact.
    let link: (Uuid, String) =
        sqlx::query_as("SELECT id, state FROM scim_users WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(link.0, scim_id);
    assert_eq!(link.1, "active");
}

#[tokio::test]
async fn scim_update_never_touches_signedin_users_row() {
    let fx = ScimFx::new().await;
    // A real signed-in user.
    let signed = fx.h.sign_in("human@x.io").await;
    // Provision the same e-mail: the existing row is adopted, not recreated.
    let u = fx
        .provision("human@x.io", json!({"displayName": "Impersonator"}))
        .await;
    let aurora = fx.aurora_user(u["id"].as_str().unwrap()).await;
    assert_eq!(aurora, signed.user_id, "adopted, not duplicated");
    // The global e-mail and display name are unchanged (the dashboard name wins).
    let (email, display): (String, Option<String>) =
        sqlx::query_as("SELECT email::text, display_name FROM users WHERE id = $1")
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(email, "human@x.io", "users.email untouched");
    // `display_name` was set at user creation; a SCIM update of the link's display name
    // must not change it.
    let sid = u["id"].as_str().unwrap().to_owned();
    let (s, _, _) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "human@x.io", "displayName": "Second Try"})),
        )
        .await;
    assert_eq!(s, 200);
    let display2: Option<String> =
        sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(
        display2, display,
        "signed-in user's global row untouched by SCIM"
    );
    // But the link row's display_name did change (the directory's view).
    let link_display: Option<String> = sqlx::query_scalar(
        "SELECT display_name FROM scim_users WHERE org_id = $1 AND user_id = $2",
    )
    .bind(fx.org)
    .bind(aurora)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(link_display.as_deref(), Some("Second Try"));
}

#[tokio::test]
async fn external_id_never_links_or_authenticates() {
    let fx = ScimFx::new().await;
    // A person exists with a verified identity.
    let signed = fx.h.sign_in("existing@x.io").await;
    // SCIM provisions someone *else* claiming that person's e-mail as externalId only:
    // linking goes by e-mail, never by externalId — a new user id, no adoption.
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({
                "userName": "other@x.io",
                "externalId": signed.user_id.to_string(),
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let aurora = fx.aurora_user(v["id"].as_str().unwrap()).await;
    assert_ne!(aurora, signed.user_id, "externalId never links");
}

#[tokio::test]
async fn unverified_claim_does_not_bind() {
    let fx = ScimFx::new().await;
    // Provision a@x.io; a *different* global user exists for another e-mail…
    let u = fx.provision_ok("target@x.io").await;
    let target = fx.aurora_user(u["id"].as_str().unwrap()).await;
    // …and a distinct person signs in with their own verified e-mail.
    let signed = fx.h.sign_in("someone-else@x.io").await;
    assert_ne!(signed.user_id, target, "no cross-binding");
    // The link still points at the pre-provisioned row.
    let link: Uuid =
        sqlx::query_scalar("SELECT user_id FROM scim_users WHERE org_id = $1 AND id = $2")
            .bind(fx.org)
            .bind(u["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(link, target);
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Linking (SEC-73, docs/18 §SCIM): SCIM pre-provisions an unverified person; only the OIDC
//! sign-in with a verified e-mail claim binds the link; a signed-in user's global row is
//! never mutated by SCIM; existing users are linked without touching their row.

use crate::common::bearer;
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

/// FIX-034: `PUT /Users/{id}` enforces the e-mail domain allow-list like POST and PATCH —
/// a `userName` outside the org's `email_domains` is 400 `invalidValue` before any write;
/// the link and its membership are unchanged; the same PUT with the original domain stays 200.
#[tokio::test]
async fn put_rejects_email_domain_outside_org_allow_list() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("linked@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let aurora = fx.aurora_user(&sid).await;
    // The state a rejected PUT must leave untouched: the link row and the membership.
    let before: (Uuid, String, Option<String>, String, Option<String>) = sqlx::query_as(
        "SELECT user_id, user_name::text, external_id, state, display_name \
         FROM scim_users WHERE id = $1",
    )
    .bind(sid.parse::<Uuid>().unwrap())
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    let seat_before: (String, String) =
        sqlx::query_as("SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();

    // PUT to a domain outside the org allow-list (`x.io`): 400 invalidValue, same detail
    // as PATCH, and nothing written — the link still points at the original user.
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "moved@other.io", "displayName": "Moved"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("invalidValue"), "{v}");
    assert_eq!(
        v["detail"], "The e-mail domain is not allowed for this organization.",
        "{v}"
    );
    let after: (Uuid, String, Option<String>, String, Option<String>) = sqlx::query_as(
        "SELECT user_id, user_name::text, external_id, state, display_name \
         FROM scim_users WHERE id = $1",
    )
    .bind(sid.parse::<Uuid>().unwrap())
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(after, before, "link untouched by the rejected PUT");
    let seat_after: (String, String) =
        sqlx::query_as("SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(seat_after, seat_before, "membership untouched");
    // No stray user was created for the refused address either.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = 'moved@other.io'")
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "no write before the domain check");

    // The same PUT on the original domain is still a full replacement: 200.
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "linked@x.io", "displayName": "Linked"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let name: String = sqlx::query_scalar("SELECT user_name::text FROM scim_users WHERE id = $1")
        .bind(sid.parse::<Uuid>().unwrap())
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(name, "linked@x.io");
}

/// FIX-036: PUT (or PATCH) re-pointing a link at an existing global user with no membership
/// in this org inserts the membership (role `member`, seat-capped) instead of answering 500:
/// the e-mail is free here but taken globally (another org provisioned, invited or JIT'd the
/// person — a state this code creates itself). A membership that already exists is adopted
/// as it is.
#[tokio::test]
async fn put_adopting_global_user_without_membership_inserts_member() {
    let fx = ScimFx::new().await;
    let other = other_org(&fx.h).await;
    // Org A provisions a@x.io (the shared global user); org B provisions b@x.io — org B's
    // own link, the one the PUT re-points.
    let a = fx.provision_ok("a@x.io").await;
    let (s, _, _) = fx
        .scim_with(
            Method::POST,
            USERS,
            &bearer(&other.token),
            Some(json!({"userName": "b@x.io"})),
        )
        .await;
    assert_eq!(s, 201);
    let a_id = a["id"].as_str().unwrap().to_owned();
    let aurora = fx.aurora_user(&a_id).await;
    let b_link: Uuid =
        sqlx::query_scalar("SELECT id FROM scim_users WHERE org_id = $1 AND user_name = 'b@x.io'")
            .bind(other.org)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(other.org)
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0, "org B has no membership for a@x.io yet");
    // Org B PUTs its link to a@x.io: 200, the response reflects the link — and org B gains
    // a member seat for the shared user.
    let (s, _, v) = fx
        .scim_with(
            Method::PUT,
            &format!("{USERS}/{b_link}"),
            &bearer(&other.token),
            Some(json!({"userName": "a@x.io"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["id"], b_link.to_string(), "{v}");
    assert_eq!(v["userName"], "a@x.io", "{v}");
    let seat: (String, String) =
        sqlx::query_as("SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(other.org)
            .bind(aurora)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(seat, ("member".into(), "active".into()));
    // Org A's link and membership are untouched.
    let (a_state, a_user): (String, Uuid) =
        sqlx::query_as("SELECT state, user_id FROM scim_users WHERE org_id = $1 AND id = $2")
            .bind(fx.org)
            .bind(a_id.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(a_state, "active");
    assert_eq!(
        a_user, aurora,
        "org A's link still points at the shared user"
    );
    // The addition is audited with the SCIM `via` (like `create_user`'s `member.add`).
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.add' \
         AND target_id = $2::text AND outcome = 'success'",
    )
    .bind(other.org)
    .bind(aurora.to_string())
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "org B audited the adopted member.add");

    // PATCH inherits the same path: org C provisions its own d@x.io link, then PATCHes it
    // onto c@x.io — a global user org A provisioned, with no seat in org C.
    let third = other_org(&fx.h).await;
    let c = fx.provision_ok("c@x.io").await;
    let c_id = c["id"].as_str().unwrap().to_owned();
    let aurora_c = fx.aurora_user(&c_id).await;
    let (s, _, _) = fx
        .scim_with(
            Method::POST,
            USERS,
            &bearer(&third.token),
            Some(json!({"userName": "d@x.io"})),
        )
        .await;
    assert_eq!(s, 201);
    let c_link: Uuid =
        sqlx::query_scalar("SELECT id FROM scim_users WHERE org_id = $1 AND user_name = 'd@x.io'")
            .bind(third.org)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    let (s, _, v) = fx
        .scim_with(
            Method::PATCH,
            &format!("{USERS}/{c_link}"),
            &bearer(&third.token),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "emails[type eq \"work\"].value",
                                "value": "c@x.io"}],
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["userName"], "c@x.io", "{v}");
    let seat: (String, String) =
        sqlx::query_as("SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(third.org)
            .bind(aurora_c)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(seat, ("member".into(), "active".into()));
}

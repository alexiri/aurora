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

/// D-167(b) (was FIX-036's re-point for every user): the link of a still-unbound placeholder
/// whose new e-mail belongs to another global user moves to that user exactly like a POST
/// would link it — a missing membership is inserted (role `member`, seat-capped, audited), an
/// existing one adopted — and the placeholder's membership in this org is removed while its
/// `users` row stays (SCIM never deletes a user, SEC-73).
#[tokio::test]
async fn put_unbound_placeholder_onto_existing_global_user_moves_link() {
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
    let b_placeholder = link_user(&fx.h, b_link).await;
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
    // The placeholder left org B; its user row stays, still unbound, with its own e-mail.
    assert_eq!(membership(&fx.h, other.org, b_placeholder).await, None);
    assert_eq!(
        account(&fx.h, b_placeholder).await,
        Some(("b@x.io".to_owned(), false)),
        "the placeholder user is never deleted or rewritten"
    );

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

// ---- D-167 helpers ------------------------------------------------------------------

const PATCH_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

/// The Aurora user a link points at.
async fn link_user(h: &crate::common::Harness, link: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT user_id FROM scim_users WHERE id = $1")
        .bind(link)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

/// `(users.email, signed in)` of a user, `None` when the row is gone.
async fn account(h: &crate::common::Harness, user: Uuid) -> Option<(String, bool)> {
    sqlx::query_as(
        "SELECT email::text, (email_verified_at IS NOT NULL \
             OR EXISTS (SELECT 1 FROM user_identities i WHERE i.user_id = users.id)) \
         FROM users WHERE id = $1",
    )
    .bind(user)
    .fetch_optional(h.pool())
    .await
    .unwrap()
}

/// `(role, status)` of a membership, `None` when absent.
async fn membership(h: &crate::common::Harness, org: Uuid, user: Uuid) -> Option<(String, String)> {
    sqlx::query_as("SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2")
        .bind(org)
        .bind(user)
        .fetch_optional(h.pool())
        .await
        .unwrap()
}

async fn users_with_email(h: &crate::common::Harness, email: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1::citext")
        .bind(email)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

fn patch_user_name(v: &str) -> serde_json::Value {
    json!({
        "schemas": [PATCH_SCHEMA],
        "Operations": [{"op": "replace", "path": "userName", "value": v}],
    })
}

/// D-167(a), the FIX-062 finding: after the person signed in, PUT and PATCH of a new
/// `userName` land on the link only — the link keeps its user, `users.email` is untouched,
/// no other account is created or adopted (also when the new address belongs to another
/// global user) — and a later `active=false` suspends **the person's** membership (C-474).
/// Before FIX-062 the PUT re-pointed the link at a new pre-provisioned `pa2@x.io` user and
/// the suspend then hit that user while the real person stayed active.
#[tokio::test]
async fn signed_in_user_name_change_lands_on_link_and_deprovisioning_hits_the_person() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("pa@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let link: Uuid = sid.parse().unwrap();
    let person = fx.aurora_user(&sid).await;
    let signed = fx.h.sign_in("pa@x.io").await;
    assert_eq!(signed.user_id, person, "the sign-in bound the placeholder");
    assert_eq!(
        account(&fx.h, person).await,
        Some(("pa@x.io".to_owned(), true))
    );

    // PUT a new, globally free address.
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "pa2@x.io", "displayName": "Renamed"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["userName"], "pa2@x.io",
        "the response shows the link's value: {v}"
    );
    assert_eq!(
        link_user(&fx.h, link).await,
        person,
        "PUT never moves the link"
    );
    assert_eq!(
        account(&fx.h, person).await,
        Some(("pa@x.io".to_owned(), true)),
        "users.email untouched"
    );
    assert_eq!(
        users_with_email(&fx.h, "pa2@x.io").await,
        0,
        "no new account"
    );

    // PATCH onto an address another global user owns: still the link only.
    let other = fx.h.user("pa-other@x.io", false).await;
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(patch_user_name("pa-other@x.io")),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["userName"], "pa-other@x.io", "{v}");
    assert_eq!(
        link_user(&fx.h, link).await,
        person,
        "PATCH never moves the link"
    );
    assert_eq!(
        membership(&fx.h, fx.org, other).await,
        None,
        "the other account is not adopted"
    );
    assert_eq!(
        account(&fx.h, person).await,
        Some(("pa@x.io".to_owned(), true))
    );
    assert_eq!(
        membership(&fx.h, fx.org, person).await,
        Some(("member".to_owned(), "active".to_owned()))
    );

    // Deprovisioning acts on the person who signed in.
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": [PATCH_SCHEMA],
                "Operations": [{"op": "replace", "path": "active", "value": false}],
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["active"], false, "{v}");
    assert_eq!(
        membership(&fx.h, fx.org, person).await,
        Some(("member".to_owned(), "suspended".to_owned())),
        "the original membership is suspended"
    );
    // Sessions per the C-474 matrix: revoked only when no other active membership remains.
    let other_active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org_members WHERE user_id = $1 AND status = 'active'",
    )
    .bind(person)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    let live_sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(person)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    if other_active == 0 {
        assert_eq!(live_sessions, 0, "no other membership: sessions revoked");
    } else {
        assert!(
            live_sessions > 0,
            "another active membership: sessions kept"
        );
    }
}

/// D-167(b): an unbound placeholder of this org alone whose new e-mail is free globally is
/// renamed in place (same user id, `users.email` updated, still unverified).
#[tokio::test]
async fn unbound_placeholder_free_email_renamed_in_place() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("pb@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let link: Uuid = sid.parse().unwrap();
    let placeholder = fx.aurora_user(&sid).await;
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "pb2@x.io"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["userName"], "pb2@x.io", "{v}");
    assert_eq!(link_user(&fx.h, link).await, placeholder, "same user id");
    assert_eq!(
        account(&fx.h, placeholder).await,
        Some(("pb2@x.io".to_owned(), false)),
        "users.email updated in place, still unverified"
    );
    assert_eq!(users_with_email(&fx.h, "pb@x.io").await, 0);
    assert_eq!(
        membership(&fx.h, fx.org, placeholder).await,
        Some(("member".to_owned(), "active".to_owned()))
    );
    // The person can now sign in with the new address and lands on the same account.
    let signed = fx.h.sign_in("pb2@x.io").await;
    assert_eq!(signed.user_id, placeholder);
}

/// D-167(b): the new e-mail belongs to another global user → the link moves to it (its
/// existing membership adopted as it is), the placeholder's membership in this org is
/// removed, its user row kept, and the link's SCIM groups follow it.
#[tokio::test]
async fn unbound_placeholder_onto_existing_member_moves_link_and_groups() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("pc@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let link: Uuid = sid.parse().unwrap();
    let placeholder = fx.aurora_user(&sid).await;
    let (s, _, g) = fx
        .scim(
            Method::POST,
            "/scim/v2/Groups",
            Some(json!({"displayName": "pc-group", "members": [{"value": sid}]})),
        )
        .await;
    assert_eq!(s, 201, "{g}");
    let gid: Uuid = g["id"].as_str().unwrap().parse().unwrap();
    // The target: a person already in the org (an admin, adopted as is).
    let target = fx.h.user("pc-real@x.io", false).await;
    fx.h.member(fx.org, target, "admin", "active").await;

    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(patch_user_name("pc-real@x.io")),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["id"], sid, "{v}");
    assert_eq!(link_user(&fx.h, link).await, target, "the link moved");
    assert_eq!(
        membership(&fx.h, fx.org, target).await,
        Some(("admin".to_owned(), "active".to_owned())),
        "adopted as it is"
    );
    assert_eq!(membership(&fx.h, fx.org, placeholder).await, None);
    assert_eq!(
        account(&fx.h, placeholder).await,
        Some(("pc@x.io".to_owned(), false)),
        "the placeholder row stays"
    );
    // The group still lists the link (now backed by the target).
    let (s, _, g) = fx
        .scim(Method::GET, &format!("/scim/v2/Groups/{gid}"), None)
        .await;
    assert_eq!(s, 200, "{g}");
    let ids: Vec<&str> = g["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["value"].as_str())
        .collect();
    assert_eq!(ids, vec![sid.as_str()], "{g}");
    let in_group: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM group_members WHERE group_id = $1 AND user_id = $2)",
    )
    .bind(gid)
    .bind(target)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert!(in_group);
}

/// D-167(b), shared placeholder: org A provisioned p@x.io, org B linked the same unbound
/// user. Org B's PUT of a free address must not rename the account org A links: org B's link
/// moves to a new pre-provisioned user, only org B's membership of the placeholder is
/// removed, and org A's link, membership and the placeholder row are untouched.
#[tokio::test]
async fn shared_placeholder_rename_in_one_org_leaves_the_other_untouched() {
    let fx = ScimFx::new().await;
    let other = other_org(&fx.h).await;
    let a = fx.provision_ok("ps@x.io").await;
    let a_link: Uuid = a["id"].as_str().unwrap().parse().unwrap();
    let placeholder = link_user(&fx.h, a_link).await;
    let (s, _, b) = fx
        .scim_with(
            Method::POST,
            USERS,
            &bearer(&other.token),
            Some(json!({"userName": "ps@x.io"})),
        )
        .await;
    assert_eq!(s, 201, "{b}");
    let b_link: Uuid = b["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        link_user(&fx.h, b_link).await,
        placeholder,
        "org B linked it"
    );

    let (s, _, v) = fx
        .scim_with(
            Method::PUT,
            &format!("{USERS}/{b_link}"),
            &bearer(&other.token),
            Some(json!({"userName": "ps-new@x.io"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["userName"], "ps-new@x.io", "{v}");
    let fresh = link_user(&fx.h, b_link).await;
    assert_ne!(fresh, placeholder, "org B's link moved to a new user");
    assert_eq!(
        account(&fx.h, fresh).await,
        Some(("ps-new@x.io".to_owned(), false)),
        "a new pre-provisioned user"
    );
    assert_eq!(
        membership(&fx.h, other.org, fresh).await,
        Some(("member".to_owned(), "active".to_owned()))
    );
    assert_eq!(membership(&fx.h, other.org, placeholder).await, None);
    // Org A: untouched.
    assert_eq!(
        account(&fx.h, placeholder).await,
        Some(("ps@x.io".to_owned(), false)),
        "the shared account is not renamed"
    );
    assert_eq!(link_user(&fx.h, a_link).await, placeholder);
    assert_eq!(
        membership(&fx.h, fx.org, placeholder).await,
        Some(("member".to_owned(), "active".to_owned()))
    );
    let a_name: String = sqlx::query_scalar("SELECT user_name::text FROM scim_users WHERE id = $1")
        .bind(a_link)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(a_name, "ps@x.io");
}

/// D-167 consequence: a link whose `userName` differs from the person's `users.email` (an
/// IdP rename after sign-in) is removed and POSTed again under that `userName`: the
/// tombstone is revived on its pinned user — no 500, no new account, no re-point.
#[tokio::test]
async fn post_revives_renamed_tombstone_on_its_person() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("pr@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let person = fx.aurora_user(&sid).await;
    fx.h.sign_in("pr@x.io").await;
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "pr2@x.io"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    assert_eq!(membership(&fx.h, fx.org, person).await, None);

    let (s, _, v) = fx
        .scim(Method::POST, USERS, Some(json!({"userName": "pr2@x.io"})))
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["id"], sid, "the tombstone is revived: {v}");
    assert_eq!(v["userName"], "pr2@x.io", "{v}");
    assert_eq!(
        link_user(&fx.h, sid.parse().unwrap()).await,
        person,
        "no re-point"
    );
    assert_eq!(
        users_with_email(&fx.h, "pr2@x.io").await,
        0,
        "no new account"
    );
    assert_eq!(
        account(&fx.h, person).await,
        Some(("pr@x.io".to_owned(), true))
    );
    // (D-175, FIX-082) The person is an existing, signed-in account and no longer a member:
    // the revival only prepares the membership; their own next sign-in activates it.
    assert_eq!(
        membership(&fx.h, fx.org, person).await,
        Some(("member".to_owned(), "pending".to_owned()))
    );
    fx.h.try_sign_in("dev", "pr@x.io").await.unwrap();
    assert_eq!(
        membership(&fx.h, fx.org, person).await,
        Some(("member".to_owned(), "active".to_owned()))
    );
}

/// FIX-070: a SCIM in-place rename of a pre-provisioned user (D-167(b)) racing the OIDC
/// sign-in that verified the **old** address. Interleaving, driven deterministically on
/// separate connections: the rename transaction (SCIM's `lock_account` + `UPDATE users SET
/// email`) holds the row; the sign-in of `old@` starts and blocks on it; the rename commits.
/// The sign-in must not bind to — or mark verified — the account that now carries the new,
/// unverified address: the old address is free again and resolves afresh. Before FIX-070 the
/// sign-in read the pre-rename row, its `mark_email_verified` waited and then verified the
/// renamed account and linked the identity to it.
#[tokio::test]
async fn signin_racing_a_placeholder_rename_never_binds_the_renamed_account() {
    use sqlx::{ConnectOptions, Connection};
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("race-old@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let placeholder = fx.aurora_user(&sid).await;
    let opts = fx.h.pool().connect_options();
    // The rename, exactly SCIM's statements, left uncommitted.
    let mut rename = opts.connect().await.unwrap();
    let mut tx = rename.begin().await.unwrap();
    let rename_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(placeholder)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET email = 'race-new@x.io', updated_at = now() WHERE id = $1")
        .bind(placeholder)
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut watcher = opts.connect().await.unwrap();
    let (signed, ()) = tokio::join!(fx.h.sign_in("race-old@x.io"), async {
        // Wait (bounded) until the sign-in is blocked behind the rename, then commit it.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                 WHERE $1 = ANY (pg_blocking_pids(pid)))",
            )
            .bind(rename_pid)
            .fetch_one(&mut watcher)
            .await
            .unwrap();
            if blocked {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the sign-in never reached the renamed row"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tx.commit().await.unwrap();
    });
    assert_ne!(
        signed.user_id, placeholder,
        "the sign-in of the old address never binds the renamed account"
    );
    assert_eq!(
        account(&fx.h, placeholder).await,
        Some(("race-new@x.io".to_owned(), false)),
        "the renamed placeholder is not verified and has no identity"
    );
    assert_eq!(
        account(&fx.h, signed.user_id).await,
        Some(("race-old@x.io".to_owned(), true)),
        "the verified old address resolved afresh"
    );
    assert_eq!(
        link_user(&fx.h, sid.parse().unwrap()).await,
        placeholder,
        "the link stays on its placeholder"
    );
}

// ---- FIX-082 (D-175): SCIM prepares a membership, only the person's sign-in activates it --------

/// The statuses a signed-in `cookie` gets for the org-scoped reads a member could do.
async fn org_surface(fx: &ScimFx, cookie: &str, org: Uuid, ws: Uuid) -> Vec<u16> {
    let mut out = Vec::new();
    for uri in [
        format!("/v1/orgs/{org}"),
        format!("/v1/orgs/{org}/workspaces"),
        format!("/v1/workspaces/{ws}"),
    ] {
        let (s, _) = fx.h.get(&uri, &[("cookie", cookie)]).await;
        out.push(s.as_u16());
    }
    out
}

async fn audit_count(fx: &ScimFx, user: Uuid, extra: &str) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.add' \
         AND outcome = 'success' AND target_id = $2 {extra}"
    ))
    .bind(fx.org)
    .bind(user.to_string())
    .fetch_one(fx.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn fix082_existing_account_is_prepared_pending_and_activated_by_its_own_sign_in() {
    let fx = ScimFx::new().await;
    // A person who already has an account (they signed in; `open` gave them an org of their own).
    let person = fx.h.sign_in("person@x.io").await;
    let stranger = fx.h.sign_in("stranger@x.io").await;
    let ws = fx.h.workspace(fx.org).await;
    // Seat cap: the owner and the pending member fill it.
    sqlx::query(
        "UPDATE organizations SET settings = settings || '{\"max_members\": 2}'::jsonb WHERE id = $1",
    )
    .bind(fx.org)
    .execute(fx.h.pool())
    .await
    .unwrap();

    // The directory names that e-mail: the membership is `pending`, not `active`.
    let u = fx.provision_ok("person@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    assert_eq!(
        fx.aurora_user(&sid).await,
        person.user_id,
        "linked to the existing account"
    );
    assert_eq!(
        membership(&fx.h, fx.org, person.user_id).await,
        Some(("member".into(), "pending".into()))
    );
    // The IdP's own value is reported, so it never loops.
    let (s, _, v) = fx.scim(Method::GET, &format!("{USERS}/{sid}"), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["active"], true, "{v}");
    // The seat is taken: another provisioning is refused.
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "second@x.io"})),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["detail"], "member_limit_reached", "{v}");

    // A group the SCIM client fills with that person, bound to the workspace (a
    // group-derived binding in the org), changes nothing while the membership is pending.
    let (s, _, g) = fx
        .scim(
            Method::POST,
            "/scim/v2/Groups",
            Some(json!({"displayName": "eng", "members": [{"value": sid}]})),
        )
        .await;
    assert_eq!(s, 201, "{g}");
    let gid: Uuid = g["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(gid)
    .bind(ws)
    .bind(fx.org)
    .execute(fx.h.pool())
    .await
    .unwrap();
    // No access: every org-scoped read answers exactly what it answers a stranger, and an
    // org API key of the pending member is dead.
    let expected = org_surface(&fx, &stranger.cookie, fx.org, ws).await;
    assert!(expected.iter().all(|s| *s != 200), "{expected:?}");
    assert_eq!(
        org_surface(&fx, &person.cookie, fx.org, ws).await,
        expected,
        "a pending membership behaves as a non-member"
    );
    let (_, key) = fx.h.api_key(person.user_id, fx.org, None, false).await;
    let (s, _) =
        fx.h.get(
            &format!("/v1/orgs/{}", fx.org),
            &[("authorization", &bearer(&key))],
        )
        .await;
    assert_ne!(
        s.as_u16(),
        200,
        "an API key of a pending member is not live"
    );
    assert_eq!(
        audit_count(&fx, person.user_id, "AND metadata->>'was' = 'pending'").await,
        0
    );

    // The person's own next verified sign-in activates it: one audited `member.add`.
    let again = fx.h.try_sign_in("dev", "person@x.io").await.unwrap();
    assert_eq!(again.user_id, person.user_id);
    assert_eq!(
        membership(&fx.h, fx.org, person.user_id).await,
        Some(("member".into(), "active".into()))
    );
    assert_eq!(
        audit_count(
            &fx,
            person.user_id,
            "AND metadata->>'via' = 'scim' AND metadata->>'was' = 'pending'"
        )
        .await,
        1,
        "one member.add row, via scim"
    );
    // The group-derived binding is now effective.
    let (s, _) =
        fx.h.get(
            &format!("/v1/workspaces/{ws}"),
            &[("cookie", &again.cookie)],
        )
        .await;
    assert_eq!(
        s.as_u16(),
        200,
        "group-derived access applies after activation"
    );
    // Signing in again activates nothing more.
    fx.h.try_sign_in("dev", "person@x.io").await.unwrap();
    assert_eq!(
        audit_count(&fx, person.user_id, "AND metadata->>'was' = 'pending'").await,
        1
    );
}

#[tokio::test]
async fn fix082_deactivate_or_delete_before_sign_in_removes_the_pending_membership() {
    let fx = ScimFx::new().await;
    for (email, how) in [("p-patch@x.io", "patch"), ("p-del@x.io", "delete")] {
        let person = fx.h.sign_in(email).await;
        let u = fx.provision_ok(email).await;
        let sid = u["id"].as_str().unwrap().to_owned();
        assert_eq!(
            membership(&fx.h, fx.org, person.user_id).await.map(|m| m.1),
            Some("pending".into())
        );
        let sessions_before: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(person.user_id)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
        if how == "patch" {
            let (s, _, v) = fx
                .scim(
                    Method::PATCH,
                    &format!("{USERS}/{sid}"),
                    Some(json!({
                        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                        "Operations": [{"op": "replace", "path": "active", "value": false}],
                    })),
                )
                .await;
            assert_eq!(s, 200, "{v}");
        } else {
            let (s, _, _) = fx
                .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
                .await;
            assert_eq!(s, 204);
        }
        assert_eq!(
            membership(&fx.h, fx.org, person.user_id).await,
            None,
            "{how}"
        );
        // Nothing to deprovision: the person's sessions (their other orgs) are untouched.
        let sessions_after: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(person.user_id)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
        assert_eq!(sessions_after, sessions_before, "{how}: sessions kept");
        // A later sign-in activates nothing.
        fx.h.try_sign_in("dev", email).await.unwrap();
        assert_eq!(
            membership(&fx.h, fx.org, person.user_id).await,
            None,
            "{how}"
        );
    }
}

#[tokio::test]
async fn fix082_reactivating_a_deactivated_prepared_person_prepares_again() {
    let fx = ScimFx::new().await;
    let person = fx.h.sign_in("again@x.io").await;
    let u = fx.provision_ok("again@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let patch = |active: bool| {
        json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "replace", "path": "active", "value": active}],
        })
    };
    let (s, _, v) = fx
        .scim(Method::PATCH, &format!("{USERS}/{sid}"), Some(patch(false)))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(membership(&fx.h, fx.org, person.user_id).await, None);
    // The link alone remains (suspended); attribute updates work on it.
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "again@x.io", "displayName": "Again", "active": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _, v) = fx
        .scim(Method::PATCH, &format!("{USERS}/{sid}"), Some(patch(true)))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        membership(&fx.h, fx.org, person.user_id).await.map(|m| m.1),
        Some("pending".into()),
        "prepared again, never active without the person's sign-in"
    );
}

#[tokio::test]
async fn fix082_inactive_provisioning_of_an_existing_account_creates_the_link_alone() {
    let fx = ScimFx::new().await;
    let person = fx.h.sign_in("inactive@x.io").await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "inactive@x.io", "active": false})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["active"], false, "{v}");
    assert_eq!(membership(&fx.h, fx.org, person.user_id).await, None);
    fx.h.try_sign_in("dev", "inactive@x.io").await.unwrap();
    assert_eq!(
        membership(&fx.h, fx.org, person.user_id).await,
        None,
        "a suspended link activates nothing"
    );
}

#[tokio::test]
async fn fix082_existing_member_is_adopted_unchanged_and_new_address_stays_preprovisioned() {
    let fx = ScimFx::new().await;
    // An existing admin of the org is adopted as they are.
    let admin = fx.h.user("adm@x.io", false).await;
    fx.h.member(fx.org, admin, "admin", "active").await;
    fx.provision_ok("adm@x.io").await;
    assert_eq!(
        membership(&fx.h, fx.org, admin).await,
        Some(("admin".into(), "active".into()))
    );
    // A brand-new address: a pre-provisioned user with the existing rule (active, inert).
    let u = fx.provision_ok("brandnew@x.io").await;
    let new = fx.aurora_user(u["id"].as_str().unwrap()).await;
    assert_eq!(
        membership(&fx.h, fx.org, new).await,
        Some(("member".into(), "active".into()))
    );
    assert_eq!(
        account(&fx.h, new).await,
        Some(("brandnew@x.io".into(), false))
    );
}

#[tokio::test]
async fn fix082_an_admin_cannot_activate_or_set_pending_and_may_remove() {
    let fx = ScimFx::new().await;
    let person = fx.h.sign_in("adm-act@x.io").await;
    fx.provision_ok("adm-act@x.io").await;
    let uri = format!("/v1/orgs/{}/members/{}", fx.org, person.user_id);
    for st in ["active", "suspended"] {
        let (s, v) = fx
            .mgmt(Method::PATCH, &uri, Some(json!({"status": st})))
            .await;
        assert_eq!(s, 409, "{st}: {v}");
        assert_eq!(v["error"]["code"], "member_pending", "{v}");
    }
    let (s, _) = fx
        .mgmt(Method::PATCH, &uri, Some(json!({"status": "pending"})))
        .await;
    assert_eq!(s, 400);
    // The member list shows the row as pending.
    let (s, v) = fx
        .mgmt(Method::GET, &format!("/v1/orgs/{}/members", fx.org), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["email"] == "adm-act@x.io" && m["status"] == "pending"),
        "{v}"
    );
    // Removing a pending member is allowed.
    let (s, _) = fx.mgmt(Method::DELETE, &uri, None).await;
    assert_eq!(s, 204);
    assert_eq!(membership(&fx.h, fx.org, person.user_id).await, None);
}

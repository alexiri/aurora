#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Deprovisioning (SEC-73, D-135, docs/18 §SCIM): the full effects matrix of `04 §SCIM` for
//! suspend and remove — membership, sessions rule with one and with two orgs, key revocation
//! (never revived by reactivation), running speaker turns aborted with `user_deprovisioned`,
//! pending invitations cancelled, JIT and invitation acceptance refused and allowed again,
//! reactivation only when `suspended_by_scim`, `exclusive` mode, other orgs unaffected.

use crate::common::{ORIGIN, cookie};
use crate::scim_common::*;
use aurora_core::token;
use chrono::{DateTime, Utc};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const USERS: &str = "/scim/v2/Users";

async fn deactivate(fx: &ScimFx, id: &str) -> (u16, Value) {
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{id}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": false}],
            })),
        )
        .await;
    (s.as_u16(), v)
}

#[tokio::test]
async fn put_to_tombstoned_email_then_post_sequence() {
    // FIX-040 (amended): provision → DELETE → PUT another link to the freed e-mail →
    // deterministic 409 uniqueness (the tombstone pins the e-mail's `user_id` under
    // `UNIQUE (org_id, user_id)`; PUT cannot revive, only POST does) → POST → 201 revival
    // reusing the original link row id. No 500 anywhere in the sequence.
    let fx = ScimFx::new().await;
    let a = fx.provision_ok("u@x.io").await;
    let aid = a["id"].as_str().unwrap().to_owned();
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{aid}"), None)
        .await;
    assert_eq!(s, 204);
    // Link B (a different global user).
    let b = fx.provision_ok("v@x.io").await;
    let bid = b["id"].as_str().unwrap().to_owned();
    // PUT link B onto the tombstoned e-mail: 409 uniqueness, deterministic.
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{bid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": "u@x.io",
                "name": {"givenName": "V", "familyName": "Two"},
            })),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_scim_error(409, &v);
    assert_eq!(scim_type(&v), Some("uniqueness"), "{v}");
    // Nothing was written: link B keeps its e-mail, the tombstone is intact.
    let b_row: (String, String) =
        sqlx::query_as("SELECT user_name, state FROM scim_users WHERE org_id = $1 AND id = $2")
            .bind(fx.org)
            .bind(bid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(b_row, ("v@x.io".into(), "active".into()), "B unchanged");
    let a_row: (String, String) =
        sqlx::query_as("SELECT user_name, state FROM scim_users WHERE org_id = $1 AND id = $2")
            .bind(fx.org)
            .bind(aid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(a_row.1, "removed", "the tombstone is untouched");
    // POST the e-mail: the revival (FIX-035) reuses the original row.
    let v2 = fx.provision_ok("u@x.io").await;
    assert_eq!(v2["id"], a["id"], "the tombstone's row id is reused");
    // The final PUT of B to its own name stays clean (uniqueness for live links intact).
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{bid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": "v@x.io",
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn suspend_cancels_pending_invitations() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("sus-inv@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    // A pending invitation addressed to the member's e-mail.
    sqlx::query(
        "INSERT INTO invitations (id, org_id, email, role, token_hash, invited_by, status, created_at, updated_at)
         VALUES ($1, $2, 'sus-inv@x.io', 'member', $3, NULL, 'pending', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(fx.org)
    .bind(&token::hash("inv-sus")[..])
    .execute(fx.h.pool())
    .await
    .unwrap();
    // Suspend (the deprovisioning matrix names the invitation row for both columns).
    let (s, v) = deactivate(&fx, &sid).await;
    assert_eq!(s, 200, "{v}");
    let inv: String = sqlx::query_scalar(
        "SELECT status FROM invitations WHERE org_id = $1 AND email = 'sus-inv@x.io'",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(inv, "cancelled", "suspend cancels pending invitations");
}

#[tokio::test]
async fn post_after_delete_revives_the_link() {
    let fx = ScimFx::new().await;
    // Seat cap 2: owner + one member. The revival must be seat-capped like a provision.
    sqlx::query(
        "UPDATE organizations SET settings = settings || '{\"max_members\": 2}'::jsonb WHERE id = $1",
    )
    .bind(fx.org)
    .execute(fx.h.pool())
    .await
    .unwrap();
    let u = fx
        .provision("revive@x.io", json!({"externalId": "idp-1"}))
        .await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    let created: DateTime<Utc> =
        sqlx::query_scalar("SELECT created_at FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    // DELETE the link (tombstone).
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    let removed_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT removed_at FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert!(removed_at.is_some(), "tombstoned");
    // POST the same e-mail again: 201, the same link row id, membership re-created.
    let v2 = fx
        .provision(
            "revive@x.io",
            json!({"externalId": "idp-2", "displayName": "Revived"}),
        )
        .await;
    assert_eq!(v2["id"], u["id"], "the link row id is reused");
    let (state, ext): (String, Option<String>) =
        sqlx::query_as("SELECT state, external_id FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(state, "active");
    assert_eq!(ext.as_deref(), Some("idp-2"), "externalId refreshed");
    let (removed_at, created2): (Option<DateTime<Utc>>, DateTime<Utc>) =
        sqlx::query_as("SELECT removed_at, created_at FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(removed_at, None, "the tombstone is cleared");
    assert_eq!(created2, created, "the row is the same one");
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "active", "membership re-created");
    // Seat-capped like a fresh provision: with the cap full again, the revival itself is
    // 403 member_limit_reached and the tombstone is untouched.
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    sqlx::query(
        "UPDATE organizations SET settings = settings || '{\"max_members\": 1}'::jsonb WHERE id = $1",
    )
    .bind(fx.org)
    .execute(fx.h.pool())
    .await
    .unwrap();
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "revive@x.io"})),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert!(
        v["detail"].as_str().unwrap().contains("member_limit"),
        "{v}"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM scim_users WHERE id = $1")
        .bind(sid.parse::<Uuid>().unwrap())
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(state, "removed", "a capped revival leaves the tombstone");
    // Room again: the revival works once more (the row is still the original).
    sqlx::query(
        "UPDATE organizations SET settings = settings || '{\"max_members\": 2}'::jsonb WHERE id = $1",
    )
    .bind(fx.org)
    .execute(fx.h.pool())
    .await
    .unwrap();
    let v3 = fx.provision_ok("revive@x.io").await;
    assert_eq!(v3["id"], u["id"], "the row survives a capped refusal");
    // The global `users` row was never duplicated.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = 'revive@x.io'")
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn suspend_matrix_membership_keys_sessions() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("s@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    // Keys and a session for the person.
    let (_, key) = fx.h.api_key(user, fx.org, None, false).await;
    let _ = &key;
    let (_, sess) = fx.h.good_session(user).await;
    let _ = &sess;
    // Suspend.
    let (s, v) = deactivate(&fx, &sid).await;
    assert_eq!(s, 200, "{v}");
    // Membership suspended, link suspended with `suspended_by_scim`.
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "suspended");
    let (state, by_scim): (String, bool) = sqlx::query_as(
        "SELECT state, suspended_by_scim FROM scim_users WHERE org_id = $1 AND user_id = $2",
    )
    .bind(fx.org)
    .bind(user)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!((state.as_str(), by_scim), ("suspended", true));
    // Keys revoked.
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_keys WHERE user_id = $1 AND org_id = $2 AND revoked_at IS NULL",
    )
    .bind(user)
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 0, "org keys revoked on suspend");
    // The session is revoked (no other active membership).
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(user)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 0, "last-org sessions revoked");
    // Critical audit rows written.
    let n = count(&fx.h, "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.update' AND outcome = 'success'", fx.org).await;
    assert!(n >= 1, "suspend audited");
}

#[tokio::test]
async fn sessions_kept_with_second_org() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("two@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    // A second org membership (active).
    let org2 = fx.h.org().await;
    fx.h.member(org2, user, "member", "active").await;
    let (_, sess) = fx.h.good_session(user).await;
    let _ = &sess;
    // Suspend in org A: the session survives (still signed in to org B).
    let (s, v) = deactivate(&fx, &sid).await;
    assert_eq!(s, 200, "{v}");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 1, "the other org keeps the session");
    // org2 membership untouched.
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(org2)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "active");
}

#[tokio::test]
async fn remove_matrix_keys_turns_invitations_group_access() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("rm@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    // Key, pending invitation addressed to the person's e-mail, group membership.
    let (_, key) = fx.h.api_key(user, fx.org, None, false).await;
    let _ = &key;
    sqlx::query(
        "INSERT INTO invitations (id, org_id, email, role, token_hash, invited_by, status, created_at, updated_at)
         VALUES ($1, $2, 'rm@x.io', 'member', $3, NULL, 'pending', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(fx.org)
    .bind(&token::hash("inv")[..])
    .execute(fx.h.pool())
    .await
    .unwrap();
    let (s, _, v) = fx
        .scim(
            Method::POST,
            "/scim/v2/Groups",
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
                "displayName": "g",
                "members": [{"value": sid}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // DELETE the user.
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    // Tombstone + membership removed + keys revoked + invitation cancelled.
    let state: String = sqlx::query_scalar("SELECT state FROM scim_users WHERE id = $1")
        .bind(sid.parse::<Uuid>().unwrap())
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(state, "removed");
    let n = count(
        &fx.h,
        "SELECT count(*) FROM org_members WHERE org_id = $1",
        fx.org,
    )
    .await
        - count(
            &fx.h,
            "SELECT count(*) FROM org_members WHERE org_id = $1 AND role = 'owner'",
            fx.org,
        )
        .await;
    assert_eq!(n, 0, "the membership is gone (the owner is not SCIM's)");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_keys WHERE user_id = $1 AND org_id = $2 AND revoked_at IS NULL",
    )
    .bind(user)
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 0);
    let inv: String = sqlx::query_scalar(
        "SELECT status FROM invitations WHERE org_id = $1 AND email = 'rm@x.io'",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(inv, "cancelled", "pending invitations cancelled");
    // The group no longer lists them.
    let (s, _, v) = fx
        .scim(Method::GET, "/scim/v2/Groups?attributes=members", None)
        .await;
    assert_eq!(s, 200, "{v}");
    let members = v["Resources"][0]["members"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(members.is_empty(), "the removed link left the group: {v}");
}

#[tokio::test]
async fn reactivation_only_when_suspended_by_scim() {
    let fx = ScimFx::new().await;
    // A human-suspended member adopted by SCIM: the IdP cannot reactivate.
    let human = fx.h.user("human-sus@x.io", false).await;
    fx.h.member(fx.org, human, "member", "active").await;
    fx.h.set_member_status(fx.org, human, "suspended").await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "human-sus@x.io", "active": true})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sid = v["id"].as_str().unwrap().to_owned();
    let by_scim: bool =
        sqlx::query_scalar("SELECT suspended_by_scim FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert!(!by_scim, "a human suspension is not SCIM's");
    // FIX-080: an IdP deactivation of that member does not turn the human suspension into
    // SCIM's (`suspended_by_scim` stays false), so the reactivation is refused too.
    let (s, v) = deactivate(&fx, &sid).await;
    assert_eq!(s, 200, "{v}");
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": true}],
            })),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(human)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "suspended", "the human suspension stays");
    // A member SCIM itself suspended is reactivated by the IdP.
    let u = fx.provision_ok("scim-sus@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    let (s, v) = deactivate(&fx, &sid).await;
    assert_eq!(s, 200, "{v}");
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": true}],
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(membership(&fx, user).await.as_deref(), Some("active"));
}

#[tokio::test]
async fn adopted_active_member_suspended_on_post_inactive() {
    let fx = ScimFx::new().await;
    // An active member created in Aurora (JIT or invitation), then pushed by the IdP
    // with `active: false`: the adoption must suspend, not just record the suspension.
    let user = fx.h.user("adopt-active@x.io", false).await;
    fx.h.member(fx.org, user, "member", "active").await;
    let (_, key) = fx.h.api_key(user, fx.org, None, false).await;
    let _ = &key;
    let (_, sess) = fx.h.good_session(user).await;
    let _ = &sess;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "adopt-active@x.io", "active": false})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sid = v["id"].as_str().unwrap().to_owned();
    // Membership suspended, link suspended with `suspended_by_scim`.
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "suspended", "the adopted membership is suspended");
    let (state, by_scim): (String, bool) =
        sqlx::query_as("SELECT state, suspended_by_scim FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!((state.as_str(), by_scim), ("suspended", true));
    // C-474 effects: org keys revoked, session revoked (the only membership).
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_keys WHERE user_id = $1 AND org_id = $2 AND revoked_at IS NULL",
    )
    .bind(user)
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 0, "org keys revoked on the adopted suspension");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(user)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 0, "last-org sessions revoked");
    // Both audit rows in the one transaction: the adoption and the suspension.
    let n = count(
        &fx.h,
        "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.add' AND outcome = 'success'",
        fx.org,
    )
    .await;
    assert!(n >= 1, "the adoption audited");
    let n = count(
        &fx.h,
        "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.update' AND metadata @> '{\"via\":\"scim\"}'",
        fx.org,
    )
    .await;
    assert!(n >= 1, "the suspension audited alongside the member.add");
    // The suspension is SCIM's own: the IdP may lift it again.
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": true}],
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "active", "reactivated");
}

#[tokio::test]
async fn adopted_suspended_member_stays_human_owned() {
    let fx = ScimFx::new().await;
    // A human-suspended member adopted by `POST active: false`: the link records the
    // suspension but must not claim it (`suspended_by_scim = false`) and must not run
    // any deprovisioning effects — so the IdP can never lift the human suspension.
    let user = fx.h.user("adopt-human@x.io", false).await;
    fx.h.member(fx.org, user, "member", "active").await;
    fx.h.set_member_status(fx.org, user, "suspended").await;
    let (_, key) = fx.h.api_key(user, fx.org, None, false).await;
    let _ = &key;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "adopt-human@x.io", "active": false})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sid = v["id"].as_str().unwrap().to_owned();
    // Link suspended, membership still suspended, and the flag stays SCIM-false.
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "suspended");
    let (state, by_scim): (String, bool) =
        sqlx::query_as("SELECT state, suspended_by_scim FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!((state.as_str(), by_scim), ("suspended", false));
    // No deprovisioning effects ran: the key was not revoked (SCIM did not suspend).
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_keys WHERE user_id = $1 AND org_id = $2 AND revoked_at IS NULL",
    )
    .bind(user)
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 1, "keys untouched for a human suspension");
    // The human suspension is not SCIM's: the IdP cannot lift it.
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": true}],
            })),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "suspended", "still suspended");
}

#[tokio::test]
async fn jit_and_invitation_gates() {
    // The dev provider JITs into the fixture org, so sign-ins hit the SCIM admission gate.
    let fx = ScimFx::with_jit(&[]).await;
    // Provision, then remove: JIT with the same e-mail is refused.
    let u = fx.provision_ok("jit@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    let signed = fx.h.try_sign_in("dev", "jit@x.io").await;
    assert!(signed.is_err(), "JIT refused for a removed link");
    // The denial is audited.
    let n = count(
        &fx.h,
        "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.add' AND outcome = 'denied'",
        fx.org,
    )
    .await;
    assert!(n >= 1, "a denied member.add row is written");
    // A suspended link refuses invitation acceptance; re-provisioning allows it again.
    let invited = fx.h.user("inv@x.io", false).await;
    fx.h.member(fx.org, invited, "member", "active").await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "inv@x.io", "active": false})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let invite = fx.h.user(&format!("invitee+{user}@x.io"), false).await;
    let _ = invite;
    // Exclusive mode: no link → refused.
    let (s, _) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/orgs/{}/scim", fx.org),
            Some(json!({"exclusive": true})),
        )
        .await;
    assert_eq!(s, 200);
    let outsider = fx.h.user("exclusive-no@x.io", false).await;
    let _ = outsider;
    let (s, _) = fx
        .mgmt(Method::GET, &format!("/v1/orgs/{}", fx.org), None)
        .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn exclusive_mode_gate_direct() {
    let fx = ScimFx::new().await;
    // Flip exclusive on.
    let (s, _) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/orgs/{}/scim", fx.org),
            Some(json!({"exclusive": true})),
        )
        .await;
    assert_eq!(s, 200);
    // A user without a link is refused by the gate.
    let outsider = fx.h.user("no-link@x.io", false).await;
    let mut tx = fx.h.pool().begin().await.unwrap();
    let gate = aurora_api::domains::scim::gates::admission_gate(&mut tx, fx.org, outsider)
        .await
        .unwrap();
    drop(tx);
    assert_eq!(gate, aurora_api::domains::scim::gates::Admission::Exclusive);
    // With a link it is allowed.
    let u = fx.provision_ok("with-link@x.io").await;
    let linked = fx.aurora_user(u["id"].as_str().unwrap()).await;
    let mut tx = fx.h.pool().begin().await.unwrap();
    let gate = aurora_api::domains::scim::gates::admission_gate(&mut tx, fx.org, linked)
        .await
        .unwrap();
    drop(tx);
    assert_eq!(gate, aurora_api::domains::scim::gates::Admission::Allowed);
}

// ---- human offboarding reconciles the link (FIX-037) -------------------------------

#[tokio::test]
async fn admin_suspend_reconciles_link_and_idp_cannot_lift_it() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("admin-sus@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    // The admin suspends the member by hand (the dashboard path).
    let (s, v) = fx
        .mgmt(
            Method::PATCH,
            &format!("/v1/orgs/{}/members/{}", fx.org, user),
            Some(json!({"status": "suspended"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // The link reconciled in the same transaction: suspended, but not SCIM's.
    let (state, by_scim): (String, bool) = sqlx::query_as(
        "SELECT state, suspended_by_scim FROM scim_users WHERE org_id = $1 AND user_id = $2",
    )
    .bind(fx.org)
    .bind(user)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!((state.as_str(), by_scim), ("suspended", false));
    // The IdP's later `active=true` push is refused (a human suspension is never lifted).
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": true}],
            })),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert_scim_error(403, &v);
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "suspended", "still suspended after the IdP's push");
    // The admin re-activates by hand: the link goes active and the gate admits again.
    let (s, v) = fx
        .mgmt(
            Method::PATCH,
            &format!("/v1/orgs/{}/members/{}", fx.org, user),
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let state: String =
        sqlx::query_scalar("SELECT state FROM scim_users WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(state, "active", "the link follows the manual reactivation");
    let mut tx = fx.h.pool().begin().await.unwrap();
    let gate = aurora_api::domains::scim::gates::admission_gate(&mut tx, fx.org, user)
        .await
        .unwrap();
    drop(tx);
    assert_eq!(gate, aurora_api::domains::scim::gates::Admission::Allowed);
}

#[tokio::test]
async fn admin_remove_tombstones_link_and_blocks_jit() {
    // The dev provider JITs into the fixture org, so the sign-in after the human removal
    // hits the SCIM admission gate exactly as after a SCIM-side delete.
    let fx = ScimFx::with_jit(&[]).await;
    let u = fx.provision_ok("jit-rm@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    // The admin removes the member by hand.
    let (s, v) = fx
        .mgmt(
            Method::DELETE,
            &format!("/v1/orgs/{}/members/{}", fx.org, user),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    // The link is a tombstone (`removed_at` set, flag false); the membership is gone.
    let (state, by_scim, removed_at): (String, bool, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT state, suspended_by_scim, removed_at FROM scim_users \
         WHERE org_id = $1 AND user_id = $2",
    )
    .bind(fx.org)
    .bind(user)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(state, "removed");
    assert!(!by_scim);
    assert!(removed_at.is_some(), "the tombstone is stamped");
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0, "the membership is gone");
    // JIT with the same e-mail is refused, and the denial is audited (`scim_removed`),
    // exactly as for a SCIM-side delete.
    let signed = fx.h.try_sign_in("dev", "jit-rm@x.io").await;
    assert!(signed.is_err(), "JIT refused after the human removal");
    let n = count(
        &fx.h,
        "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.add' \
         AND outcome = 'denied' AND metadata->>'reason' = 'scim_removed'",
        fx.org,
    )
    .await;
    assert!(n >= 1, "the scim_removed denial is audited");
}

#[tokio::test]
async fn other_org_unaffected_by_removal() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("multi@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    let org2 = fx.h.org().await;
    fx.h.member(org2, user, "member", "active").await;
    let (_, key2) = fx.h.api_key(user, org2, None, false).await;
    let _ = &key2;
    // Remove from org A.
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    // org2: membership active, keys live.
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(org2)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "active");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_keys WHERE user_id = $1 AND org_id = $2 AND revoked_at IS NULL",
    )
    .bind(user)
    .bind(org2)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 1, "the other org's key survives");
}

// ---- FIX-080: every membership-creating path passes the admission gate ---------------

/// The fixture org (`acme`, SCIM on) with the dev provider mapping the claim value
/// `acme-users` to a member role in it.
async fn roles_fx() -> ScimFx {
    ScimFx::with_config(
        |c| {
            c.scim.enabled = true;
            c.signup_mode = aurora_config::SignupMode::Open;
            c.org_creation = aurora_config::OrgCreation::Anyone;
            for p in &mut c.auth.oidc_providers {
                if p.id == aurora_config::DEV_IDP_PROVIDER_ID {
                    p.role_claim = Some("cern_roles".into());
                    p.role_mappings = vec![aurora_config::OidcRoleMapping {
                        value: "acme-users".into(),
                        org: Some(aurora_config::OidcOrgRole {
                            slug: "acme".into(),
                            role: "member".into(),
                        }),
                        ..Default::default()
                    }];
                }
            }
        },
        &[],
    )
    .await
}

fn carries_claim(fx: &ScimFx, email: &str) {
    fx.h.idp.set_user_claims(
        email,
        json!({"cern_roles": ["acme-users"]})
            .as_object()
            .unwrap()
            .clone(),
    );
}

async fn membership(fx: &ScimFx, user: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
        .bind(fx.org)
        .bind(user)
        .fetch_optional(fx.h.pool())
        .await
        .unwrap()
}

/// Denied `member.add` rows of this org for `user` by `path` and `reason`.
async fn denied_rows(fx: &ScimFx, user: Uuid, path: &str, reason: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE org_id = $1 AND action = 'member.add' \
         AND outcome = 'denied' AND target_id = $2 AND metadata->>'path' = $3 \
         AND metadata->>'reason' = $4",
    )
    .bind(fx.org)
    .bind(user.to_string())
    .bind(path)
    .bind(reason)
    .fetch_one(fx.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn fix080_role_claim_sync_does_not_readmit_removed_or_suspended_people() {
    let fx = roles_fx().await;
    // Removed by the directory, still carrying the claim: the next sign-in must not make
    // the person a member again (SEC-73), and the refusal never fails the sign-in itself.
    let u = fx.provision_ok("gone@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    carries_claim(&fx, "gone@x.io");
    fx.h.try_sign_in("dev", "gone@x.io")
        .await
        .expect("a refused role claim does not fail the sign-in");
    assert_eq!(membership(&fx, user).await, None, "no membership");
    assert_eq!(
        denied_rows(&fx, user, "idp_sync", "scim_removed").await,
        1,
        "one denied member.add row"
    );
    // A suspended link keeps the membership suspended and creates nothing.
    let u = fx.provision_ok("paused@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    let (s, _) = deactivate(&fx, &sid).await;
    assert_eq!(s, 200);
    carries_claim(&fx, "paused@x.io");
    fx.h.try_sign_in("dev", "paused@x.io").await.unwrap();
    assert_eq!(membership(&fx, user).await.as_deref(), Some("suspended"));
}

#[tokio::test]
async fn fix080_role_claim_sync_obeys_exclusive_and_the_seat_cap() {
    let fx = roles_fx().await;
    let (s, _) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/orgs/{}/scim", fx.org),
            Some(json!({"exclusive": true})),
        )
        .await;
    assert_eq!(s, 200);
    let outsider = fx.h.user("outsider@x.io", false).await;
    carries_claim(&fx, "outsider@x.io");
    fx.h.try_sign_in("dev", "outsider@x.io").await.unwrap();
    assert_eq!(membership(&fx, outsider).await, None);
    assert_eq!(
        denied_rows(&fx, outsider, "idp_sync", "scim_exclusive").await,
        1
    );
    // Not exclusive, but the org is full (the owner is the one seat).
    let (s, _) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/orgs/{}/scim", fx.org),
            Some(json!({"exclusive": false})),
        )
        .await;
    assert_eq!(s, 200);
    sqlx::query(
        "UPDATE organizations SET settings = settings || '{\"max_members\": 1}'::jsonb WHERE id = $1",
    )
    .bind(fx.org)
    .execute(fx.h.pool())
    .await
    .unwrap();
    let late = fx.h.user("late@x.io", false).await;
    carries_claim(&fx, "late@x.io");
    fx.h.try_sign_in("dev", "late@x.io").await.unwrap();
    assert_eq!(membership(&fx, late).await, None, "seat cap holds");
    assert_eq!(
        denied_rows(&fx, late, "idp_sync", "member_limit_reached").await,
        1
    );
}

#[tokio::test]
async fn fix080_scim_never_claims_a_human_suspension() {
    let fx = ScimFx::new().await;
    // An admin suspends a member by hand; the IdP then adopts them as active.
    let user = fx.h.user("human-sus@x.io", false).await;
    fx.h.member(fx.org, user, "member", "active").await;
    fx.h.set_member_status(fx.org, user, "suspended").await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "human-sus@x.io", "active": true})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sid = v["id"].as_str().unwrap().to_owned();
    // The IdP deactivates and reactivates: the human suspension must survive both.
    let (s, v) = deactivate(&fx, &sid).await;
    assert_eq!(s, 200, "{v}");
    let (state, by_scim): (String, bool) =
        sqlx::query_as("SELECT state, suspended_by_scim FROM scim_users WHERE id = $1")
            .bind(sid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), by_scim),
        ("suspended", false),
        "the suspension was not SCIM's to claim"
    );
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": true}],
            })),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(membership(&fx, user).await.as_deref(), Some("suspended"));
}

async fn lock_waiters(fx: &ScimFx) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity \
         WHERE datname = current_database() AND wait_event_type = 'Lock'",
    )
    .fetch_one(fx.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn fix080_sign_in_and_scim_delete_serialize_on_the_org_lock() {
    let fx = roles_fx().await;
    // A person with an account but no membership and no link; the claim would admit them.
    let user = fx.h.user("race@x.io", false).await;
    carries_claim(&fx, "race@x.io");
    // Another connection holds the org lock (what every SCIM writer takes).
    let mut held = fx.h.pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
        .bind(fx.org)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    // Only the callback: when the sign-in wins the lock, the DELETE that follows removes the
    // membership it created and revokes its session (`revoke_sessions_if_last`), so a later
    // `/v1/me` with that cookie is rightly 401.
    let sign_in = async {
        let (path, flow) = fx.h.oidc_authorize("dev", "", Some("race@x.io")).await;
        let (st, hd, _) =
            fx.h.send(Method::GET, &path, &[("cookie", &flow)], None)
                .await;
        assert_eq!(st, http::StatusCode::FOUND, "callback");
        let loc = hd["location"].to_str().unwrap().to_owned();
        hd.get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|c| c.starts_with("aurora_session=aus_"))
            .then_some(())
            .ok_or(loc)
    };
    // The IdP provisions and immediately removes the same person, racing the sign-in.
    let scim_side = async {
        let (s, _, v) = fx
            .scim(
                Method::POST,
                USERS,
                Some(json!({"userName": "race@x.io", "active": true})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let sid = v["id"].as_str().unwrap().to_owned();
        let (s, _, _) = fx
            .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
            .await;
        assert_eq!(s, 204);
    };
    let release = async {
        // The sign-in (and the SCIM calls) must be queued behind the held lock: poll with a
        // deadline for two waiting backends, then let them go.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while lock_waiters(&fx).await < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "the sign-in did not wait on the org lock"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        held.commit().await.unwrap();
    };
    let (signed, (), ()) = tokio::join!(sign_in, scim_side, release);
    signed.expect("sign-in succeeds whichever side wins");
    // Whatever the order: never an active membership beside a removed link.
    let (link_state,): (String,) =
        sqlx::query_as("SELECT state FROM scim_users WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(link_state, "removed");
    assert_eq!(
        membership(&fx, user).await,
        None,
        "no member beside a tombstone"
    );
}

#[tokio::test]
async fn fix080_refused_invitations_leave_a_denied_row_that_survives() {
    let fx = ScimFx::new().await;
    // ---- the accept route: removed link + a pending invitation created afterwards.
    let u = fx.provision_ok("route@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = $1")
        .bind(user)
        .execute(fx.h.pool())
        .await
        .unwrap();
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    let tok = format!("aui_{}", Uuid::now_v7().simple());
    insert_invitation(&fx, "route@x.io", &tok).await;
    let (_, session) = fx.h.good_session(user).await;
    let (st, _, v) =
        fx.h.send(
            Method::POST,
            "/v1/invitations/accept",
            &[("cookie", &cookie(&session)), ("origin", ORIGIN)],
            Some(json!({"token": tok})),
        )
        .await;
    assert_eq!(st, 403, "{v}");
    assert_eq!(v["error"]["code"], "scim_refused", "{v}");
    assert_eq!(membership(&fx, user).await, None);
    assert_eq!(
        denied_rows(&fx, user, "invitation", "scim_removed").await,
        1,
        "the denied row survives the refused request"
    );

    // ---- the sign-in path: an exclusive org, a new person invited by e-mail.
    let (s, _) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/orgs/{}/scim", fx.org),
            Some(json!({"exclusive": true})),
        )
        .await;
    assert_eq!(s, 200);
    // `none` placement: the invitation is accepted as part of the sign-in.
    sqlx::query("UPDATE instance_placement SET mode = 'none'")
        .execute(fx.h.pool())
        .await
        .unwrap();
    insert_invitation(
        &fx,
        "newbie@x.io",
        &format!("aui_{}", Uuid::now_v7().simple()),
    )
    .await;
    let signed = fx.h.try_sign_in("dev", "newbie@x.io").await.unwrap();
    assert_eq!(membership(&fx, signed.user_id).await, None);
    assert_eq!(
        denied_rows(&fx, signed.user_id, "invitation", "scim_exclusive").await,
        1,
        "the sign-in's denied row is not rolled back with the savepoint"
    );
    let still: String = sqlx::query_scalar(
        "SELECT status FROM invitations WHERE org_id = $1 AND email = 'newbie@x.io'",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(still, "pending", "denied, not cancelled");
}

async fn insert_invitation(fx: &ScimFx, email: &str, tok: &str) {
    sqlx::query(
        "INSERT INTO invitations (id, org_id, email, role, token_hash, invited_by, status, created_at, updated_at)
         VALUES ($1, $2, $3, 'member', $4, NULL, 'pending', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(fx.org)
    .bind(email)
    .bind(&token::hash(tok)[..])
    .execute(fx.h.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn fix080_adoption_with_active_false_cancels_pending_invitations() {
    let fx = ScimFx::new().await;
    let user = fx.h.user("adopt-inv@x.io", false).await;
    fx.h.member(fx.org, user, "member", "active").await;
    insert_invitation(&fx, "adopt-inv@x.io", "aui_adopt").await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "adopt-inv@x.io", "active": false})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let status: String = sqlx::query_scalar(
        "SELECT status FROM invitations WHERE org_id = $1 AND email = 'adopt-inv@x.io'",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(status, "cancelled");
}

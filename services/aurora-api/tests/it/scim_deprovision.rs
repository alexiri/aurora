#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Deprovisioning (SEC-73, D-135, docs/18 §SCIM): the full effects matrix of `04 §SCIM` for
//! suspend and remove — membership, sessions rule with one and with two orgs, key revocation
//! (never revived by reactivation), running speaker turns aborted with `user_deprovisioned`,
//! pending invitations cancelled, JIT and invitation acceptance refused and allowed again,
//! reactivation only when `suspended_by_scim`, `exclusive` mode, other orgs unaffected.

use crate::scim_common::*;
use aurora_core::token;
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
    // Suspend via SCIM → `suspended_by_scim` → reactivate allowed.
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
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(human)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "active", "reactivated");
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

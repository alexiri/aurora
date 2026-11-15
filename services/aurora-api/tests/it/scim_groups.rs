#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Groups through SCIM (docs/18 §SCIM): SCIM groups appear in `/v1` as `source = scim` with
//! read-only name and members (`group_managed_by_scim`), members must be this org's links,
//! removed links are rejected, `PUT` replaces atomically with one audit row per change,
//! deleting in the IdP deletes bindings, and the access/gateway effects match the `/v1`
//! groups.

use crate::scim_common::*;
use chrono::{DateTime, Utc};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const USERS: &str = "/scim/v2/Users";
const GROUPS: &str = "/scim/v2/Groups";
const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";

#[tokio::test]
async fn scim_group_visible_in_v1_read_only() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("gm@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "scim-made",
                "members": [{"value": sid}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // In /v1 the group shows `source = scim`.
    let (s, v) = fx
        .mgmt(Method::GET, &format!("/v1/groups/{gid}"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["source"], "scim", "{v}");
    assert_eq!(v["name"], "scim-made", "{v}");
    let (s, v) = fx
        .mgmt(Method::GET, &format!("/v1/groups/{gid}/members"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    // The /v1 list carries it too.
    let (s, v) = fx
        .mgmt(Method::GET, &format!("/v1/orgs/{}/groups", fx.org), None)
        .await;
    assert_eq!(s, 200, "{v}");
    let found = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["id"] == v_json(&gid));
    assert!(found, "{v}");
    // Writes through /v1 are refused with `group_managed_by_scim`.
    let (s, v) = fx
        .mgmt(
            Method::PATCH,
            &format!("/v1/groups/{gid}"),
            Some(json!({"revision": v["revision"], "name": "renamed-here"})),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "group_managed_by_scim", "{v}");
    // Members cannot be edited through /v1 either.
    let member = fx.h.user("other-m@x.io", false).await;
    fx.h.member(fx.org, member, "member", "active").await;
    let (s, v) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/groups/{gid}/members/{member}"),
            None,
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "group_managed_by_scim", "{v}");
}

fn v_json(id: &Uuid) -> Value {
    Value::String(id.to_string())
}

#[tokio::test]
async fn members_must_be_this_orgs_links() {
    let fx = ScimFx::new().await;
    // A member of another org (a real Aurora user, but no link here).
    let stranger = fx.h.user("stranger@x.io", false).await;
    let other = fx.h.org().await;
    fx.h.member(other, stranger, "member", "active").await;
    // A random uuid and a non-uuid are equally invalidValue.
    for bad in [stranger.to_string(), "not-a-uuid".to_owned()] {
        let (s, _, v) = fx
            .scim(
                Method::POST,
                GROUPS,
                Some(json!({
                    "schemas": [GROUP_SCHEMA],
                    "displayName": "g",
                    "members": [{"value": bad}],
                })),
            )
            .await;
        assert_eq!(s, 400, "{bad}: {v}");
        assert_eq!(scim_type(&v), Some("invalidValue"), "{bad}: {v}");
    }
    // Nothing was created.
    let (s, _, v) = fx.scim(Method::GET, GROUPS, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["totalResults"], 0, "{v}");
}

#[tokio::test]
async fn removed_link_rejected_as_member() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("bye@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 204);
    // A removed link is rejected as a member.
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "g",
                "members": [{"value": sid}],
            })),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("invalidValue"), "{v}");
}

#[tokio::test]
async fn put_replaces_atomically_one_audit_row_per_change() {
    let fx = ScimFx::new().await;
    let a = fx.provision_ok("a1@x.io").await;
    let b = fx.provision_ok("b1@x.io").await;
    let c = fx.provision_ok("c1@x.io").await;
    let (sid_a, sid_b, sid_c) = (
        a["id"].as_str().unwrap().to_owned(),
        b["id"].as_str().unwrap().to_owned(),
        c["id"].as_str().unwrap().to_owned(),
    );
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "atomic",
                "members": [{"value": sid_a}, {"value": sid_b}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_owned();
    // One of the member ids is bogus: the whole PUT fails, nothing changed.
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{GROUPS}/{gid}"),
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "atomic",
                "members": [{"value": sid_c}, {"value": "11111111-1111-1111-1111-111111111111"}],
            })),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    let (s, _, v) = fx.scim(Method::GET, &format!("{GROUPS}/{gid}"), None).await;
    assert_eq!(s, 200, "{v}");
    let members: Vec<&str> = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["value"].as_str())
        .collect();
    assert_eq!(members.len(), 2, "unchanged after the failed PUT: {v}");
    // A good PUT: replace [a,b] with [b,c] → one add, one remove, two audit rows.
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action IN ('group_member.add','group_member.remove') AND target_id = $1::text AND org_id = $2",
    )
    .bind(gid.clone())
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{GROUPS}/{gid}"),
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "atomic",
                "members": [{"value": sid_b}, {"value": sid_c}],
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let members: Vec<&str> = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["value"].as_str())
        .collect();
    assert_eq!(members, vec![sid_b.as_str(), sid_c.as_str()], "{v}");
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action IN ('group_member.add','group_member.remove') AND target_id = $1::text AND org_id = $2",
    )
    .bind(gid.clone())
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(after - before, 2, "one add + one remove audited");
}

#[tokio::test]
async fn idp_delete_removes_bindings_and_access_effects() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("bind@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    // A workspace and a group granting access to it (org_role admin through the group).
    let ws = fx.h.workspace(fx.org).await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "ws-access",
                "members": [{"value": sid}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_owned();
    // Bind the group to the workspace via /v1.
    let (s, v) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{gid}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // The user has access (group-derived: with `expand=groups`).
    let (s, v) = fx
        .mgmt(
            Method::GET,
            &format!("/v1/workspaces/{ws}/access?expand=groups"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let has = v["derived_members"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["user"]["id"] == v_json(&user));
    assert!(has, "the user has group-derived access: {v}");
    // Deleting the group in the IdP deletes the binding and the derived access.
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{GROUPS}/{gid}"), None)
        .await;
    assert_eq!(s, 204);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM group_workspace_access WHERE group_id = $1")
            .bind(gid.parse::<Uuid>().unwrap())
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0, "bindings deleted with the group");
    let (s, v) = fx
        .mgmt(
            Method::GET,
            &format!("/v1/workspaces/{ws}/access?expand=groups"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let has = v["derived_members"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["user"]["id"] == v_json(&user));
    assert!(!has, "the derived access is gone: {v}");
}

#[tokio::test]
async fn scim_group_members_derive_workspace_access() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("derive@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let user = fx.aurora_user(&sid).await;
    let ws = fx.h.workspace(fx.org).await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "derivers",
                "members": [{"value": sid}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_owned();
    let (s, v) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{gid}"),
            Some(json!({"role": "approver"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // The access view lists the user (derived from the SCIM group).
    let (s, v) = fx
        .mgmt(
            Method::GET,
            &format!("/v1/workspaces/{ws}/access?expand=groups"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let entry = v["derived_members"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["user"]["id"] == v_json(&user))
        .cloned()
        .unwrap_or(Value::Null);
    assert!(!entry.is_null(), "{v}");
    // The derived_members/sources view names the SCIM group as the source.
    let src = entry.to_string();
    assert!(
        src.contains("derivers") || src.contains(&gid),
        "source naming: {entry}"
    );
}

/// `revision`/`updated_at` of one group row.
async fn group_row(pool: &sqlx::PgPool, gid: &str) -> (i64, DateTime<Utc>) {
    sqlx::query_as("SELECT revision, updated_at FROM groups WHERE id = $1")
        .bind(gid.parse::<Uuid>().unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Audit rows of one action for a group.
async fn audit_count(pool: &sqlx::PgPool, org: Uuid, gid: &str, action: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = $1 AND target_id = $2::text AND org_id = $3",
    )
    .bind(action)
    .bind(gid)
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn put_same_body_twice_writes_nothing_the_second_time() {
    let fx = ScimFx::new().await;
    let a = fx.provision_ok("noop-a@x.io").await;
    let b = fx.provision_ok("noop-b@x.io").await;
    let (sid_a, sid_b) = (
        a["id"].as_str().unwrap().to_owned(),
        b["id"].as_str().unwrap().to_owned(),
    );
    let body = json!({
        "schemas": [GROUP_SCHEMA],
        "displayName": "no-op-2",
        "externalId": "noop-1",
        "members": [{"value": sid_a}, {"value": sid_b}],
    });
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "no-op",
                "externalId": "noop-1",
                "members": [{"value": sid_a}, {"value": sid_b}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_owned();
    // The first PUT renames (`no-op` → `no-op-2`): a real change, audited and
    // revision-bumped as before.
    let (s, _, v) = fx
        .scim(Method::PUT, &format!("{GROUPS}/{gid}"), Some(body.clone()))
        .await;
    assert_eq!(s, 200, "{v}");
    let (revision, updated_at) = group_row(fx.h.pool(), &gid).await;
    assert_eq!(
        audit_count(fx.h.pool(), fx.org, &gid, "group.update").await,
        1,
        "the rename PUT audited the group.update"
    );
    // The second identical PUT (the name/externalId equal the stored values):
    // no rename UPDATE (revision unchanged), no new `group.update` row
    // (C-478: idempotent no-ops write nothing).
    let (s, _, v) = fx
        .scim(Method::PUT, &format!("{GROUPS}/{gid}"), Some(body.clone()))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["externalId"], "noop-1", "{v}");
    let (revision_after, updated_at_after) = group_row(fx.h.pool(), &gid).await;
    assert_eq!(revision_after, revision, "no revision bump");
    assert_eq!(updated_at_after, updated_at, "not touched");
    assert_eq!(
        audit_count(fx.h.pool(), fx.org, &gid, "group.update").await,
        1,
        "no `group.update` row for a no-op PUT"
    );
    // Member diffs keep auditing: a PUT that removes a member (same name,
    // externalId) audits the removal and still no `group.update`.
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{GROUPS}/{gid}"),
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "no-op-2",
                "externalId": "noop-1",
                "members": [{"value": sid_b}],
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let members: Vec<&str> = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["value"].as_str())
        .collect();
    assert_eq!(members, vec![sid_b.as_str()], "{v}");
    assert_eq!(
        audit_count(fx.h.pool(), fx.org, &gid, "group_member.remove").await,
        1,
        "the removal audited"
    );
    assert_eq!(
        audit_count(fx.h.pool(), fx.org, &gid, "group.update").await,
        1,
        "no group.update for a members-only change"
    );
}

/// FIX-062 (SEC-71: a removal is effective for the next request, not after a TTL): an IdP
/// `DELETE /Groups/{id}` of a group with an `org_role` mapping and members sends exactly the
/// dashboard delete's invalidation set — scope `user` for each member and scope `org` for
/// the mapping — on `aurora_gateway_invalidate`. Before FIX-062 only the workspace scopes
/// were sent and the gateway kept the deleted group's grants cached.
#[tokio::test]
async fn idp_delete_invalidates_member_and_org_identities() {
    let fx = ScimFx::new().await;
    let a = fx.provision_ok("gd-a@x.io").await;
    let b = fx.provision_ok("gd-b@x.io").await;
    let (sa, sb) = (
        a["id"].as_str().unwrap().to_owned(),
        b["id"].as_str().unwrap().to_owned(),
    );
    let (ua, ub) = (fx.aurora_user(&sa).await, fx.aurora_user(&sb).await);
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "gd-admins",
                "members": [{"value": sa}, {"value": sb}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_owned();
    // An admin maps the SCIM group to an org role in the dashboard.
    let (s, v) = fx
        .mgmt(
            Method::PATCH,
            &format!("/v1/groups/{gid}"),
            Some(json!({"org_role": "admin"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    let mut l = sqlx::postgres::PgListener::connect_with(fx.h.pool())
        .await
        .unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{GROUPS}/{gid}"), None)
        .await;
    assert_eq!(s, 204);
    let want: Vec<(String, String)> = vec![
        ("user".into(), ua.to_string()),
        ("user".into(), ub.to_string()),
        ("org".into(), fx.org.to_string()),
    ];
    let mut seen: Vec<(String, String)> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !want.iter().all(|w| seen.contains(w)) {
        let n = tokio::time::timeout_at(deadline, l.recv())
            .await
            .unwrap_or_else(|_| panic!("missing invalidations: want {want:?}, seen {seen:?}"))
            .unwrap();
        let p: Value = serde_json::from_str(n.payload()).unwrap();
        seen.push((
            p["scope"].as_str().unwrap_or("").to_owned(),
            p["id"].as_str().unwrap_or("").to_owned(),
        ));
    }
}

/// FIX-062: concurrent `PATCH add` operations on one group (Entra/Okta send one per member)
/// compose — the members are read inside the transaction after the group row lock, so no
/// request overwrites another's addition. Before FIX-062 the members were read before the
/// transaction and the whole set written back: parallel adds lost members.
#[tokio::test]
async fn concurrent_patch_adds_keep_every_member() {
    const N: usize = 8;
    let fx = ScimFx::new().await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({"schemas": [GROUP_SCHEMA], "displayName": "race"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_owned();
    let mut sids = Vec::with_capacity(N);
    for i in 0..N {
        let u = fx.provision_ok(&format!("race-{i}@x.io")).await;
        sids.push(u["id"].as_str().unwrap().to_owned());
    }
    // Every request waits at the barrier, then all are sent at once.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(N));
    let rs = futures::future::join_all(sids.iter().map(|sid| {
        let barrier = barrier.clone();
        let fx = &fx;
        let gid = gid.clone();
        async move {
            barrier.wait().await;
            fx.scim(
                Method::PATCH,
                &format!("{GROUPS}/{gid}"),
                Some(json!({
                    "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                    "Operations": [{"op": "add", "path": "members", "value": [{"value": sid}]}],
                })),
            )
            .await
        }
    }))
    .await;
    for (s, _, v) in &rs {
        assert_eq!(*s, 200, "{v}");
    }
    let (s, _, v) = fx.scim(Method::GET, &format!("{GROUPS}/{gid}"), None).await;
    assert_eq!(s, 200, "{v}");
    let mut members: Vec<String> = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["value"].as_str().map(str::to_owned))
        .collect();
    members.sort();
    let mut want = sids.clone();
    want.sort();
    assert_eq!(members, want, "every concurrent add kept");
}

/// FIX-083: every SCIM group write takes the org lock before the group row lock, so a sign-in
/// group sync (which inserts members under the org lock) cannot race a PATCH or DELETE into a
/// foreign-key failure. Another connection holds the org lock; the write must queue behind it.
#[tokio::test]
async fn group_patch_and_delete_queue_behind_the_org_lock() {
    let fx = ScimFx::new().await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({"schemas": [GROUP_SCHEMA], "displayName": "locked"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_owned();
    for (label, method, body, want) in [
        (
            "patch",
            Method::PATCH,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "displayName", "value": "renamed"}],
            })),
            200u16,
        ),
        ("delete", Method::DELETE, None, 204u16),
    ] {
        let mut held = fx.h.pool().begin().await.unwrap();
        sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
            .bind(fx.org)
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let uri = format!("{GROUPS}/{gid}");
        let write = fx.scim(method, &uri, body);
        let release = async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let waiting: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_stat_activity \
                     WHERE datname = current_database() AND wait_event_type = 'Lock'",
                )
                .fetch_one(fx.h.pool())
                .await
                .unwrap();
                if waiting >= 1 {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "{label}: the SCIM group write did not wait for the org lock"
                );
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            held.commit().await.unwrap();
        };
        let ((s, _, v), ()) = tokio::join!(write, release);
        assert_eq!(s.as_u16(), want, "{label}: {v}");
    }
}

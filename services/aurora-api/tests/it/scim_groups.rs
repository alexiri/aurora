#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Groups through SCIM (docs/18 §SCIM): SCIM groups appear in `/v1` as `source = scim` with
//! read-only name and members (`group_managed_by_scim`), members must be this org's links,
//! removed links are rejected, `PUT` replaces atomically with one audit row per change,
//! deleting in the IdP deletes bindings, and the access/gateway effects match the `/v1`
//! groups.

use crate::scim_common::*;
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
        "SELECT count(*) FROM audit_log WHERE action IN ('group_member.add','group_member.remove') AND target_id = $1::text",
    )
    .bind(gid.clone())
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
        "SELECT count(*) FROM audit_log WHERE action IN ('group_member.add','group_member.remove') AND target_id = $1::text",
    )
    .bind(gid.clone())
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

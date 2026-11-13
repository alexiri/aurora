#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-125: the instance audit log (`GET /v1/admin/audit`) and the inventory of instance-admin
//! writes: each one leaves an audit row without an organization (or with the affected org's id).

use crate::common::*;
use aurora_crypto::generate_signing_key;
use aurora_db::repo::audit::{self, NewAudit};
use aurora_registry::builder::PackageBuilder;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

async fn call(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let c = cookie(tok);
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

async fn put(h: &Harness, org: Option<Uuid>, actor: Uuid, action: &str) {
    audit::insert(
        h.pool(),
        &NewAudit {
            org_id: org,
            workspace_id: None,
            actor_kind: "instance_admin",
            actor_id: Some(actor),
            actor_label: "root@x.io",
            action,
            target_kind: None,
            target_id: None,
            outcome: "success",
            source: "api",
            ip: None,
            metadata: &json!({}),
        },
    )
    .await
    .unwrap();
}

fn actions(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["action"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn instance_audit_scope_filters_pagination_and_permissions() {
    let h = Harness::new().await;
    let root = h.user("root@x.io", true).await;
    let owner = h.user("o@x.io", false).await;
    let org = h.org().await;
    h.member(org, owner, "owner", "active").await;
    let (_, rt) = h.good_session(root).await;
    let (_, ot) = h.good_session(owner).await;
    for a in [
        "trusted_key.create",
        "plugin.approve",
        "plugin.ingest",
        "runner.note",
    ] {
        put(&h, None, root, a).await;
    }
    put(&h, Some(org), root, "org.freeze").await;
    put(&h, Some(org), owner, "org.update").await;

    // Non-admins: 403, and the org's own audit is unaffected (no instance rows in it).
    assert_eq!(
        call(&h, &ot, Method::GET, "/v1/admin/audit", None).await.0,
        403
    );
    let (s, v) = call(&h, &ot, Method::GET, &format!("/v1/orgs/{org}/audit"), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);

    // Default scope = instance rows only.
    let (s, v) = call(&h, &rt, Method::GET, "/v1/admin/audit", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 4);
    assert!(v["items"][0]["org_id"].is_null() && v["items"][0]["org_slug"].is_null());
    // scope=all adds the org rows, with the org slug.
    let (_, v) = call(&h, &rt, Method::GET, "/v1/admin/audit?scope=all", None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 6);
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["org_id"] == json!(org) && r["org_slug"].is_string())
    );
    // org filter; action prefix; actor; critical.
    let (_, v) = call(
        &h,
        &rt,
        Method::GET,
        &format!("/v1/admin/audit?org={org}"),
        None,
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let (_, v) = call(&h, &rt, Method::GET, "/v1/admin/audit?action=plugin.", None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let (_, v) = call(
        &h,
        &rt,
        Method::GET,
        &format!("/v1/admin/audit?scope=all&actor_id={owner}"),
        None,
    )
    .await;
    assert_eq!(actions(&v), ["org.update"]);
    let (_, v) = call(&h, &rt, Method::GET, "/v1/admin/audit?critical=true", None).await;
    let mut a = actions(&v);
    a.sort();
    assert_eq!(a, ["plugin.approve", "trusted_key.create"]);
    // since/until bounds.
    let (_, v) = call(
        &h,
        &rt,
        Method::GET,
        "/v1/admin/audit?since=2999-01-01T00:00:00Z",
        None,
    )
    .await;
    assert!(v["items"].as_array().unwrap().is_empty());
    let (_, v) = call(
        &h,
        &rt,
        Method::GET,
        "/v1/admin/audit?until=2999-01-01T00:00:00Z&limit=2",
        None,
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    // Bad scope → 400.
    assert_eq!(
        call(&h, &rt, Method::GET, "/v1/admin/audit?scope=x", None)
            .await
            .0,
        400
    );

    // Cursor pages are stable and complete.
    let mut seen = Vec::new();
    let mut cur: Option<String> = None;
    loop {
        let uri = match &cur {
            Some(c) => format!("/v1/admin/audit?scope=all&limit=2&cursor={c}"),
            None => "/v1/admin/audit?scope=all&limit=2".to_owned(),
        };
        let (_, v) = call(&h, &rt, Method::GET, &uri, None).await;
        for r in v["items"].as_array().unwrap() {
            seen.push(r["id"].as_str().unwrap().to_owned());
        }
        match v["next_cursor"].as_str() {
            Some(c) => cur = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 6);
    let mut u = seen.clone();
    u.sort();
    u.dedup();
    assert_eq!(u.len(), 6);
}

/// One audit row for `action` must exist after `f` ran; returns it.
async fn row(h: &Harness, t: &str, action: &str) -> Value {
    let (_, v) = call(
        h,
        t,
        Method::GET,
        &format!("/v1/admin/audit?scope=all&action={action}"),
        None,
    )
    .await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{action}: {v}");
    items[0].clone()
}

#[tokio::test]
async fn every_instance_admin_write_in_the_inventory_leaves_an_audit_row() {
    let h = Harness::new().await;
    let root = h.user("root@x.io", true).await;
    let other = h.user("u@x.io", false).await;
    let org = h.org().await;
    let (_, t) = h.good_session(root).await;

    let key = generate_signing_key().unwrap();
    let (s, v) = call(
        &h,
        &t,
        Method::POST,
        "/v1/admin/trusted-keys",
        Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let key_id = v["id"].as_str().unwrap().to_owned();
    let (s, v) = call(
        &h,
        &t,
        Method::POST,
        "/v1/admin/runners",
        Some(json!({"name": "r1"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let runner = v["runner"]["id"].as_str().unwrap().to_owned();
    let (s, _) = call(
        &h,
        &t,
        Method::POST,
        &format!("/v1/admin/runners/{runner}/token"),
        None,
    )
    .await;
    assert_eq!(s, 200);
    let (s, _) = call(
        &h,
        &t,
        Method::DELETE,
        &format!("/v1/admin/runners/{runner}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
    let (s, _) = call(
        &h,
        &t,
        Method::DELETE,
        &format!("/v1/admin/trusted-keys/{key_id}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
    let (s, v) = call(
        &h,
        &t,
        Method::PATCH,
        &format!("/v1/admin/users/{other}"),
        Some(json!({"is_instance_admin": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(
        &h,
        &t,
        Method::POST,
        &format!("/v1/admin/orgs/{org}/freeze"),
        Some(json!({"reason": "incident"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(
        &h,
        &t,
        Method::POST,
        &format!("/v1/admin/orgs/{org}/unfreeze"),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(
        &h,
        &t,
        Method::PUT,
        "/v1/admin/placement",
        Some(json!({"mode": "none"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");

    // (action, org-scoped?) — each exactly once, critical, written by an instance admin.
    for (action, scoped) in [
        ("trusted_key.create", false),
        ("trusted_key.revoke", false),
        ("admin.runner_create", false),
        ("admin.runner_rotate", false),
        ("admin.runner_delete", false),
        ("admin.user_update", false),
        ("org.freeze", true),
        ("org.unfreeze", true),
        ("placement.update", false),
    ] {
        let r = row(&h, &t, action).await;
        assert_eq!(r["critical"], true, "{action}");
        assert_eq!(r["actor_kind"], "instance_admin", "{action}");
        if scoped {
            assert_eq!(r["org_id"], json!(org), "{action}");
        } else {
            assert!(r["org_id"].is_null(), "{action}: {r}");
        }
    }
}

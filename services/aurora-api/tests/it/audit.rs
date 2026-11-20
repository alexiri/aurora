#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_db::repo::audit::{self, NewAudit};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

async fn call(h: &Harness, tok: &str, uri: &str) -> (u16, Value) {
    h.call_tok(tok, Method::GET, uri, None).await
}

async fn put(h: &Harness, org: Uuid, ws: Option<Uuid>, actor: Uuid, action: &str) {
    audit::insert(
        h.pool(),
        &NewAudit {
            org_id: Some(org),
            workspace_id: ws,
            actor_kind: "user",
            actor_id: Some(actor),
            actor_label: "a@x.io",
            action,
            target_kind: None,
            target_id: None,
            outcome: "success",
            source: "api",
            ip: Some("10.0.0.1".parse().unwrap()),
            metadata: &json!({}),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn org_audit_permissions_filters_pagination() {
    let h = Harness::new().await;
    let owner = h.user("o@x.io", false).await;
    let member = h.user("m@x.io", false).await;
    let org = h.org().await;
    let other = h.org().await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, member, "member", "active").await;
    let (_, ot) = h.good_session(owner).await;
    let (_, mt) = h.good_session(member).await;
    for _ in 0..3 {
        put(&h, org, None, owner, "org.update").await;
    }
    put(&h, org, None, member, "member.invite").await;
    put(&h, other, None, owner, "org.update").await;
    let uri = format!("/v1/orgs/{org}/audit");
    let (s, _) = call(&h, &mt, &uri).await;
    assert_eq!(s, 403);
    let (s, v) = call(&h, &ot, &uri).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 4);
    assert_eq!(v["items"][0]["ip"], "10.0.0.1");
    let (_, v) = call(&h, &ot, &format!("{uri}?action=member.invite")).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (_, v) = call(&h, &ot, &format!("{uri}?actor_id={owner}")).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 3);
    let (_, p1) = call(&h, &ot, &format!("{uri}?limit=3")).await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 3);
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = call(&h, &ot, &format!("{uri}?limit=3&cursor={cur}")).await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert!(p2["next_cursor"].is_null());
    // outsider: 404
    let out = h.user("x@x.io", false).await;
    let (_, xt) = h.good_session(out).await;
    assert_eq!(call(&h, &xt, &uri).await.0, 404);
}

/// (P5-REFINE-6, C-1088) `include_system=false` leaves out the platform's own events and says
/// how many; the default (and `true`) lists them; the callers are unchanged.
#[tokio::test]
async fn system_events_can_be_hidden_and_are_counted() {
    let h = Harness::new().await;
    let owner = h.user("o@x.io", false).await;
    let org = h.org().await;
    h.member(org, owner, "owner", "active").await;
    let (_, ot) = h.good_session(owner).await;
    put(&h, org, None, owner, "org.update").await;
    for _ in 0..2 {
        audit::insert(
            h.pool(),
            &NewAudit {
                org_id: Some(org),
                workspace_id: None,
                actor_kind: "system",
                actor_id: None,
                actor_label: "plugin-builtin",
                action: "plugin.install",
                target_kind: None,
                target_id: None,
                outcome: "success",
                source: "system",
                ip: None,
                metadata: &json!({}),
            },
        )
        .await
        .unwrap();
    }
    let uri = format!("/v1/orgs/{org}/audit");
    let (s, v) = call(&h, &ot, &uri).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 3);
    assert!(v.get("hidden_system").is_none(), "{v}");
    let (_, v) = call(&h, &ot, &format!("{uri}?include_system=false")).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["items"][0]["actor_kind"], "user");
    assert_eq!(v["hidden_system"], 2);
    // The instance list (instance admins only) the same way.
    let root = h.user("root@x.io", true).await;
    let (_, rt) = h.good_session(root).await;
    let (s, v) = call(
        &h,
        &rt,
        &format!("/v1/admin/audit?org={org}&include_system=false"),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["hidden_system"], 2);
    assert_eq!(
        call(&h, &ot, "/v1/admin/audit?include_system=false")
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn workspace_audit_requires_can_own() {
    let h = Harness::new().await;
    let owner = h.user("o@x.io", false).await;
    let member = h.user("m@x.io", false).await;
    let org = h.org().await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, member, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, member, "member").await;
    let (_, ot) = h.good_session(owner).await;
    let (_, mt) = h.good_session(member).await;
    put(&h, org, Some(ws), owner, "agent.create").await;
    put(&h, org, None, owner, "org.update").await;
    let uri = format!("/v1/workspaces/{ws}/audit");
    assert_eq!(call(&h, &mt, &uri).await.0, 403);
    let (s, v) = call(&h, &ot, &uri).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

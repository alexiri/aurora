#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-UX-2 (D-349, C-981): `WhoCan` in every refusal of the shared role helpers, and `contacts`
//! on a workspace and an organization.

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    member: String,
    outsider_admin: String,
}

async fn named(h: &Harness, email: &str, name: &str, admin: bool) -> Uuid {
    let u = h.user(email, admin).await;
    sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
        .bind(u)
        .bind(name)
        .execute(h.pool())
        .await
        .unwrap();
    u
}

/// An organization with one owner (Zoe), admin (Adam), five more workspace owners, a plain
/// member, and an instance admin who is a member of nothing (Root).
async fn world() -> W {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let zoe = named(&h, "zoe@x.io", "Zoe Owner", false).await;
    h.member(org, zoe, "owner", "active").await;
    let adam = named(&h, "adam@x.io", "Adam Admin", false).await;
    h.member(org, adam, "admin", "active").await;
    for (i, n) in ["Eve", "Bob", "Dan", "Cy", "Abe"].into_iter().enumerate() {
        let u = named(&h, &format!("wo{i}@x.io"), n, false).await;
        h.member(org, u, "member", "active").await;
        h.bind_ws(ws, u, "owner").await;
    }
    let m = named(&h, "mem@x.io", "Mia Member", false).await;
    h.member(org, m, "member", "active").await;
    h.bind_ws(ws, m, "member").await;
    let root = named(&h, "root@x.io", "Root Admin", true).await;
    let (_, mt) = h.good_session(m).await;
    let (_, rt) = h.good_session(root).await;
    W {
        h,
        org,
        ws,
        member: cookie(&mt),
        outsider_admin: cookie(&rt),
    }
}

impl W {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.h.call(who, m, uri, body).await
    }
}

fn names(v: &Value) -> Vec<String> {
    v["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_workspace_refusal_names_the_role_and_up_to_three_owners() {
    let w = world().await;
    let (s, v) = w
        .call(
            &w.member,
            Method::PATCH,
            &format!("/v1/workspaces/{}", w.ws),
            Some(json!({"name": "New"})),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "forbidden");
    let who = &v["error"]["details"]["who_can"];
    assert_eq!(who["role"], "workspace_owner", "{v}");
    assert_eq!(who["you_can"], false);
    assert_eq!(who["action"], "change_this");
    assert!(who.get("scope").is_none(), "the scope is internal: {v}");
    // Owners first (explicit workspace owners and the implicit org owner/admin all hold the
    // effective role), then by name; at most three; no e-mail; never the instance admin.
    let n = names(who);
    assert_eq!(n.len(), 3, "{v}");
    assert!(
        !n.iter().any(|x| x.contains("Root") || x.contains('@')),
        "{n:?}"
    );
    assert!(!v.to_string().contains("@x.io"), "no e-mail anywhere: {v}");
}

#[tokio::test]
async fn an_org_refusal_names_org_owners_first() {
    let w = world().await;
    let (s, v) = w
        .call(
            &w.member,
            Method::PATCH,
            &format!("/v1/orgs/{}", w.org),
            Some(json!({"name": "New"})),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    let who = &v["error"]["details"]["who_can"];
    assert_eq!(who["role"], "org_owner");
    assert_eq!(names(who), ["Zoe Owner"], "{v}");
}

#[tokio::test]
async fn an_instance_admin_only_route_names_no_one() {
    let w = world().await;
    let (s, v) = w
        .call(&w.member, Method::GET, "/v1/admin/users", None)
        .await;
    assert_eq!(s, 403, "{v}");
    let who = &v["error"]["details"]["who_can"];
    assert_eq!(who["role"], "instance_admin", "{v}");
    assert_eq!(who["people"], json!([]));
    // And the instance admin is not named when someone else is refused.
    let (_, v) = w
        .call(
            &w.member,
            Method::PATCH,
            &format!("/v1/orgs/{}", w.org),
            Some(json!({"name": "x"})),
        )
        .await;
    assert!(!v.to_string().contains("Root"), "{v}");
    let _ = &w.outsider_admin;
}

#[tokio::test]
async fn contacts_are_readable_by_a_member_and_hold_at_most_three_names() {
    let w = world().await;
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let owners = v["contacts"]["owners"].as_array().unwrap();
    assert_eq!(owners.len(), 3, "{v}");
    assert!(v["contacts"].get("admins").is_none());
    let (s, v) = w
        .call(&w.member, Method::GET, &format!("/v1/orgs/{}", w.org), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["contacts"]["owners"],
        json!([{"name": "Zoe Owner"}]),
        "{v}"
    );
    let admins: Vec<&str> = v["contacts"]["admins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(admins, ["Zoe Owner", "Adam Admin"], "owners first: {v}");
    // The list and the patch answers carry no contacts.
    let (_, v) = w.call(&w.member, Method::GET, "/v1/orgs", None).await;
    assert!(v.to_string().find("contacts").is_none(), "{v}");
}

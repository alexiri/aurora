#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GRP-1 (D-130, D-131, D-156, SEC-71): directory groups through the real routes.
//! Docs: 04 §Groups, 05 §Groups, 18 §Groups.

use crate::common::*;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct Who {
    id: Uuid,
    cookie: String,
    ws_key: String,
    org_key: String,
}

struct Fx {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    /// Org owner.
    owner: Who,
    /// Org admin.
    admin: Who,
    /// Workspace owner (binding), org member.
    wsowner: Who,
    /// Plain org member bound as `member`.
    member: Who,
    /// Plain org member, no binding: the one groups give access to.
    plain: Who,
    /// Member of another org.
    outsider: Who,
}

async fn who(h: &Harness, org: Uuid, ws: Uuid, email: &str, role: &str, bind: Option<&str>) -> Who {
    let id = h.user(email, false).await;
    h.member(org, id, role, "active").await;
    if let Some(b) = bind {
        h.bind_ws(ws, id, b).await;
    }
    let (_, tok) = h.good_session(id).await;
    let (_, wk) = h.api_key(id, org, Some(ws), false).await;
    let (_, ok) = h.api_key(id, org, None, false).await;
    Who {
        id,
        cookie: cookie(&tok),
        ws_key: bearer(&wk),
        org_key: bearer(&ok),
    }
}

async fn fx() -> Fx {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let other = h.org().await;
    let other_ws = h.workspace(other).await;
    let owner = who(&h, org, ws, "owner@x.io", "owner", None).await;
    let admin = who(&h, org, ws, "admin@x.io", "admin", None).await;
    let wsowner = who(&h, org, ws, "wso@x.io", "member", Some("owner")).await;
    let member = who(&h, org, ws, "m@x.io", "member", Some("member")).await;
    let plain = who(&h, org, ws, "p@x.io", "member", None).await;
    let outsider = who(&h, other, other_ws, "out@x.io", "member", None).await;
    Fx {
        h,
        org,
        ws,
        owner,
        admin,
        wsowner,
        member,
        plain,
        outsider,
    }
}

impl Fx {
    async fn call(
        &self,
        w: &Who,
        m: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(
                m,
                uri,
                &[("cookie", w.cookie.as_str()), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn as_key(&self, key: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("authorization", key), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn create(&self, name: &str, extra: Value) -> Uuid {
        let mut body = json!({ "name": name });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (s, v) = self
            .call(
                &self.owner,
                Method::POST,
                &format!("/v1/orgs/{}/groups", self.org),
                Some(body),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    async fn add(&self, g: Uuid, u: Uuid) -> (u16, Value) {
        self.call(
            &self.owner,
            Method::PUT,
            &format!("/v1/groups/{g}/members/{u}"),
            None,
        )
        .await
    }

    async fn bind(&self, g: Uuid, role: &str) -> (u16, Value) {
        self.call(
            &self.wsowner,
            Method::PUT,
            &format!("/v1/workspaces/{}/access/groups/{g}", self.ws),
            Some(json!({"role": role})),
        )
        .await
    }

    async fn ws_status(&self, w: &Who) -> u16 {
        self.call(w, Method::GET, &format!("/v1/workspaces/{}", self.ws), None)
            .await
            .0
    }

    async fn n(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(self.h.pool()).await.unwrap()
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

// ---------------------------------------------------------------- CRUD, limits, errors

#[tokio::test]
async fn crud_names_errors_and_optimistic_concurrency() {
    let f = fx().await;
    let uri = format!("/v1/orgs/{}/groups", f.org);
    let (s, v) = f
        .call(&f.admin, Method::POST, &uri, Some(json!({"name": "  Platform  ", "description": "d"})))
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["name"], "Platform");
    assert_eq!(v["source"], "manual");
    assert_eq!(v["revision"], 1);
    assert_eq!(v["member_count"], 0);
    let g = v["id"].as_str().unwrap().to_owned();
    // names are unique per org, case-insensitively
    let (s, v) = f
        .call(&f.admin, Method::POST, &uri, Some(json!({"name": "platform"})))
        .await;
    assert_eq!((s, code(&v)), (409, "group_name_taken"));
    for bad in ["", "   ", &"x".repeat(81), "a\u{0}b"] {
        let (s, _) = f.call(&f.admin, Method::POST, &uri, Some(json!({"name": bad}))).await;
        assert_eq!(s, 400, "{bad:?}");
    }
    // `owner` can never be mapped; neither can an unknown role (SEC-71)
    for r in ["owner", "root"] {
        let (s, v) = f
            .call(&f.admin, Method::POST, &uri, Some(json!({"name": "m", "org_role": r})))
            .await;
        assert_eq!((s, code(&v)), (422, "group_org_role_invalid"), "{r}");
    }
    let (s, v) = f
        .call(&f.admin, Method::POST, &uri, Some(json!({"name": "s", "source": "scim"})))
        .await;
    assert_eq!(s, 400, "{v}");

    // PATCH: stale revision, If-Match, mapping change audited with `change`
    let one = format!("/v1/groups/{g}");
    let (s, v) = f
        .call(&f.admin, Method::PATCH, &one, Some(json!({"name": "Platform team", "revision": 1})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["revision"], 2);
    let (s, v) = f
        .call(&f.admin, Method::PATCH, &one, Some(json!({"name": "again", "revision": 1})))
        .await;
    assert_eq!((s, code(&v)), (409, "stale_revision"));
    let (s, _, v) = f
        .h
        .send(
            Method::PATCH,
            &one,
            &[("cookie", f.admin.cookie.as_str()), ("origin", ORIGIN), ("if-match", "\"1\"")],
            Some(json!({"description": null})),
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (409, "stale_revision"));
    let (s, v) = f
        .call(&f.admin, Method::PATCH, &one, Some(json!({"org_role": "owner"})))
        .await;
    assert_eq!((s, code(&v)), (422, "group_org_role_invalid"));
    let (s, v) = f
        .call(&f.admin, Method::PATCH, &one, Some(json!({"org_role": "member"})))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = f
        .call(&f.admin, Method::PATCH, &one, Some(json!({"org_role": "admin"})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["org_role"], "admin");
    let meta: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'group.update' AND metadata->'change' ? 'org_role' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(meta["change"]["org_role"], "member → admin");
    // list filters
    let (s, v) = f.call(&f.admin, Method::GET, &format!("{uri}?q=PLAT"), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (_, v) = f.call(&f.admin, Method::GET, &format!("{uri}?source=idp"), None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 0);
    // delete
    let (s, _) = f.call(&f.admin, Method::DELETE, &one, None).await;
    assert_eq!(s, 204);
    let (s, _) = f.call(&f.admin, Method::GET, &one, None).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn group_and_user_limits() {
    let f = fx().await;
    // GROUPS_PER_ORG
    sqlx::query(
        "INSERT INTO groups (id, org_id, name) SELECT gen_random_uuid(), $1, 'g' || i FROM generate_series(1, 500) i",
    )
    .bind(f.org)
    .execute(f.h.pool())
    .await
    .unwrap();
    let (s, v) = f
        .call(&f.owner, Method::POST, &format!("/v1/orgs/{}/groups", f.org), Some(json!({"name": "one too many"})))
        .await;
    assert_eq!((s, code(&v)), (422, "group_limit_reached"));
    // GROUPS_PER_USER: the plain member is already in 100 groups
    sqlx::query(
        "INSERT INTO group_members (group_id, org_id, user_id) \
         SELECT id, org_id, $2 FROM groups WHERE org_id = $1 ORDER BY name LIMIT 100",
    )
    .bind(f.org)
    .bind(f.plain.id)
    .execute(f.h.pool())
    .await
    .unwrap();
    let extra: Uuid = sqlx::query_scalar(
        "SELECT id FROM groups WHERE org_id = $1 AND id NOT IN (SELECT group_id FROM group_members WHERE user_id = $2) LIMIT 1",
    )
    .bind(f.org)
    .bind(f.plain.id)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    let (s, v) = f.add(extra, f.plain.id).await;
    assert_eq!((s, code(&v)), (422, "group_user_limit_reached"));
}

// ---------------------------------------------------------------- permission matrix per route

#[tokio::test]
async fn permission_matrix_per_route() {
    let f = fx().await;
    let g = f.create("matrix", json!({})).await;
    let gu = format!("/v1/groups/{g}");
    let org_groups = format!("/v1/orgs/{}/groups", f.org);
    let routes: Vec<(Method, String, Option<Value>, &str)> = vec![
        // (method, uri, body, who may: "admin" = org owner/admin, "reader" = + workspace owner)
        (Method::GET, org_groups.clone(), None, "reader"),
        (Method::POST, org_groups.clone(), Some(json!({"name": "x"})), "admin"),
        (Method::GET, gu.clone(), None, "reader"),
        (Method::PATCH, gu.clone(), Some(json!({"description": "d"})), "admin"),
        (Method::GET, format!("{gu}/members"), None, "admin"),
        (Method::PUT, format!("{gu}/members/{}", f.member.id), None, "admin"),
        (Method::DELETE, format!("{gu}/members/{}", f.member.id), None, "admin"),
        (
            Method::POST,
            format!("{gu}/members:batch"),
            Some(json!({"add": [], "remove": []})),
            "admin",
        ),
        (Method::GET, format!("{gu}/workspaces"), None, "admin"),
        (Method::GET, format!("{gu}/impact"), None, "admin"),
    ];
    let people: Vec<(&str, &Who)> = vec![
        ("owner", &f.owner),
        ("admin", &f.admin),
        ("wsowner", &f.wsowner),
        ("member", &f.member),
        ("plain", &f.plain),
        ("outsider", &f.outsider),
    ];
    for (m, uri, body, who_may) in &routes {
        for (name, w) in &people {
            let (s, v) = f.call(w, m.clone(), uri, body.clone()).await;
            let allowed = match *name {
                "owner" | "admin" => true,
                "wsowner" => *who_may == "reader",
                _ => false,
            };
            if allowed {
                assert!(s < 300 || s == 409, "{m} {uri} as {name}: {s} {v}");
            } else if *name == "outsider" {
                assert_eq!(s, 404, "{m} {uri} as {name}");
            } else {
                assert_eq!((s, code(&v)), (403, "forbidden"), "{m} {uri} as {name}");
            }
        }
        // workspace keys cannot use organization routes; an org key answers like its owner
        let (s, _) = f.as_key(&f.owner.ws_key, m.clone(), uri, body.clone()).await;
        assert_eq!(s, 403, "{m} {uri} ws key");
        let (s, v) = f.as_key(&f.owner.org_key, m.clone(), uri, body.clone()).await;
        assert!(s < 300 || s == 409, "{m} {uri} org key of the owner: {s} {v}");
        let (s, _) = f.as_key(&f.member.org_key, m.clone(), uri, body.clone()).await;
        assert_eq!(s, 403, "{m} {uri} org key of a member");
    }
    // anonymous: 401
    let (s, _, _) = f.h.send(Method::GET, &org_groups, &[], None).await;
    assert_eq!(s, 401);
    // binding a group: `can_own` of the workspace, not org membership alone
    let bind_uri = format!("/v1/workspaces/{}/access/groups/{g}", f.ws);
    for w in [&f.member, &f.plain] {
        let (s, v) = f.call(w, Method::PUT, &bind_uri, Some(json!({"role": "member"}))).await;
        assert!(s == 403 || s == 404, "{s} {v}");
    }
    for w in [&f.wsowner, &f.admin] {
        let (s, v) = f.call(w, Method::PUT, &bind_uri, Some(json!({"role": "member"}))).await;
        assert_eq!(s, 200, "{v}");
    }
    // the groups of another org are 404, never 403
    let og: Uuid = f.create("mine", json!({})).await;
    let (s, _) = f
        .call(&f.outsider, Method::GET, &format!("/v1/groups/{og}"), None)
        .await;
    assert_eq!(s, 404);
}

// ---------------------------------------------------------------- membership and access

#[tokio::test]
async fn membership_never_creates_a_membership_and_is_idempotent() {
    let f = fx().await;
    let g = f.create("team", json!({})).await;
    // a user of another org, and a user known to nobody
    let (s, v) = f.add(g, f.outsider.id).await;
    assert_eq!((s, code(&v)), (422, "not_an_org_member"));
    let (s, v) = f.add(g, Uuid::now_v7()).await;
    assert_eq!((s, code(&v)), (422, "not_an_org_member"));
    assert_eq!(
        f.n(&format!("SELECT count(*) FROM org_members WHERE user_id = '{}' AND org_id = '{}'", f.outsider.id, f.org)).await,
        0
    );
    let (s, v) = f.add(g, f.plain.id).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["added"].as_array().unwrap().len(), 1);
    let (s, v) = f.add(g, f.plain.id).await;
    assert_eq!(s, 200);
    assert_eq!(v["added"].as_array().unwrap().len(), 0, "idempotent");
    // one audit row only for the change that happened
    assert_eq!(
        f.n("SELECT count(*) FROM audit_log WHERE action = 'group_member.add'").await,
        1
    );
    // batch: one transaction, one audit row per change, atomic on error
    let (s, v) = f
        .call(
            &f.owner,
            Method::POST,
            &format!("/v1/groups/{g}/members:batch"),
            Some(json!({"add": [f.member.id, f.outsider.id], "remove": [f.plain.id]})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "not_an_org_member"));
    assert_eq!(
        f.n(&format!("SELECT count(*) FROM group_members WHERE group_id = '{g}'")).await,
        1,
        "nothing applied"
    );
    let (s, v) = f
        .call(
            &f.owner,
            Method::POST,
            &format!("/v1/groups/{g}/members:batch"),
            Some(json!({"add": [f.member.id, f.wsowner.id], "remove": [f.plain.id]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["added"].as_array().unwrap().len(), 2);
    assert_eq!(v["removed"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.n("SELECT count(*) FROM audit_log WHERE action IN ('group_member.add','group_member.remove')").await,
        4
    );
    let too_many: Vec<Uuid> = (0..101).map(|_| Uuid::now_v7()).collect();
    let (s, _) = f
        .call(&f.owner, Method::POST, &format!("/v1/groups/{g}/members:batch"), Some(json!({"add": too_many})))
        .await;
    assert_eq!(s, 400);
    // delete is idempotent
    let (s, _) = f
        .call(&f.owner, Method::DELETE, &format!("/v1/groups/{g}/members/{}", f.plain.id), None)
        .await;
    assert_eq!(s, 204);
    // members listing
    let (s, v) = f
        .call(&f.owner, Method::GET, &format!("/v1/groups/{g}/members"), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn group_access_is_effective_on_the_next_request_and_for_keys() {
    let f = fx().await;
    let g = f.create("infra", json!({})).await;
    assert_eq!(f.ws_status(&f.plain).await, 404, "no access yet");
    let (s, v) = f.bind(g, "member").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["role"], "member");
    // binding alone grants nothing: nobody is in the group
    assert_eq!(f.ws_status(&f.plain).await, 404);
    let (_, key) = f.h.api_key(f.plain.id, f.org, Some(f.ws), false).await;
    let key = bearer(&key);
    let (s, _) = f.as_key(&key, Method::GET, &format!("/v1/workspaces/{}", f.ws), None).await;
    assert_eq!(s, 404);
    assert_eq!(f.add(g, f.plain.id).await.0, 200);
    assert_eq!(f.ws_status(&f.plain).await, 200, "added: next request");
    let (s, _) = f.as_key(&key, Method::GET, &format!("/v1/workspaces/{}", f.ws), None).await;
    assert_eq!(s, 200, "the key owner's group change reaches the key's next request");
    // a member (not an owner) of the workspace through the group cannot manage it
    let (s, v) = f
        .call(&f.plain, Method::GET, &format!("/v1/workspaces/{}/access", f.ws), None)
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"));
    // raise the group to owner: can_own through the group
    assert_eq!(f.bind(g, "owner").await.0, 200);
    let (s, _) = f
        .call(&f.plain, Method::GET, &format!("/v1/workspaces/{}/access", f.ws), None)
        .await;
    assert_eq!(s, 200);
    // /v1/me shows the workspace and the group
    let (_, me) = f.h.get("/v1/me", &[("cookie", f.plain.cookie.as_str())]).await;
    assert_eq!(me["groups"][0]["name"], "infra");
    assert_eq!(me["memberships"][0]["workspaces"][0]["my_role"], "owner");
    assert_eq!(me["memberships"][0]["role"], "member");
    // removed: the next request is 404 again
    let (s, _) = f
        .call(&f.owner, Method::DELETE, &format!("/v1/groups/{g}/members/{}", f.plain.id), None)
        .await;
    assert_eq!(s, 204);
    assert_eq!(f.ws_status(&f.plain).await, 404);
    let (s, _) = f.as_key(&key, Method::GET, &format!("/v1/workspaces/{}", f.ws), None).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn org_role_mapping_and_effective_role_in_me() {
    let f = fx().await;
    let g = f.create("admins", json!({"org_role": "admin"})).await;
    assert_eq!(f.add(g, f.plain.id).await.0, 200);
    // an effective admin may manage groups and owns every workspace
    assert_eq!(f.ws_status(&f.plain).await, 200);
    let (s, _) = f
        .call(&f.plain, Method::GET, &format!("/v1/orgs/{}/groups", f.org), None)
        .await;
    assert_eq!(s, 200);
    let (_, me) = f.h.get("/v1/me", &[("cookie", f.plain.cookie.as_str())]).await;
    let m = &me["memberships"][0];
    assert_eq!(m["role"], "member", "`role` stays the direct role");
    assert_eq!(m["effective_role"], "admin");
    assert_eq!(m["sources"][1]["kind"], "group");
    // clearing the mapping takes it away on the next request
    let (s, v) = f
        .call(&f.owner, Method::PATCH, &format!("/v1/groups/{g}"), Some(json!({"org_role": null})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(f.ws_status(&f.plain).await, 404);
    // a suspended member derives nothing, whatever the groups say
    sqlx::query("UPDATE groups SET org_role = 'admin' WHERE id = $1")
        .bind(g)
        .execute(f.h.pool())
        .await
        .unwrap();
    f.h.set_member_status(f.org, f.plain.id, "suspended").await;
    assert_eq!(f.ws_status(&f.plain).await, 404);
}

#[tokio::test]
async fn workspace_access_expand_groups_and_derived_members() {
    let f = fx().await;
    let g = f.create("infra", json!({})).await;
    f.bind(g, "approver").await;
    f.add(g, f.plain.id).await;
    f.add(g, f.member.id).await;
    let base = format!("/v1/workspaces/{}/access", f.ws);
    // unchanged without `expand`
    let (s, v) = f.call(&f.wsowner, Method::GET, &base, None).await;
    assert_eq!(s, 200);
    assert!(v.get("group_bindings").is_none() && v.get("derived_members").is_none());
    let (s, v) = f
        .call(&f.wsowner, Method::GET, &format!("{base}?expand=groups"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["group_bindings"][0]["group"]["name"], "infra");
    assert_eq!(v["group_bindings"][0]["group"]["member_count"], 2);
    assert_eq!(v["group_bindings"][0]["role"], "approver");
    let derived = v["derived_members"]["items"].as_array().unwrap();
    // plain (group only), member (direct member + group approver → approver), owner/admin (org role)
    let by = |id: Uuid| derived.iter().find(|d| d["user"]["id"] == id.to_string()).cloned();
    let p = by(f.plain.id).expect("plain");
    assert_eq!(p["role"], "approver");
    assert_eq!(p["sources"][0]["kind"], "group");
    let m = by(f.member.id).expect("member");
    assert_eq!(m["role"], "approver");
    let kinds: Vec<_> = m["sources"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap().to_owned()).collect();
    assert!(kinds.contains(&"direct".to_owned()) && kinds.contains(&"group".to_owned()), "{kinds:?}");
    assert!(by(f.owner.id).is_some_and(|d| d["sources"][0]["kind"] == "org_role"));
    assert!(by(f.wsowner.id).is_none(), "direct-only people are not derived members");
    // a plain member of the workspace cannot read it
    let (s, _) = f
        .call(&f.member, Method::GET, &format!("{base}?expand=groups"), None)
        .await;
    assert_eq!(s, 403);
    // unbind is idempotent
    let (s, _) = f
        .call(&f.wsowner, Method::DELETE, &format!("{base}/groups/{g}"), None)
        .await;
    assert_eq!(s, 204);
    let (s, _) = f
        .call(&f.wsowner, Method::DELETE, &format!("{base}/groups/{g}"), None)
        .await;
    assert_eq!(s, 204);
    assert_eq!(f.ws_status(&f.plain).await, 404);
    assert_eq!(
        f.n("SELECT count(*) FROM audit_log WHERE action = 'group_access.remove'").await,
        1
    );
}

#[tokio::test]
async fn a_group_binds_only_workspaces_of_its_own_org() {
    let f = fx().await;
    let g = f.create("mine", json!({})).await;
    let other_ws: Uuid = sqlx::query_scalar(
        "SELECT id FROM workspaces WHERE org_id <> $1 LIMIT 1",
    )
    .bind(f.org)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    // the outsider owns nothing there either; give them an owner binding and try the cross-org bind
    f.h.bind_ws(other_ws, f.outsider.id, "owner").await;
    let (s, _) = f
        .call(
            &f.outsider,
            Method::PUT,
            &format!("/v1/workspaces/{other_ws}/access/groups/{g}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 404, "a group of another org is 404");
    assert_eq!(f.n("SELECT count(*) FROM group_workspace_access").await, 0);
}

#[tokio::test]
async fn impact_view_counts_users_that_would_lose_access() {
    let f = fx().await;
    let g = f.create("infra", json!({})).await;
    f.bind(g, "member").await;
    f.add(g, f.plain.id).await;
    f.add(g, f.member.id).await; // keeps a direct binding: loses nothing
    let (s, v) = f
        .call(&f.owner, Method::GET, &format!("/v1/groups/{g}/impact"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    let w = &v["workspaces"][0];
    assert_eq!(w["total"], 1);
    assert_eq!(w["users_losing_access"][0]["id"], f.plain.id.to_string());
    // a creator-anchored schedule of the group-only user would be disabled
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(f.org).bind(f.ws).execute(f.h.pool()).await.unwrap();
    let (s, v) = f
        .call(&f.owner, Method::GET, &format!("/v1/groups/{g}/impact?remove_member={}", f.member.id), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["workspaces"].as_array().unwrap().len(), 0, "nothing lost for the direct member");
    let (_, v) = f
        .call(&f.owner, Method::GET, &format!("/v1/groups/{g}/impact?remove_member={}", f.plain.id), None)
        .await;
    assert_eq!(v["workspaces"][0]["total"], 1);
    let _ = agent;
    // another group still gives access: the user loses nothing
    let g2 = f.create("also", json!({})).await;
    f.bind(g2, "member").await;
    f.add(g2, f.plain.id).await;
    let (_, v) = f
        .call(&f.owner, Method::GET, &format!("/v1/groups/{g}/impact"), None)
        .await;
    assert_eq!(v["workspaces"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn offboarding_views_carry_via_groups() {
    let f = fx().await;
    let g = f.create("infra", json!({})).await;
    f.bind(g, "member").await;
    f.add(g, f.plain.id).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(f.org).bind(f.ws).execute(f.h.pool()).await.unwrap();
    sqlx::query("INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, enabled, anchor, creator_user_id, next_fire_at) VALUES (gen_random_uuid(), $1, 's', 'p', '0 * * * *', 'UTC', true, 'creator', $2, now())")
        .bind(agent).bind(f.plain.id).execute(f.h.pool()).await.unwrap();
    let (s, v) = f
        .call(
            &f.wsowner,
            Method::GET,
            &format!("/v1/workspaces/{}/members/{}/offboarding", f.ws, f.plain.id),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["schedules"][0]["via_groups"][0]["name"], "infra");
    let (s, v) = f
        .call(
            &f.owner,
            Method::GET,
            &format!("/v1/orgs/{}/members/{}/offboarding", f.org, f.plain.id),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["schedules"][0]["via_groups"][0]["name"], "infra");
}

// ---------------------------------------------------------------- policy

#[tokio::test]
async fn policy_group_identity_authoring_and_warnings() {
    let f = fx().await;
    let g = f.create("infra", json!({})).await;
    let draft = format!("/v1/workspaces/{}/policy/draft", f.ws);
    let rule = |id: Uuid| {
        json!({"revision": 0, "default_action": "none", "rules": [{
            "name": "r", "action": "block",
            "identities": [{"kind": "group", "id": id}],
            "targets": [{"type": "network", "host": "a.example.com"}]}]})
    };
    // a group of another org: 422 unknown_identity
    let other_group = Uuid::now_v7();
    let other_org: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE id <> $1 LIMIT 1")
        .bind(f.org)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'theirs')")
        .bind(other_group)
        .bind(other_org)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (s, v) = f.call(&f.wsowner, Method::PUT, &draft, Some(rule(other_group))).await;
    assert_eq!((s, code(&v)), (422, "unknown_identity"), "{v}");
    let (s, v) = f.call(&f.wsowner, Method::PUT, &draft, Some(rule(Uuid::now_v7()))).await;
    assert_eq!((s, code(&v)), (422, "unknown_identity"));
    let (s, v) = f.call(&f.wsowner, Method::PUT, &draft, Some(rule(g))).await;
    assert_eq!(s, 200, "{v}");
    // RULE_GROUP_IDENTITIES_MAX
    let mut many = rule(g);
    many["revision"] = json!(1);
    many["rules"][0]["identities"] = json!((0..21).map(|_| json!({"kind":"group","id":g})).collect::<Vec<_>>());
    let (s, _) = f.call(&f.wsowner, Method::PUT, &draft, Some(many)).await;
    assert_eq!(s, 422);
    // warnings: needs speaker + empty group, then a removed group
    let w = format!("/v1/workspaces/{}/policy/warnings", f.ws);
    let (_, v) = f.call(&f.wsowner, Method::GET, &w, None).await;
    let codes: Vec<&str> = v["items"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"group_rule_needs_speaker"), "{codes:?}");
    assert!(codes.contains(&"group_rule_empty"), "{codes:?}");
    // deleting the group does NOT rewrite the rule; the author is told
    let (s, _) = f.call(&f.owner, Method::DELETE, &format!("/v1/groups/{g}"), None).await;
    assert_eq!(s, 204);
    let stored: Value = sqlx::query_scalar(
        "SELECT r.identities FROM policy_rules r JOIN policy_generations g ON g.id = r.generation_id \
         WHERE g.workspace_id = $1 AND g.status = 'draft'",
    )
    .bind(f.ws)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(stored[0]["id"], g.to_string(), "rules are not rewritten");
    let (_, v) = f.call(&f.wsowner, Method::GET, &w, None).await;
    let removed: Vec<&Value> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "rule_names_removed_group")
        .collect();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["details"]["group_id"], g.to_string());
}

// ---------------------------------------------------------------- audit

#[tokio::test]
async fn every_group_route_writes_its_critical_row_and_fails_closed() {
    let f = fx().await;
    let g = f.create("a", json!({})).await;
    f.add(g, f.plain.id).await;
    f.bind(g, "member").await;
    let (s, _) = f
        .call(&f.owner, Method::PATCH, &format!("/v1/groups/{g}"), Some(json!({"name": "b"})))
        .await;
    assert_eq!(s, 200);
    f.call(&f.wsowner, Method::DELETE, &format!("/v1/workspaces/{}/access/groups/{g}", f.ws), None)
        .await;
    f.call(&f.owner, Method::DELETE, &format!("/v1/groups/{g}/members/{}", f.plain.id), None)
        .await;
    f.call(&f.owner, Method::DELETE, &format!("/v1/groups/{g}"), None).await;
    for a in [
        "group.create",
        "group.update",
        "group.delete",
        "group_member.add",
        "group_member.remove",
        "group_access.set",
        "group_access.remove",
    ] {
        assert_eq!(
            f.n(&format!(
                "SELECT count(*) FROM audit_log WHERE action = '{a}' AND critical"
            ))
            .await,
            1,
            "{a}"
        );
    }
    // forced audit failure: 503 audit_unavailable and nothing changed
    let g2 = f.create("c", json!({})).await;
    sqlx::query("CREATE FUNCTION audit_critical_fail() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'audit down'; END $$ LANGUAGE plpgsql")
        .execute(f.h.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER audit_critical_fail BEFORE INSERT ON audit_log FOR EACH ROW WHEN (NEW.critical) EXECUTE FUNCTION audit_critical_fail()")
        .execute(f.h.pool()).await.unwrap();
    let (s, v) = f.add(g2, f.plain.id).await;
    assert_eq!((s, code(&v)), (503, "audit_unavailable"));
    assert_eq!(f.n("SELECT count(*) FROM group_members").await, 0);
    let (s, v) = f.bind(g2, "member").await;
    assert_eq!((s, code(&v)), (503, "audit_unavailable"));
    assert_eq!(f.n("SELECT count(*) FROM group_workspace_access").await, 0);
    let (s, v) = f
        .call(&f.owner, Method::POST, &format!("/v1/orgs/{}/groups", f.org), Some(json!({"name": "d"})))
        .await;
    assert_eq!((s, code(&v)), (503, "audit_unavailable"));
    let (s, v) = f.call(&f.owner, Method::DELETE, &format!("/v1/groups/{g2}"), None).await;
    assert_eq!((s, code(&v)), (503, "audit_unavailable"));
    assert_eq!(f.n("SELECT count(*) FROM groups").await, 1, "the delete did not happen");
}

// ---------------------------------------------------------------- streams

async fn open(f: &Fx, app: &axum::Router, uri: &str, w: &Who) -> Body {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header("cookie", w.cookie.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{uri}");
    let _ = f;
    Body::new(res.into_body())
}

/// All text until the body ends; `None` when it is still open after `wait`.
async fn drain(body: &mut Body, wait: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut out = String::new();
    loop {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Err(_) => return None,
            Ok(None) | Ok(Some(Err(_))) => return Some(out),
            Ok(Some(Ok(fr))) => {
                if let Ok(d) = fr.into_data() {
                    out.push_str(&String::from_utf8_lossy(&d));
                }
            }
        }
    }
}

#[tokio::test]
async fn access_notification_closes_the_removed_users_stream_only() {
    let f = fx().await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(f.org).bind(f.ws).execute(f.h.pool()).await.unwrap();
    // a group conversation of the workspace (visible to every user)
    let conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, workspace_id, agent_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'group','x', now())")
        .bind(conv).bind(f.ws).bind(agent).execute(f.h.pool()).await.unwrap();
    let g = f.create("infra", json!({})).await;
    f.bind(g, "member").await;
    f.add(g, f.plain.id).await;
    let app = test_router(f.h.state.clone().with_stream_recheck(Duration::from_secs(3600)));
    let uri = format!("/v1/conversations/{conv}/stream");
    let mut removed = open(&f, &app, &uri, &f.plain).await;
    let mut direct = open(&f, &app, &uri, &f.member).await;
    assert!(drain(&mut removed, Duration::from_millis(300)).await.is_none());
    f.call(&f.owner, Method::DELETE, &format!("/v1/groups/{g}/members/{}", f.plain.id), None)
        .await;
    // what the aurora_auth_revoked listener does on NOTIFY
    f.h.state.session_cache.invalidate(&json!({"kind": "access", "id": f.plain.id, "org_id": f.org}));
    let text = drain(&mut removed, Duration::from_secs(5)).await.expect("closed");
    assert!(text.contains("event: access.changed"), "{text}");
    // an org-wide ping leaves the still-authorized stream untouched
    f.h.state.session_cache.invalidate(&json!({"kind": "access", "id": null, "org_id": f.org}));
    assert!(drain(&mut direct, Duration::from_millis(500)).await.is_none());
}

#[tokio::test]
async fn access_notification_ends_the_removed_users_long_poll() {
    let f = fx().await;
    let g = f.create("approvers", json!({})).await;
    f.bind(g, "approver").await;
    f.add(g, f.plain.id).await;
    let (s, v) = f
        .call(&f.plain, Method::GET, "/v1/approvals/pending", None)
        .await;
    assert_eq!((s, v["items"].as_array().map(Vec::len)), (200, Some(0)));
    let h = &f.h;
    let uri = "/v1/approvals/pending?wait=20";
    let cookie = f.plain.cookie.clone();
    let poll = async {
        let t = tokio::time::Instant::now();
        let (s, _, v) = h
            .send(Method::GET, uri, &[("cookie", cookie.as_str())], None)
            .await;
        (s.as_u16(), v, t.elapsed())
    };
    let kick = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        sqlx::query("DELETE FROM group_members WHERE user_id = $1")
            .bind(f.plain.id)
            .execute(h.pool())
            .await
            .unwrap();
        h.state
            .session_cache
            .invalidate(&json!({"kind": "access", "id": f.plain.id, "org_id": f.org}));
    };
    let ((s, v, took), ()) = tokio::join!(poll, kick);
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 0);
    assert!(took < Duration::from_secs(10), "returned at once, not after the wait: {took:?}");
}

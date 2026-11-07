#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

struct Who(String);

impl Who {
    async fn new(h: &Harness, email: &str, admin: bool) -> (Uuid, Who) {
        let u = h.user(email, admin).await;
        let (_, tok) = h.good_session(u).await;
        (u, Who(tok))
    }
}

async fn call(h: &Harness, who: &Who, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let c = cookie(&who.0);
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

async fn count(h: &Harness, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn setup_org(h: &Harness, who: &Who, name: &str) -> Value {
    let (s, v) = call(
        h,
        who,
        Method::POST,
        "/v1/orgs",
        Some(json!({"name": name})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v
}

fn id(v: &Value) -> Uuid {
    v.as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn create_org_transaction_and_validation() {
    let h = Harness::new().await;
    let (_, plain) = Who::new(&h, "p@x.io", false).await;
    let (_, admin) = Who::new(&h, "a@x.io", true).await;
    let (s, v) = call(
        &h,
        &plain,
        Method::POST,
        "/v1/orgs",
        Some(json!({"name": "Acme"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "org_creation_forbidden"));

    let v = setup_org(&h, &admin, "Acme Corp").await;
    assert_eq!(v["org"]["slug"], "acme-corp");
    assert_eq!(v["org"]["my_role"], "owner");
    assert_eq!(v["org"]["external_id"].as_str().unwrap().len(), 32);
    assert_eq!(v["workspace"]["slug"], "default");
    assert_eq!(v["workspace"]["name"], "Default");
    assert_eq!(v["workspace"]["my_role"], "owner");
    assert_eq!(v["default_agent"]["handle"], "daneel");
    assert_eq!(v["default_agent"]["kind"], "hosted");
    assert_eq!(v["default_agent"]["is_default"], true);
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);

    let gens: Vec<(i32, String)> = sqlx::query_as(
        "SELECT number, status FROM policy_generations WHERE org_id=$1 AND workspace_id IS NULL ORDER BY number",
    )
    .bind(org)
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(gens, vec![(1, "published".into()), (2, "draft".into())]);
    let n = count(&h, "SELECT count(*) FROM policy_generations WHERE workspace_id=$1 AND ((number=1 AND status='published' AND published_at IS NOT NULL) OR (number=2 AND status='draft'))", ws).await;
    assert_eq!(n, 2);

    // derived slug collision gets a suffix; explicit collision is 409
    let v2 = setup_org(&h, &admin, "Acme Corp").await;
    assert_eq!(v2["org"]["slug"], "acme-corp-2");
    let (s, v) = call(
        &h,
        &admin,
        Method::POST,
        "/v1/orgs",
        Some(json!({"name": "X", "slug": "acme-corp"})),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "slug_taken"));
    let (s, _) = call(
        &h,
        &admin,
        Method::POST,
        "/v1/orgs",
        Some(json!({"name": "  "})),
    )
    .await;
    assert_eq!(s, 400);
    let (s, _) = call(
        &h,
        &admin,
        Method::POST,
        "/v1/orgs",
        Some(json!({"name": "ok", "slug": "Bad Slug"})),
    )
    .await;
    assert_eq!(s, 400);

    // second workspace gets its own Daneel; validation
    let uri = format!("/v1/orgs/{org}/workspaces");
    let (s, v) = call(
        &h,
        &admin,
        Method::POST,
        &uri,
        Some(json!({"name": "Second"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["workspace"]["slug"], "second");
    assert_eq!(v["default_agent"]["handle"], "daneel");
    assert_ne!(v["default_agent"]["workspace_id"], json!(ws));
    let (s, v) = call(
        &h,
        &admin,
        Method::POST,
        &uri,
        Some(json!({"name": "T", "timezone": "Mars/Base"})),
    )
    .await;
    assert_eq!(s, 400);
    assert_eq!(v["error"]["details"]["fields"][0]["path"], "/timezone");
    let (s, v) = call(
        &h,
        &admin,
        Method::POST,
        &uri,
        Some(json!({"name": "T", "memory_language": "klingon"})),
    )
    .await;
    assert_eq!(s, 400);
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"],
        "/memory_language"
    );
    let (s, _) = call(
        &h,
        &admin,
        Method::POST,
        &uri,
        Some(json!({"name": "T", "memory_language": "simple", "timezone": "Europe/Zurich"})),
    )
    .await;
    assert_eq!(s, 201);
    let (s, v) = call(
        &h,
        &admin,
        Method::POST,
        &uri,
        Some(json!({"name": "T", "slug": "second"})),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "slug_taken"));

    // list / get / patch org
    let (s, v) = call(&h, &admin, Method::GET, "/v1/orgs", None).await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 2));
    let (s, v) = call(&h, &admin, Method::PATCH, &format!("/v1/orgs/{org}"),
        Some(json!({"name": "Renamed", "settings": {"max_members": 5, "members_can_create_workspaces": false}}))).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["settings"]["max_members"], 5);
    assert_eq!(v["settings"]["members_can_create_workspaces"], false);
    let (_, v) = call(
        &h,
        &admin,
        Method::PATCH,
        &format!("/v1/orgs/{org}"),
        Some(json!({"settings": {"max_members": null}})),
    )
    .await;
    assert_eq!(v["settings"]["max_members"], Value::Null);
    assert_eq!(v["name"], "Renamed");
}

#[tokio::test]
async fn anyone_may_create_when_configured() {
    let h = Harness::with_config(|c| c.org_creation = aurora_config::OrgCreation::Anyone).await;
    let (_, plain) = Who::new(&h, "p@x.io", false).await;
    setup_org(&h, &plain, "Mine").await;
}

#[tokio::test]
async fn members_and_last_owner() {
    let h = Harness::new().await;
    let (owner, ot) = Who::new(&h, "o@x.io", true).await;
    let org = id(&setup_org(&h, &ot, "Org").await["org"]["id"]);
    let (admin, at) = Who::new(&h, "ad@x.io", false).await;
    let (member, mt) = Who::new(&h, "m@x.io", false).await;
    h.member(org, admin, "admin", "active").await;
    h.member(org, member, "member", "active").await;
    let mem = |u: Uuid| format!("/v1/orgs/{org}/members/{u}");

    // admin cannot touch owners nor grant owner
    let (s, _) = call(
        &h,
        &at,
        Method::PATCH,
        &mem(owner),
        Some(json!({"role": "member"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &h,
        &at,
        Method::PATCH,
        &mem(member),
        Some(json!({"role": "owner"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(&h, &at, Method::DELETE, &mem(owner), None).await;
    assert_eq!(s, 403);
    // plain members cannot list
    let (s, _) = call(
        &h,
        &mt,
        Method::GET,
        &format!("/v1/orgs/{org}/members"),
        None,
    )
    .await;
    assert_eq!(s, 403);
    let (s, v) = call(
        &h,
        &at,
        Method::GET,
        &format!("/v1/orgs/{org}/members?status=active"),
        None,
    )
    .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 3));

    // last owner protection
    for body in [json!({"role": "admin"}), json!({"status": "suspended"})] {
        let (s, v) = call(&h, &ot, Method::PATCH, &mem(owner), Some(body)).await;
        assert_eq!((s, code(&v)), (409, "last_owner"));
    }
    let (s, v) = call(&h, &ot, Method::DELETE, &mem(owner), None).await;
    assert_eq!((s, code(&v)), (409, "last_owner"));
    // with a second owner it works
    let (s, _) = call(
        &h,
        &ot,
        Method::PATCH,
        &mem(admin),
        Some(json!({"role": "owner"})),
    )
    .await;
    assert_eq!(s, 200);
    let (s, v) = call(
        &h,
        &at,
        Method::PATCH,
        &mem(owner),
        Some(json!({"role": "admin"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");

    // removal drops bindings
    let (s, _) = call(&h, &at, Method::DELETE, &mem(member), None).await;
    assert_eq!(s, 204);
    let (s, _) = call(&h, &at, Method::DELETE, &mem(member), None).await;
    assert_eq!(s, 404);
    let (s, _) = call(&h, &mt, Method::GET, &format!("/v1/orgs/{org}"), None).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn workspace_visibility_and_ownership() {
    let h = Harness::new().await;
    let (_, ot) = Who::new(&h, "o@x.io", true).await;
    let v = setup_org(&h, &ot, "Org").await;
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);
    let (m, mt) = Who::new(&h, "m@x.io", false).await;
    h.member(org, m, "member", "active").await;
    let wsu = format!("/v1/workspaces/{ws}");

    // unbound member: 404 on workspace, org admin ok, not in list
    assert_eq!(call(&h, &mt, Method::GET, &wsu, None).await.0, 404);
    assert_eq!(
        call(
            &h,
            &mt,
            Method::GET,
            &format!("/v1/workspaces/{ws}/agents"),
            None
        )
        .await
        .0,
        404
    );
    let (_, l) = call(
        &h,
        &mt,
        Method::GET,
        &format!("/v1/orgs/{org}/workspaces"),
        None,
    )
    .await;
    assert_eq!(l["items"].as_array().unwrap().len(), 0);
    let (s, got) = call(&h, &ot, Method::GET, &wsu, None).await;
    assert_eq!((s, got["my_role"].as_str().unwrap()), (200, "owner"));

    // bind as member: visible, agents readable, but patch is 403
    let (s, b) = call(
        &h,
        &ot,
        Method::PUT,
        &format!("{wsu}/access/{m}"),
        Some(json!({"role": "member"})),
    )
    .await;
    assert_eq!(
        (s, b["role"].as_str().unwrap(), b["implicit"].clone()),
        (200, "member", json!(false))
    );
    let (s, got) = call(&h, &mt, Method::GET, &wsu, None).await;
    assert_eq!((s, got["my_role"].as_str().unwrap()), (200, "member"));
    let (s, ag) = call(&h, &mt, Method::GET, &format!("{wsu}/agents"), None).await;
    assert_eq!((s, ag["items"].as_array().unwrap().len()), (200, 1));
    let aid = ag["items"][0]["id"].as_str().unwrap();
    assert_eq!(
        call(&h, &mt, Method::GET, &format!("/v1/agents/{aid}"), None)
            .await
            .0,
        200
    );
    let (s, _) = call(&h, &mt, Method::PATCH, &wsu, Some(json!({"name": "Nope"}))).await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &h,
        &mt,
        Method::PUT,
        &format!("{wsu}/access/{m}"),
        Some(json!({"role": "owner"})),
    )
    .await;
    assert_eq!(s, 403);
    // a member may create workspaces by default; once disabled, not
    let (s, _) = call(
        &h,
        &mt,
        Method::POST,
        &format!("/v1/orgs/{org}/workspaces"),
        Some(json!({"name": "Mine"})),
    )
    .await;
    assert_eq!(s, 201);
    call(
        &h,
        &ot,
        Method::PATCH,
        &format!("/v1/orgs/{org}"),
        Some(json!({"settings": {"members_can_create_workspaces": false}})),
    )
    .await;
    let (s, _) = call(
        &h,
        &mt,
        Method::POST,
        &format!("/v1/orgs/{org}/workspaces"),
        Some(json!({"name": "Again"})),
    )
    .await;
    assert_eq!(s, 403);

    // access list includes implicit owner + explicit member
    let (s, acc) = call(&h, &ot, Method::GET, &format!("{wsu}/access"), None).await;
    assert_eq!(s, 200);
    let items = acc["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .any(|i| i["implicit"] == true && i["role"] == "owner")
    );

    // promote to owner, patch works
    call(
        &h,
        &ot,
        Method::PUT,
        &format!("{wsu}/access/{m}"),
        Some(json!({"role": "owner"})),
    )
    .await;
    let (s, got) = call(
        &h,
        &mt,
        Method::PATCH,
        &wsu,
        Some(json!({"name": "Renamed", "timezone": "Europe/Zurich"})),
    )
    .await;
    assert_eq!((s, got["name"].as_str().unwrap()), (200, "Renamed"));

    // suspended member loses visibility immediately
    let (s, _) = call(
        &h,
        &ot,
        Method::PATCH,
        &format!("/v1/orgs/{org}/members/{m}"),
        Some(json!({"status": "suspended"})),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(call(&h, &mt, Method::GET, &wsu, None).await.0, 404);

    // binding rules
    let (o2, _) = Who::new(&h, "x@x.io", false).await;
    let (s, v) = call(
        &h,
        &ot,
        Method::PUT,
        &format!("{wsu}/access/{o2}"),
        Some(json!({"role": "member"})),
    )
    .await;
    assert_eq!((s, code(&v)), (422, "not_an_org_member"));
    let (_, other) = Who::new(&h, "o3@x.io", false).await;
    let _ = other;
    let owner_id: Uuid =
        sqlx::query_scalar("SELECT user_id FROM org_members WHERE org_id=$1 AND role='owner'")
            .bind(org)
            .fetch_one(h.pool())
            .await
            .unwrap();
    let (s, v) = call(
        &h,
        &ot,
        Method::DELETE,
        &format!("{wsu}/access/{owner_id}"),
        None,
    )
    .await;
    assert_eq!((s, code(&v)), (422, "implicit_access"));
    let (s, _) = call(&h, &ot, Method::DELETE, &format!("{wsu}/access/{m}"), None).await;
    assert_eq!(s, 204);
    let (s, _) = call(&h, &ot, Method::DELETE, &format!("{wsu}/access/{m}"), None).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn resolve_and_deletes() {
    let h = Harness::new().await;
    let (_, ot) = Who::new(&h, "o@x.io", true).await;
    let v = setup_org(&h, &ot, "Org").await;
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);
    let (_, st) = Who::new(&h, "s@x.io", false).await;

    let (s, r) = call(
        &h,
        &ot,
        Method::GET,
        "/v1/resolve?org=org&workspace=default",
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(r["org"]["id"], json!(org));
    assert_eq!(r["workspace"]["id"], json!(ws));
    let (_, r) = call(&h, &ot, Method::GET, "/v1/resolve?org=org", None).await;
    assert_eq!(r["workspace"], Value::Null);
    assert_eq!(
        call(&h, &st, Method::GET, "/v1/resolve?org=org", None)
            .await
            .0,
        404
    );
    assert_eq!(
        call(&h, &ot, Method::GET, "/v1/resolve?org=nope", None)
            .await
            .0,
        404
    );
    assert_eq!(
        call(
            &h,
            &ot,
            Method::GET,
            "/v1/resolve?org=org&workspace=nope",
            None
        )
        .await
        .0,
        404
    );

    // workspace delete: confirmation, cascade
    let wsu = format!("/v1/workspaces/{ws}");
    let (s, e) = call(
        &h,
        &ot,
        Method::DELETE,
        &wsu,
        Some(json!({"confirm": "wrong"})),
    )
    .await;
    assert_eq!((s, code(&e)), (400, "confirmation_mismatch"));
    let (s, _) = call(
        &h,
        &ot,
        Method::DELETE,
        &wsu,
        Some(json!({"confirm": "default"})),
    )
    .await;
    assert_eq!(s, 204);
    assert_eq!(
        count(&h, "SELECT count(*) FROM agents WHERE workspace_id=$1", ws).await,
        0
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM policy_generations WHERE workspace_id=$1",
            ws
        )
        .await,
        0
    );

    // org delete
    let (_, v2) = call(
        &h,
        &ot,
        Method::POST,
        &format!("/v1/orgs/{org}/workspaces"),
        Some(json!({"name": "Two"})),
    )
    .await;
    assert_eq!(v2["workspace"]["slug"], "two");
    let ou = format!("/v1/orgs/{org}");
    let (s, e) = call(&h, &ot, Method::DELETE, &ou, Some(json!({"confirm": "x"}))).await;
    assert_eq!((s, code(&e)), (400, "confirmation_mismatch"));
    let (s, e) = call(
        &h,
        &ot,
        Method::DELETE,
        &ou,
        Some(json!({"confirm": "org"})),
    )
    .await;
    assert_eq!(s, 204, "{e}");
    for t in [
        "organizations WHERE id",
        "workspaces WHERE org_id",
        "agents WHERE org_id",
        "policy_generations WHERE org_id",
        "org_members WHERE org_id",
    ] {
        assert_eq!(
            count(&h, &format!("SELECT count(*) FROM {t}=$1"), org).await,
            0,
            "{t}"
        );
    }
}

async fn audit_n(h: &Harness, action: &str) -> i64 {
    for _ in 0..100 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(h.pool())
            .await
            .unwrap();
        if n > 0 {
            return n;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    0
}

/// SEC-32: role changes and member removal are audited.
#[tokio::test]
async fn sec32_role_changes_and_removal_are_audited() {
    let h = Harness::new().await;
    let (_, ot) = Who::new(&h, "o@x.io", true).await;
    let org = id(&setup_org(&h, &ot, "Org").await["org"]["id"]);
    let (member, _) = Who::new(&h, "m@x.io", false).await;
    h.member(org, member, "member", "active").await;
    let mem = format!("/v1/orgs/{org}/members/{member}");
    let (s, v) = call(&h, &ot, Method::PATCH, &mem, Some(json!({"role": "admin"}))).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(audit_n(&h, "member.update").await, 1);
    let (s, _) = call(&h, &ot, Method::DELETE, &mem, None).await;
    assert_eq!(s, 204);
    assert_eq!(audit_n(&h, "member.remove").await, 1);
}

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

// ---------------------------------------------------------------- FIX-097: deletion completeness

/// A harness whose vault keeps managed values in an inspectable memory store.
async fn memory_harness() -> (Harness, std::sync::Arc<aurora_secrets::MemoryStore>) {
    use base64::Engine;
    let mut h = Harness::new().await;
    let mem = std::sync::Arc::new(aurora_secrets::MemoryStore::new("memory"));
    let k = base64::engine::general_purpose::STANDARD.encode([b'a'; 32]);
    let keyring =
        std::sync::Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{k}"), "k1").unwrap());
    let mut sr =
        aurora_secrets::StoreRegistry::from_config(&h.state.config.secrets, keyring).unwrap();
    sr.register(mem.clone());
    sr.set_default("memory").unwrap();
    h.state = h
        .state
        .clone()
        .with_vault(std::sync::Arc::new(aurora_secrets::SecretVault::new(sr)));
    h.app = test_router(h.state.clone());
    (h, mem)
}

/// Seal a managed value for `owner` and return its stored reference text.
async fn seal(
    h: &Harness,
    purpose: aurora_secrets::Purpose,
    owner: Uuid,
    org: Uuid,
    ws: Option<Uuid>,
) -> String {
    let ctx = aurora_secrets::SealContext {
        purpose,
        owner_id: owner,
        org_id: org,
        workspace_id: ws,
    };
    h.state
        .vault
        .seal(&ctx, aurora_core::Secret::new(b"s3cret".to_vec()))
        .await
        .unwrap()
        .to_string()
}

/// (FIX-097 1, 5) An organization with a directory group (bound to a workspace), a SCIM token
/// and a SCIM link can be deleted (the RESTRICT children used to roll the whole delete back),
/// and the managed secret values of the org's secrets, connections and OAuth clients are
/// discarded after the commit while the gateways are told to drop their caches.
#[tokio::test]
async fn org_delete_with_groups_and_scim_rows_discards_managed_values_and_invalidates() {
    use aurora_secrets::Purpose;
    let (h, mem) = memory_harness().await;
    let (owner, ot) = Who::new(&h, "o@x.io", true).await;
    let v = setup_org(&h, &ot, "Org").await;
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);
    let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
        .bind(org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    // Directory rows.
    let group = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name, source) VALUES ($1,$2,'g','manual')")
        .bind(group)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) VALUES ($1,$2,$3,'member')")
        .bind(group)
        .bind(ws)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO scim_tokens (id, org_id, name, token_hash, display_prefix, expires_at) VALUES ($1,$2,'t',$3,'aud_x', now() + interval '1 day')")
        .bind(Uuid::now_v7())
        .bind(org)
        .bind(vec![3u8; 32])
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO scim_users (id, org_id, user_id, user_name) VALUES ($1,$2,$3,'o@x.io')",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(owner)
    .execute(h.pool())
    .await
    .unwrap();
    // Managed values: an org secret, a workspace secret, a workspace connection, an org OAuth client.
    let before = mem.managed_count();
    for (ws_scope, purpose) in [
        (None, Purpose::SecretValue),
        (Some(ws), Purpose::SecretValue),
    ] {
        let sid = Uuid::now_v7();
        let r = seal(&h, purpose, sid, org, ws_scope).await;
        sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,$4,'generic','h.example','{}','managed',$5)")
            .bind(sid)
            .bind(org)
            .bind(ws_scope)
            .bind(sid.to_string())
            .bind(&r)
            .execute(h.pool())
            .await
            .unwrap();
    }
    let cid = Uuid::now_v7();
    let r = seal(&h, Purpose::ConnectionCredentials, cid, org, Some(ws)).await;
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active',$4)")
        .bind(cid)
        .bind(org)
        .bind(ws)
        .bind(&r)
        .execute(h.pool())
        .await
        .unwrap();
    let oid = Uuid::now_v7();
    let r = seal(&h, Purpose::OauthClientSecret, oid, org, None).await;
    sqlx::query("INSERT INTO app_oauth_clients (id, org_id, package_id, client_key, fields, secret_ref) VALUES ($1,$2,'p','c','{}',$3)")
        .bind(oid)
        .bind(org)
        .bind(&r)
        .execute(h.pool())
        .await
        .unwrap();
    assert_eq!(mem.managed_count(), before + 4);

    let mut l = sqlx::postgres::PgListener::connect_with(h.pool())
        .await
        .unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, e) = call(
        &h,
        &ot,
        Method::DELETE,
        &format!("/v1/orgs/{org}"),
        Some(json!({"confirm": slug})),
    )
    .await;
    assert_eq!(s, 204, "{e}");
    assert_eq!(
        count(&h, "SELECT count(*) FROM organizations WHERE id=$1", org).await,
        0
    );
    for t in [
        "groups",
        "scim_tokens",
        "scim_users",
        "secrets",
        "app_connections",
        "app_oauth_clients",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t} WHERE org_id = $1"))
            .bind(org)
            .fetch_one(h.pool())
            .await
            .unwrap();
        assert_eq!(n, 0, "{t}");
    }
    assert_eq!(
        mem.managed_count(),
        before,
        "every managed value was discarded"
    );
    // The org's invalidation reached the bus (read what is queued, bounded).
    let mut seen = false;
    while let Ok(Ok(n)) = tokio::time::timeout(std::time::Duration::from_secs(2), l.recv()).await {
        let p: Value = serde_json::from_str(n.payload()).unwrap();
        seen |= p["scope"] == "org" && p["id"] == json!(org);
        if seen {
            break;
        }
    }
    assert!(seen, "the gateways were told to drop the org's caches");
}

/// (FIX-097 5) Workspace deletion discards the managed values of its own secrets and
/// connections only, and tells the gateways.
#[tokio::test]
async fn workspace_delete_discards_its_managed_values_and_invalidates() {
    use aurora_secrets::Purpose;
    let (h, mem) = memory_harness().await;
    let (_, ot) = Who::new(&h, "o@x.io", true).await;
    let v = setup_org(&h, &ot, "Org").await;
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);
    let before = mem.managed_count();
    let sid = Uuid::now_v7();
    let r = seal(&h, Purpose::SecretValue, sid, org, Some(ws)).await;
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'w','generic','h.example','{}','managed',$4)")
        .bind(sid).bind(org).bind(ws).bind(&r).execute(h.pool()).await.unwrap();
    // An org-level value must survive a workspace delete.
    let osid = Uuid::now_v7();
    let r = seal(&h, Purpose::SecretValue, osid, org, None).await;
    sqlx::query("INSERT INTO secrets (id, org_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,'o','generic','h.example','{}','managed',$3)")
        .bind(osid).bind(org).bind(&r).execute(h.pool()).await.unwrap();
    assert_eq!(mem.managed_count(), before + 2);
    let mut l = sqlx::postgres::PgListener::connect_with(h.pool())
        .await
        .unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, e) = call(
        &h,
        &ot,
        Method::DELETE,
        &format!("/v1/workspaces/{ws}"),
        Some(json!({"confirm": "default"})),
    )
    .await;
    assert_eq!(s, 204, "{e}");
    assert_eq!(
        mem.managed_count(),
        before + 1,
        "only the workspace's value went"
    );
    let mut seen = false;
    while let Ok(Ok(n)) = tokio::time::timeout(std::time::Duration::from_secs(2), l.recv()).await {
        let p: Value = serde_json::from_str(n.payload()).unwrap();
        seen |= p["scope"] == "workspace" && p["id"] == json!(ws);
        if seen {
            break;
        }
    }
    assert!(
        seen,
        "the gateways were told to drop the workspace's caches"
    );
}

/// (FIX-097 5) Deleting an agent leaves no grant-sourced derived rule behind: the grants
/// cascade in SQL, their compiled rules do not.
#[tokio::test]
async fn agent_delete_removes_its_grant_derived_rules() {
    let h = Harness::new().await;
    let (_, ot) = Who::new(&h, "o@x.io", true).await;
    let v = setup_org(&h, &ot, "Org").await;
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x')")
        .bind(conn).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let grant = Uuid::now_v7();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(grant).bind(ws).bind(agent).bind(conn).execute(h.pool()).await.unwrap();
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'grant',$4,0,$5,'{}')")
        .bind(Uuid::now_v7()).bind(org).bind(ws).bind(grant).bind(Uuid::now_v7())
        .execute(h.pool()).await.unwrap();
    // Another agent's grant rule is untouched.
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'grant',$4,0,$5,'{}')")
        .bind(Uuid::now_v7()).bind(org).bind(ws).bind(other).bind(Uuid::now_v7())
        .execute(h.pool()).await.unwrap();
    let (s, e) = call(
        &h,
        &ot,
        Method::DELETE,
        &format!("/v1/agents/{agent}"),
        None,
    )
    .await;
    assert_eq!(s, 204, "{e}");
    assert_eq!(
        count(&h, "SELECT count(*) FROM grants WHERE id = $1", grant).await,
        0
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM derived_rules WHERE source_id = $1",
            grant
        )
        .await,
        0,
        "the deleted agent's compiled rule is gone"
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM derived_rules WHERE source_id = $1",
            other
        )
        .await,
        1
    );
}

/// Inserts a webhook with a sealed verification secret on `agent`; returns the stored ref.
async fn webhook_with_secret(h: &Harness, org: Uuid, ws: Uuid, agent: Uuid) -> (Uuid, String) {
    let hook = Uuid::now_v7();
    let r = seal(
        h,
        aurora_secrets::Purpose::WebhookVerification,
        hook,
        org,
        Some(ws),
    )
    .await;
    sqlx::query("INSERT INTO webhooks (id, agent_id, name, instructions, token_hash, display_prefix, verification) VALUES ($1,$2,'h','do it',$3,'aur_x',$4)")
        .bind(hook)
        .bind(agent)
        .bind(Uuid::now_v7().as_bytes().repeat(2))
        .bind(json!({"kind": "bearer", "header": "Authorization", "secret_ref": r}))
        .execute(h.pool())
        .await
        .unwrap();
    (hook, r)
}

async fn plain_agent(h: &Harness, org: Uuid, ws: Uuid, handle: &str) -> Uuid {
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,'Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).bind(handle).execute(h.pool()).await.unwrap();
    agent
}

async fn integration_with_credentials(h: &Harness, org: Uuid, tenant: &str) -> (Uuid, String) {
    sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('chat', $1, 'chat', '{}', 'active') ON CONFLICT DO NOTHING")
        .bind(vec![0u8; 32]).execute(h.pool()).await.unwrap();
    let integ = Uuid::now_v7();
    let r = seal(
        h,
        aurora_secrets::Purpose::IntegrationCredentials,
        integ,
        org,
        None,
    )
    .await;
    sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,'chat',$3,$3,'{}',$4,'active')")
        .bind(integ).bind(org).bind(tenant).bind(&r).execute(h.pool()).await.unwrap();
    (integ, r)
}

async fn presence_with_credentials(h: &Harness, org: Uuid, agent: Uuid, integ: Uuid) -> String {
    let presence = Uuid::now_v7();
    let r = seal(
        h,
        aurora_secrets::Purpose::PresenceCredentials,
        presence,
        org,
        None,
    )
    .await;
    sqlx::query("INSERT INTO channel_presences (id, agent_id, integration_id, mode, credentials_ref, status) VALUES ($1,$2,$3,'dedicated',$4,'active')")
        .bind(presence).bind(agent).bind(integ).bind(&r).execute(h.pool()).await.unwrap();
    r
}

/// (FIX-122) Deleting an agent discards its webhook signing secret and its dedicated
/// presence's credentials from the secret store; another agent's values stay.
#[tokio::test]
async fn agent_delete_discards_its_webhook_and_presence_secrets() {
    let (h, mem) = memory_harness().await;
    let (_, ot) = Who::new(&h, "o@x.io", true).await;
    let v = setup_org(&h, &ot, "Org").await;
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);
    let before = mem.managed_count();
    let gone = plain_agent(&h, org, ws, "gone").await;
    let kept = plain_agent(&h, org, ws, "kept").await;
    let (integ, _) = integration_with_credentials(&h, org, "T1").await;
    webhook_with_secret(&h, org, ws, gone).await;
    webhook_with_secret(&h, org, ws, kept).await;
    presence_with_credentials(&h, org, gone, integ).await;
    // integration + 2 webhooks + 1 presence
    assert_eq!(mem.managed_count(), before + 4);
    let (s, e) = call(&h, &ot, Method::DELETE, &format!("/v1/agents/{gone}"), None).await;
    assert_eq!(s, 204, "{e}");
    assert_eq!(
        mem.managed_count(),
        before + 2,
        "the agent's webhook secret and presence credentials went; the others stayed"
    );
}

/// (FIX-122) Deleting a workspace or an organization discards the same, plus the
/// organization's integration credentials.
#[tokio::test]
async fn workspace_and_org_delete_discard_webhook_and_integration_secrets() {
    let (h, mem) = memory_harness().await;
    let (_, ot) = Who::new(&h, "o@x.io", true).await;
    let v = setup_org(&h, &ot, "Org").await;
    let org = id(&v["org"]["id"]);
    let ws = id(&v["workspace"]["id"]);
    let before = mem.managed_count();
    let agent = plain_agent(&h, org, ws, "bot").await;
    let (integ, _) = integration_with_credentials(&h, org, "T2").await;
    webhook_with_secret(&h, org, ws, agent).await;
    presence_with_credentials(&h, org, agent, integ).await;
    assert_eq!(mem.managed_count(), before + 3);
    // The workspace delete takes the agent's webhook secret and presence credentials, not the
    // org's integration credentials.
    let (s, e) = call(
        &h,
        &ot,
        Method::DELETE,
        &format!("/v1/workspaces/{ws}"),
        Some(json!({"confirm": "default"})),
    )
    .await;
    assert_eq!(s, 204, "{e}");
    assert_eq!(mem.managed_count(), before + 1);
    let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
        .bind(org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (s, e) = call(
        &h,
        &ot,
        Method::DELETE,
        &format!("/v1/orgs/{org}"),
        Some(json!({"confirm": slug})),
    )
    .await;
    assert_eq!(s, 204, "{e}");
    assert_eq!(mem.managed_count(), before);
}

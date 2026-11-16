#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-ADMDEL (D-234, C-733): instance admins list organization metadata and delete any
//! organization or workspace — the owner routes' deletion services, plus the placement prune.

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

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

struct E {
    h: Harness,
    mem: std::sync::Arc<aurora_secrets::MemoryStore>,
    /// Session token of the instance admin. FIX-157 (D-317): never re-authenticated; no
    /// operation asks for it.
    ia_tok: String,
}

impl E {
    async fn new() -> E {
        let (h, mem) = memory_harness().await;
        let ia = h.user("ia@x.io", true).await;
        let (_, ia_tok) = h.good_session(ia).await;
        E { h, mem, ia_tok }
    }

    async fn call(&self, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let c = cookie(tok);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn ia(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.call(&self.ia_tok, m, uri, body).await
    }

    async fn n(&self, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// An org with an owner (a user with a session), a workspace, two agents, a managed
    /// org secret and a managed workspace secret. Returns `(org, ws, owner token, org slug,
    /// ws slug)`.
    async fn org_fixture(&self, tag: &str) -> (Uuid, Uuid, String, String, String) {
        use aurora_secrets::Purpose;
        let org = self.h.org().await;
        let owner = self.h.user(&format!("owner-{tag}@x.io"), false).await;
        self.h.member(org, owner, "owner", "active").await;
        let (_, tok) = self.h.good_session(owner).await;
        let ws = self.h.workspace(org).await;
        for h in ["a1", "a2"] {
            sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,$4,$4,'hosted','fake')")
                .bind(Uuid::now_v7()).bind(org).bind(ws).bind(h)
                .execute(self.h.pool()).await.unwrap();
        }
        for (ws_scope, purpose) in [
            (None, Purpose::SecretValue),
            (Some(ws), Purpose::SecretValue),
        ] {
            let sid = Uuid::now_v7();
            let ctx = aurora_secrets::SealContext {
                purpose,
                owner_id: sid,
                org_id: org,
                workspace_id: ws_scope,
            };
            let r = self
                .h
                .state
                .vault
                .seal(&ctx, aurora_core::Secret::new(b"s3cret".to_vec()))
                .await
                .unwrap()
                .to_string();
            sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,$4,'generic','h.example','{}','managed',$5)")
                .bind(sid).bind(org).bind(ws_scope).bind(sid.to_string()).bind(&r)
                .execute(self.h.pool()).await.unwrap();
        }
        let os: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_one(self.h.pool())
            .await
            .unwrap();
        let ws_slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
            .bind(ws)
            .fetch_one(self.h.pool())
            .await
            .unwrap();
        (org, ws, tok, os, ws_slug)
    }

    /// The set of tables (with their `org_id` column) still holding rows for `org`.
    async fn org_rows(&self, org: Uuid) -> Vec<String> {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT table_name::text FROM information_schema.columns
             WHERE table_schema = 'public' AND column_name = 'org_id'
               AND table_name IN (SELECT table_name FROM information_schema.tables
                                  WHERE table_schema = 'public' AND table_type = 'BASE TABLE')
             ORDER BY 1",
        )
        .fetch_all(self.h.pool())
        .await
        .unwrap();
        let mut left = Vec::new();
        for t in tables {
            // The org's own ledgers outlive it (docs/04 §Deletion semantics): not FK-linked.
            if t == "audit_log" || t == "request_log" || t.starts_with("runtime_") {
                continue;
            }
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {t} WHERE org_id = $1"
            )))
            .bind(org)
            .fetch_one(self.h.pool())
            .await
            .unwrap();
            if n > 0 {
                left.push(format!("{t}={n}"));
            }
        }
        left
    }

    async fn listener(&self) -> sqlx::postgres::PgListener {
        let mut l = sqlx::postgres::PgListener::connect_with(self.h.pool())
            .await
            .unwrap();
        l.listen("aurora_gateway_invalidate").await.unwrap();
        l
    }

    async fn saw_invalidate(l: &mut sqlx::postgres::PgListener, scope: &str, id: Uuid) -> bool {
        while let Ok(Ok(n)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), l.recv()).await
        {
            let p: Value = serde_json::from_str(n.payload()).unwrap();
            if p["scope"] == scope && p["id"] == json!(id) {
                return true;
            }
        }
        false
    }
}

#[tokio::test]
async fn admin_orgs_list_detail_and_workspaces_carry_counts_and_no_content() {
    let e = E::new().await;
    let (org, ws, _, slug, ws_slug) = e.org_fixture("a").await;
    // A second, ownerless org with one pending and one active member.
    let org2 = e.h.org().await;
    let m = e.h.user("m@x.io", false).await;
    e.h.member(org2, m, "member", "active").await;
    let p = e.h.user("p@x.io", false).await;
    e.h.member(org2, p, "member", "pending").await;
    let ws2 = e.h.workspace(org2).await;
    sqlx::query("UPDATE workspaces SET personal = true WHERE id = $1")
        .bind(ws2)
        .execute(e.h.pool())
        .await
        .unwrap();

    let (s, v) = e.ia(Method::GET, "/v1/admin/orgs", None).await;
    assert_eq!(s, 200, "{v}");
    let row = |id: Uuid| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id))
            .cloned()
            .unwrap()
    };
    let r1 = row(org);
    assert_eq!(r1["ownerless"], false);
    assert_eq!(r1["member_count"], 1);
    assert_eq!(r1["workspace_count"], 1);
    assert_eq!(r1["agent_count"], 2);
    let r2 = row(org2);
    assert_eq!(r2["ownerless"], true);
    assert_eq!(r2["member_count"], 1, "pending members are not counted");

    // Detail: the exact field set, nothing of the org's content.
    let (s, d) = e
        .ia(Method::GET, &format!("/v1/admin/orgs/{org}"), None)
        .await;
    assert_eq!(s, 200, "{d}");
    let mut keys: Vec<&str> = d.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent_count",
            "created_at",
            "frozen",
            "id",
            "member_count",
            "name",
            "ownerless",
            "slug",
            "workspace_count"
        ]
    );
    assert_eq!(d["slug"], json!(slug));

    // Workspaces: metadata only.
    let (s, w) = e
        .ia(
            Method::GET,
            &format!("/v1/admin/orgs/{org}/workspaces"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{w}");
    assert_eq!(w["items"].as_array().unwrap().len(), 1);
    let item = &w["items"][0];
    let mut keys: Vec<&str> = item
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent_count",
            "created_at",
            "id",
            "name",
            "personal",
            "slug"
        ]
    );
    assert_eq!(item["id"], json!(ws));
    assert_eq!(item["slug"], json!(ws_slug));
    assert_eq!(item["agent_count"], 2);
    assert_eq!(item["personal"], false);
    let (_, w2) = e
        .ia(
            Method::GET,
            &format!("/v1/admin/orgs/{org2}/workspaces"),
            None,
        )
        .await;
    assert_eq!(w2["items"][0]["personal"], true);
    // Pagination.
    e.h.workspace(org).await;
    let (_, p1) = e
        .ia(
            Method::GET,
            &format!("/v1/admin/orgs/{org}/workspaces?limit=1"),
            None,
        )
        .await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 1);
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = e
        .ia(
            Method::GET,
            &format!("/v1/admin/orgs/{org}/workspaces?limit=1&cursor={cur}"),
            None,
        )
        .await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert_ne!(p1["items"][0]["id"], p2["items"][0]["id"]);

    // Unknown ids.
    let ghost = Uuid::now_v7();
    for uri in [
        format!("/v1/admin/orgs/{ghost}"),
        format!("/v1/admin/orgs/{ghost}/workspaces"),
    ] {
        assert_eq!(e.ia(Method::GET, &uri, None).await.0, 404, "{uri}");
    }
}

#[tokio::test]
async fn admin_org_routes_refuse_org_roles_and_api_keys() {
    let e = E::new().await;
    let (org, ws, owner_tok, slug, ws_slug) = e.org_fixture("b").await;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM org_members WHERE org_id = $1")
        .bind(org)
        .fetch_one(e.h.pool())
        .await
        .unwrap();
    let (_, key) = e.h.api_key(owner, org, None, false).await;
    let calls = [
        (Method::GET, "/v1/admin/orgs".to_owned(), None),
        (Method::GET, format!("/v1/admin/orgs/{org}"), None),
        (
            Method::GET,
            format!("/v1/admin/orgs/{org}/workspaces"),
            None,
        ),
        (
            Method::DELETE,
            format!("/v1/admin/orgs/{org}"),
            Some(json!({"confirm": slug})),
        ),
        (
            Method::DELETE,
            format!("/v1/admin/workspaces/{ws}"),
            Some(json!({"confirm": ws_slug})),
        ),
    ];
    for (m, uri, body) in calls {
        let (s, v) = e.call(&owner_tok, m.clone(), &uri, body.clone()).await;
        assert_eq!(s, 403, "{m} {uri}: {v}");
        let (s, _, _) =
            e.h.send(
                m.clone(),
                &uri,
                &[("authorization", &bearer(&key)), ("origin", ORIGIN)],
                body.clone(),
            )
            .await;
        assert!(s == 401 || s == 403, "{m} {uri}: api key got {s}");
        let (s, _, _) = e.h.send(m.clone(), &uri, &[("origin", ORIGIN)], body).await;
        assert_eq!(s, 401, "{m} {uri}");
    }
    assert_eq!(
        e.n("SELECT count(*) FROM organizations WHERE id=$1", org)
            .await,
        1
    );
    assert_eq!(
        e.n("SELECT count(*) FROM workspaces WHERE id=$1", ws).await,
        1
    );
}

#[tokio::test]
async fn admin_deletes_need_the_slug_not_a_reauth_and_delete_nothing_otherwise() {
    let e = E::new().await;
    let (org, ws, _, slug, ws_slug) = e.org_fixture("c").await;
    let org_uri = format!("/v1/admin/orgs/{org}");
    let ws_uri = format!("/v1/admin/workspaces/{ws}");
    let audits = |a: &'static str| {
        let h = &e.h;
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log WHERE action = $1")
                .bind(a)
                .fetch_one(h.pool())
                .await
                .unwrap()
        }
    };
    let intact = async || {
        e.n("SELECT count(*) FROM organizations WHERE id=$1", org)
            .await
            == 1
            && e.n("SELECT count(*) FROM workspaces WHERE id=$1", ws).await == 1
            && e.n("SELECT count(*) FROM agents WHERE org_id=$1", org)
                .await
                == 2
            && e.mem.managed_count() == 2
    };

    // The admin session never re-authenticated (FIX-157, D-317): the typed slug alone gates.
    // Wrong slug: 400.
    for uri in [&org_uri, &ws_uri] {
        let (s, v) = e
            .ia(Method::DELETE, uri, Some(json!({"confirm": "nope"})))
            .await;
        assert_eq!((s, code(&v)), (400, "confirmation_mismatch"), "{uri}");
        let (s, v) = e.ia(Method::DELETE, uri, Some(json!({}))).await;
        assert_eq!(s, 400, "a body without `confirm` is invalid: {v}");
    }
    // The other kind's slug is no confirmation either.
    let (s, _) = e
        .ia(Method::DELETE, &org_uri, Some(json!({"confirm": ws_slug})))
        .await;
    assert_eq!(s, 400);
    assert!(intact().await);
    assert_eq!(audits("admin.org.delete").await, 0);
    assert_eq!(audits("admin.workspace.delete").await, 0);

    // Unknown ids: 404 (before the confirmation is looked at).
    let ghost = Uuid::now_v7();
    for uri in [
        format!("/v1/admin/orgs/{ghost}"),
        format!("/v1/admin/workspaces/{ghost}"),
    ] {
        let (s, _) = e
            .ia(Method::DELETE, &uri, Some(json!({"confirm": "x"})))
            .await;
        assert_eq!(s, 404, "{uri}");
    }
    assert!(intact().await);

    // The correct slug deletes on the first request, no re-authentication (D-317).
    let (s, v) = e
        .ia(Method::DELETE, &ws_uri, Some(json!({"confirm": ws_slug})))
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(
        e.n("SELECT count(*) FROM workspaces WHERE id=$1", ws).await,
        0
    );
    let (s, v) = e
        .ia(Method::DELETE, &org_uri, Some(json!({"confirm": slug})))
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(
        e.n("SELECT count(*) FROM organizations WHERE id=$1", org)
            .await,
        0
    );
}

/// The owner and the admin path leave the same post-state on equivalent fixtures; an admin
/// delete also works on a frozen organization, and the audit row is an instance-level one.
#[tokio::test]
async fn admin_org_delete_matches_the_owner_delete_and_works_when_frozen() {
    let e = E::new().await;
    let (org_a, _, owner_tok, slug_a, _) = e.org_fixture("own").await;
    let (org_b, ws_b, _, slug_b, _) = e.org_fixture("adm").await;
    assert_eq!(e.mem.managed_count(), 4);
    // The admin deletes a frozen organization.
    let (s, v) = e
        .ia(
            Method::POST,
            &format!("/v1/admin/orgs/{org_b}/freeze"),
            Some(json!({"reason": "incident"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let mut l = e.listener().await;

    let (s, v) = e
        .call(
            &owner_tok,
            Method::DELETE,
            &format!("/v1/orgs/{org_a}"),
            Some(json!({"confirm": slug_a})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert!(E::saw_invalidate(&mut l, "org", org_a).await);
    let (s, v) = e
        .ia(
            Method::DELETE,
            &format!("/v1/admin/orgs/{org_b}"),
            Some(json!({"confirm": slug_b})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert!(
        E::saw_invalidate(&mut l, "org", org_b).await,
        "gateways were told to drop the org's caches"
    );
    // Same post-state: no row left anywhere (by org_id), the workspace and agents gone,
    // every managed value discarded.
    let (left_a, left_b) = (e.org_rows(org_a).await, e.org_rows(org_b).await);
    assert!(left_a.is_empty(), "owner path left {left_a:?}");
    assert!(left_b.is_empty(), "admin path left {left_b:?}");
    assert_eq!(
        e.n("SELECT count(*) FROM workspaces WHERE id=$1", ws_b)
            .await,
        0
    );
    assert_eq!(e.mem.managed_count(), 0, "managed values discarded");

    // Audit: one critical instance-level row, target and metadata per C-733.
    type AuditRow = (bool, Option<Uuid>, String, Option<String>, Value, String);
    let rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT critical, org_id, target_kind, target_id, metadata, actor_kind
         FROM audit_log WHERE action = 'admin.org.delete'",
    )
    .fetch_all(e.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    let (critical, row_org, kind, target, meta, actor) = &rows[0];
    assert!(*critical && row_org.is_none());
    assert_eq!(
        (kind.as_str(), actor.as_str()),
        ("organization", "instance_admin")
    );
    assert_eq!(target.as_deref(), Some(org_b.to_string().as_str()));
    assert_eq!(
        meta,
        &json!({"org_id": org_b, "slug": slug_b, "name": "o",
                "member_count": 1, "workspace_count": 1, "agent_count": 2})
    );
    // It shows on the instance Audit page's default scope.
    let (s, v) = e.ia(Method::GET, "/v1/admin/audit", None).await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "admin.org.delete" && r["org_id"].is_null())
    );
    // Both deleted organizations are gone from the admin list and 404 on detail.
    assert_eq!(
        e.ia(Method::GET, &format!("/v1/admin/orgs/{org_b}"), None)
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn admin_workspace_delete_matches_the_owner_path_and_audits() {
    let e = E::new().await;
    let (org, ws, _, _, ws_slug) = e.org_fixture("w").await;
    // A second workspace, deleted by the owner (the reference post-state).
    let ws_other = e.h.workspace(org).await;
    let owner_tok = {
        let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM org_members WHERE org_id = $1")
            .bind(org)
            .fetch_one(e.h.pool())
            .await
            .unwrap();
        let (_, tok) = e.h.good_session(owner).await;
        tok
    };
    let other_slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
        .bind(ws_other)
        .fetch_one(e.h.pool())
        .await
        .unwrap();
    let mut l = e.listener().await;
    let (s, v) = e
        .call(
            &owner_tok,
            Method::DELETE,
            &format!("/v1/workspaces/{ws_other}"),
            Some(json!({"confirm": other_slug})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert!(E::saw_invalidate(&mut l, "workspace", ws_other).await);

    assert_eq!(e.mem.managed_count(), 2);
    let (s, v) = e
        .ia(
            Method::DELETE,
            &format!("/v1/admin/workspaces/{ws}"),
            Some(json!({"confirm": ws_slug})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert!(E::saw_invalidate(&mut l, "workspace", ws).await);
    assert_eq!(
        e.n("SELECT count(*) FROM workspaces WHERE id=$1", ws).await,
        0
    );
    assert_eq!(
        e.n("SELECT count(*) FROM agents WHERE workspace_id=$1", ws)
            .await,
        0
    );
    assert_eq!(
        e.n("SELECT count(*) FROM secrets WHERE workspace_id=$1", ws)
            .await,
        0
    );
    // The org and its org-level secret stay; only the workspace's managed value is discarded.
    assert_eq!(
        e.n("SELECT count(*) FROM organizations WHERE id=$1", org)
            .await,
        1
    );
    assert_eq!(
        e.n("SELECT count(*) FROM secrets WHERE org_id=$1", org)
            .await,
        1
    );
    assert_eq!(e.mem.managed_count(), 1);

    let (critical, row_org, target, meta): (bool, Option<Uuid>, Option<String>, Value) =
        sqlx::query_as(
            "SELECT critical, org_id, target_id, metadata FROM audit_log
             WHERE action = 'admin.workspace.delete'",
        )
        .fetch_one(e.h.pool())
        .await
        .unwrap();
    assert!(critical && row_org.is_none());
    assert_eq!(target, Some(ws.to_string()));
    assert_eq!(
        meta,
        json!({"org_id": org, "slug": ws_slug, "name": "w", "agent_count": 2})
    );
}

/// (D-234) A deleted workspace leaves `instance_placement.shared_workspaces` on every delete
/// path: the owner's workspace and org routes and the admin's.
#[tokio::test]
async fn deleted_workspaces_leave_the_placement_shared_list_on_every_path() {
    let e = E::new().await;
    let (org, ws1, owner_tok, slug, _) = e.org_fixture("p").await;
    let ws2 = e.h.workspace(org).await;
    let ws3 = e.h.workspace(org).await;
    let (org2, keep, _, slug2, _) = e.org_fixture("q").await;
    let slug_of = |w: Uuid| {
        let h = &e.h;
        async move {
            sqlx::query_scalar::<_, String>("SELECT slug FROM workspaces WHERE id = $1")
                .bind(w)
                .fetch_one(h.pool())
                .await
                .unwrap()
        }
    };
    let set = async |ids: &[Uuid]| {
        let shared: Vec<Value> = ids
            .iter()
            .map(|w| json!({"workspace_id": w, "role": "member"}))
            .collect();
        sqlx::query(
            "INSERT INTO instance_placement (singleton, mode, shared_workspaces)
             VALUES (true, 'none', $1)
             ON CONFLICT (singleton) DO UPDATE SET shared_workspaces = $1",
        )
        .bind(Value::Array(shared))
        .execute(e.h.pool())
        .await
        .unwrap();
    };
    let shared = async || -> Vec<Uuid> {
        let v: Value = sqlx::query_scalar("SELECT shared_workspaces FROM instance_placement")
            .fetch_one(e.h.pool())
            .await
            .unwrap();
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x["workspace_id"].as_str().unwrap().parse().unwrap())
            .collect()
    };
    set(&[ws1, ws2, ws3, keep]).await;

    // Owner workspace delete.
    let (s, v) = e
        .call(
            &owner_tok,
            Method::DELETE,
            &format!("/v1/workspaces/{ws1}"),
            Some(json!({"confirm": slug_of(ws1).await})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(shared().await, vec![ws2, ws3, keep]);
    // Admin workspace delete.
    let (s, v) = e
        .ia(
            Method::DELETE,
            &format!("/v1/admin/workspaces/{ws2}"),
            Some(json!({"confirm": slug_of(ws2).await})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(shared().await, vec![ws3, keep]);
    // Owner org delete drops the org's remaining workspace, not another org's.
    let (s, v) = e
        .call(
            &owner_tok,
            Method::DELETE,
            &format!("/v1/orgs/{org}"),
            Some(json!({"confirm": slug})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(shared().await, vec![keep]);
    // Admin org delete.
    set(&[keep]).await;
    let (s, v) = e
        .ia(
            Method::DELETE,
            &format!("/v1/admin/orgs/{org2}"),
            Some(json!({"confirm": slug2})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert!(shared().await.is_empty());
    // The placement row itself stays.
    assert_eq!(
        e.n(
            "SELECT count(*) FROM instance_placement WHERE $1::uuid IS NOT NULL",
            keep
        )
        .await,
        1
    );
}

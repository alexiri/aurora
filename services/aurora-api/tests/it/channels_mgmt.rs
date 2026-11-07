#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-074: channel management API (API-006, C-33..C-37): integrations, presences, bindings,
//! user links + confirm, DM defaults, reach, deprovision on delete, credential_dead hook.
//! The connector side is played through the BE-071 connector-plane routes (`serve`).

use crate::common::*;
use aurora_crypto::generate_signing_key;
use aurora_registry::builder::PackageBuilder;
use http::{Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use uuid::Uuid;

const SECRET: &str = "SECRET-credential-123";

struct Conn {
    id: String,
    tok: String,
    replica: Uuid,
}

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    fake: Conn,
    manual: Conn,
    owner: String,
    admin: String,
    wsowner: String,
    wsmember: String,
    plain: String,
    outsider: String,
    users: Users,
}

struct Users {
    owner: Uuid,
    wsmember: Uuid,
    plain: Uuid,
    outsider: Uuid,
}

fn hd(c: &str) -> [(&str, &str); 2] {
    [("cookie", c), ("origin", ORIGIN)]
}

fn manifest(id: &str, presence_setup: Value) -> Value {
    json!({
        "package": {"id": id, "version": "1.0.0", "contract": 1, "publisher": "acme"},
        "about": {"name": id},
        "capabilities": {"threads": true, "presence_dedicated": true, "presence_shared": true,
                         "email_verified": id == "fake"},
        "integration_setup": {"fields": []},
        "presence_setup": presence_setup,
        "limits": {},
        "public_callbacks": []
    })
}

async fn plane(
    h: &Harness,
    tok: &str,
    replica: Option<Uuid>,
    m: Method,
    path: &str,
    body: Option<Value>,
) -> (u16, http::HeaderMap, Value) {
    let b = bearer(tok);
    let r = replica.map(|r| r.to_string());
    let mut hs = vec![("authorization", b.as_str())];
    if let Some(r) = &r {
        hs.push(("x-aurora-replica", r.as_str()));
    }
    let (s, hm, v) = h.send(m, &format!("/v1/connector{path}"), &hs, body).await;
    (s.as_u16(), hm, v)
}

impl W {
    async fn new() -> W {
        let h = Harness::new().await;
        let root = h.user("root@x.io", true).await;
        let admin_c = cookie(&h.good_session(root).await.1);
        let key = generate_signing_key().unwrap();
        let (s, _, v) = h
            .send(
                Method::POST,
                "/v1/admin/trusted-keys",
                &hd(&admin_c),
                Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let mut conns = Vec::new();
        for (id, presence_setup) in [
            ("fake", json!({})),
            (
                "manual",
                json!({"dedicated": {"fields": [{"name": "token", "label": "Bot token", "type": "secret", "required": true}]}}),
            ),
        ] {
            let (s, _, v) = h
                .send(
                    Method::POST,
                    "/v1/admin/connectors",
                    &hd(&admin_c),
                    Some(json!({"id": id, "display_name": id})),
                )
                .await;
            assert_eq!(s, 201, "{v}");
            let tok = v["token"].as_str().unwrap().to_owned();
            let m = manifest(id, presence_setup);
            let typed: aurora_protocol::connector::ConnectorManifest =
                serde_json::from_value(m.clone()).unwrap();
            let canon = serde_json::to_value(&typed).unwrap();
            let d: [u8; 32] =
                Sha256::digest(aurora_registry::snapshot::canonical_bytes(&canon)).into();
            let sig = PackageBuilder::sign_digest(&key, &PackageBuilder::key_id(&key), &d);
            let (s, _, v) = plane(
                &h,
                &tok,
                None,
                Method::POST,
                "/register",
                Some(json!({"manifest": m, "signature": sig, "version": "1.0.0"})),
            )
            .await;
            assert_eq!(s, 200, "{v}");
            let replica: Uuid = v["replica_id"].as_str().unwrap().parse().unwrap();
            let digest: String =
                sqlx::query_scalar("SELECT manifest_digest FROM connectors WHERE id=$1")
                    .bind(id)
                    .fetch_one(h.pool())
                    .await
                    .unwrap();
            let (s, _, v) = h
                .send(
                    Method::POST,
                    &format!("/v1/admin/connectors/{id}/approve"),
                    &hd(&admin_c),
                    Some(json!({ "manifest_digest": digest })),
                )
                .await;
            assert_eq!(s, 200, "{v}");
            conns.push(Conn {
                id: id.to_owned(),
                tok,
                replica,
            });
        }
        let manual = conns.pop().unwrap();
        let fake = conns.pop().unwrap();

        let org = h.org().await;
        let ws = h.workspace(org).await;
        let owner = h.user("owner@x.io", false).await;
        let adm = h.user("adm@x.io", false).await;
        let wso = h.user("wso@x.io", false).await;
        let wsm = h.user("wsm@x.io", false).await;
        let plain = h.user("plain@x.io", false).await;
        let outsider = h.user("out@x.io", false).await;
        h.member(org, owner, "owner", "active").await;
        h.member(org, adm, "admin", "active").await;
        h.member(org, wso, "member", "active").await;
        h.member(org, wsm, "member", "active").await;
        h.member(org, plain, "member", "active").await;
        h.bind_ws(ws, wso, "owner").await;
        h.bind_ws(ws, wsm, "member").await;
        let ck = |u: Uuid| {
            let h = &h;
            async move { cookie(&h.good_session(u).await.1) }
        };
        W {
            owner: ck(owner).await,
            admin: ck(adm).await,
            wsowner: ck(wso).await,
            wsmember: ck(wsm).await,
            plain: ck(plain).await,
            outsider: ck(outsider).await,
            users: Users {
                owner,
                wsmember: wsm,
                plain,
                outsider,
            },
            h,
            org,
            ws,
            fake,
            manual,
        }
    }

    async fn call(&self, who: &str, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self.h.send(m, path, &hd(who), body).await;
        (s.as_u16(), v)
    }

    /// Plays the connector: claims ops of `c` and answers each with `f(op)` until `expect`
    /// were answered (or 8 s passed). Returns the answered ops.
    async fn serve(&self, c: &Conn, expect: usize, f: impl Fn(&Value) -> Value) -> Vec<Value> {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while seen.len() < expect && Instant::now() < deadline {
            let (s, _, v) = plane(
                &self.h,
                &c.tok,
                Some(c.replica),
                Method::POST,
                "/ops/claim",
                Some(json!({"max": 10, "wait_secs": 1})),
            )
            .await;
            assert_eq!(s, 200, "{v}");
            for op in v["ops"].as_array().unwrap() {
                let mut r = f(op);
                r["op_id"] = op["op_id"].clone();
                let (s, _, v) = plane(
                    &self.h,
                    &c.tok,
                    Some(c.replica),
                    Method::POST,
                    "/ops/results",
                    Some(json!({"results": [r]})),
                )
                .await;
                assert_eq!(s, 200, "{v}");
                seen.push(op.clone());
            }
        }
        seen
    }

    async fn create_integration(
        &self,
        c: &Conn,
        who: &str,
        body: Value,
        answer: impl Fn(&Value) -> Value,
        expect_ops: usize,
    ) -> (u16, Value, Vec<Value>) {
        let path = format!("/v1/orgs/{}/channel-integrations", self.org);
        let ((s, v), ops) = tokio::join!(
            self.call(who, Method::POST, &path, Some(body),),
            self.serve(c, expect_ops, answer)
        );
        (s, v, ops)
    }

    fn create_body(&self, c: &Conn) -> Value {
        json!({"connector_id": c.id, "config": {"region": "eu"}, "credentials": {"token": SECRET}})
    }

    /// An active integration with a shared bot, owned by the connector's replica (it has
    /// polled its config, which is what makes ops of the integration claimable).
    async fn integration(&self, c: &Conn, tenant: &str) -> Uuid {
        let (s, v, _) = self
            .create_integration(
                c,
                &self.owner,
                self.create_body(c),
                |_| validate_ok(tenant),
                1,
            )
            .await;
        assert_eq!(s, 201, "{v}");
        self.feed_etag(c).await;
        v["id"].as_str().unwrap().parse().unwrap()
    }

    async fn agent(&self, handle: &str) -> Uuid {
        let agent = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,$4,$4,'hosted','fake')")
            .bind(agent).bind(self.org).bind(self.ws).bind(handle).execute(self.h.pool()).await.unwrap();
        agent
    }

    async fn sync_gen(&self, agent: Uuid) -> i64 {
        sqlx::query_scalar("SELECT home_sync_generation FROM agents WHERE id=$1")
            .bind(agent)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn feed_etag(&self, c: &Conn) -> String {
        let (s, hm, v) = plane(
            &self.h,
            &c.tok,
            Some(c.replica),
            Method::GET,
            "/config",
            None,
        )
        .await;
        assert_eq!(s, 200, "{v}");
        hm.get("etag").unwrap().to_str().unwrap().to_owned()
    }

    async fn attach(&self, who: &str, agent: Uuid, integ: Uuid, mode: &str) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/agents/{agent}/presences"),
            Some(json!({"integration_id": integ, "mode": mode})),
        )
        .await
    }

    async fn ops_of(&self, kind: &str) -> Vec<(Uuid, Option<Uuid>, Option<Uuid>, String, Value)> {
        sqlx::query_as(
            "SELECT id, integration_id, presence_id, status, payload FROM channel_ops WHERE kind=$1 ORDER BY created_at",
        )
        .bind(kind)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }
}

fn validate_ok(tenant: &str) -> Value {
    json!({"outcome": "ok", "data": {
        "tenant_ref": tenant, "tenant_label": format!("Tenant {tenant}"),
        "shared_bot": {"platform_user_ref": "B1", "handle": "daneel", "display_name": "Daneel"}}})
}

fn op_err(code: &str, msg: &str) -> Value {
    json!({"outcome": "error", "error": {"code": code, "message": msg}})
}

// ------------------------------------------------------------------ integrations

#[tokio::test]
async fn integration_create_success_hides_credentials() {
    let w = W::new().await;
    let body = w.create_body(&w.fake);
    let answer = |op: &Value| {
        // The connector sees the plaintext credentials and config.
        assert_eq!(op["kind"], "credentials.validate");
        assert_eq!(op["payload"]["credentials"]["token"], SECRET);
        assert_eq!(op["payload"]["config"]["region"], "eu");
        json!({"outcome": "ok", "data": {
            "tenant_ref": "T1", "tenant_label": "Acme",
            "shared_bot": {"platform_user_ref": "B1", "handle": "daneel", "display_name": "Daneel"},
            "capabilities": {"threads": false, "presence_dedicated": true, "bogus": true}}})
    };
    let (s, v, ops) = w
        .create_integration(&w.fake, &w.owner, body, answer, 1)
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(ops.len(), 1);
    assert_eq!(v["status"], "active");
    assert_eq!(v["tenant_ref"], "T1");
    assert_eq!(v["tenant_label"], "Acme");
    assert_eq!(
        v["shared_bot"],
        json!({"handle": "daneel", "display_name": "Daneel"})
    );
    assert_eq!(v["config"]["region"], "eu");
    // Narrowing only: switched off is kept, unknown capabilities are dropped.
    assert_eq!(
        v["config"]["_capabilities"],
        json!({"threads": false, "presence_dedicated": true})
    );
    let id = v["id"].as_str().unwrap().to_owned();

    // Credentials are write-only everywhere.
    assert!(!v.to_string().contains(SECRET));
    let (s, got) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/channel-integrations/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert!(!got.to_string().contains(SECRET));
    let (s, list) = w
        .call(
            &w.admin,
            Method::GET,
            &format!("/v1/orgs/{}/channel-integrations", w.org),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(SECRET));
    // ...and do not stay at rest in the finished validate op.
    for (_, _, _, status, payload) in w.ops_of("credentials.validate").await {
        assert_eq!(status, "done");
        assert!(!payload.to_string().contains(SECRET), "{payload}");
    }
    let stored: String = sqlx::query_scalar("SELECT credentials_ref FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert!(!stored.contains(SECRET));

    // The connector's config feed serves the plaintext (to its owner only).
    let (s, _, feed) = plane(
        &w.h,
        &w.fake.tok,
        Some(w.fake.replica),
        Method::GET,
        "/config",
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(feed["integrations"][0]["credentials"]["token"], SECRET);
}

#[tokio::test]
async fn integration_create_requires_org_admin() {
    let w = W::new().await;
    let body = w.create_body(&w.fake);
    let path = format!("/v1/orgs/{}/channel-integrations", w.org);
    assert_eq!(
        w.call(&w.plain, Method::POST, &path, Some(body.clone()))
            .await
            .0,
        403
    );
    assert_eq!(
        w.call(&w.wsowner, Method::POST, &path, Some(body.clone()))
            .await
            .0,
        403
    );
    assert_eq!(
        w.call(&w.outsider, Method::POST, &path, Some(body.clone()))
            .await
            .0,
        404
    );
    assert_eq!(w.call(&w.plain, Method::GET, &path, None).await.0, 403);
    // Unknown connector: validation error, nothing queued.
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &path,
            Some(json!({"connector_id": "nope", "credentials": {"a": "b"}})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert!(w.ops_of("credentials.validate").await.is_empty());
    // Credentials are required and must be an object.
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            &path,
            Some(json!({"connector_id": "fake", "credentials": {}})),
        )
        .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn integration_create_invalid_credentials_removes_row() {
    let w = W::new().await;
    for code in ["invalid", "forbidden", "credential_dead"] {
        let (s, v, _) = w
            .create_integration(
                &w.fake,
                &w.owner,
                w.create_body(&w.fake),
                |_| op_err(code, "bad token"),
                1,
            )
            .await;
        assert_eq!(s, 422, "{code}: {v}");
        assert_eq!(v["error"]["code"], "credentials_invalid");
        assert_eq!(v["error"]["message"], "bad token");
    }
    // Other connector errors: 502 connector_error.
    let (s, v, _) = w
        .create_integration(
            &w.fake,
            &w.owner,
            w.create_body(&w.fake),
            |_| op_err("not_found", "no such tenant"),
            1,
        )
        .await;
    assert_eq!(s, 502, "{v}");
    assert_eq!(v["error"]["code"], "connector_error");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn integration_create_timeout_is_503_and_removes_row() {
    let w = W::new().await;
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/orgs/{}/channel-integrations", w.org),
            Some(w.create_body(&w.fake)),
        )
        .await;
    assert_eq!(s, 503, "{v}");
    assert_eq!(v["error"]["code"], "connector_unavailable");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    // The abandoned op is not left claimable.
    let ops = w.ops_of("credentials.validate").await;
    assert!(ops.iter().all(|o| o.3 != "pending"), "{ops:?}");
}

#[tokio::test]
async fn integration_tenant_conflict_is_409() {
    let w = W::new().await;
    w.integration(&w.fake, "T1").await;
    // Same tenant again (same org)...
    let (s, v, _) = w
        .create_integration(
            &w.fake,
            &w.owner,
            w.create_body(&w.fake),
            |_| validate_ok("T1"),
            1,
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "tenant_already_bound");
    // ...and from another organization.
    let org2 = w.h.org().await;
    let o2 = w.h.user("o2@x.io", false).await;
    w.h.member(org2, o2, "owner", "active").await;
    let c2 = cookie(&w.h.good_session(o2).await.1);
    let path2 = format!("/v1/orgs/{org2}/channel-integrations");
    let ((s, v), _) = tokio::join!(
        w.call(&c2, Method::POST, &path2, Some(w.create_body(&w.fake)),),
        w.serve(&w.fake, 1, |_| validate_ok("T1"))
    );
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "tenant_already_bound");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn verified_email_autolink_setting_is_validated_and_audited() {
    let w = W::new().await;
    // Audit writes are fire-and-forget: wait (briefly) for the expected count.
    let audits = |action: &'static str, want: i64| {
        let pool = w.h.pool().clone();
        async move {
            let mut n = 0;
            for _ in 0..60 {
                n = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log WHERE action=$1")
                    .bind(action)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                if n >= want {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            n
        }
    };
    // A connector without `email_verified` cannot be switched to auto-linking.
    let mut body = w.create_body(&w.manual);
    body["config"]["verified_email_autolink"] = json!(true);
    let (s, v, _) = w
        .create_integration(&w.manual, &w.owner, body, |_| validate_ok("TM"), 1)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("autolink_unsupported")),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "the refused integration is not kept");
    // Default off: absent means off and nothing is audited.
    let id = w.integration(&w.fake, "T1").await;
    let path = format!("/v1/channel-integrations/{id}");
    let (_, v) = w.call(&w.owner, Method::GET, &path, None).await;
    assert!(v["config"].get("verified_email_autolink").is_none(), "{v}");
    assert_eq!(audits("channel_integration.autolink_enable", 0).await, 0);
    // Not a boolean: 400 before any connector round trip.
    let (s, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"config": {"verified_email_autolink": "yes"}})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    // Plain members cannot change it.
    let patch = json!({"config": {"verified_email_autolink": true}});
    assert_eq!(
        w.call(&w.plain, Method::PATCH, &path, Some(patch.clone()))
            .await
            .0,
        403
    );
    // An org admin enables it (re-validated), audited; then disables it, audited.
    let ((s, v), _) = tokio::join!(
        w.call(&w.admin, Method::PATCH, &path, Some(patch)),
        w.serve(&w.fake, 1, |_| validate_ok("T1"))
    );
    assert_eq!(
        (s, v["config"]["verified_email_autolink"].as_bool()),
        (200, Some(true)),
        "{v}"
    );
    assert_eq!(audits("channel_integration.autolink_enable", 1).await, 1);
    let ((s, v), _) = tokio::join!(
        w.call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"config": {"verified_email_autolink": false}}))
        ),
        w.serve(&w.fake, 1, |_| validate_ok("T1"))
    );
    assert_eq!(
        (s, v["config"]["verified_email_autolink"].as_bool()),
        (200, Some(false)),
        "{v}"
    );
    assert_eq!(audits("channel_integration.autolink_disable", 1).await, 1);
}

#[tokio::test]
async fn integration_patch_and_delete() {
    let w = W::new().await;
    let id = w.integration(&w.fake, "T1").await;
    let path = format!("/v1/channel-integrations/{id}");
    // Disable / enable.
    let (s, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("disabled")), "{v}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &path,
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");
    assert_eq!(
        w.call(
            &w.plain,
            Method::PATCH,
            &path,
            Some(json!({"status": "active"}))
        )
        .await
        .0,
        403
    );
    // Re-validated credential replacement.
    let before: i64 = sqlx::query_scalar("SELECT credentials_version FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let ((s, v), ops) = tokio::join!(
        w.call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"credentials": {"token": "NEW-secret-9"}, "config": {"region": "us"}}))
        ),
        w.serve(&w.fake, 1, |op| {
            assert_eq!(op["payload"]["credentials"]["token"], "NEW-secret-9");
            validate_ok("T1")
        })
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(ops.len(), 1);
    assert_eq!(v["config"]["region"], "us");
    assert!(!v.to_string().contains("NEW-secret-9"));
    let after: i64 = sqlx::query_scalar("SELECT credentials_version FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(after, before + 1);
    // Credentials of another tenant are rejected and change nothing.
    let ((s, v), _) = tokio::join!(
        w.call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"credentials": {"token": "other"}}))
        ),
        w.serve(&w.fake, 1, |_| validate_ok("T-OTHER"))
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("credentials_invalid")),
        "{v}"
    );
    let (_, v) = w.call(&w.owner, Method::GET, &path, None).await;
    assert_eq!(v["tenant_ref"], "T1");
    // Connector rejects re-validation: 422, the integration stays.
    let ((s, _), _) = tokio::join!(
        w.call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"config": {"region": "x"}}))
        ),
        w.serve(&w.fake, 1, |_| op_err("invalid", "no"))
    );
    assert_eq!(s, 422);
    assert_eq!(w.call(&w.owner, Method::GET, &path, None).await.0, 200);

    assert_eq!(w.call(&w.plain, Method::DELETE, &path, None).await.0, 403);
    assert_eq!(w.call(&w.admin, Method::DELETE, &path, None).await.0, 204);
    assert_eq!(w.call(&w.owner, Method::GET, &path, None).await.0, 404);
}

// ------------------------------------------------------------------- presences

#[tokio::test]
async fn presence_shared_attach_detach() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let etag0 = w.feed_etag(&w.fake).await;
    let gen0 = w.sync_gen(agent).await;

    // can_use is enough; an outsider and a non-bound member cannot see the agent.
    assert_eq!(w.attach(&w.outsider, agent, integ, "shared").await.0, 404);
    assert_eq!(w.attach(&w.plain, agent, integ, "shared").await.0, 404);
    let (s, v) = w.attach(&w.wsmember, agent, integ, "shared").await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "active");
    assert_eq!(v["mode"], "shared");
    assert_eq!(v["handle"], "daneel");
    assert_eq!(v["connector_id"], "fake");
    let pid = v["id"].as_str().unwrap().to_owned();
    assert!(w.sync_gen(agent).await > gen0, "home sync bumped");
    // The config feed changed with the presence.
    assert_ne!(w.feed_etag(&w.fake).await, etag0);
    // Duplicate attach.
    let (s, v) = w.attach(&w.wsmember, agent, integ, "shared").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("conflict")),
        "{v}"
    );
    // Listing.
    let (s, v) = w
        .call(
            &w.wsmember,
            Method::GET,
            &format!("/v1/agents/{agent}/presences"),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    // Detach: no deprovision op for a shared presence.
    let gen1 = w.sync_gen(agent).await;
    let (s, _) = w
        .call(
            &w.wsmember,
            Method::DELETE,
            &format!("/v1/presences/{pid}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert!(w.ops_of("presence.deprovision").await.is_empty());
    assert!(w.sync_gen(agent).await > gen1);
    assert_eq!(
        w.call(
            &w.wsmember,
            Method::DELETE,
            &format!("/v1/presences/{pid}"),
            None
        )
        .await
        .0,
        404
    );
}

#[tokio::test]
async fn presence_no_shared_bot_and_unsupported_mode() {
    let w = W::new().await;
    let agent = w.agent("a1").await;
    // An integration without a shared bot.
    let (s, v, _) = w
        .create_integration(
            &w.fake,
            &w.owner,
            w.create_body(&w.fake),
            |_| {
                json!({"outcome": "ok", "data": {"tenant_ref": "T2", "tenant_label": "NoBot",
                "capabilities": {"presence_dedicated": false}}})
            },
            1,
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let integ: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert!(v["shared_bot"].is_null());
    let (s, v) = w.attach(&w.owner, agent, integ, "shared").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("no_shared_bot")),
        "{v}"
    );
    // Narrowed by the integration: no dedicated bots.
    let (s, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("mode_unsupported")),
        "{v}"
    );
    // An integration of another org is not visible.
    let other_org = w.h.org().await;
    sqlx::query("UPDATE channel_integrations SET org_id=$1")
        .bind(other_org)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(w.attach(&w.owner, agent, integ, "shared").await.0, 400);
}

#[tokio::test]
async fn presence_dedicated_provision_and_detach() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let (s, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "pending_setup");
    let pid: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    let ops = w
        .serve(&w.fake, 1, |op| {
            assert_eq!(op["kind"], "presence.provision");
            assert_eq!(op["payload"]["agent"]["handle"], "a1");
            json!({"outcome": "ok", "data": {
                "platform_user_ref": "BOT-9", "handle": "a1-bot", "credentials": {"bot_token": SECRET}}})
        })
        .await;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["presence_id"], json!(pid));
    let (s, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/agents/{agent}/presences"),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["status"], "active", "{v}");
    assert_eq!(v["items"][0]["handle"], "a1-bot");
    assert!(!v.to_string().contains(SECRET));
    // The connector's feed carries the unsealed presence credentials.
    let (_, _, feed) = plane(
        &w.h,
        &w.fake.tok,
        Some(w.fake.replica),
        Method::GET,
        "/config",
        None,
    )
    .await;
    assert_eq!(feed["presences"][0]["credentials"]["bot_token"], SECRET);

    // Detach enqueues a deprovision op and removes the row.
    let (s, _) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/presences/{pid}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let d = w.ops_of("presence.deprovision").await;
    assert_eq!(d.len(), 1);
    assert_eq!(
        (d[0].1, d[0].2, d[0].3.as_str()),
        (Some(integ), Some(pid), "pending")
    );
    assert_eq!(d[0].4["presence_id"], json!(pid));
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_presences")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn presence_provision_failure_needs_attention_then_reattach() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let (_, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    let pid = v["id"].as_str().unwrap().to_owned();
    w.serve(&w.fake, 1, |_| op_err("forbidden", "cannot create bots"))
        .await;
    let (_, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/agents/{agent}/presences"),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["status"], "needs_attention", "{v}");
    assert_eq!(v["items"][0]["status_reason"], "forbidden");
    // Re-attach re-runs provisioning.
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/presences/{pid}/reattach"),
            None,
        )
        .await;
    assert_eq!(
        (s, v["status"].as_str()),
        (200, Some("pending_setup")),
        "{v}"
    );
    w.serve(&w.fake, 1, |_| {
        json!({"outcome": "ok", "data": {
        "platform_user_ref": "BOT-1", "handle": "h", "credentials": {"t": "x"}}})
    })
    .await;
    let (_, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/agents/{agent}/presences"),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["status"], "active", "{v}");
    assert_eq!(w.ops_of("presence.provision").await.len(), 2);
}

#[tokio::test]
async fn presence_manual_setup_fields_are_validated() {
    let w = W::new().await;
    let integ = w.integration(&w.manual, "M1").await;
    let agent = w.agent("a1").await;
    let path = format!("/v1/agents/{agent}/presences");
    // Missing / unknown fields are 400/422 before anything is queued.
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            &path,
            Some(json!({"integration_id": integ, "mode": "dedicated"})),
        )
        .await;
    assert_eq!(s, 400, "required field missing");
    // Connector rejects the token: 422, no presence left behind.
    let body = json!({"integration_id": integ, "mode": "dedicated", "fields": {"token": SECRET}});
    let ((s, v), _) = tokio::join!(
        w.call(&w.owner, Method::POST, &path, Some(body.clone())),
        w.serve(&w.manual, 1, |op| {
            assert!(op["presence_id"].is_string());
            assert_eq!(op["payload"]["credentials"]["token"], SECRET);
            op_err("invalid", "token rejected")
        })
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("credentials_invalid")),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_presences")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    // Accepted.
    let ((s, v), _) = tokio::join!(
        w.call(&w.owner, Method::POST, &path, Some(body)),
        w.serve(&w.manual, 1, |_| validate_ok("M1"))
    );
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "active");
    assert!(!v.to_string().contains(SECRET));
    for (_, _, _, _, payload) in w.ops_of("credentials.validate").await {
        assert!(!payload.to_string().contains(SECRET));
    }
    // A wrong tenant answer is not accepted.
    let agent2 = w.agent("a2").await;
    let path2 = format!("/v1/agents/{agent2}/presences");
    let ((s, v), _) = tokio::join!(
        w.call(
            &w.owner,
            Method::POST,
            &path2,
            Some(json!({"integration_id": integ, "mode": "dedicated", "fields": {"token": "t"}})),
        ),
        w.serve(&w.manual, 1, |_| validate_ok("M-OTHER"))
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("credentials_invalid")),
        "{v}"
    );
}

#[tokio::test]
async fn agent_rename_enqueues_presence_rename() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let (_, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    let pid = v["id"].as_str().unwrap().to_owned();
    w.serve(&w.fake, 1, |_| {
        json!({"outcome": "ok", "data": {
        "platform_user_ref": "BOT-1", "handle": "h", "credentials": {"t": "x"}}})
    })
    .await;
    let (s, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &format!("/v1/agents/{agent}"),
            Some(json!({"name": "Renamed"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let r = w.ops_of("presence.rename").await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].4, json!({"presence_id": pid, "name": "Renamed"}));
}

#[tokio::test]
async fn avatar_change_enqueues_presence_set_icon_and_feeds_url() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let (_, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    let pid = v["id"].as_str().unwrap().to_owned();
    // A dedicated presence is only synced once active.
    w.serve(&w.fake, 1, |_| {
        json!({"outcome": "ok", "data": {
        "platform_user_ref": "BOT-1", "handle": "h", "credentials": {"t": "x"}}})
    })
    .await;
    let png = {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(32, 32))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    };
    let req = |m: Method| {
        http::Request::builder()
            .method(m)
            .uri(format!("/v1/agents/{agent}/avatar"))
            .header("cookie", &w.owner)
            .header("origin", ORIGIN)
    };
    use tower::ServiceExt;
    let res =
        w.h.app
            .clone()
            .oneshot(req(Method::PUT).body(axum::body::Body::from(png)).unwrap())
            .await
            .unwrap();
    assert_eq!(res.status(), 200);
    let r = w.ops_of("presence.set_icon").await;
    assert_eq!(r.len(), 1, "{r:?}");
    let url = r[0].4["avatar_url"].as_str().unwrap().to_owned();
    assert!(
        url.starts_with("http") && url.contains("/v1/public/avatars/"),
        "{url}"
    );
    assert_eq!(r[0].4["presence_id"], pid);

    // The connector config feed carries the same public URL.
    let (s, _, feed) = plane(
        &w.h,
        &w.fake.tok,
        Some(w.fake.replica),
        Method::GET,
        "/config",
        None,
    )
    .await;
    assert_eq!(s, 200, "{feed}");
    assert_eq!(feed["presences"][0]["agent"]["avatar_url"], url);

    // Removal queues a second op carrying null.
    let res =
        w.h.app
            .clone()
            .oneshot(req(Method::DELETE).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
    assert_eq!(res.status(), 204);
    let r = w.ops_of("presence.set_icon").await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].4["avatar_url"], Value::Null);
}

// ------------------------------------------------------------------- deletion

#[tokio::test]
async fn agent_delete_enqueues_deprovision() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let shared_agent = w.agent("a2").await;
    let (_, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    let pid: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    w.serve(&w.fake, 1, |_| {
        json!({"outcome": "ok", "data": {
        "platform_user_ref": "BOT-1", "handle": "h", "credentials": {"t": "x"}}})
    })
    .await;
    w.attach(&w.owner, shared_agent, integ, "shared").await;

    let (s, v) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/agents/{agent}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let d = w.ops_of("presence.deprovision").await;
    assert_eq!(d.len(), 1, "only the dedicated presence is deprovisioned");
    assert_eq!(
        (d[0].1, d[0].2, d[0].3.as_str()),
        (Some(integ), Some(pid), "pending")
    );
    // The connector receives it.
    let ops = w
        .serve(&w.fake, 1, |_| json!({"outcome": "ok", "data": {}}))
        .await;
    assert_eq!(ops[0]["kind"], "presence.deprovision");

    let (s, _) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/agents/{shared_agent}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(w.ops_of("presence.deprovision").await.len(), 1);
}

#[tokio::test]
async fn workspace_and_integration_delete_enqueue_deprovision() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    w.attach(&w.owner, a1, integ, "dedicated").await;
    // Workspace delete (typed confirmation).
    let slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id=$1")
        .bind(w.ws)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/workspaces/{}", w.ws),
            Some(json!({"confirm": slug})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(w.ops_of("presence.deprovision").await.len(), 1);

    // Integration delete: the ops outlive the integration (no integration id).
    let ws2 = w.h.workspace(w.org).await;
    let a2 = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,'b1','b1','hosted','fake')")
        .bind(a2).bind(w.org).bind(ws2).execute(w.h.pool()).await.unwrap();
    w.attach(&w.owner, a2, integ, "dedicated").await;
    let (s, _) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/channel-integrations/{integ}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    // The workspace's op named the integration, so it went with it; this one outlives it.
    let d = w.ops_of("presence.deprovision").await;
    assert_eq!(d.len(), 1);
    assert!(d[0].1.is_none() && d[0].3 == "pending");
    // Org delete. First let the connector take the pending deprovision (a queued op would
    // otherwise be served before the validation below).
    let drained = w
        .serve(&w.fake, 1, |_| json!({"outcome": "ok", "data": {}}))
        .await;
    assert_eq!(drained[0]["kind"], "presence.deprovision");
    let integ2 = w.integration(&w.fake, "T2").await;
    w.attach(&w.owner, a2, integ2, "dedicated").await;
    let org_slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id=$1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/orgs/{}", w.org),
            Some(json!({"confirm": org_slug})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(w.ops_of("presence.deprovision").await.len(), 2);
}

// -------------------------------------------------------------------- bindings

#[tokio::test]
async fn binding_patch_delete_need_can_own() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    w.attach(&w.owner, agent, integ, "shared").await;
    let bid = Uuid::now_v7();
    sqlx::query("INSERT INTO space_bindings (id, integration_id, space_ref, space_label, agent_id, bound_by_user_id) VALUES ($1,$2,'C1','#general',$3,$4)")
        .bind(bid).bind(integ).bind(agent).bind(w.users.owner).execute(w.h.pool()).await.unwrap();
    let etag0 = w.feed_etag(&w.fake).await;

    let list = format!("/v1/agents/{agent}/space-bindings");
    assert_eq!(w.call(&w.wsmember, Method::GET, &list, None).await.0, 403);
    assert_eq!(w.call(&w.outsider, Method::GET, &list, None).await.0, 404);
    let (s, v) = w.call(&w.wsowner, Method::GET, &list, None).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["space_label"], "#general");
    assert_eq!(v["items"][0]["listen"], "mentions");
    assert_eq!(v["items"][0]["bound_by"]["id"], json!(w.users.owner));

    let path = format!("/v1/space-bindings/{bid}");
    let patch = json!({"listen": "all", "followups_need_mention": true});
    assert_eq!(
        w.call(&w.wsmember, Method::PATCH, &path, Some(patch.clone()))
            .await
            .0,
        403
    );
    assert_eq!(
        w.call(&w.outsider, Method::PATCH, &path, Some(patch.clone()))
            .await
            .0,
        404
    );
    let (s, v) = w.call(&w.wsowner, Method::PATCH, &path, Some(patch)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["listen"].as_str(), v["followups_need_mention"].as_bool()),
        (Some("all"), Some(true))
    );
    assert_ne!(
        w.feed_etag(&w.fake).await,
        etag0,
        "feed changed with the binding"
    );

    assert_eq!(
        w.call(&w.wsmember, Method::DELETE, &path, None).await.0,
        403
    );
    assert_eq!(w.call(&w.wsowner, Method::DELETE, &path, None).await.0, 204);
    assert_eq!(w.call(&w.wsowner, Method::DELETE, &path, None).await.0, 404);
    assert_eq!(
        w.call(
            &w.wsowner,
            Method::PATCH,
            &path,
            Some(json!({"listen": "all"}))
        )
        .await
        .0,
        404
    );
}

// ----------------------------------------------------------------- user links

#[tokio::test]
async fn admin_user_links_and_already_linked() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let path = format!("/v1/channel-integrations/{integ}/user-links");
    let body = json!({"platform_user_ref": "U-1", "user_id": w.users.wsmember});
    assert_eq!(
        w.call(&w.plain, Method::POST, &path, Some(body.clone()))
            .await
            .0,
        403
    );
    let (s, v) = w
        .call(&w.admin, Method::POST, &path, Some(body.clone()))
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["method"], "admin");
    assert_eq!(v["platform_user_ref"], "U-1");
    assert_eq!(v["connector_id"], "fake");
    assert_eq!(v["user"]["id"], json!(w.users.wsmember));
    let link = v["id"].as_str().unwrap().to_owned();
    // The platform user is linked to at most one Aurora user.
    let (s, v) = w.call(&w.admin, Method::POST, &path, Some(body)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_linked")),
        "{v}"
    );
    let other = json!({"platform_user_ref": "U-1", "user_id": w.users.plain});
    assert_eq!(
        w.call(&w.admin, Method::POST, &path, Some(other)).await.1["error"]["code"],
        "already_linked"
    );
    // Only members of the org can be linked.
    let out = json!({"platform_user_ref": "U-2", "user_id": w.users.outsider});
    assert_eq!(
        w.call(&w.admin, Method::POST, &path, Some(out)).await.0,
        400
    );
    // Listing; people see their own links.
    let (s, v) = w.call(&w.owner, Method::GET, &path, None).await;
    assert_eq!((s, v["items"].as_array().map(Vec::len)), (200, Some(1)));
    let (s, v) = w
        .call(&w.wsmember, Method::GET, "/v1/me/channel-links", None)
        .await;
    assert_eq!(
        (s, v["items"].as_array().map(Vec::len)),
        (200, Some(1)),
        "{v}"
    );
    let (_, v) = w
        .call(&w.plain, Method::GET, "/v1/me/channel-links", None)
        .await;
    assert!(v["items"].as_array().unwrap().is_empty());
    // Not someone else's link.
    assert_eq!(
        w.call(
            &w.plain,
            Method::DELETE,
            &format!("/v1/me/channel-links/{link}"),
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        w.call(
            &w.plain,
            Method::DELETE,
            &format!("/v1/user-links/{link}"),
            None
        )
        .await
        .0,
        403
    );
    assert_eq!(
        w.call(
            &w.admin,
            Method::DELETE,
            &format!("/v1/user-links/{link}"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        w.call(
            &w.admin,
            Method::DELETE,
            &format!("/v1/user-links/{link}"),
            None
        )
        .await
        .0,
        404
    );
}

async fn link_url(w: &W, integ: Uuid, platform: &str) -> String {
    // The integration must be owned by the connector replica (link-start is owner-only).
    sqlx::query("UPDATE channel_integrations SET owner_replica_id=$1, owner_lease_until=now() + interval '1 hour' WHERE id=$2")
        .bind(w.fake.replica)
        .bind(integ)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, _, v) = plane(
        &w.h,
        &w.fake.tok,
        Some(w.fake.replica),
        Method::POST,
        "/link-start",
        Some(json!({"integration_id": integ, "platform_user_ref": platform})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let url = v["url"].as_str().unwrap();
    url.split("token=").nth(1).unwrap().to_owned()
}

#[tokio::test]
async fn confirm_flow_expired_used_and_cross_user() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let confirm = |who: &str, token: &str| {
        let who = who.to_owned();
        let token = token.to_owned();
        let w = &w;
        async move {
            w.call(
                &who,
                Method::POST,
                "/v1/channel-links/confirm",
                Some(json!({"token": token})),
            )
            .await
        }
    };
    let t1 = link_url(&w, integ, "U-1").await;
    // Not a member of the org: the token stays usable.
    assert_eq!(confirm(&w.outsider, &t1).await.0, 404);
    let (s, v) = confirm(&w.wsmember, &t1).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["method"], "sign_in_link");
    assert_eq!(v["platform_user_ref"], "U-1");
    assert_eq!(v["user"]["id"], json!(w.users.wsmember));
    // Used.
    let (s, v) = confirm(&w.wsmember, &t1).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (410, Some("link_expired")),
        "{v}"
    );
    // Expired.
    let t2 = link_url(&w, integ, "U-2").await;
    sqlx::query("UPDATE link_requests SET expires_at = now() - interval '1 second'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = confirm(&w.wsmember, &t2).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (410, Some("link_expired")),
        "{v}"
    );
    // Unknown token.
    assert_eq!(confirm(&w.wsmember, "aui_unknown").await.0, 410);
    // Cross-user: the platform user already belongs to another Aurora user.
    let t3 = link_url(&w, integ, "U-1").await;
    let (s, v) = confirm(&w.plain, &t3).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_linked")),
        "{v}"
    );
    // The same user confirming again is idempotent and consumes the token.
    let (s, _) = confirm(&w.wsmember, &t3).await;
    assert_eq!(s, 200);
    assert_eq!(confirm(&w.wsmember, &t3).await.0, 410);
    // Session only.
    let (_, key) = w.h.api_key(w.users.wsmember, w.org, None, false).await;
    let (s, _, v) =
        w.h.send(
            Method::POST,
            "/v1/channel-links/confirm",
            &[("authorization", bearer(&key).as_str())],
            Some(json!({"token": "aui_x"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("session_required"))
    );
}

// ------------------------------------------------------------------ DM defaults

#[tokio::test]
async fn dm_defaults_require_reachable_agent() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let no_presence = w.agent("a2").await;
    w.attach(&w.owner, agent, integ, "shared").await;
    let put = |who: &str, agent: Uuid| {
        let who = who.to_owned();
        let w = &w;
        async move {
            w.call(
                &who,
                Method::PUT,
                "/v1/me/dm-defaults",
                Some(json!({"integration_id": integ, "agent_id": agent})),
            )
            .await
        }
    };
    // Needs a user link first.
    assert_eq!(put(&w.wsmember, agent).await.0, 404);
    w.call(
        &w.admin,
        Method::POST,
        &format!("/v1/channel-integrations/{integ}/user-links"),
        Some(json!({"platform_user_ref": "U-1", "user_id": w.users.wsmember})),
    )
    .await;
    w.call(
        &w.admin,
        Method::POST,
        &format!("/v1/channel-integrations/{integ}/user-links"),
        Some(json!({"platform_user_ref": "U-2", "user_id": w.users.plain})),
    )
    .await;
    let (s, v) = w
        .call(&w.wsmember, Method::GET, "/v1/me/dm-defaults", None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        v["items"],
        json!([{"integration_id": integ, "platform_user_ref": "U-1", "agent_id": null}])
    );

    let (s, v) = put(&w.wsmember, agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["agent_id"], json!(agent));
    let (_, v) = w
        .call(&w.wsmember, Method::GET, "/v1/me/dm-defaults", None)
        .await;
    assert_eq!(v["items"][0]["agent_id"], json!(agent));
    // No active shared presence.
    let (s, v) = put(&w.wsmember, no_presence).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("agent_not_reachable")),
        "{v}"
    );
    // A person without can_use on the agent's workspace...
    let (s, v) = put(&w.plain, agent).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("agent_not_reachable")),
        "{v}"
    );
    // ...unless they were approved as a person.
    sqlx::query("INSERT INTO reach_grants (id, agent_id, integration_id, subject_kind, subject_ref, subject_label, state) VALUES ($1,$2,$3,'person','U-2','Una','approved')")
        .bind(Uuid::now_v7()).bind(agent).bind(integ).execute(w.h.pool()).await.unwrap();
    assert_eq!(put(&w.plain, agent).await.0, 200);
    // Unknown agent.
    assert_eq!(
        put(&w.plain, Uuid::now_v7()).await.1["error"]["code"],
        "agent_not_reachable"
    );
    // Unlinking drops the default.
    let (_, v) = w
        .call(&w.wsmember, Method::GET, "/v1/me/channel-links", None)
        .await;
    let link = v["items"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        w.call(
            &w.wsmember,
            Method::DELETE,
            &format!("/v1/me/channel-links/{link}"),
            None
        )
        .await
        .0,
        204
    );
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dm_defaults WHERE platform_user_ref='U-1'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0);
}

// ------------------------------------------------------------------------ reach

#[tokio::test]
async fn reach_decision_and_stale_revision() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let gid = Uuid::now_v7();
    sqlx::query("INSERT INTO reach_grants (id, agent_id, integration_id, subject_kind, subject_ref, subject_label, state) VALUES ($1,$2,$3,'space','C9','#ops','pending')")
        .bind(gid).bind(agent).bind(integ).execute(w.h.pool()).await.unwrap();

    let (s, v) = w
        .call(
            &w.wsmember,
            Method::GET,
            &format!("/v1/agents/{agent}/reach"),
            None,
        )
        .await;
    assert_eq!(s, 200, "can_use may list");
    assert_eq!(v["items"][0]["state"], "pending");
    assert_eq!(v["items"][0]["subject_kind"], "space");
    assert_eq!(v["items"][0]["revision"], 0);
    assert_eq!(
        w.call(
            &w.outsider,
            Method::GET,
            &format!("/v1/agents/{agent}/reach"),
            None
        )
        .await
        .0,
        404
    );

    let path = format!("/v1/reach-grants/{gid}/decision");
    let approve = json!({"decision": "approve", "listen": "all", "revision": 0});
    assert_eq!(
        w.call(&w.wsmember, Method::POST, &path, Some(approve.clone()))
            .await
            .0,
        403
    );
    assert_eq!(
        w.call(&w.outsider, Method::POST, &path, Some(approve.clone()))
            .await
            .0,
        404
    );
    let (s, v) = w
        .call(&w.wsowner, Method::POST, &path, Some(approve.clone()))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["state"].as_str(),
            v["listen"].as_str(),
            v["revision"].as_i64()
        ),
        (Some("approved"), Some("all"), Some(1))
    );
    assert!(v["decided_by"]["id"].is_string());
    assert!(v["decided_at"].is_string());
    // Stale.
    let (s, v) = w.call(&w.wsowner, Method::POST, &path, Some(approve)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_revision")),
        "{v}"
    );
    // Block with the current revision; listen defaults to the current value.
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &path,
            Some(json!({"decision": "block", "revision": 1})),
        )
        .await;
    assert_eq!(
        (s, v["state"].as_str(), v["listen"].as_str()),
        (200, Some("blocked"), Some("all")),
        "{v}"
    );
    assert_eq!(
        w.call(
            &w.owner,
            Method::POST,
            &format!("/v1/reach-grants/{}/decision", Uuid::now_v7()),
            Some(json!({"decision": "block", "revision": 0}))
        )
        .await
        .0,
        404
    );
}

// ------------------------------------------------------------ credential_dead

#[tokio::test]
async fn credential_dead_disables_presence_and_integration() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let (_, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    let pid: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    w.serve(&w.fake, 1, |_| {
        json!({"outcome": "ok", "data": {
        "platform_user_ref": "BOT-1", "handle": "h", "credentials": {"t": "x"}}})
    })
    .await;
    // A presence-level op comes back credential_dead.
    let gen0 = w.sync_gen(agent).await;
    let mut tx = w.h.pool().begin().await.unwrap();
    aurora_api::domains::connectors::ops::enqueue(
        &mut tx,
        aurora_api::domains::connectors::ops::NewOp {
            kind: "status.set".to_owned(),
            connector_id: None,
            integration_id: Some(integ),
            presence_id: Some(pid),
            payload: json!({}),
            dedupe_key: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    w.serve(&w.fake, 1, |_| op_err("credential_dead", "token revoked"))
        .await;
    let (_, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/agents/{agent}/presences"),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["status"], "disabled", "{v}");
    assert_eq!(v["items"][0]["status_reason"], "credential_dead");
    assert!(w.sync_gen(agent).await > gen0);
    // Operators are told in the audit log.
    let mut seen: i64 = 0;
    for _ in 0..20 {
        seen = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action='channel.credential_dead' AND org_id=$1",
        )
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        if seen > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(seen, 1);

    // Integration-level.
    let mut tx = w.h.pool().begin().await.unwrap();
    aurora_api::domains::connectors::ops::enqueue(
        &mut tx,
        aurora_api::domains::connectors::ops::NewOp {
            kind: "status.set".to_owned(),
            connector_id: None,
            integration_id: Some(integ),
            presence_id: None,
            payload: json!({}),
            dedupe_key: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    w.serve(&w.fake, 1, |_| op_err("credential_dead", "revoked"))
        .await;
    let (_, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/channel-integrations/{integ}"),
            None,
        )
        .await;
    assert_eq!(
        (v["status"].as_str(), v["status_reason"].as_str()),
        (Some("disabled"), Some("credential_dead"))
    );
    // An admin recovers it by re-validating credentials.
    let ipath = format!("/v1/channel-integrations/{integ}");
    let ((s, v), _) = tokio::join!(
        w.call(
            &w.owner,
            Method::PATCH,
            &ipath,
            Some(json!({"credentials": {"token": "fresh"}}))
        ),
        w.serve(&w.fake, 1, |_| validate_ok("T1"))
    );
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");
}

// ------------------------------------------------------------- presence options (C-39)

#[tokio::test]
async fn presence_options_for_members_are_metadata_only() {
    let w = W::new().await;
    let agent = w.agent("a1").await;
    let shared = w.integration(&w.fake, "T1").await;
    // Narrowed: no dedicated bots, and no shared bot.
    let (s, v, _) = w
        .create_integration(
            &w.manual,
            &w.owner,
            w.create_body(&w.manual),
            |_| {
                json!({"outcome": "ok", "data": {"tenant_ref": "T2", "tenant_label": "NoBot",
                "capabilities": {"presence_dedicated": false}}})
            },
            1,
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let nobot: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let path = format!("/v1/agents/{agent}/presence-options");

    // Non-members and members without access to the agent get 404.
    assert_eq!(w.call(&w.outsider, Method::GET, &path, None).await.0, 404);
    assert_eq!(w.call(&w.plain, Method::GET, &path, None).await.0, 404);

    // A can_use member (not an org admin) sees the options without credentials or config.
    let (s, v) = w.call(&w.wsmember, Method::GET, &path, None).await;
    assert_eq!(s, 200, "{v}");
    let raw = v.to_string();
    assert!(!raw.contains(SECRET) && !raw.contains("credentials") && !raw.contains("region"));
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{v}");
    let by = |id: Uuid| {
        items
            .iter()
            .find(|i| i["integration_id"] == id.to_string())
            .unwrap()
    };
    let a = by(shared);
    assert_eq!(a["connector_id"], "fake");
    assert_eq!(a["tenant_label"], "Tenant T1");
    let mut modes: Vec<&str> = a["modes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    modes.sort_unstable();
    assert_eq!(modes, ["dedicated", "shared"]);
    assert_eq!(a["attached"], false);
    let b = by(nobot);
    assert_eq!(b["modes"], json!([]));
    assert_eq!(b["presence_setup"], json!({}));

    // Attaching flips `attached`.
    assert_eq!(w.attach(&w.wsmember, agent, shared, "shared").await.0, 201);
    let (_, v) = w.call(&w.wsmember, Method::GET, &path, None).await;
    let items = v["items"].as_array().unwrap();
    let a = items
        .iter()
        .find(|i| i["integration_id"] == shared.to_string())
        .unwrap();
    assert_eq!(a["attached"], true);
}

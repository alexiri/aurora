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

/// The test keyring's key (see `common::config`).
fn base64_key() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
}

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
    wsowner: Uuid,
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
        Self::build(None).await
    }

    /// As `new`, with a memory store as the default secret store (so a test can make it
    /// unavailable and count its managed values).
    async fn with_memory_store() -> (W, std::sync::Arc<aurora_secrets::MemoryStore>) {
        let mem = std::sync::Arc::new(aurora_secrets::MemoryStore::new("memory"));
        (Self::build(Some(mem.clone())).await, mem)
    }

    async fn build(mem: Option<std::sync::Arc<aurora_secrets::MemoryStore>>) -> W {
        let mut h = Harness::new().await;
        if let Some(mem) = mem {
            let keyring = std::sync::Arc::new(
                aurora_crypto::Keyring::parse(&format!("k1:{}", base64_key()), "k1").unwrap(),
            );
            let mut sr =
                aurora_secrets::StoreRegistry::from_config(&h.state.config.secrets, keyring)
                    .unwrap();
            sr.register(mem);
            sr.set_default("memory").unwrap();
            h.state = h
                .state
                .clone()
                .with_vault(std::sync::Arc::new(aurora_secrets::SecretVault::new(sr)));
            h.app = test_router(h.state.clone());
        }
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
                wsowner: wso,
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

    /// A dedicated presence whose provisioning completed with the bot `bot_ref`.
    async fn dedicated(&self, agent: Uuid, integ: Uuid, bot_ref: &str) -> Uuid {
        let (s, v) = self.attach(&self.owner, agent, integ, "dedicated").await;
        assert_eq!(s, 201, "{v}");
        let pid: Uuid = v["id"].as_str().unwrap().parse().unwrap();
        let bot = bot_ref.to_owned();
        let served = self
            .serve(&self.fake, 1, move |_| {
                json!({"outcome": "ok", "data": {
                    "platform_user_ref": bot, "handle": "h", "credentials": {"t": "x"}}})
            })
            .await;
        assert_eq!(served[0]["kind"], "presence.provision");
        pid
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

/// FUP-M7 (`13 §4.2`): `presence.provision` carries `agent.avatar_url`, the absolute public URL
/// that `presence.set_icon` and the config feed use, and the connector protocol accepts it.
#[tokio::test]
async fn presence_provision_carries_the_agent_avatar_url() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    sqlx::query(
        "UPDATE agents SET avatar_blob_key = 'b', avatar_public_key = 'pubkey123' WHERE id = $1",
    )
    .bind(agent)
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) = w.attach(&w.owner, agent, integ, "dedicated").await;
    assert_eq!(s, 201, "{v}");
    let ops = w
        .serve(&w.fake, 1, |_| {
            json!({"outcome": "ok", "data": {
                "platform_user_ref": "BOT-1", "handle": "h", "credentials": {"t": "x"}}})
        })
        .await;
    let payload = &ops[0]["payload"];
    let url = payload["agent"]["avatar_url"].as_str().unwrap();
    assert!(
        url.starts_with("http") && url.ends_with("/v1/public/avatars/pubkey123"),
        "{url}"
    );
    let op: aurora_protocol::connector::PresenceProvisionOp =
        serde_json::from_value(payload.clone()).unwrap();
    assert_eq!(op.agent.avatar_url.as_deref(), Some(url));
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
            // No avatar: the field is present and null (`13 §4.2`).
            assert_eq!(op["payload"]["agent"]["avatar_url"], Value::Null);
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
    // Self-contained (RFE-024): no integration id, the payload names the bot.
    assert_eq!(
        (d[0].1, d[0].2, d[0].3.as_str()),
        (None, Some(pid), "pending")
    );
    assert_eq!(d[0].4["presence_id"], json!(pid));
    assert_eq!(d[0].4["reason"], "detached");
    assert_eq!(d[0].4["platform_user_ref"], "BOT-9");
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
        (None, Some(pid), "pending"),
        "the op names no integration (self-contained)"
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
    w.dedicated(a1, integ, "BOT-1").await;
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
    // The connector takes it (a queued op would otherwise be served before the next provision).
    w.serve(&w.fake, 1, |_| json!({"outcome": "ok", "data": {}}))
        .await;

    // Integration delete: the ops outlive the integration (no integration id).
    let ws2 = w.h.workspace(w.org).await;
    let a2 = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,'b1','b1','hosted','fake')")
        .bind(a2).bind(w.org).bind(ws2).execute(w.h.pool()).await.unwrap();
    w.dedicated(a2, integ, "BOT-2").await;
    let (s, _) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/channel-integrations/{integ}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    // Both ops outlive the integration (no integration id).
    let d = w.ops_of("presence.deprovision").await;
    assert_eq!(d.len(), 2);
    assert!(d.iter().all(|o| o.1.is_none()));
    assert_eq!(d[1].3, "pending");
    // Org delete. First let the connector take the pending deprovision (a queued op would
    // otherwise be served before the validation below).
    let drained = w
        .serve(&w.fake, 1, |_| json!({"outcome": "ok", "data": {}}))
        .await;
    assert_eq!(drained[0]["kind"], "presence.deprovision");
    let integ2 = w.integration(&w.fake, "T2").await;
    w.dedicated(a2, integ2, "BOT-3").await;
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
    assert_eq!(w.ops_of("presence.deprovision").await.len(), 3);
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
    let confirm = |who: &str, token: &str, platform: &str| {
        let who = who.to_owned();
        let token = token.to_owned();
        let platform = platform.to_owned();
        let w = &w;
        async move {
            w.call(
                &who,
                Method::POST,
                "/v1/channel-links/confirm",
                Some(json!({"token": token, "platform_user_ref": platform})),
            )
            .await
        }
    };
    let t1 = link_url(&w, integ, "U-1").await;
    // Not a member of the org: the token stays usable.
    assert_eq!(confirm(&w.outsider, &t1, "U-1").await.0, 404);
    let (s, v) = confirm(&w.wsmember, &t1, "U-1").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["method"], "sign_in_link");
    assert_eq!(v["platform_user_ref"], "U-1");
    assert_eq!(v["user"]["id"], json!(w.users.wsmember));
    // Used.
    let (s, v) = confirm(&w.wsmember, &t1, "U-1").await;
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
    let (s, v) = confirm(&w.wsmember, &t2, "U-2").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (410, Some("link_expired")),
        "{v}"
    );
    // Unknown token.
    assert_eq!(confirm(&w.wsmember, "aui_unknown", "U-1").await.0, 410);
    // Cross-user: the platform user already belongs to another Aurora user.
    let t3 = link_url(&w, integ, "U-1").await;
    let (s, v) = confirm(&w.plain, &t3, "U-1").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_linked")),
        "{v}"
    );
    // The same user confirming again is idempotent and consumes the token.
    let (s, _) = confirm(&w.wsmember, &t3, "U-1").await;
    assert_eq!(s, 200);
    assert_eq!(confirm(&w.wsmember, &t3, "U-1").await.0, 410);
    // Session only.
    let (_, key) = w.h.api_key(w.users.wsmember, w.org, None, false).await;
    for (method, uri, body) in [
        (
            Method::POST,
            "/v1/channel-links/confirm",
            Some(json!({"token": "aui_x", "platform_user_ref": "U-1"})),
        ),
        (Method::GET, "/v1/channel-links/preview?token=aui_x", None),
    ] {
        let (s, _, v) =
            w.h.send(
                method,
                uri,
                &[("authorization", bearer(&key).as_str())],
                body,
            )
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("session_required")),
            "{uri}"
        );
    }
}

/// FIX-112 (CHAN-1): the confirm page can show which chat account a link binds, and confirming
/// repeats that account: a request naming another one is refused and leaves the token unused.
#[tokio::test]
async fn link_preview_shows_the_chat_account_and_confirm_must_repeat_it() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let tok = link_url(&w, integ, "U-77").await;
    sqlx::query("UPDATE link_requests SET platform_display_name='Mallory Chat', platform_email_hint='m@evil.example'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let uri = format!("/v1/channel-links/preview?token={tok}");
    let (s, v) = w.call(&w.wsmember, Method::GET, &uri, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["platform_user_ref"], "U-77");
    assert_eq!(v["platform_display_name"], "Mallory Chat");
    assert_eq!(v["platform_email_hint"], "m@evil.example");
    assert_eq!(v["integration_id"], json!(integ));
    assert_eq!(v["connector_id"], "fake");
    // Not a member of the org: nothing is revealed.
    assert_eq!(w.call(&w.outsider, Method::GET, &uri, None).await.0, 404);
    // Unknown token.
    let (s, v) = w
        .call(
            &w.wsmember,
            Method::GET,
            "/v1/channel-links/preview?token=aui_nope",
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (410, Some("link_expired"))
    );
    // A confirm that repeats another account is refused and consumes nothing.
    for body in [
        json!({"token": tok, "platform_user_ref": "U-1"}),
        json!({"token": tok, "platform_user_ref": ""}),
    ] {
        let (s, v) = w
            .call(
                &w.wsmember,
                Method::POST,
                "/v1/channel-links/confirm",
                Some(body),
            )
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("link_reference_mismatch")),
            "{v}"
        );
    }
    let linked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_links WHERE platform_user_ref='U-77'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(linked, 0);
    let (s, v) = w
        .call(
            &w.wsmember,
            Method::POST,
            "/v1/channel-links/confirm",
            Some(json!({"token": tok, "platform_user_ref": "U-77"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // Used tokens no longer preview.
    assert_eq!(w.call(&w.wsmember, Method::GET, &uri, None).await.0, 410);
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

/// (FIX-111 4) A person reach grant approved with `expires_in` is ineffective after it and the
/// 60 s sweep marks it `expired`; one approved without an expiry persists; `expires_in` is
/// refused on a block and out of bounds.
#[tokio::test]
async fn reach_grant_expiry_is_set_by_the_decision_and_swept() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    let mk = |subject: &'static str| {
        let (pool, id) = (w.h.pool().clone(), Uuid::now_v7());
        async move {
            sqlx::query("INSERT INTO reach_grants (id, agent_id, integration_id, subject_kind, subject_ref, subject_label, state) VALUES ($1,$2,$3,'person',$4,'p','pending')")
                .bind(id).bind(agent).bind(integ).bind(subject).execute(&pool).await.unwrap();
            id
        }
    };
    let (timed, plain) = (mk("U-timed").await, mk("U-plain").await);
    let effective = |who: &'static str| {
        let pool = w.h.pool().clone();
        async move {
            aurora_db::repo::reach_grants::approved_person_reach(
                &pool,
                agent,
                integ,
                &[who.to_owned()],
            )
            .await
            .unwrap()
        }
    };
    let path = |g: Uuid| format!("/v1/reach-grants/{g}/decision");
    for bad in [
        json!({"decision": "block", "revision": 0, "expires_in": 3600}),
        json!({"decision": "approve", "revision": 0, "expires_in": 5}),
        json!({"decision": "approve", "revision": 0, "expires_in": 999_999_999}),
    ] {
        let (s, v) = w
            .call(&w.wsowner, Method::POST, &path(timed), Some(bad))
            .await;
        assert_eq!(s, 400, "{v}");
    }
    let (s, v) = w
        .call(
            &w.wsowner,
            Method::POST,
            &path(timed),
            Some(json!({"decision": "approve", "revision": 0, "expires_in": 3600})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["expires_at"].is_string(), "{v}");
    let (s, v) = w
        .call(
            &w.wsowner,
            Method::POST,
            &path(plain),
            Some(json!({"decision": "approve", "revision": 0})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["expires_at"].is_null(), "{v}");
    assert!(effective("U-timed").await && effective("U-plain").await);
    // Not yet due: the sweep changes nothing.
    assert_eq!(
        aurora_api::jobs::expire_reach_grants(w.h.pool())
            .await
            .unwrap(),
        0
    );
    // Time passes.
    sqlx::query("UPDATE reach_grants SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(timed)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert!(
        !effective("U-timed").await,
        "ineffective after its expiry, before the sweep"
    );
    assert!(effective("U-plain").await);
    assert_eq!(
        aurora_api::jobs::expire_reach_grants(w.h.pool())
            .await
            .unwrap(),
        1
    );
    let st = |g: Uuid| {
        let pool = w.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT state FROM reach_grants WHERE id = $1")
                .bind(g)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        (st(timed).await.as_str(), st(plain).await.as_str()),
        ("expired", "approved")
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

// ------------------------------------------- self-contained deprovision (RFE-024, SEC-58)

fn failed_total() -> f64 {
    counter_value("aurora_channel_deprovision_failed_total")
}

impl W {
    async fn second_replica(&self, c: &Conn) -> Conn {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO connector_replicas (id, connector_id, version, last_seen_at) VALUES ($1,$2,'1.0.0',now())")
            .bind(id)
            .bind(&c.id)
            .execute(self.h.pool())
            .await
            .unwrap();
        Conn {
            id: c.id.clone(),
            tok: c.tok.clone(),
            replica: id,
        }
    }

    /// One claim by `c` without waiting.
    async fn claim_now(&self, c: &Conn) -> Vec<Value> {
        let (s, _, v) = plane(
            &self.h,
            &c.tok,
            Some(c.replica),
            Method::POST,
            "/ops/claim",
            Some(json!({"max": 10, "wait_secs": 0})),
        )
        .await;
        assert_eq!(s, 200, "{v}");
        v["ops"].as_array().unwrap().clone()
    }

    async fn report(&self, c: &Conn, op: &Value, mut result: Value) {
        result["op_id"] = op["op_id"].clone();
        let (s, _, v) = plane(
            &self.h,
            &c.tok,
            Some(c.replica),
            Method::POST,
            "/ops/results",
            Some(json!({"results": [result]})),
        )
        .await;
        assert_eq!(s, 200, "{v}");
    }

    /// The deprovision op of a presence: `(id, integration_id, status, error_code, payload)`.
    async fn dep_op(&self, presence: Uuid) -> (Uuid, Option<Uuid>, String, Option<String>, Value) {
        sqlx::query_as(
            "SELECT id, integration_id, status, error_code, payload FROM channel_ops
              WHERE kind='presence.deprovision' AND presence_id=$1",
        )
        .bind(presence)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    async fn delete_ok(&self, path: &str, body: Option<Value>) {
        let (s, v) = self.call(&self.owner, Method::DELETE, path, body).await;
        assert_eq!(s, 204, "{v}");
    }

    async fn ws_slug(&self, ws: Uuid) -> String {
        sqlx::query_scalar("SELECT slug FROM workspaces WHERE id=$1")
            .bind(ws)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// The best-effort failure audit row of a presence (written asynchronously).
    async fn failure_audit(&self, presence: Uuid) -> Option<(Option<Uuid>, String, String, Value)> {
        for _ in 0..50 {
            let row: Option<(Option<Uuid>, String, String, Value)> = sqlx::query_as(
                "SELECT org_id, actor_kind, target_kind, metadata FROM audit_log
                  WHERE action='presence.deprovision_failed' AND target_id=$1",
            )
            .bind(presence.to_string())
            .fetch_optional(self.h.pool())
            .await
            .unwrap();
            if row.is_some() {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    }

    async fn agent_in(&self, ws: Uuid, handle: &str) -> Uuid {
        let agent = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,$4,$4,'hosted','fake')")
            .bind(agent).bind(self.org).bind(ws).bind(handle).execute(self.h.pool()).await.unwrap();
        agent
    }
}

fn dep_creds() -> Value {
    json!({"token": SECRET})
}

/// 18 §Self-contained deprovision, *Atomicity* (1/2): the delete of an agent, a workspace, an
/// org, an integration or a single presence commits one op per dedicated presence, in the same
/// transaction; shared presences enqueue nothing.
#[tokio::test]
async fn deprovision_ops_commit_with_agent_workspace_org_integration_and_detach() {
    let (w, _mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let integ2 = w.integration(&w.fake, "T2").await;
    // Everything is set up first: a queued deprovision would be served in place of the next
    // provisioning or validation answer.
    let a1 = w.agent("a1").await;
    let shared = w.agent("a2").await;
    let p1 = w.dedicated(a1, integ, "BOT-1").await;
    assert_eq!(w.attach(&w.owner, shared, integ, "shared").await.0, 201);
    let ws2 = w.h.workspace(w.org).await;
    let b1 = w.agent_in(ws2, "b1").await;
    let p2 = w.dedicated(b1, integ, "BOT-2").await;
    let c1 = w.agent("c1").await;
    let p3 = w.dedicated(c1, integ, "BOT-3").await;
    let c2 = w.agent("c2").await;
    let p4 = w.dedicated(c2, integ, "BOT-4").await;
    let d1 = w.agent("d1").await;
    let d2 = w.agent("d2").await;
    let p5 = w.dedicated(d1, integ2, "BOT-5").await;
    assert_eq!(w.attach(&w.owner, d2, integ2, "shared").await.0, 201);

    // Agent: one op, none for the shared presence.
    w.delete_ok(&format!("/v1/agents/{a1}"), None).await;
    w.delete_ok(&format!("/v1/agents/{shared}"), None).await;
    let d = w.ops_of("presence.deprovision").await;
    assert_eq!(d.len(), 1, "shared presences enqueue nothing");
    let (_, op_integ, status, _, payload) = w.dep_op(p1).await;
    assert_eq!((op_integ, status.as_str()), (None, "pending"));
    assert_eq!(payload["reason"], "agent_deleted");
    assert_eq!(payload["platform_user_ref"], "BOT-1");
    assert_eq!(payload["handle"], "h");
    assert_eq!(payload["tenant_ref"], "T1");
    assert_eq!(payload["tenant_label"], "Tenant T1");
    assert_eq!(payload["config"], json!({"region": "eu"}));
    assert!(
        payload["credentials_ref"]
            .as_str()
            .is_some_and(|r| !r.is_empty())
    );
    assert!(
        !payload.to_string().contains(SECRET),
        "the stored payload never holds plaintext"
    );
    let gone: i64 =
        sqlx::query_scalar("SELECT count(*) FROM channel_presences WHERE agent_id = ANY($1)")
            .bind(vec![a1, shared])
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(gone, 0);

    // Workspace.
    w.delete_ok(
        &format!("/v1/workspaces/{ws2}"),
        Some(json!({"confirm": w.ws_slug(ws2).await})),
    )
    .await;
    assert_eq!(w.dep_op(p2).await.4["reason"], "workspace_deleted");

    // Detach of a single presence.
    w.delete_ok(&format!("/v1/presences/{p3}"), None).await;
    assert_eq!(w.dep_op(p3).await.4["reason"], "detached");

    // Integration delete cascades its presences the same way.
    w.delete_ok(&format!("/v1/channel-integrations/{integ}"), None)
        .await;
    let (_, op_integ, _, _, payload) = w.dep_op(p4).await;
    assert_eq!(op_integ, None);
    assert_eq!(payload["reason"], "detached");

    // Org: its integration goes with it; the op does not.
    let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id=$1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    w.delete_ok(
        &format!("/v1/orgs/{}", w.org),
        Some(json!({"confirm": slug})),
    )
    .await;
    let (_, op_integ, status, _, payload) = w.dep_op(p5).await;
    assert_eq!((op_integ, status.as_str()), (None, "pending"));
    assert_eq!(payload["reason"], "org_deleted");
    assert_eq!(payload["tenant_label"], "Tenant T2");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(left, 0, "the integration is gone, the op is not");
    assert_eq!(
        w.ops_of("presence.deprovision").await.len(),
        5,
        "one op per dedicated presence; the org's shared bot and shared presence: none"
    );
}

/// *Atomicity* (2/2): an injected failure after the op inserts (here in the cascade) rolls back
/// the delete and the ops together, and the sealed copies are discarded; once the failure is
/// gone the same delete commits both.
#[tokio::test]
async fn deprovision_rolls_back_with_a_failed_delete() {
    let (w, mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    let p1 = w.dedicated(a1, integ, "BOT-1").await;
    let before = mem.managed_count();
    sqlx::query("CREATE FUNCTION p3024_fail() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$ LANGUAGE plpgsql")
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER p3024_fail BEFORE DELETE ON channel_presences FOR EACH ROW EXECUTE FUNCTION p3024_fail()")
        .execute(w.h.pool())
        .await
        .unwrap();
    let slug = w.ws_slug(w.ws).await;
    let org_slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id=$1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    for (path, body) in [
        (format!("/v1/agents/{a1}"), None),
        (format!("/v1/presences/{p1}"), None),
        (
            format!("/v1/workspaces/{}", w.ws),
            Some(json!({"confirm": slug})),
        ),
        (
            format!("/v1/orgs/{}", w.org),
            Some(json!({"confirm": org_slug})),
        ),
    ] {
        let (s, v) = w.call(&w.owner, Method::DELETE, &path, body).await;
        assert!(s >= 500, "{path}: {s} {v}");
        assert!(w.ops_of("presence.deprovision").await.is_empty(), "{path}");
        let alive: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM agents WHERE id=$1)
                  + (SELECT count(*) FROM channel_presences WHERE id=$2)
                  + (SELECT count(*) FROM organizations WHERE id=$3)",
        )
        .bind(a1)
        .bind(p1)
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        assert_eq!(alive, 3, "{path}: nothing was deleted");
        assert_eq!(
            mem.managed_count(),
            before,
            "{path}: the sealed copy of the rolled-back delete is discarded"
        );
    }
    sqlx::query("DROP TRIGGER p3024_fail ON channel_presences")
        .execute(w.h.pool())
        .await
        .unwrap();
    w.delete_ok(&format!("/v1/agents/{a1}"), None).await;
    assert_eq!(w.ops_of("presence.deprovision").await.len(), 1);
}

/// *Self-containment*: with the integration, presence, agent and org rows deleted, a replica
/// that never owned the integration claims the op, gets the credentials next to a payload that
/// holds everything else, and the plaintext exists nowhere at rest.
#[tokio::test]
async fn deprovision_survives_org_delete_and_runs_on_another_replica() {
    let (w, mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    let p1 = w.dedicated(a1, integ, "BOT-1").await;
    let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id=$1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    w.delete_ok(
        &format!("/v1/orgs/{}", w.org),
        Some(json!({"confirm": slug})),
    )
    .await;
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM channel_integrations) + (SELECT count(*) FROM channel_presences)
              + (SELECT count(*) FROM agents) + (SELECT count(*) FROM organizations WHERE id=$1)",
    )
    .bind(w.org)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows, 0, "everything the connector used to look up is gone");

    let other = w.second_replica(&w.fake).await;
    let ops = w.claim_now(&other).await;
    assert_eq!(ops.len(), 1, "{ops:?}");
    let op = &ops[0];
    assert_eq!(op["kind"], "presence.deprovision");
    assert!(op.get("integration_id").is_none_or(Value::is_null));
    assert_eq!(op["presence_id"], json!(p1));
    assert_eq!(op["credentials"], dep_creds());
    let typed: aurora_protocol::connector::Operation = serde_json::from_value(op.clone()).unwrap();
    let aurora_protocol::connector::OpPayload::PresenceDeprovision(p) = typed.parse().unwrap()
    else {
        panic!("not a deprovision payload");
    };
    assert_eq!(p.platform_user_ref, "BOT-1");
    assert_eq!(p.tenant_ref, "T1");
    assert_eq!(p.config, json!({"region": "eu"}));
    let keys: Vec<&str> = op["payload"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        !keys.contains(&"credentials_ref") && !keys.contains(&"org_id"),
        "internal payload keys are never sent: {keys:?}"
    );

    // Plaintext only in the claim response.
    let at_rest: String = sqlx::query_scalar(
        "SELECT string_agg(payload::text || coalesce(result::text,''), '') FROM channel_ops",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(!at_rest.contains(SECRET));
    let before = mem.managed_count();
    w.report(&other, op, json!({"outcome": "ok", "data": {}}))
        .await;
    let (_, _, status, _, payload) = w.dep_op(p1).await;
    assert_eq!(status, "done");
    assert_eq!(payload["credentials_ref"], Value::Null);
    assert_eq!(mem.managed_count(), before - 1, "the copy was discarded");
}

/// *Credential copy*: sealed under the op's context; every other context refuses it.
#[tokio::test]
async fn deprovision_copy_is_bound_to_the_op_context() {
    use aurora_api::domains::channels::deprovision::op_seal_ctx;
    use aurora_api::domains::connectors::{integration_seal_ctx, presence_seal_ctx};
    use aurora_secrets::{SecretError, SecretRef};
    let (w, _mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    let p1 = w.dedicated(a1, integ, "BOT-1").await;
    w.delete_ok(&format!("/v1/agents/{a1}"), None).await;
    let (op, _, _, _, payload) = w.dep_op(p1).await;
    let sealed: SecretRef = payload["credentials_ref"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let vault = &w.h.state.vault;
    let plain = vault.open(&op_seal_ctx(op, w.org), &sealed).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(plain.expose()).unwrap(),
        dep_creds()
    );
    for ctx in [
        presence_seal_ctx(p1, w.org),
        integration_seal_ctx(integ, w.org),
        op_seal_ctx(Uuid::now_v7(), w.org),
        op_seal_ctx(op, Uuid::now_v7()),
    ] {
        assert!(
            matches!(
                vault.open(&ctx, &sealed).await,
                Err(SecretError::SecretRefMismatch)
            ),
            "{ctx:?} opened the copy"
        );
    }
}

/// *Credential copy*: discarded and nulled at done / failed / dropped and by pruning (a finished
/// op whose discard failed), and by the expiry of an op that ran out of its window.
#[tokio::test]
async fn deprovision_copy_is_discarded_at_every_end_and_by_pruning() {
    let (w, mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let mut pres = Vec::new();
    let mut agents = Vec::new();
    for (i, h) in ["a1", "a2", "a3", "a4", "a5"].into_iter().enumerate() {
        let a = w.agent(h).await;
        pres.push(w.dedicated(a, integ, &format!("BOT-{i}")).await);
        agents.push(a);
    }
    for a in agents {
        w.delete_ok(&format!("/v1/agents/{a}"), None).await;
    }
    let (ok, dead, gone, pruned, expired) = (pres[0], pres[1], pres[2], pres[3], pres[4]);
    let before = mem.managed_count();
    let answer = |op: &Value| {
        let p = op["presence_id"].as_str().unwrap().to_owned();
        if p == ok.to_string() {
            json!({"outcome": "ok", "data": {}})
        } else if p == dead.to_string() {
            op_err("credential_dead", "revoked")
        } else if p == gone.to_string() {
            op_err("not_found", "no such bot")
        } else {
            // The remaining two stay claimed-and-unanswered: answer unavailable (retried).
            op_err("unavailable", "later")
        }
    };
    w.serve(&w.fake, 5, answer).await;
    for (p, want) in [(ok, "done"), (dead, "failed"), (gone, "dropped")] {
        let (_, _, status, _, payload) = w.dep_op(p).await;
        assert_eq!(status, want);
        assert_eq!(payload["credentials_ref"], Value::Null, "{want}");
    }
    assert_eq!(mem.managed_count(), before - 3);

    // Pruning: a finished op whose discard did not happen.
    let (op, _, _, _, _) = w.dep_op(pruned).await;
    sqlx::query("UPDATE channel_ops SET status='done', completed_at=now() WHERE id=$1")
        .bind(op)
        .execute(w.h.pool())
        .await
        .unwrap();
    // Expiry: an op past its 24 h window.
    let (op2, _, _, _, _) = w.dep_op(expired).await;
    sqlx::query("UPDATE channel_ops SET status='pending', created_at = now() - interval '25 hours' WHERE id=$1")
        .bind(op2)
        .execute(w.h.pool())
        .await
        .unwrap();
    let before = mem.managed_count();
    let (n_expired, n_pruned) = aurora_api::domains::channels::deprovision::sweep(&w.h.state)
        .await
        .unwrap();
    assert_eq!(
        (n_expired, n_pruned),
        (1, 2),
        "pruned = the done op and the expired one"
    );
    let (_, _, status, code, payload) = w.dep_op(pruned).await;
    assert_eq!(
        (status.as_str(), payload["credentials_ref"].clone()),
        ("done", Value::Null)
    );
    assert!(code.is_none());
    let (_, _, status, code, payload) = w.dep_op(expired).await;
    assert_eq!(
        (
            status.as_str(),
            code.as_deref(),
            payload["credentials_ref"].clone()
        ),
        ("failed", Some("unavailable"), Value::Null)
    );
    assert_eq!(mem.managed_count(), before - 2);
    let (org, actor, target, meta) = w.failure_audit(expired).await.expect("failure audit");
    assert_eq!(
        (org, actor.as_str(), target.as_str()),
        (Some(w.org), "system", "presence")
    );
    assert_eq!(meta["error_code"], "unavailable");
}

/// *Retry*: an op is never claimed without its credentials; while the copy cannot be opened it
/// stays pending, and it is handed out (with credentials) once it can.
#[tokio::test]
async fn deprovision_is_never_claimed_without_its_credentials() {
    let (w, mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    let p1 = w.dedicated(a1, integ, "BOT-1").await;
    w.delete_ok(&format!("/v1/agents/{a1}"), None).await;
    mem.set_unavailable(true);
    assert!(
        w.claim_now(&w.fake).await.is_empty(),
        "claimed without credentials"
    );
    let (_, _, status, _, payload) = w.dep_op(p1).await;
    assert_eq!(status, "pending");
    assert!(payload["credentials_ref"].is_string(), "the copy is kept");
    mem.set_unavailable(false);
    sqlx::query("UPDATE channel_ops SET not_before = now() WHERE kind='presence.deprovision'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let ops = w.claim_now(&w.fake).await;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["credentials"], dep_creds());
}

/// *Delete routes*: a seal failure (the store is unavailable) refuses every route with 503
/// `secret_store_unavailable` and deletes nothing; a delete that needs no copy is unaffected.
#[tokio::test]
async fn delete_is_refused_when_the_secret_store_is_unavailable() {
    let (w, mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    let plain = w.agent("a2").await;
    let p1 = w.dedicated(a1, integ, "BOT-1").await;
    let slug = w.ws_slug(w.ws).await;
    let org_slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id=$1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    mem.set_unavailable(true);
    for (path, body) in [
        (format!("/v1/agents/{a1}"), None),
        (format!("/v1/presences/{p1}"), None),
        (format!("/v1/channel-integrations/{integ}"), None),
        (
            format!("/v1/workspaces/{}", w.ws),
            Some(json!({"confirm": slug})),
        ),
        (
            format!("/v1/orgs/{}", w.org),
            Some(json!({"confirm": org_slug})),
        ),
    ] {
        let (s, v) = w.call(&w.owner, Method::DELETE, &path, body).await;
        assert_eq!(s, 503, "{path}: {v}");
        assert_eq!(v["error"]["code"], "secret_store_unavailable", "{path}");
        assert!(!v.to_string().contains(SECRET));
        let alive: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM agents WHERE id=$1)
                  + (SELECT count(*) FROM channel_presences WHERE id=$2)
                  + (SELECT count(*) FROM channel_integrations WHERE id=$3)
                  + (SELECT count(*) FROM workspaces WHERE id=$4)
                  + (SELECT count(*) FROM organizations WHERE id=$5)",
        )
        .bind(a1)
        .bind(p1)
        .bind(integ)
        .bind(w.ws)
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        assert_eq!(alive, 5, "{path}: nothing was deleted");
        assert!(w.ops_of("presence.deprovision").await.is_empty(), "{path}");
    }
    // No copy is needed for an agent without a dedicated presence.
    w.delete_ok(&format!("/v1/agents/{plain}"), None).await;
    mem.set_unavailable(false);
    w.delete_ok(&format!("/v1/agents/{a1}"), None).await;
    assert_eq!(w.ops_of("presence.deprovision").await.len(), 1);
}

/// *Retry/failure* (clock test): `unavailable` is retried for the 24 h window regardless of the
/// attempt count, then the op fails (`unavailable`) with the failure audit, the metric and the
/// copy discarded.
#[tokio::test]
async fn deprovision_retries_unavailable_for_the_window_then_fails() {
    let _ = failed_total();
    let (w, mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    let p1 = w.dedicated(a1, integ, "BOT-1").await;
    w.delete_ok(&format!("/v1/agents/{a1}"), None).await;
    let before_total = failed_total();
    for round in 0..3 {
        let ops = w.claim_now(&w.fake).await;
        assert_eq!(ops.len(), 1, "round {round}");
        assert_eq!(ops[0]["credentials"], dep_creds());
        w.report(&w.fake, &ops[0], op_err("unavailable", "platform down"))
            .await;
        let (_, _, status, _, payload) = w.dep_op(p1).await;
        assert_eq!(status, "pending", "round {round}: still inside the window");
        assert!(payload["credentials_ref"].is_string());
        // Far past OP_MAX_ATTEMPTS: the window, not the attempt count, limits the retries.
        sqlx::query("UPDATE channel_ops SET attempts = 50, not_before = now() WHERE kind='presence.deprovision'")
            .execute(w.h.pool())
            .await
            .unwrap();
    }
    let ops = w.claim_now(&w.fake).await;
    assert_eq!(ops.len(), 1);
    // The clock moves past the window while the connector works.
    sqlx::query("UPDATE channel_ops SET created_at = now() - interval '25 hours' WHERE kind='presence.deprovision'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let before = mem.managed_count();
    w.report(&w.fake, &ops[0], op_err("unavailable", "platform down"))
        .await;
    let (_, _, status, code, payload) = w.dep_op(p1).await;
    assert_eq!(
        (status.as_str(), code.as_deref()),
        ("failed", Some("unavailable"))
    );
    assert_eq!(payload["credentials_ref"], Value::Null);
    assert_eq!(mem.managed_count(), before - 1);
    assert!(w.failure_audit(p1).await.is_some());
    assert!(failed_total() > before_total);
}

/// *Failure record* (C-364): `credential_dead` and `forbidden` fail the op, `not_found` drops it
/// (the bot is already gone: no residual). Failures write the audit row with the documented
/// metadata (never credentials) and count the metric.
#[tokio::test]
async fn deprovision_failure_audit_metric_and_dropped() {
    let _ = failed_total();
    let (w, _mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let mut pres = Vec::new();
    let mut agents = Vec::new();
    for (i, h) in ["a1", "a2", "a3"].into_iter().enumerate() {
        let a = w.agent(h).await;
        pres.push(w.dedicated(a, integ, &format!("BOT-{i}")).await);
        agents.push(a);
    }
    for a in agents {
        w.delete_ok(&format!("/v1/agents/{a}"), None).await;
    }
    let (dead, forbidden, gone) = (pres[0], pres[1], pres[2]);
    let before_total = failed_total();
    w.serve(&w.fake, 3, |op| {
        let p = op["presence_id"].as_str().unwrap();
        if p == dead.to_string() {
            op_err("credential_dead", "token revoked")
        } else if p == forbidden.to_string() {
            op_err("forbidden", "not an admin")
        } else {
            op_err("not_found", "no such bot")
        }
    })
    .await;
    assert_eq!(w.dep_op(dead).await.2, "failed");
    assert_eq!(w.dep_op(forbidden).await.2, "failed");
    assert_eq!(w.dep_op(gone).await.2, "dropped");
    for (p, code, bot) in [
        (dead, "credential_dead", "BOT-0"),
        (forbidden, "forbidden", "BOT-1"),
    ] {
        let (org, actor, target, meta) = w.failure_audit(p).await.expect("failure audit");
        assert_eq!(
            (org, actor.as_str(), target.as_str()),
            (Some(w.org), "system", "presence")
        );
        assert_eq!(
            meta,
            json!({
                "connector_id": "fake", "tenant_label": "Tenant T1",
                "platform_user_ref": bot, "handle": "h", "error_code": code
            })
        );
        assert!(!meta.to_string().contains(SECRET));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        w.failure_audit(gone).await.is_none(),
        "a dropped op is not a residual"
    );
    assert!(
        failed_total() >= before_total + 2.0,
        "aurora_channel_deprovision_failed_total counts each failed op"
    );
    // No credential in any result or ledger row.
    let at_rest: String = sqlx::query_scalar(
        "SELECT coalesce(string_agg(coalesce(result::text,'') || payload::text, ''), '') FROM channel_ops",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    let audit: String =
        sqlx::query_scalar("SELECT coalesce(string_agg(metadata::text, ''), '') FROM audit_log")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(!at_rest.contains(SECRET) && !audit.contains(SECRET));
}

// ------------------------------------------------------------ FIX-127 (C-710..C-714)

fn probe_ok() -> Value {
    json!({"outcome": "ok", "data": {
        "tenant_ref": "", "tenant_label": "@daneel on mm.test",
        "choices": {"team": [{"value": "T1", "label": "Team One"}, {"value": "T2", "label": "Team Two"}]},
        "warnings": ["No admin token."]}})
}

/// C-711: the probe returns choices and warnings, sends `mode: "probe"`, needs an org admin,
/// stores nothing and leaves no plaintext credential in the op payload.
#[tokio::test]
async fn probe_returns_choices_stores_nothing_and_scrubs_the_op() {
    let w = W::new().await;
    let path = format!("/v1/orgs/{}/channel-integrations:probe", w.org);
    let body = w.create_body(&w.fake);
    // Not an org admin: refused before any op exists.
    assert_eq!(
        w.call(&w.wsowner, Method::POST, &path, Some(body.clone()))
            .await
            .0,
        403
    );
    let ((s, v), ops) = tokio::join!(
        w.call(&w.admin, Method::POST, &path, Some(body.clone())),
        w.serve(&w.fake, 1, |_| probe_ok())
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(ops[0]["kind"], "credentials.validate");
    assert_eq!(ops[0]["payload"]["mode"], "probe");
    assert_eq!(v["choices"]["team"][1]["label"], "Team Two");
    assert_eq!(v["tenant_label"], "@daneel on mm.test");
    assert_eq!(v["warnings"][0], "No admin token.");
    assert!(!v.to_string().contains(SECRET), "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_integrations")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "a probe stores no integration");
    let payloads: Vec<(Value,)> = sqlx::query_as("SELECT payload FROM channel_ops")
        .fetch_all(w.h.pool())
        .await
        .unwrap();
    assert!(
        payloads.iter().all(|(p,)| !p.to_string().contains(SECRET)),
        "the probe payload is scrubbed"
    );
}

#[tokio::test]
async fn probe_maps_connector_refusals() {
    let w = W::new().await;
    let path = format!("/v1/orgs/{}/channel-integrations:probe", w.org);
    let body = w.create_body(&w.fake);
    // A connector without probe support.
    let ((s, v), _) = tokio::join!(
        w.call(&w.admin, Method::POST, &path, Some(body.clone())),
        w.serve(&w.fake, 1, |_| op_err("unsupported", "no probe"))
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("probe_unsupported")),
        "{v}"
    );
    // The platform refuses (what the URL guard answers: `invalid`) → like create.
    let ((s, v), _) = tokio::join!(
        w.call(&w.admin, Method::POST, &path, Some(body)),
        w.serve(&w.fake, 1, |_| op_err(
            "invalid",
            "server_url must point at a public address"
        ))
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("credentials_invalid")),
        "{v}"
    );
}

/// C-712: states per connector row, and a plain member sees names but no ids or e-mails.
#[tokio::test]
async fn connector_availability_states_and_instance_admin_names_only() {
    let w = W::new().await;
    let (s, v) = w
        .call(&w.plain, Method::GET, "/v1/connector-availability", None)
        .await;
    assert_eq!(s, 200, "{v}");
    let state = |v: &Value, id: &str| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type_id"] == id)
            .map(|i| i["setup_state"].as_str().unwrap().to_owned())
    };
    assert_eq!(state(&v, "fake").as_deref(), Some("active"));
    assert_eq!(v["instance_admins"], json!([{"name": "U"}]));
    let text = v.to_string();
    assert!(!text.contains("root@x.io"), "{text}");
    let root: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE is_instance_admin")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert!(!text.contains(&root.to_string()), "{text}");
    for (status, want) in [
        ("disabled", "disabled"),
        ("pending_approval", "needs_approval"),
        ("awaiting_registration", "waiting_for_host"),
    ] {
        sqlx::query("UPDATE connectors SET status=$1 WHERE id='fake'")
            .bind(status)
            .execute(w.h.pool())
            .await
            .unwrap();
        let (_, v) = w
            .call(&w.plain, Method::GET, "/v1/connector-availability", None)
            .await;
        assert_eq!(state(&v, "fake").as_deref(), Some(want), "{v}");
    }
    // A type announced by a live host with no connector row: `not_added`.
    let m = manifest("newtype", json!({"fields": []}));
    aurora_db::repo::connector_hosts::upsert_host(
        w.h.pool(),
        Uuid::now_v7(),
        "1.0.0",
        &json!([{"type_id": "newtype", "manifest": m, "manifest_digest": "d".repeat(64)}]),
    )
    .await
    .unwrap();
    let (s, v) = w
        .call(&w.plain, Method::GET, "/v1/connector-availability", None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(state(&v, "newtype").as_deref(), Some("not_added"), "{v}");
    let item = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type_id"] == "newtype")
        .unwrap();
    assert!(item["connector_id"].is_null(), "{v}");
}

/// C-714: a caller without a linked chat account gets `link_required`; the probe never echoes a
/// credential, not even in its refusals.
#[tokio::test]
async fn my_spaces_need_a_link_and_probe_errors_do_not_echo_credentials() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let mine = format!("/v1/integrations/{integ}/spaces?mine=true");
    let (s, v) = w.call(&w.wsowner, Method::GET, &mine, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("link_required")),
        "{v}"
    );
    let path = format!("/v1/orgs/{}/channel-integrations:probe", w.org);
    let body = w.create_body(&w.fake);
    for (code, want) in [
        ("invalid", "credentials_invalid"),
        ("credential_dead", "credentials_invalid"),
    ] {
        let ((s, v), _) = tokio::join!(
            w.call(&w.admin, Method::POST, &path, Some(body.clone())),
            w.serve(&w.fake, 1, |_| op_err(code, "rejected"))
        );
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some(want)),
            "{code}: {v}"
        );
        assert!(!v.to_string().contains(SECRET), "{v}");
    }
}

fn user_json(r: &str, bot: bool) -> Value {
    json!({"ref": r, "display_name": r, "handle": r, "email_verified": false, "is_bot": bot})
}

/// The platform: spaces C1 (the caller and the bot are in), C3 (the bot is in, the caller is
/// not), D1 (a direct conversation); C9 is not the bot's (members: forbidden).
fn platform(op: &Value) -> Value {
    match op["kind"].as_str().unwrap() {
        "spaces.list" => json!({"outcome": "ok", "data": {"spaces": [
            {"ref": "C1", "kind": "channel", "label": "general"},
            {"ref": "C3", "kind": "private", "label": "secret"},
            {"ref": "D1", "kind": "dm", "label": "dm"}]}}),
        "space.members" => match op["payload"]["space_ref"].as_str().unwrap() {
            "C1" => {
                json!({"outcome": "ok", "data": {"users": [user_json("U-1", false), user_json("B1", true)]}})
            }
            "C3" => {
                json!({"outcome": "ok", "data": {"users": [user_json("U-9", false), user_json("B1", true)]}})
            }
            "D1" => {
                json!({"outcome": "ok", "data": {"users": [user_json("U-1", false), user_json("B1", true)]}})
            }
            _ => op_err("forbidden", "bot not in channel"),
        },
        _ => json!({"outcome": "ok", "data": {"message_ref": "m1", "thread_ref": "m1"}}),
    }
}

/// C-713/C-714: bind from the dashboard only a space the caller is in and the bot is in.
#[tokio::test]
async fn dashboard_binding_requires_membership_and_the_bot() {
    let w = W::new().await;
    let integ = w.integration(&w.fake, "T1").await;
    let agent = w.agent("a1").await;
    assert_eq!(w.attach(&w.owner, agent, integ, "shared").await.0, 201);
    let url = format!("/v1/agents/{agent}/space-bindings");
    let body = |space: &str| json!({"integration_id": integ, "space_ref": space});

    // Not linked yet.
    let (s, v) = w
        .call(&w.wsowner, Method::POST, &url, Some(body("C1")))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("link_required")),
        "{v}"
    );
    let link = format!("/v1/channel-integrations/{integ}/user-links");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &link,
            Some(json!({"platform_user_ref": "U-1", "user_id": w.users.wsowner})),
        )
        .await;
    assert_eq!(s, 201, "{v}");

    // A workspace member without can_own.
    assert_eq!(
        w.call(&w.wsmember, Method::POST, &url, Some(body("C1")))
            .await
            .0,
        403
    );
    // Another integration id is a 404.
    let (s, v) = w
        .call(
            &w.wsowner,
            Method::POST,
            &url,
            Some(json!({"integration_id": Uuid::now_v7(), "space_ref": "C1"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (404, Some("integration_not_found")),
        "{v}"
    );

    // The caller is not in the space.
    let ((s, v), _) = tokio::join!(
        w.call(&w.wsowner, Method::POST, &url, Some(body("C3"))),
        w.serve(&w.fake, 2, platform)
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("not_a_member")),
        "{v}"
    );
    // A direct conversation.
    let ((s, v), _) = tokio::join!(
        w.call(&w.wsowner, Method::POST, &url, Some(body("D1"))),
        w.serve(&w.fake, 1, platform)
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("space_not_bindable")),
        "{v}"
    );
    // The bot is not in the space: 409 with the invite hint.
    let ((s, v), _) = tokio::join!(
        w.call(&w.wsowner, Method::POST, &url, Some(body("C9"))),
        w.serve(&w.fake, 2, platform)
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("bot_not_in_space")),
        "{v}"
    );
    assert!(
        v["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("@daneel"),
        "{v}"
    );

    // Member and bot present: created, audited, announced.
    let ((s, v), ops) = tokio::join!(
        w.call(&w.wsowner, Method::POST, &url, Some(body("C1"))),
        w.serve(&w.fake, 3, platform)
    );
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["listen"], "mentions");
    assert_eq!(v["space_label"], "general");
    assert_eq!(v["bound_by"]["id"], json!(w.users.wsowner));
    assert!(ops.iter().any(|o| o["kind"] == "message.post"), "announced");
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='space_binding.create'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(audits, 1);
    // Again: already bound.
    let ((s, v), _) = tokio::join!(
        w.call(&w.wsowner, Method::POST, &url, Some(body("C1"))),
        w.serve(&w.fake, 2, platform)
    );
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_bound")),
        "{v}"
    );

    // C-714: only spaces the caller is in (C1), never the DM.
    let mine = format!("/v1/integrations/{integ}/spaces?mine=true");
    let ((s, v), _) = tokio::join!(
        w.call(&w.wsowner, Method::GET, &mine, None),
        w.serve(&w.fake, 3, platform)
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["items"][0]["space_ref"], "C1");
    assert_eq!(v["items"][0]["bot_present"], true);
    let no_mine = format!("/v1/integrations/{integ}/spaces");
    assert_eq!(w.call(&w.wsowner, Method::GET, &no_mine, None).await.0, 400);
}

/// A shared presence has no bot of its own: nothing is enqueued by any delete.
#[tokio::test]
async fn shared_presences_enqueue_nothing_on_org_delete() {
    let (w, _mem) = W::with_memory_store().await;
    let integ = w.integration(&w.fake, "T1").await;
    let a1 = w.agent("a1").await;
    assert_eq!(w.attach(&w.owner, a1, integ, "shared").await.0, 201);
    let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id=$1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    w.delete_ok(
        &format!("/v1/orgs/{}", w.org),
        Some(json!({"confirm": slug})),
    )
    .await;
    assert!(w.ops_of("presence.deprovision").await.is_empty());
}

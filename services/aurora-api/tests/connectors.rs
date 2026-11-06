#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-070: connector provisioning, registration/trust, ownership leases, config feed,
//! credential write-back and retire (docs/13 §2–§4, API_CONTRACTS C-31/C-32).
mod common;

use aurora_api::domains::connectors::{integration_seal_ctx, presence_seal_ctx};
use aurora_core::Secret;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::{PackageBuilder, png_stub};
use base64::Engine;
use common::*;
use http::{HeaderMap, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

struct W {
    h: Harness,
    admin: String,
    key: SigningKey,
    org: Uuid,
}

fn hd(c: &str) -> [(&str, &str); 2] {
    [("cookie", c), ("origin", ORIGIN)]
}

async fn world() -> W {
    let h = Harness::new().await;
    let root = h.user("root@x.io", true).await;
    let (_, tok) = h.good_session(root).await;
    let admin = cookie(&tok);
    let key = generate_signing_key().unwrap();
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/trusted-keys",
            &hd(&admin),
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let org = h.org().await;
    W { h, admin, key, org }
}

fn manifest(id: &str, caps: Value, callbacks: Value, fields: Value) -> Value {
    json!({
        "package": {"id": id, "version": "1.0.0", "contract": 1, "publisher": "acme"},
        "about": {"name": "Fake"},
        "capabilities": caps,
        "integration_setup": {"fields": fields},
        "presence_setup": {},
        "limits": {},
        "public_callbacks": callbacks
    })
}

fn base_manifest(id: &str) -> Value {
    manifest(
        id,
        json!({"threads": true, "reactions": true, "files": {"max_bytes": 10}}),
        json!(["actions", "commands"]),
        json!([{"name": "token", "label": "Token", "type": "secret"}]),
    )
}

impl W {
    fn sign(&self, m: &Value) -> String {
        let typed: aurora_protocol::connector::ConnectorManifest =
            serde_json::from_value(m.clone()).unwrap();
        let v = serde_json::to_value(&typed).unwrap();
        let d: [u8; 32] = Sha256::digest(aurora_registry::snapshot::canonical_bytes(&v)).into();
        PackageBuilder::sign_digest(&self.key, &PackageBuilder::key_id(&self.key), &d)
    }

    async fn admin(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self.h.send(m, uri, &hd(&self.admin), body).await;
        (s.as_u16(), v)
    }

    /// Provision connector `id`; returns its token.
    async fn provision(&self, id: &str) -> String {
        let (s, v) = self
            .admin(
                Method::POST,
                "/v1/admin/connectors",
                Some(json!({"id": id, "display_name": id})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["token"].as_str().unwrap().to_owned()
    }

    async fn plane(
        &self,
        tok: &str,
        replica: Option<Uuid>,
        m: Method,
        path: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (u16, HeaderMap, Value) {
        let b = bearer(tok);
        let r = replica.map(|r| r.to_string());
        let mut h = vec![("authorization", b.as_str())];
        if let Some(r) = &r {
            h.push(("x-aurora-replica", r.as_str()));
        }
        h.extend_from_slice(extra);
        let (s, hm, v) = self
            .h
            .send(m, &format!("/v1/connector{path}"), &h, body)
            .await;
        (s.as_u16(), hm, v)
    }

    async fn register_raw(&self, tok: &str, m: &Value, sig: &str) -> (u16, Value) {
        let (s, _, v) = self
            .plane(
                tok,
                None,
                Method::POST,
                "/register",
                Some(json!({"manifest": m, "signature": sig, "version": "1.2.3"})),
                &[],
            )
            .await;
        (s, v)
    }

    /// Signed registration; returns (status, replica id).
    async fn register(&self, tok: &str, m: &Value) -> (String, Uuid) {
        let (s, v) = self.register_raw(tok, m, &self.sign(m)).await;
        assert_eq!(s, 200, "{v}");
        (
            v["status"].as_str().unwrap().to_owned(),
            v["replica_id"].as_str().unwrap().parse().unwrap(),
        )
    }

    async fn config(
        &self,
        tok: &str,
        replica: Uuid,
        etag: Option<&str>,
    ) -> (u16, HeaderMap, Value) {
        let extra: Vec<(&str, &str)> = etag.map(|e| ("if-none-match", e)).into_iter().collect();
        self.plane(tok, Some(replica), Method::GET, "/config", None, &extra)
            .await
    }

    async fn connector(&self, id: &str) -> Value {
        let (s, v) = self.admin(Method::GET, "/v1/admin/connectors", None).await;
        assert_eq!(s, 200, "{v}");
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()
            .clone()
    }

    async fn approve(&self, id: &str) {
        let c = self.connector(id).await;
        let (s, v) = self
            .admin(
                Method::POST,
                &format!("/v1/admin/connectors/{id}/approve"),
                Some(json!({"manifest_digest": c["manifest_digest"]})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["status"], "active");
    }

    async fn integration(&self, connector: &str, tenant: &str, creds: Value, status: &str) -> Uuid {
        let id = Uuid::now_v7();
        let r = self
            .h
            .state
            .vault
            .seal(
                &integration_seal_ctx(id, self.org),
                Secret::from(serde_json::to_vec(&creds).unwrap()),
            )
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label,
                config, credentials_ref, status) VALUES ($1,$2,$3,$4,$4,'{}',$5,$6)",
        )
        .bind(id)
        .bind(self.org)
        .bind(connector)
        .bind(tenant)
        .bind(r.to_column())
        .bind(status)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    async fn owner(&self, integration: Uuid) -> Option<Uuid> {
        sqlx::query_scalar("SELECT owner_replica_id FROM channel_integrations WHERE id=$1")
            .bind(integration)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn ids(cfg: &Value) -> Vec<String> {
    let mut v: Vec<String> = cfg["integrations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

fn sorted(v: &[Uuid]) -> Vec<String> {
    let mut v: Vec<String> = v.iter().map(ToString::to_string).collect();
    v.sort();
    v
}

#[tokio::test]
async fn provision_register_approve_and_serve_config() {
    let w = world().await;
    let tok = w.provision("fake").await;
    assert!(tok.starts_with("aun_"));
    let c = w.connector("fake").await;
    assert_eq!(c["status"], "awaiting_registration");
    assert!(c["manifest"].is_null());
    // The token hash never appears.
    assert!(c.get("token_hash").is_none());

    let a = w
        .integration("fake", "T1", json!({"bot_token": "xoxb-1"}), "active")
        .await;
    let b = w
        .integration("fake", "T2", json!({"bot_token": "xoxb-2"}), "active")
        .await;
    let validating = w
        .integration("fake", "T3", json!({"bot_token": "x"}), "validating")
        .await;
    let _ = validating;

    // Unsigned / badly signed manifests are refused.
    let m = base_manifest("fake");
    let (s, v) = w.register_raw(&tok, &m, "").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("signature_malformed")),
        "{v}"
    );
    let (s, v) = w
        .register_raw(&tok, &m, "aurora-sig-v1\nAAAA\nAAAA\n")
        .await;
    assert_eq!(s, 422, "{v}");
    let other = generate_signing_key().unwrap();
    let d: [u8; 32] = Sha256::digest(aurora_registry::snapshot::canonical_bytes(&m)).into();
    let sig = PackageBuilder::sign_digest(&other, &PackageBuilder::key_id(&other), &d);
    let (s, v) = w.register_raw(&tok, &m, &sig).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("signature_untrusted")),
        "{v}"
    );
    // Valid signature over a different manifest.
    let mut tampered = m.clone();
    tampered["capabilities"]["edits"] = json!(true);
    let (s, v) = w.register_raw(&tok, &tampered, &w.sign(&m)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("digest_mismatch")),
        "{v}"
    );
    // Package id must be this connector.
    let wrong = base_manifest("another");
    let (s, v) = w.register_raw(&tok, &wrong, &w.sign(&wrong)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("connector_id_mismatch")),
        "{v}"
    );

    let (status, replica) = w.register(&tok, &m).await;
    assert_eq!(status, "pending_approval");
    let c = w.connector("fake").await;
    assert_eq!(c["status"], "pending_approval");
    assert_eq!(c["version"], "1.2.3");
    assert_eq!(c["contract"], 1);
    assert!(!c["widening"].as_array().unwrap().is_empty());
    assert_eq!(c["replicas"].as_array().unwrap().len(), 1);
    assert_eq!(c["integration_count"], 3);
    assert!(c["approved_manifest_digest"].is_null());
    assert!(c["manifest"].get("_approved").is_none() && c["manifest"].get("_widening").is_none());

    // Not served until approved.
    let (s, _, cfg) = w.config(&tok, replica, None).await;
    assert_eq!(s, 200);
    assert!(cfg["integrations"].as_array().unwrap().is_empty());
    assert!(w.owner(a).await.is_none());

    // Approval must name the pending digest.
    let (s, v) = w
        .admin(
            Method::POST,
            "/v1/admin/connectors/fake/approve",
            Some(json!({"manifest_digest": "deadbeef"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_manifest")),
        "{v}"
    );
    w.approve("fake").await;
    let c = w.connector("fake").await;
    assert_eq!(c["approved_manifest_digest"], c["manifest_digest"]);
    assert!(c["widening"].as_array().unwrap().is_empty());
    assert!(c["approved_at"].is_string());

    let (s, hm, cfg) = w.config(&tok, replica, None).await;
    assert_eq!(s, 200, "{cfg}");
    // Only `active` integrations are served; this replica owns them all.
    assert_eq!(ids(&cfg), sorted(&[a, b]));
    let first = &cfg["integrations"][0];
    assert!(
        first["credentials"]["bot_token"]
            .as_str()
            .unwrap()
            .starts_with("xoxb-")
    );
    assert_eq!(first["credentials_version"], 1);
    assert!(cfg["lease_secs"].as_u64().unwrap() > 0);
    assert_eq!(w.owner(a).await, Some(replica));
    assert_eq!(w.owner(validating).await, None);

    // ETag: 304 when nothing changed.
    let etag = cfg["etag"].as_str().unwrap().to_owned();
    assert_eq!(
        hm.get("etag").unwrap().to_str().unwrap(),
        format!("\"{etag}\"")
    );
    let (s, _, _) = w.config(&tok, replica, Some(&etag)).await;
    assert_eq!(s, 304);
    let (s, _, _) = w.config(&tok, replica, Some(&format!("\"{etag}\""))).await;
    assert_eq!(s, 304);
    let (s, _, _) = w.config(&tok, replica, Some("\"other\"")).await;
    assert_eq!(s, 200);
    // A poll renews the lease even on 304.
    sqlx::query("UPDATE channel_integrations SET owner_lease_until = now() + interval '1 second'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, _, _) = w.config(&tok, replica, Some(&etag)).await;
    assert_eq!(s, 304);
    let lease: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT owner_lease_until FROM channel_integrations WHERE id=$1")
            .bind(a)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(lease > chrono::Utc::now() + chrono::Duration::seconds(10));

    // Decrypted credentials never show in management responses.
    let (_, v) = w.admin(Method::GET, "/v1/admin/connectors", None).await;
    assert!(!v.to_string().contains("xoxb-"));
}

#[tokio::test]
async fn narrowing_stays_active_and_widening_needs_approval() {
    let w = world().await;
    let tok = w.provision("fake").await;
    let (st, replica) = w.register(&tok, &base_manifest("fake")).await;
    assert_eq!(st, "pending_approval");
    w.approve("fake").await;
    let i = w.integration("fake", "T1", json!({"k": 1}), "active").await;

    // Same manifest again: still active.
    let (st, _) = w.register(&tok, &base_manifest("fake")).await;
    assert_eq!(st, "active");

    // Narrower: capability dropped, callback dropped, setup field dropped.
    let narrow = manifest(
        "fake",
        json!({"threads": true, "reactions": false}),
        json!(["actions"]),
        json!([]),
    );
    let (st, _) = w.register(&tok, &narrow).await;
    assert_eq!(st, "active");
    let c = w.connector("fake").await;
    assert_eq!(c["status"], "active");
    assert_eq!(c["approved_manifest_digest"], c["manifest_digest"]);
    assert_eq!(c["manifest"]["public_callbacks"], json!(["actions"]));
    let (s, _, cfg) = w.config(&tok, replica, None).await;
    assert_eq!((s, ids(&cfg)), (200, sorted(&[i])));

    // Widening relative to the (narrowed) approved manifest: each kind goes to pending.
    for (m, needle) in [
        (
            manifest(
                "fake",
                json!({"threads": true, "reactions": true}),
                json!(["actions"]),
                json!([]),
            ),
            "reactions",
        ),
        (
            manifest(
                "fake",
                json!({"threads": true}),
                json!(["actions", "commands"]),
                json!([]),
            ),
            "commands",
        ),
        (
            manifest(
                "fake",
                json!({"threads": true}),
                json!(["actions"]),
                json!([{"name": "extra", "type": "text"}]),
            ),
            "extra",
        ),
    ] {
        let (st, _) = w.register(&tok, &m).await;
        assert_eq!(st, "pending_approval", "{m}");
        let c = w.connector("fake").await;
        assert_eq!(c["status"], "pending_approval");
        let why = c["widening"].to_string();
        assert!(why.contains(needle), "{why}");
        // Approved digest still names the previous manifest; nothing is served meanwhile.
        assert_ne!(c["approved_manifest_digest"], c["manifest_digest"]);
        let (_, _, cfg) = w.config(&tok, replica, None).await;
        assert!(cfg["integrations"].as_array().unwrap().is_empty());
        // Going back to the approved (narrow) manifest restores service without approval.
        let (st, _) = w.register(&tok, &narrow).await;
        assert_eq!(st, "active");
        let (_, _, cfg) = w.config(&tok, replica, None).await;
        assert_eq!(ids(&cfg), sorted(&[i]));
    }

    // A widening manifest stays pending until approved (re-registering it keeps it pending).
    let wide = manifest(
        "fake",
        json!({"threads": true, "edits": true}),
        json!(["actions"]),
        json!([]),
    );
    assert_eq!(w.register(&tok, &wide).await.0, "pending_approval");
    assert_eq!(w.register(&tok, &wide).await.0, "pending_approval");
    w.approve("fake").await;
    assert_eq!(w.connector("fake").await["status"], "active");
    // Approval is idempotent for the same digest.
    w.approve("fake").await;
}

#[tokio::test]
async fn replicas_split_integrations_and_take_over_lapsed_leases() {
    let w = world().await;
    let tok = w.provision("fake").await;
    let (_, a) = w.register(&tok, &base_manifest("fake")).await;
    w.approve("fake").await;
    let all: Vec<Uuid> = {
        let mut v = Vec::new();
        for n in 0..4 {
            v.push(
                w.integration("fake", &format!("T{n}"), json!({"n": n}), "active")
                    .await,
            );
        }
        v
    };

    // Replica A alone owns everything.
    let (_, _, cfg) = w.config(&tok, a, None).await;
    assert_eq!(ids(&cfg), sorted(&all));

    // A second replica appears: ceil(4/2) each. B gets nothing while A's leases are live ...
    let (st, b) = w.register(&tok, &base_manifest("fake")).await;
    assert_eq!(st, "active");
    let (_, _, cfg_b) = w.config(&tok, b, None).await;
    assert!(cfg_b["integrations"].as_array().unwrap().is_empty());
    // ... A releases its surplus at its next poll ...
    let (_, _, cfg_a) = w.config(&tok, a, None).await;
    assert_eq!(cfg_a["integrations"].as_array().unwrap().len(), 2);
    // ... and B takes it.
    let (_, _, cfg_b) = w.config(&tok, b, None).await;
    assert_eq!(cfg_b["integrations"].as_array().unwrap().len(), 2);
    let mut union = ids(&cfg_a);
    union.extend(ids(&cfg_b));
    union.sort();
    assert_eq!(union, sorted(&all), "disjoint and complete");

    // A dies: its replica is no longer seen and its leases lapse. B takes everything.
    sqlx::query(
        "UPDATE connector_replicas SET last_seen_at = now() - interval '10 minutes' WHERE id=$1",
    )
    .bind(a)
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE channel_integrations SET owner_lease_until = now() - interval '1 minute' WHERE owner_replica_id=$1")
        .bind(a)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (_, _, cfg_b) = w.config(&tok, b, None).await;
    assert_eq!(ids(&cfg_b), sorted(&all));
    for i in &all {
        assert_eq!(w.owner(*i).await, Some(b));
    }
}

#[tokio::test]
async fn never_serves_another_connectors_integrations() {
    let w = world().await;
    let t1 = w.provision("fake").await;
    let t2 = w.provision("other").await;
    let (_, r1) = w.register(&t1, &base_manifest("fake")).await;
    let (_, r2) = w.register(&t2, &base_manifest("other")).await;
    w.approve("fake").await;
    w.approve("other").await;
    let mine = w
        .integration("fake", "T1", json!({"s": "mine"}), "active")
        .await;
    let theirs = w
        .integration("other", "T1", json!({"s": "theirs"}), "active")
        .await;

    let (_, _, c1) = w.config(&t1, r1, None).await;
    assert_eq!(ids(&c1), sorted(&[mine]));
    assert!(!c1.to_string().contains("theirs"));
    let (_, _, c2) = w.config(&t2, r2, None).await;
    assert_eq!(ids(&c2), sorted(&[theirs]));

    // A replica id of another connector is not a live replica of this one.
    let (s, _, v) = w.config(&t1, r2, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("replica_unknown")),
        "{v}"
    );
    // Nor can credentials of another connector's integration be written.
    let (s, _, _) = w
        .plane(
            &t1,
            Some(r1),
            Method::POST,
            "/credentials",
            Some(json!({"target": {"integration_id": theirs}, "expected_version": 1, "credentials": {"x": 1}})),
            &[],
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn credentials_compare_and_set() {
    let w = world().await;
    let tok = w.provision("fake").await;
    let (_, a) = w.register(&tok, &base_manifest("fake")).await;
    w.approve("fake").await;
    let (_, b) = w.register(&tok, &base_manifest("fake")).await;
    let i = w
        .integration("fake", "T1", json!({"refresh": "r1"}), "active")
        .await;
    let (_, _, cfg) = w.config(&tok, a, None).await;
    assert_eq!(ids(&cfg), sorted(&[i]));
    let etag = cfg["etag"].as_str().unwrap().to_owned();

    let write = |replica: Uuid, target: Value, version: u64, creds: Value| {
        let w = &w;
        let tok = tok.clone();
        async move {
            let (s, _, v) = w
                .plane(
                    &tok,
                    Some(replica),
                    Method::POST,
                    "/credentials",
                    Some(json!({"target": target, "expected_version": version, "credentials": creds})),
                    &[],
                )
                .await;
            (s, v)
        }
    };
    // Only the owner may write.
    let (s, _) = write(
        b,
        json!({"integration_id": i}),
        1,
        json!({"refresh": "evil"}),
    )
    .await;
    assert_eq!(s, 404);
    // Exactly one target.
    let (s, _) = write(a, json!({}), 1, json!({})).await;
    assert_eq!(s, 400);
    // Success bumps the version.
    let (s, v) = write(a, json!({"integration_id": i}), 1, json!({"refresh": "r2"})).await;
    assert_eq!((s, v["version"].as_u64()), (200, Some(2)), "{v}");
    // Stale version.
    let (s, v) = write(a, json!({"integration_id": i}), 1, json!({"refresh": "r3"})).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_version")),
        "{v}"
    );
    // The new credential is served with the new version, under a new ETag.
    let (s, _, cfg) = w.config(&tok, a, Some(&etag)).await;
    assert_eq!(s, 200);
    assert_eq!(cfg["integrations"][0]["credentials"]["refresh"], "r2");
    assert_eq!(cfg["integrations"][0]["credentials_version"], 2);
    assert_ne!(cfg["etag"], etag);
}

#[tokio::test]
async fn config_serves_presences_and_bindings_and_presence_credentials_cas() {
    let w = world().await;
    let tok = w.provision("fake").await;
    let (_, a) = w.register(&tok, &base_manifest("fake")).await;
    w.approve("fake").await;
    let i = w
        .integration("fake", "T1", json!({"shared": 1}), "active")
        .await;
    let ws = w.h.workspace(w.org).await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'daneel','Daneel','hosted','fake')",
    )
    .bind(agent)
    .bind(w.org)
    .bind(ws)
    .execute(w.h.pool())
    .await
    .unwrap();
    let pid = Uuid::now_v7();
    let pref =
        w.h.state
            .vault
            .seal(
                &presence_seal_ctx(pid, w.org),
                Secret::from(serde_json::to_vec(&json!({"bot": "b1"})).unwrap()),
            )
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO channel_presences (id, agent_id, integration_id, mode, platform_user_ref,
            handle, credentials_ref, status) VALUES ($1,$2,$3,'dedicated','U1','daneel',$4,'active')",
    )
    .bind(pid)
    .bind(agent)
    .bind(i)
    .bind(pref.to_column())
    .execute(w.h.pool())
    .await
    .unwrap();
    // A pending presence and an inactive binding are not served.
    let agent2 = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'pending','Pending','hosted','fake')",
    )
    .bind(agent2)
    .bind(w.org)
    .bind(ws)
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO channel_presences (id, agent_id, integration_id, mode, status)
         VALUES ($1,$2,$3,'shared','pending_setup')",
    )
    .bind(Uuid::now_v7())
    .bind(agent2)
    .bind(i)
    .execute(w.h.pool())
    .await
    .unwrap();
    for (space, active) in [("C1", true), ("C2", false)] {
        sqlx::query(
            "INSERT INTO space_bindings (id, integration_id, space_ref, space_label, agent_id, active)
             VALUES ($1,$2,$3,$3,$4,$5)",
        )
        .bind(Uuid::now_v7())
        .bind(i)
        .bind(space)
        .bind(agent)
        .bind(active)
        .execute(w.h.pool())
        .await
        .unwrap();
    }

    let (s, _, cfg) = w.config(&tok, a, None).await;
    assert_eq!(s, 200, "{cfg}");
    let p = cfg["presences"].as_array().unwrap();
    assert_eq!(p.len(), 1, "{cfg}");
    assert_eq!(p[0]["id"], pid.to_string());
    assert_eq!(p[0]["mode"], "dedicated");
    assert_eq!(p[0]["agent"]["handle"], "daneel");
    assert_eq!(p[0]["credentials"]["bot"], "b1");
    assert_eq!(p[0]["credentials_version"], 1);
    let b = cfg["space_bindings"].as_array().unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0]["space_ref"], "C1");
    assert_eq!(b[0]["listen"], "mentions");

    let (s, _, v) = w
        .plane(
            &tok,
            Some(a),
            Method::POST,
            "/credentials",
            Some(json!({"target": {"presence_id": pid}, "expected_version": 1, "credentials": {"bot": "b2"}})),
            &[],
        )
        .await;
    assert_eq!((s, v["version"].as_u64()), (200, Some(2)), "{v}");
    let (s, _, v) = w
        .plane(
            &tok,
            Some(a),
            Method::POST,
            "/credentials",
            Some(json!({"target": {"presence_id": pid}, "expected_version": 1, "credentials": {"bot": "b3"}})),
            &[],
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_version")),
        "{v}"
    );
    let (_, _, cfg) = w.config(&tok, a, None).await;
    assert_eq!(cfg["presences"][0]["credentials"]["bot"], "b2");
}

#[tokio::test]
async fn retire_releases_leases() {
    let w = world().await;
    let tok = w.provision("fake").await;
    let (_, a) = w.register(&tok, &base_manifest("fake")).await;
    w.approve("fake").await;
    let (_, b) = w.register(&tok, &base_manifest("fake")).await;
    let i = w.integration("fake", "T1", json!({"k": 1}), "active").await;
    let (_, _, cfg) = w.config(&tok, a, None).await;
    assert_eq!(ids(&cfg), sorted(&[i]));
    // B (quota 1 of 1 integration, live=2) cannot take it while A holds the lease.
    let (_, _, cfg) = w.config(&tok, b, None).await;
    assert!(cfg["integrations"].as_array().unwrap().is_empty());

    let (s, _, v) = w
        .plane(&tok, Some(a), Method::POST, "/retire", None, &[])
        .await;
    assert_eq!((s, v["ok"].as_bool()), (200, Some(true)), "{v}");
    assert_eq!(w.owner(i).await, None);
    let retired: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT retired_at FROM connector_replicas WHERE id=$1")
            .bind(a)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(retired.is_some());
    // The retired replica is refused from now on.
    let (s, _, v) = w.config(&tok, a, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("replica_unknown")),
        "{v}"
    );
    // B takes over at its next poll.
    let (_, _, cfg) = w.config(&tok, b, None).await;
    assert_eq!(ids(&cfg), sorted(&[i]));
}

#[tokio::test]
async fn plane_authentication() {
    let w = world().await;
    let tok = w.provision("fake").await;
    let (_, a) = w.register(&tok, &base_manifest("fake")).await;
    w.approve("fake").await;

    // No token / wrong token / wrong prefix.
    let (s, _, _) =
        w.h.send(Method::GET, "/v1/connector/config", &[], None)
            .await;
    assert_eq!(s, 401);
    for bad in ["aun_nope", "aur_nope", "nope"] {
        let (s, _, _) = w
            .plane(bad, Some(a), Method::GET, "/config", None, &[])
            .await;
        assert_eq!(s, 401, "{bad}");
        let (s, _) = w.register_raw(bad, &base_manifest("fake"), "x").await;
        assert_eq!(s, 401, "{bad}");
    }
    // Missing / malformed / unknown replica header.
    let (s, _, v) = w.plane(&tok, None, Method::GET, "/config", None, &[]).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("replica_unknown")),
        "{v}"
    );
    let (s, _, _) = w
        .plane(
            &tok,
            None,
            Method::GET,
            "/config",
            None,
            &[("x-aurora-replica", "not-a-uuid")],
        )
        .await;
    assert_eq!(s, 403);
    let (s, _, _) = w
        .plane(
            &tok,
            Some(Uuid::now_v7()),
            Method::GET,
            "/config",
            None,
            &[],
        )
        .await;
    assert_eq!(s, 403);
    for (m, p) in [(Method::POST, "/credentials"), (Method::POST, "/retire")] {
        let (s, _, _) = w.plane(&tok, None, m, p, Some(json!({})), &[]).await;
        assert_eq!(s, 403, "{p}");
    }
    // Newer protocol version.
    let (s, _, v) = w
        .plane(
            &tok,
            Some(a),
            Method::GET,
            "/config",
            None,
            &[("x-aurora-protocol", "connector/2")],
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (426, Some("protocol_unsupported")),
        "{v}"
    );
    let (s, _, _) = w
        .plane(
            &tok,
            Some(a),
            Method::GET,
            "/config",
            None,
            &[("x-aurora-protocol", "connector/1")],
        )
        .await;
    assert_eq!(s, 200);

    // Rotating the token invalidates the old one.
    let (s, v) = w
        .admin(Method::POST, "/v1/admin/connectors/fake/token", None)
        .await;
    assert_eq!(s, 200, "{v}");
    let new = v["token"].as_str().unwrap().to_owned();
    assert!(new.starts_with("aun_") && new != tok);
    let (s, _, _) = w.config(&tok, a, None).await;
    assert_eq!(s, 401);
    let (s, _, _) = w.config(&new, a, None).await;
    assert_eq!(s, 200);

    // A disabled connector is refused everywhere.
    sqlx::query("UPDATE connectors SET status='disabled' WHERE id='fake'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, _, v) = w.config(&new, a, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("connector_disabled")),
        "{v}"
    );
    let (s, v) = w
        .register_raw(
            &new,
            &base_manifest("fake"),
            &w.sign(&base_manifest("fake")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("connector_disabled")),
        "{v}"
    );
}

#[tokio::test]
async fn admin_routes_are_instance_admin_only() {
    let w = world().await;
    let user = w.h.user("u@x.io", false).await;
    let (_, t) = w.h.good_session(user).await;
    let uc = cookie(&t);
    let tok = w.provision("fake").await;
    let _ = tok;
    for (m, p, body) in [
        (Method::GET, "/v1/admin/connectors", None),
        (
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "x1", "display_name": "x"})),
        ),
        (Method::DELETE, "/v1/admin/connectors/fake", None),
        (
            Method::POST,
            "/v1/admin/connectors/fake/approve",
            Some(json!({"manifest_digest": "x"})),
        ),
        (Method::POST, "/v1/admin/connectors/fake/token", None),
    ] {
        let (s, _, v) = w.h.send(m.clone(), p, &hd(&uc), body.clone()).await;
        assert_eq!(s, 403, "{m} {p}: {v}");
        let (s, _, _) = w.h.send(m.clone(), p, &[("origin", ORIGIN)], body).await;
        assert_eq!(s, 401, "{m} {p}");
    }
    // Validation, duplicates, unknown ids.
    for bad in [
        json!({"id": "A", "display_name": "x"}),
        json!({"id": "x", "display_name": "x"}),
        json!({"id": "ok-id", "display_name": "  "}),
    ] {
        let (s, v) = w
            .admin(Method::POST, "/v1/admin/connectors", Some(bad.clone()))
            .await;
        assert_eq!(s, 400, "{bad}: {v}");
    }
    let (s, v) = w
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "fake", "display_name": "dup"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("conflict")),
        "{v}"
    );
    let (s, _) = w
        .admin(Method::POST, "/v1/admin/connectors/nope/token", None)
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .admin(Method::DELETE, "/v1/admin/connectors/nope", None)
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .admin(
            Method::POST,
            "/v1/admin/connectors/nope/approve",
            Some(json!({"manifest_digest": "x"})),
        )
        .await;
    assert_eq!(s, 404);
    // Cannot approve before any registration.
    let (s, v) = w
        .admin(
            Method::POST,
            "/v1/admin/connectors/fake/approve",
            Some(json!({"manifest_digest": "x"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_manifest")),
        "{v}"
    );
}

#[tokio::test]
async fn delete_is_blocked_while_integrations_exist() {
    let w = world().await;
    let _ = w.provision("fake").await;
    let _ = w.provision("empty").await;
    w.integration("fake", "T1", json!({}), "disabled").await;
    let (s, v) = w
        .admin(Method::DELETE, "/v1/admin/connectors/fake", None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("connector_in_use")),
        "{v}"
    );
    let (s, _) = w
        .admin(Method::DELETE, "/v1/admin/connectors/empty", None)
        .await;
    assert_eq!(s, 204);
    let (_, v) = w.admin(Method::GET, "/v1/admin/connectors", None).await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["integration_count"], 1);
}

#[tokio::test]
async fn catalogue_lists_only_active_connectors_with_sanitized_icon() {
    let w = world().await;
    let user = w.h.user("u@x.io", false).await;
    let (_, t) = w.h.good_session(user).await;
    let uc = cookie(&t);
    let tok = w.provision("fake").await;
    let _ = w.provision("silent").await;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut m = base_manifest("fake");
    m["about"]["icon_png_b64"] = json!(b64.encode(png_stub(64, 64)));
    m["about"]["setup_guide_md"] = json!("# Guide\n\nPaste the **token**.");
    w.register(&tok, &m).await;

    let list = |c: String| {
        let w = &w;
        async move {
            let (s, _, v) =
                w.h.send(Method::GET, "/v1/connectors", &[("cookie", &c)], None)
                    .await;
            assert_eq!(s, 200, "{v}");
            v["items"].as_array().unwrap().clone()
        }
    };
    // Pending: not listed, icon not served.
    assert!(list(uc.clone()).await.is_empty());
    let (s, _, _) =
        w.h.send(
            Method::GET,
            "/v1/connectors/fake/icon",
            &[("cookie", &uc)],
            None,
        )
        .await;
    assert_eq!(s, 404);
    w.approve("fake").await;

    let items = list(uc.clone()).await;
    assert_eq!(items.len(), 1);
    let c = &items[0];
    assert_eq!(c["id"], "fake");
    assert_eq!(c["icon_url"], "/v1/connectors/fake/icon");
    assert_eq!(c["setup_guide_md"], "# Guide\n\nPaste the **token**.");
    assert_eq!(c["capabilities"]["threads"], true);
    assert_eq!(c["integration_setup"]["fields"][0]["name"], "token");
    assert_eq!(c["presence_setup"], json!({}));
    assert!(c.get("manifest").is_none() && c.get("manifest_digest").is_none());

    let req = http::Request::builder()
        .uri("/v1/connectors/fake/icon")
        .header("cookie", &uc)
        .body(axum::body::Body::empty())
        .unwrap();
    let res = tower::ServiceExt::oneshot(w.h.app.clone(), req)
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(bytes.as_ref(), png_stub(64, 64).as_slice());

    // No credentials: 401. Unknown connector / one without a manifest: 404.
    let (s, _, _) = w.h.send(Method::GET, "/v1/connectors", &[], None).await;
    assert_eq!(s, 401);
    let (s, _, _) =
        w.h.send(
            Method::GET,
            "/v1/connectors/silent/icon",
            &[("cookie", &uc)],
            None,
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn manifest_assets_and_strings_are_validated() {
    let w = world().await;
    let tok = w.provision("fake").await;
    let b64 = base64::engine::general_purpose::STANDARD;
    type Mutate = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Mutate)> = vec![
        (
            "small icon",
            Box::new(|m| {
                m["about"]["icon_png_b64"] =
                    json!(base64::engine::general_purpose::STANDARD.encode(png_stub(16, 16)))
            }),
        ),
        (
            "not png",
            Box::new(|m| {
                m["about"]["icon_png_b64"] = json!(
                    base64::engine::general_purpose::STANDARD
                        .encode(b"hello world, not a png at all")
                )
            }),
        ),
        (
            "not base64",
            Box::new(|m| m["about"]["icon_png_b64"] = json!("***")),
        ),
        (
            "raw html guide",
            Box::new(|m| m["about"]["setup_guide_md"] = json!("hi <script>alert(1)</script>")),
        ),
        (
            "control chars in name",
            Box::new(|m| m["about"]["name"] = json!("Fa\u{202e}ke")),
        ),
        (
            "newline in callback",
            Box::new(|m| m["public_callbacks"] = json!(["a\nb"])),
        ),
        (
            "contract 2",
            Box::new(|m| m["package"]["contract"] = json!(2)),
        ),
    ];
    let _ = b64;
    for (name, f) in cases {
        let mut m = base_manifest("fake");
        f(&mut m);
        let (s, v) = w.register_raw(&tok, &m, &w.sign(&m)).await;
        assert_eq!(s, 400, "{name}: {v}");
    }
    assert_eq!(w.connector("fake").await["status"], "awaiting_registration");
}

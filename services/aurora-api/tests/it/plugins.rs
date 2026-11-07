#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_api::domains::plugins::scan::{self, Seen};
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn yaml(publisher: &str, version: &str, hosts: &[&str]) -> String {
    let mut s = format!(
        "package: {{ id: acme, version: {version}, contract: 1, min_core: 0.1.0, publisher: {publisher}, name: Acme }}\napps:\n  - about: {{ id: acme_app, name: A }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n"
    );
    for h in hosts {
        s += &format!(
            "      - match: {{ host: \"{h}\" }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n"
        );
    }
    s
}

fn signed(y: &str, k: &SigningKey) -> Vec<u8> {
    let mut b = PackageBuilder::new(y);
    b.sign(k, &PackageBuilder::key_id(k));
    b.build().unwrap()
}

fn unsigned(y: &str) -> Vec<u8> {
    PackageBuilder::new(y).build().unwrap()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

struct Admin<'a> {
    h: &'a Harness,
    cookie: String,
}

impl Admin<'_> {
    async fn json(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(
                m,
                uri,
                &[("cookie", &self.cookie), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn upload(&self, bytes: &[u8]) -> (u16, Value) {
        let boundary = "XBOUNDARYX";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"package\"; filename=\"p.aurora-plugin\"\r\nContent-Type: application/gzip\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let req = Request::builder()
            .method(Method::POST)
            .uri("/v1/admin/plugins")
            .header("cookie", &self.cookie)
            .header("origin", ORIGIN)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let b = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    async fn trust(&self, publisher: &str, k: &SigningKey) -> Value {
        let (s, v) = self
            .json(
                Method::POST,
                "/v1/admin/trusted-keys",
                Some(json!({"publisher": publisher, "name": "Acme key",
                    "public_key": PackageBuilder::key_id(k)})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v
    }
}

async fn setup(h: &Harness) -> Admin<'_> {
    let u = h.user("root@x.io", true).await;
    let (_, tok) = h.good_session(u).await;
    Admin {
        h,
        cookie: cookie(&tok),
    }
}

async fn count(h: &Harness) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM plugin_packages")
        .fetch_one(h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn unsigned_refused_then_allowed_with_dev_flag() {
    let y = yaml("acme", "1.0.0", &["api.acme.example"]);
    let h = Harness::new().await;
    let a = setup(&h).await;
    let (s, v) = a.upload(&unsigned(&y)).await;
    assert_eq!((s, code(&v)), (422, "package_invalid"));
    assert_eq!(
        v["error"]["details"]["violations"][0]["code"],
        "signature_missing"
    );
    assert_eq!(count(&h).await, 0);

    let h = Harness::with_config(|c| c.plugins.allow_unsigned = true).await;
    let a = setup(&h).await;
    let (s, v) = a.upload(&unsigned(&y)).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "pending_approval");
    assert_eq!(v["status_reason"], "first_install");
    assert_eq!(v["signer"]["kind"], "unsigned");
    assert_eq!(v["apps"][0]["app_id"], "acme_app");
    assert_eq!(v["host_claims"][0]["host"], "api.acme.example");
}

#[tokio::test]
async fn untrusted_builtin_and_non_admin_refused() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let k = generate_signing_key().unwrap();
    let (s, v) = a
        .upload(&signed(&yaml("acme", "1.0.0", &["api.acme.example"]), &k))
        .await;
    assert_eq!((s, code(&v)), (422, "package_invalid"));
    assert_eq!(
        v["error"]["details"]["violations"][0]["code"],
        "signature_untrusted"
    );
    // `builtin` signatures are refused while no project key is compiled in.
    let mut b = PackageBuilder::new(&yaml("aurora", "1.0.0", &["api.acme.example"]));
    b.sign(&k, "builtin");
    let (s, v) = a.upload(&b.build().unwrap()).await;
    assert_eq!((s, code(&v)), (422, "package_invalid"));
    assert_eq!(count(&h).await, 0);

    let plain = h.user("p@x.io", false).await;
    let (_, tok) = h.good_session(plain).await;
    let (s, _, v) = h
        .send(
            Method::GET,
            "/v1/admin/plugins",
            &[("cookie", &cookie(&tok))],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "instance_admin_required"));
}

#[tokio::test]
async fn trusted_key_pending_idempotent_and_invalid_rows() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let k = generate_signing_key().unwrap();
    let key = a.trust("acme", &k).await;
    // Duplicate key conflicts; project publisher is reserved.
    let (s, _) = a
        .json(
            Method::POST,
            "/v1/admin/trusted-keys",
            Some(
                json!({"publisher": "acme", "name": "x", "public_key": PackageBuilder::key_id(&k)}),
            ),
        )
        .await;
    assert_eq!(s, 409);
    let (s, _) = a
        .json(
            Method::POST,
            "/v1/admin/trusted-keys",
            Some(json!({"publisher": "aurora", "name": "x", "public_key": PackageBuilder::key_id(&generate_signing_key().unwrap())})),
        )
        .await;
    assert_eq!(s, 400);

    let pkg = signed(&yaml("acme", "1.0.0", &["api.acme.example"]), &k);
    let (s, v) = a.upload(&pkg).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "pending_approval");
    assert_eq!(v["signer"]["kind"], "trusted_key");
    assert_eq!(v["signer"]["name"], "Acme key");
    assert_eq!(v["signer"]["key_id"], key["public_key"]);
    let (s, v2) = a.upload(&pkg).await;
    assert_eq!(s, 201);
    assert_eq!(v2["id"], v["id"]);
    assert_eq!(count(&h).await, 1);
    let (s, got) = a
        .json(
            Method::GET,
            &format!("/v1/admin/plugins/{}", v["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!((s, &got["digest"]), (200, &v["digest"]));
    let (_, l) = a
        .json(
            Method::GET,
            "/v1/admin/plugins?status=pending_approval",
            None,
        )
        .await;
    assert_eq!(l["items"].as_array().unwrap().len(), 1);
    let (s, _) = a
        .json(Method::GET, "/v1/admin/plugins?status=bogus", None)
        .await;
    assert_eq!(s, 400);

    // Reserved host => invalid row carrying violations.
    let bad = signed(&yaml("acme", "2.0.0", &["x.aurora.internal"]), &k);
    let (s, v) = a.upload(&bad).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "invalid");
    assert!(!v["violations"].as_array().unwrap().is_empty());
    assert_eq!(count(&h).await, 2);

    // Manifest that fails strict parsing still yields an invalid row.
    let broken = signed(
        &format!(
            "{}bogus_key: 1\n",
            yaml("acme", "3.0.0", &["api.acme.example"])
        ),
        &k,
    );
    let (s, v) = a.upload(&broken).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "invalid");
    assert_eq!(v["violations"][0]["code"], "unknown_field");
}

async fn activate(h: &Harness, id: &str) {
    sqlx::query("UPDATE plugin_packages SET status='active', approved_at=now() WHERE id=$1::uuid")
        .bind(id)
        .execute(h.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn upgrades_narrower_auto_activate_widening_waits() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let k = generate_signing_key().unwrap();
    a.trust("acme", &k).await;
    let (_, v1) = a
        .upload(&signed(
            &yaml("acme", "1.0.0", &["api.acme.example", "b.acme.example"]),
            &k,
        ))
        .await;
    assert_eq!(v1["status"], "pending_approval");
    activate(&h, v1["id"].as_str().unwrap()).await;

    // Narrower: one claim fewer.
    let (s, v2) = a
        .upload(&signed(&yaml("acme", "1.1.0", &["api.acme.example"]), &k))
        .await;
    assert_eq!(s, 201, "{v2}");
    assert_eq!(v2["status"], "active", "{v2}");
    assert!(v2["approved_at"].is_string());
    let (_, old) = a
        .json(
            Method::GET,
            &format!("/v1/admin/plugins/{}", v1["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(old["status"], "superseded");

    // Widening: a new host claim waits for approval.
    let (_, v3) = a
        .upload(&signed(
            &yaml("acme", "1.2.0", &["api.acme.example", "c.acme.example"]),
            &k,
        ))
        .await;
    assert_eq!(v3["status"], "pending_approval");
    assert_eq!(v3["status_reason"], "permissions_widened");
    assert_eq!(v3["widenings"][0]["kind"], "host_claim");
    let (_, again) = a
        .json(
            Method::GET,
            &format!("/v1/admin/plugins/{}", v2["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(again["status"], "active");
}

#[tokio::test]
async fn key_revocation_disables_signed_packages() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let k = generate_signing_key().unwrap();
    let key = a.trust("acme", &k).await;
    let (_, v1) = a
        .upload(&signed(&yaml("acme", "1.0.0", &["api.acme.example"]), &k))
        .await;
    activate(&h, v1["id"].as_str().unwrap()).await;
    let kid = key["id"].as_str().unwrap();
    let (s, _) = a
        .json(
            Method::DELETE,
            &format!("/v1/admin/trusted-keys/{kid}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (_, got) = a
        .json(
            Method::GET,
            &format!("/v1/admin/plugins/{}", v1["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(got["status"], "disabled");
    assert_eq!(got["status_reason"], "signing_key_revoked");
    let (_, l) = a.json(Method::GET, "/v1/admin/trusted-keys", None).await;
    assert!(l["items"][0]["revoked_at"].is_string());
    let (s, v) = a
        .upload(&signed(&yaml("acme", "1.1.0", &["api.acme.example"]), &k))
        .await;
    assert_eq!(
        (s, v["error"]["details"]["violations"][0]["code"].as_str()),
        (422, Some("key_revoked"))
    );
    let (s, _) = a
        .json(
            Method::DELETE,
            &format!("/v1/admin/trusted-keys/{}", uuid::Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn directory_scan_picks_up_file_once() {
    let dir = std::env::temp_dir().join(format!("aurora-plugins-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(|c| {
        c.plugins.dir = d;
        c.plugins.allow_unsigned = true;
    })
    .await;
    std::fs::write(
        dir.join("acme.aurora-plugin"),
        unsigned(&yaml("acme", "1.0.0", &["api.acme.example"])),
    )
    .unwrap();
    std::fs::write(dir.join("junk.aurora-plugin"), b"not a package").unwrap();
    std::fs::write(dir.join("ignored.txt"), b"x").unwrap();
    let mut seen = Seen::new();
    assert_eq!(scan::scan_once(&h.state, &mut seen).await, 1);
    assert_eq!(scan::scan_once(&h.state, &mut seen).await, 0);
    // A restart (fresh memory) is still idempotent by digest.
    assert_eq!(scan::scan_once(&h.state, &mut Seen::new()).await, 0);
    assert_eq!(count(&h).await, 1);
    let src: String = sqlx::query_scalar("SELECT source FROM plugin_packages")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(src, "directory");
    assert_eq!(scan::scan_locked(&h.state, &mut Seen::new()).await, Some(0));
    std::fs::remove_dir_all(&dir).ok();
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

/// SEC-32: trusted keys, plugin ingest and activation are audited.
#[tokio::test]
async fn sec32_trusted_keys_and_plugin_lifecycle_are_audited() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let k = generate_signing_key().unwrap();
    let key = a.trust("acme", &k).await;
    assert_eq!(audit_n(&h, "trusted_key.create").await, 1);
    let (_, v1) = a
        .upload(&signed(&yaml("acme", "1.0.0", &["api.acme.example"]), &k))
        .await;
    assert!(audit_n(&h, "plugin.ingest").await >= 1);
    let _ = &v1;
    let kid = key["id"].as_str().unwrap();
    let (s, _) = a
        .json(
            Method::DELETE,
            &format!("/v1/admin/trusted-keys/{kid}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(audit_n(&h, "trusted_key.revoke").await, 1);
}

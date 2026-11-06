#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use aurora_api::domains::plugins::{internal, registry};
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use base64::Engine;
use common::*;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

fn yaml(pkg: &str, version: &str, host: &str) -> String {
    format!(
        "package: {{ id: {pkg}, version: {version}, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}\napps:\n  - about: {{ id: {pkg}_app, name: A }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n      - match: {{ host: \"{host}\" }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n"
    )
}

fn signed(y: &str, k: &SigningKey) -> Vec<u8> {
    let mut b = PackageBuilder::new(y);
    b.sign(k, &PackageBuilder::key_id(k));
    b.build().unwrap()
}

struct Admin<'a> {
    h: &'a Harness,
    cookie: String,
    key: SigningKey,
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

    async fn upload(&self, y: &str) -> Value {
        let boundary = "XBOUNDARYX";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"package\"; filename=\"p.aurora-plugin\"\r\nContent-Type: application/gzip\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&signed(y, &self.key));
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
        assert_eq!(res.status(), 201);
        let b = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&b).unwrap()
    }

    async fn act(&self, id: &Value, action: &str) -> (u16, Value) {
        self.json(
            Method::POST,
            &format!("/v1/admin/plugins/{}/{action}", id.as_str().unwrap()),
            Some(json!({})),
        )
        .await
    }

    async fn status(&self, id: &Value) -> Value {
        self.json(
            Method::GET,
            &format!("/v1/admin/plugins/{}", id.as_str().unwrap()),
            None,
        )
        .await
        .1
    }
}

async fn setup(h: &Harness) -> Admin<'_> {
    let u = h.user("root@x.io", true).await;
    let (_, tok) = h.good_session(u).await;
    let a = Admin {
        h,
        cookie: cookie(&tok),
        key: generate_signing_key().unwrap(),
    };
    let (s, v) = a
        .json(
            Method::POST,
            "/v1/admin/trusted-keys",
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&a.key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    a
}

async fn snapshots(h: &Harness) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM registry_snapshots")
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn snapshot(a: &Admin<'_>) -> (u16, String, Value) {
    let (s, hd, v) =
        a.h.send(
            Method::GET,
            "/v1/admin/registry/snapshot",
            &[("cookie", &a.cookie)],
            None,
        )
        .await;
    let hash = hd
        .get("x-snapshot-hash")
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    (s.as_u16(), hash, v)
}

#[tokio::test]
async fn approve_activates_notifies_and_disable_removes() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let mut l = sqlx::postgres::PgListener::connect_with(h.pool())
        .await
        .unwrap();
    l.listen("aurora_registry").await.unwrap();
    let v = a.upload(&yaml("acme", "1.0.0", "api.acme.example")).await;
    assert_eq!(v["status"], "pending_approval");
    assert_eq!(snapshots(&h).await, 0);
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!((s, p["status"].as_str()), (200, Some("active")), "{p}");
    assert!(p["approved_by"]["display_name"].is_string());
    assert_eq!(snapshots(&h).await, 1);
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), l.recv())
        .await
        .unwrap()
        .unwrap();
    let payload: Value = serde_json::from_str(n.payload()).unwrap();
    let (s, hash, snap) = snapshot(&a).await;
    assert_eq!((s, payload["hash"].as_str()), (200, Some(hash.as_str())));
    assert_eq!(snap["packages"][0]["package_id"], "acme");
    assert_eq!(snap["hosts"][0]["claim_status"], "active");

    assert_eq!(a.act(&v["id"], "approve").await.0, 409);
    let (s, p) = a.act(&v["id"], "disable").await;
    assert_eq!((s, p["status"].as_str()), (200, Some("disabled")));
    let (_, hash2, snap) = snapshot(&a).await;
    assert_ne!(hash, hash2);
    assert!(snap["packages"].as_array().unwrap().is_empty());
    // Enable restores the earlier content (same hash, re-activated).
    let (s, p) = a.act(&v["id"], "enable").await;
    assert_eq!((s, p["status"].as_str()), (200, Some("active")));
    assert_eq!(snapshot(&a).await.1, hash);
    assert_eq!(snapshots(&h).await, 2);

    let v2 = a.upload(&yaml("other", "1.0.0", "api.other.example")).await;
    let (s, p) = a.act(&v2["id"], "reject").await;
    assert_eq!((s, p["status"].as_str()), (200, Some("rejected")));
    assert_eq!(a.act(&v2["id"], "enable").await.0, 409);

    // Disabling a never-approved package and enabling it must not bypass approval.
    let v3 = a.upload(&yaml("third", "1.0.0", "api.third.example")).await;
    assert_eq!(v3["status"], "pending_approval");
    let (s, p) = a.act(&v3["id"], "disable").await;
    assert_eq!((s, p["status"].as_str()), (200, Some("disabled")), "{p}");
    let (s, p) = a.act(&v3["id"], "enable").await;
    assert_eq!((s, p["error"]["code"].as_str()), (409, Some("invalid_state")), "{p}");
}

#[tokio::test]
async fn claim_conflict_held_until_decision_then_loser_lost() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let one = a.upload(&yaml("one", "1.0.0", "shared.example")).await;
    assert_eq!(a.act(&one["id"], "approve").await.0, 200);
    let two = a.upload(&yaml("two", "1.0.0", "shared.example")).await;
    assert_eq!(two["status_reason"], "claim_conflict");
    let (s, e) = a.act(&two["id"], "approve").await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("claim_conflict_unresolved"))
    );
    let st = a.status(&two["id"]).await;
    assert_eq!(
        (st["status"].as_str(), st["status_reason"].as_str()),
        (Some("pending_approval"), Some("claim_conflict"))
    );

    let (_, l) = a.json(Method::GET, "/v1/admin/host-claims", None).await;
    let g = &l["items"][0];
    assert_eq!(l["items"].as_array().unwrap().len(), 1);
    assert!(g["decision"].is_null());
    let key = g["group_key"].as_str().unwrap().to_owned();
    let fp = g["fingerprint"].as_str().unwrap().to_owned();
    let uri = format!(
        "/v1/admin/host-claims/{}",
        key.replace('|', "%7C").replace('/', "%2F")
    );
    let body = |fp: &str| json!({"winner_package_id": "one", "winner_app_id": "one_app", "fingerprint": fp});
    let (s, e) = a.json(Method::PUT, &uri, Some(body("bad"))).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("stale_fingerprint"))
    );
    let (s, _) = a
        .json(
            Method::PUT,
            &uri,
            Some(json!({"winner_package_id": "zzz", "winner_app_id": "x", "fingerprint": fp})),
        )
        .await;
    assert_eq!(s, 400);
    let (s, d) = a.json(Method::PUT, &uri, Some(body(&fp))).await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["decision"]["winner_package_id"], "one");
    assert_eq!(d["decision"]["stale"], false);

    assert_eq!(a.act(&two["id"], "approve").await.0, 200);
    let (_, _, snap) = snapshot(&a).await;
    let status = |app: &str| {
        snap["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["app_id"] == app)
            .unwrap()["claim_status"]
            .clone()
    };
    assert_eq!(status("one_app"), "active");
    assert_eq!(status("two_app"), "lost");

    // A changed fingerprint makes the decision stale; compile holds the newcomer.
    sqlx::query("UPDATE host_claim_decisions SET claim_fingerprint = 'old'")
        .execute(h.pool())
        .await
        .unwrap();
    let (_, l) = a.json(Method::GET, "/v1/admin/host-claims", None).await;
    assert_eq!(l["items"][0]["decision"]["stale"], true);
    registry::request_compile(&h.state).await.unwrap();
    let st = a.status(&two["id"]).await;
    assert_eq!(
        (st["status"].as_str(), st["status_reason"].as_str()),
        (Some("pending_approval"), Some("claim_conflict"))
    );
    assert_eq!(a.status(&one["id"]).await["status"], "active");
}

#[tokio::test]
async fn uninstall_requires_force_when_connections_live() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let v = a.upload(&yaml("acme", "1.0.0", "api.acme.example")).await;
    assert_eq!(a.act(&v["id"], "approve").await.0, 200);
    let org = h.org().await;
    let conn = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,'acme_app','acme','1.0.0','c','active','ref')")
        .bind(conn)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    let uri = format!("/v1/admin/plugins/{}", v["id"].as_str().unwrap());
    let (s, e) = a.json(Method::DELETE, &uri, None).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("live_connections"))
    );
    assert_eq!(e["error"]["details"]["count"], 1);
    let (s, _) = a
        .json(Method::DELETE, &format!("{uri}?force=true"), None)
        .await;
    assert_eq!(s, 204);
    let st: String = sqlx::query_scalar("SELECT status FROM app_connections WHERE id=$1")
        .bind(conn)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(st, "orphaned");
    assert_eq!(a.json(Method::GET, &uri, None).await.0, 404);
    let (_, _, snap) = snapshot(&a).await;
    assert!(snap["apps"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn startup_creates_empty_snapshot_once() {
    let h = Harness::new().await;
    assert!(registry::ensure_initial(&h.state).await.unwrap());
    assert!(!registry::ensure_initial(&h.state).await.unwrap());
    assert_eq!(snapshots(&h).await, 1);
    let a = setup(&h).await;
    let (s, _, snap) = snapshot(&a).await;
    assert_eq!(s, 200);
    assert!(snap["packages"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn internal_routes_need_secret_and_are_not_public() {
    let h = Harness::new().await;
    registry::ensure_initial(&h.state).await.unwrap();
    let r = internal::router(h.state.clone());
    let get = |uri: &str, auth: Option<String>| {
        let mut b = Request::builder().uri(uri.to_owned());
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        b.body(Body::empty()).unwrap()
    };
    let res = r
        .clone()
        .oneshot(get("/internal/v1/registry/active", None))
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = r
        .clone()
        .oneshot(get(
            "/internal/v1/registry/active",
            Some("Bearer nope".into()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let bearer = format!(
        "Bearer {}",
        base64::engine::general_purpose::STANDARD.encode(h.state.config.internal_secret.expose())
    );
    let res = r
        .clone()
        .oneshot(get(
            "/internal/v1/registry/active?contract_max=1",
            Some(bearer.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let hash = res.headers()["x-snapshot-hash"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(res.headers()["x-snapshot-contract"], "1");
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let hex: String = Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hex, hash);
    let res = r
        .clone()
        .oneshot(get(
            &format!("/internal/v1/registry/blobs/{}", "0".repeat(64)),
            Some(bearer),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let res = r
        .oneshot(get(
            &format!("/internal/v1/registry/blobs/{}", "0".repeat(64)),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    let (s, _) = h.get("/internal/v1/registry/active", &[]).await;
    assert_eq!(s, 404);
}

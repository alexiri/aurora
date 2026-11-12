#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-ASRC (D-164, SEC-86, C-590..C-609): addon sources against an in-process source
//! server (wiremock) reached through the guarded client's test override. Real Postgres.

use aurora_api::domains::addon_sources;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use aurora_testkit::wiremock::matchers::{method, path, path_regex};
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::*;

const HOST: &str = "api.acme.example";
/// The https origin the (https-only) URLs use; the test override maps it to the
/// in-process mock (the connect flow's `override_origin`).
const SRC: &str = "https://src.acme.example";

// ---------------------------------------------------------------- package fixtures

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

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(b);
    let mut s = String::with_capacity(64);
    use std::fmt::Write;
    for x in d {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// The manifest listing `versions` (version, file name) of one addon.
fn manifest_yaml(
    publisher: &str,
    key: &SigningKey,
    base: &str,
    versions: &[(&str, &str)],
    package_id: &str,
) -> String {
    let mut s = format!(
        "api: 1\nrepo:\n  name: Acme addons\n  description: Test repo\n  homepage: https://acme.example\npublisher: {publisher}\nsigning_key: {}\naddons:\n  - id: {package_id}\n    name: Acme\n    description: The acme app\n    versions:\n",
        PackageBuilder::key_id(key)
    );
    for (v, file) in versions {
        s += &format!(
            "      \"{v}\":\n        package: {base}/pkg/{file}\n        sha256: PENDING_{v}\n"
        );
    }
    s
}

// ---------------------------------------------------------------- the world

/// The source's mutable content, shared with the responder closures.
#[derive(Default)]
struct Files {
    manifest: Option<Vec<u8>>,
    packages: std::collections::BTreeMap<String, Vec<u8>>,
    /// The manifest answers this instead of its content when set (failure/redirect).
    manifest_answer: Option<(u16, Option<String>)>,
}

type Shared = std::sync::Arc<std::sync::Mutex<Files>>;

/// A [`wiremock::Respond`] reading the current content: the closure gets the request
/// path and the shared state.
struct FromState(
    Shared,
    Box<dyn Fn(&str, &mut Files) -> ResponseTemplate + Send + Sync>,
);

impl wiremock::Respond for FromState {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        // A poisoned lock in a test server is not worth handling.
        let mut f = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (self.1)(&path, &mut f)
    }
}

struct W {
    h: Harness,
    #[allow(dead_code)]
    src: MockServer,
    files: Shared,
    base: String,
    admin: String,
    key: SigningKey,
}

impl W {
    async fn new() -> Self {
        let src = MockServer::start().await;
        let base = src.uri();
        let mut h = Harness::with_config(|c| {
            // The https source origin is mapped to the mock by the connect override and
            // accepted as a dashboard origin (unsafe cookie routes are CORS-checked).
            c.auth.trusted_origins.push(SRC.to_owned());
        })
        .await;
        h.state = h.state.clone().with_connect_override(&base);
        // The router was built with the pre-override state: rebuild it so the app's
        // `connect` Env is the overridden one (same pattern as connect.rs).
        h.app = test_router(h.state.clone());
        let admin_id = h.user("root@x.io", true).await;
        let (_, tok) = h.good_session(admin_id).await;
        let admin = cookie(&tok);

        // Two stable mocks reading the shared state: manifest and package files.
        let files: Shared = Shared::default();
        let m_state = files.clone();
        Mock::given(method("GET"))
            .and(path("/repo/aurora-addons.yaml"))
            .respond_with(FromState(
                m_state,
                Box::new(|_path: &str, f: &mut Files| {
                    if let Some((code, loc)) = &f.manifest_answer {
                        let mut t = ResponseTemplate::new(*code);
                        if let Some(l) = loc {
                            t = t.insert_header("location", l);
                        }
                        return t;
                    }
                    match &f.manifest {
                        Some(m) => ResponseTemplate::new(200)
                            .set_body_bytes(m.clone())
                            .insert_header("content-type", "application/yaml"),
                        None => ResponseTemplate::new(404),
                    }
                }),
            ))
            .mount(&src)
            .await;
        let p_state = files.clone();
        Mock::given(method("GET"))
            .and(path_regex("^/pkg/.+"))
            .respond_with(FromState(
                p_state,
                Box::new(|path: &str, f: &mut Files| {
                    let name = path.trim_start_matches("/pkg/");
                    match f.packages.get(name) {
                        Some(b) => ResponseTemplate::new(200)
                            .set_body_bytes(b.clone())
                            .insert_header("content-type", "application/octet-stream"),
                        None => ResponseTemplate::new(404),
                    }
                }),
            ))
            .mount(&src)
            .await;

        Self {
            h,
            src,
            files,
            base: SRC.to_owned(),
            admin,
            key: generate_signing_key().unwrap(),
        }
    }

    fn hd(&self) -> [(&'static str, &str); 2] {
        [("cookie", self.admin.as_str()), ("origin", ORIGIN)]
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.h.get(uri, &self.hd()).await
    }

    async fn send(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (s, _, v) = self.h.send(method, uri, &self.hd(), body).await;
        (s, v)
    }

    /// Serve `versions` (version → archive bytes) of addon `package_id` and set the
    /// manifest; returns the manifest text.
    async fn publish(&self, package_id: &str, versions: &[(&str, Vec<u8>)]) -> String {
        let entries: Vec<(String, String, Vec<u8>)> = versions
            .iter()
            .map(|(v, b)| {
                (
                    v.to_string(),
                    format!("{package_id}-{v}.aurora-plugin"),
                    b.clone(),
                )
            })
            .collect();
        let mut m = manifest_yaml(
            "acme",
            &self.key,
            self.base.trim_end_matches('/'),
            &entries
                .iter()
                .map(|(v, f, _)| (v.as_str(), f.as_str()))
                .collect::<Vec<_>>(),
            package_id,
        );
        let mut f = self.files.lock().unwrap_or_else(|e| e.into_inner());
        f.manifest_answer = None;
        for (v, file, bytes) in &entries {
            f.packages.insert(file.clone(), bytes.clone());
            m = m.replace(&format!("PENDING_{v}"), &sha256_hex(bytes));
        }
        f.manifest = Some(m.clone().into_bytes());
        drop(f);
        m
    }

    /// Register with a pasted repo URL (normalized to the manifest URL).
    async fn register(&self) -> (StatusCode, Value) {
        self.publish(
            "acme",
            &[("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &self.key))],
        )
        .await;
        self.send(
            Method::POST,
            "/v1/admin/addon-sources",
            Some(json!({"url": format!("{}/repo", self.base)})),
        )
        .await
    }

    async fn source_id(&self) -> Uuid {
        sqlx::query_scalar("SELECT id FROM addon_sources LIMIT 1")
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// The manifest answers `status` (unreachable-source tests).
    async fn fail_manifest(&self, status: u16) {
        self.files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .manifest_answer = Some((status, None));
    }

    /// The manifest answers with a redirect (never followed for a manifest).
    async fn redirect_manifest(&self, to: &str) {
        self.files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .manifest_answer = Some((302, Some(to.to_owned())));
    }
}

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn registration_is_tofu_over_the_manifest_key() {
    let w = W::new().await;
    w.publish(
        "acme",
        &[("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key))],
    )
    .await;
    // Preview first: nothing stored.
    let (s, v) = w
        .send(
            Method::POST,
            "/v1/admin/addon-sources/preview",
            Some(json!({"url": format!("{}/repo", w.base)})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["repo"]["name"], "Acme addons");
    assert_eq!(v["publisher"], "acme");
    assert!(
        v["key_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM addon_sources")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);

    // Register: source + trusted key, audited.
    let (s, v) = w.register().await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["publisher"], "acme");
    assert_eq!(v["addons"][0]["id"], "acme");
    // The URL was normalized to the manifest URL, never the pasted repo URL.
    assert_eq!(v["url"], format!("{}/repo/aurora-addons.yaml", w.base));

    let key: String = sqlx::query_scalar("SELECT public_key FROM trusted_keys")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(key, PackageBuilder::key_id(&w.key));

    // The first addon version is ingested but never auto-activated (D-164(b)).
    let pkg: (String, String) =
        sqlx::query_as("SELECT package_id, status FROM plugin_packages WHERE source = 'addon'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(pkg, ("acme".into(), "pending_approval".into()));

    // Critical audit rows for add; `addon.ingest` is best-effort written — poll briefly.
    let mut saw_ingest = false;
    for _ in 0..50 {
        let actions: Vec<String> = sqlx::query_scalar(
            "SELECT action FROM audit_log WHERE action LIKE 'addon%' ORDER BY action",
        )
        .fetch_all(w.h.pool())
        .await
        .unwrap();
        if actions.contains(&"addon.ingest".to_owned()) {
            saw_ingest = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(saw_ingest, "no addon.ingest audit row after registration");

    // The same URL cannot be registered twice.
    let (s, v) = w
        .send(
            Method::POST,
            "/v1/admin/addon-sources",
            Some(json!({"url": format!("{}/repo/aurora-addons.yaml", w.base)})),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    // List shows the source; the plugins DTO carries provenance.
    let (s, v) = w.get("/v1/admin/addon-sources").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["id"], id);

    let (s, v) = w.get("/v1/admin/plugins?status=pending_approval").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let item = &v["items"][0];
    assert_eq!(item["source"], "addon");
    assert_eq!(item["source_id"], id);
    assert_eq!(item["source_name"], "Acme addons");
}

#[tokio::test]
async fn a_new_version_arrives_only_through_a_check() {
    let w = W::new().await;
    w.register().await;

    // Publish 1.1.0 upstream; nothing ingests until a check runs.
    w.publish(
        "acme",
        &[
            ("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key)),
            ("1.1.0", signed(&yaml("acme", "1.1.0", &[HOST]), &w.key)),
        ],
    )
    .await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM plugin_packages WHERE source = 'addon'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);

    // "Check now" ingests it. An identical permission set activates automatically
    // (the ordinary rules; only the *first* addon version waits for approval).
    let sid = w.source_id().await;
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "updated", "{v}");
    assert_eq!(v["ingested"].as_array().unwrap().len(), 1);
    assert_eq!(v["ingested"][0]["version"], "1.1.0");

    // Check again: up_to_date, nothing new.
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "up_to_date");
    assert_eq!(v["ingested"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn digest_mismatch_and_foreign_signature_never_parse() {
    let w = W::new().await;
    w.register().await;
    let sid = w.source_id().await;

    let good = signed(&yaml("acme", "2.0.0", &[HOST]), &w.key);
    let m = w
        .publish(
            "acme",
            &[
                ("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key)),
                ("2.0.0", good.clone()),
            ],
        )
        .await;
    // Corrupt the digest of 2.0.0: the archive is never parsed.
    let bad = m.replace(&sha256_hex(&good), &"a".repeat(64));
    w.files.lock().unwrap_or_else(|e| e.into_inner()).manifest = Some(bad.into_bytes());
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v["failed_versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["version"] == "2.0.0"),
        "{v}"
    );
    assert_eq!(v["ingested"].as_array().unwrap().len(), 0);

    // A package signed by a *different* key is refused (the source's key is the only
    // trust anchor; a foreign key is not in the store under this publisher either way).
    let other = generate_signing_key().unwrap();
    let forged = signed(&yaml("acme", "3.0.0", &[HOST]), &other);
    w.publish(
        "acme",
        &[
            ("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key)),
            ("3.0.0", forged),
        ],
    )
    .await;
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ingested"].as_array().unwrap().len(), 0);
    assert!(
        v["failed_versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["version"] == "3.0.0"),
        "{v}"
    );
}

#[tokio::test]
async fn a_changed_signing_key_never_auto_trusts() {
    let mut w = W::new().await;
    w.register().await;
    let sid = w.source_id().await;

    // The repo publishes under a new key.
    let k2 = generate_signing_key().unwrap();
    w.key = k2;
    w.publish(
        "acme",
        &[("1.1.0", signed(&yaml("acme", "1.1.0", &[HOST]), &w.key))],
    )
    .await;

    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "failed");

    let v = w.get(&format!("/v1/admin/addon-sources/{sid}")).await.1;
    assert_eq!(v["status"], "signing_key_changed");
    let new_fp = v["new_key_fingerprint"].as_str().unwrap().to_owned();
    assert!(new_fp.starts_with("sha256:"));

    // Nothing new ingested; the trusted key row still holds the OLD key.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM plugin_packages WHERE source = 'addon'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);

    // Re-confirm with the fingerprint the admin saw: source + trusted key move together.
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/confirm-key"),
            Some(json!({"new_key_fingerprint": new_fp})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "ok");
    let key: String = sqlx::query_scalar("SELECT public_key FROM trusted_keys")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(key, PackageBuilder::key_id(&w.key));

    // The next check ingests the queued version.
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "updated");
}

#[tokio::test]
async fn unreachable_source_fails_cleanly_and_keeps_state() {
    let w = W::new().await;
    w.register().await;
    let sid = w.source_id().await;

    w.fail_manifest(500).await;
    let (s, _v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let v = w.get(&format!("/v1/admin/addon-sources/{sid}")).await.1;
    assert_eq!(v["status"], "check_failed");
    assert!(v["check_failed_reason"].as_str().is_some());
    // The stored state still answers (name, url, created_at) with the addons degraded.
    assert_eq!(v["url"], format!("{}/repo/aurora-addons.yaml", w.base));
    assert!(v["created_at"].is_string());

    // Recovery: a healthy source checks fine again.
    w.publish(
        "acme",
        &[("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key))],
    )
    .await;
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/addon-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "up_to_date");
}

#[tokio::test]
async fn the_manifest_is_never_fetched_through_a_redirect() {
    let w = W::new().await;
    w.publish(
        "acme",
        &[("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key))],
    )
    .await;
    w.redirect_manifest(&format!("{}/elsewhere", w.base)).await;
    let (s, v) = w
        .send(
            Method::POST,
            "/v1/admin/addon-sources/preview",
            Some(json!({"url": format!("{}/repo/aurora-addons.yaml", w.base)})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["error"]["code"], "addon_unreachable");
}

#[tokio::test]
async fn non_https_and_credential_urls_are_refused_before_any_request() {
    let w = W::new().await;
    for url in [
        "http://cdn.acme.example/repo",
        "ftp://cdn.acme.example/repo",
        "https://user:pass@cdn.acme.example/repo",
    ] {
        let (s, v) = w
            .send(
                Method::POST,
                "/v1/admin/addon-sources/preview",
                Some(json!({"url": url})),
            )
            .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{url}: {v}");
        assert_eq!(v["error"]["code"], "addon_url_refused");
    }
}

#[tokio::test]
async fn removal_rules() {
    let w = W::new().await;
    w.register().await;
    let sid = w.source_id().await;

    // Approve the pending package (make it active): removal is now refused.
    let pid: Uuid =
        sqlx::query_scalar("SELECT id FROM plugin_packages WHERE source = 'addon' LIMIT 1")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let (s, v) = w
        .send(
            Method::POST,
            &format!("/v1/admin/plugins/{pid}/approve"),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    let (s, v) = w
        .send(
            Method::DELETE,
            &format!("/v1/admin/addon-sources/{sid}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "addon_source_active");

    // Disable it: removal works, packages stay (source_id nulled by the FK).
    let (s, _) = w
        .send(
            Method::POST,
            &format!("/v1/admin/plugins/{pid}/disable"),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = w
        .send(
            Method::DELETE,
            &format!("/v1/admin/addon-sources/{sid}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM plugin_packages WHERE package_id = 'acme'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 1, "packages stay after removal");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM addon_sources")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn the_poll_job_claims_due_sources_and_audits_scheduled() {
    let w = W::new().await;
    w.register().await;

    w.publish(
        "acme",
        &[
            ("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key)),
            ("1.2.0", signed(&yaml("acme", "1.2.0", &[HOST]), &w.key)),
        ],
    )
    .await;
    let n = addon_sources::poll_once(&w.h.state).await.unwrap();
    assert_eq!(n, 1);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM plugin_packages WHERE source = 'addon' AND version = '1.2.0'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
    let meta: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'addon_source.check'
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(meta["trigger"], "scheduled");

    // A source whose next_check_at is in the future is not claimed.
    sqlx::query("UPDATE addon_sources SET next_check_at = now() + interval '1 hour'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let n = addon_sources::poll_once(&w.h.state).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn interval_edits_are_validated_and_audited() {
    let w = W::new().await;
    w.register().await;
    let sid = w.source_id().await;
    let (s, v) = w
        .send(
            Method::PATCH,
            &format!("/v1/admin/addon-sources/{sid}"),
            Some(json!({"update_interval": "2h"})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = w
        .send(
            Method::PATCH,
            &format!("/v1/admin/addon-sources/{sid}"),
            Some(json!({"update_interval": "1w"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["update_interval"], "1w");
    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_log WHERE action = 'addon_source.update'")
            .fetch_all(w.h.pool())
            .await
            .unwrap();
    assert_eq!(actions.len(), 1);
}

#[tokio::test]
async fn check_all_reports_every_source() {
    let w = W::new().await;
    w.register().await;
    w.publish(
        "acme",
        &[("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key))],
    )
    .await;
    let (s, v) = w
        .send(Method::POST, "/v1/admin/addon-sources/check-all", None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["status"], "up_to_date");
}

#[tokio::test]
async fn min_core_above_the_running_core_is_listed_but_not_ingested() {
    let w = W::new().await;
    let v99 = signed(&yaml("acme", "99.0.0", &[HOST]), &w.key);
    let m = w
        .publish(
            "acme",
            &[
                ("1.0.0", signed(&yaml("acme", "1.0.0", &[HOST]), &w.key)),
                ("2.0.0", v99.clone()),
            ],
        )
        .await;
    // Annotate 2.0.0 with min_core 99.0.0.
    let m2 = m.replace(
        &format!("sha256: {}", sha256_hex(&v99)),
        &format!("sha256: {}\n        min_core: \"99.0.0\"", sha256_hex(&v99)),
    );
    assert_ne!(m, m2);
    w.files.lock().unwrap_or_else(|e| e.into_inner()).manifest = Some(m2.into_bytes());
    let (s, v) = w
        .send(
            Method::POST,
            "/v1/admin/addon-sources",
            Some(json!({"url": format!("{}/repo", w.base)})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let versions = v["addons"][0]["versions"].as_array().unwrap();
    let high = versions.iter().find(|x| x["version"] == "2.0.0").unwrap();
    assert_eq!(high["ingestable"], false);
    assert_eq!(high["not_ingestable_reason"], "core_too_new");
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM plugin_packages WHERE version = '99.0.0'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0, "not ingested");
}

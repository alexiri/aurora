#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The Vault store against a mock HTTP server (wiremock): no Vault needed.

use std::time::Duration;

use aurora_core::Secret;
use aurora_secrets::{Purpose, SealContext, SecretError, SecretRef, SecretStore, StoreHealth};
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{build, ctx};

const JWT: &str = "jwt-for-the-mock";
const VT: &str = "s.mock-client-token";

fn pt(s: &str) -> Secret<Vec<u8>> {
    Secret::new(s.as_bytes().to_vec())
}

/// Kubernetes-auth store pointing at the mock; `scopes` as in the config.
fn store_toml(server: &MockServer, jwt_file: &std::path::Path, scopes: &str) -> String {
    format!(
        "id = \"vault\"\nkind = \"hashicorp-vault\"\naddress = \"{}\"\nauth = {{ method = \"kubernetes\", role = \"aurora\", jwt_path = \"{}\" }}\n{scopes}",
        server.uri(),
        jwt_file.display()
    )
}

fn jwt_file() -> tempfile_dir::Dir {
    let d = tempfile_dir::Dir::new();
    std::fs::write(d.path().join("jwt"), format!("{JWT}\n")).unwrap();
    d
}

/// Tiny temp-dir helper (no extra crate): removed on drop.
mod tempfile_dir {
    pub struct Dir(std::path::PathBuf);
    impl Dir {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!("aurora-vault-it-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn mount_login(server: &MockServer, lease: u64, renewable: bool) {
    Mock::given(method("POST"))
        .and(path("/v1/auth/kubernetes/login"))
        .and(body_json(json!({ "role": "aurora", "jwt": JWT })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": { "client_token": VT, "lease_duration": lease, "renewable": renewable }
        })))
        .mount(server)
        .await;
}

fn kv_body(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "data": { "data": data, "metadata": {} } }))
}

fn scopes_for(org: Uuid, paths: &str) -> String {
    format!("[[reference_scopes]]\norg_id = \"{org}\"\npaths = [{paths}]\n")
}

fn managed_path(c: &SealContext) -> String {
    format!("aurora/{}/{}/{}", c.org_id, c.purpose.as_str(), c.owner_id)
}

#[tokio::test]
async fn managed_round_trip_pins_the_version() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    let c = ctx(Uuid::now_v7());
    let p = managed_path(&c);
    let stored = json!({
        "value": "aHVudGVyMg==", "purpose": c.purpose.as_str(),
        "owner_id": c.owner_id.to_string(), "org_id": c.org_id.to_string(),
    });
    Mock::given(method("POST"))
        .and(path(format!("/v1/secret/data/{p}")))
        .and(header("X-Vault-Token", VT))
        .and(body_json(json!({ "data": stored })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "version": 3 } })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v1/secret/data/{p}")))
        .and(query_param("version", "3"))
        .respond_with(kv_body(stored.clone()))
        .expect(2)
        .mount(&server)
        .await;

    let s = build(&store_toml(&server, &dir.path().join("jwt"), ""));
    let r = s.put(&c, &pt("hunter2")).await.unwrap();
    assert_eq!(r.to_column(), format!("ext:vault:{p}@3"));
    assert_eq!(s.get(&c, &r).await.unwrap().expose(), b"hunter2");
    // Survives the column round trip.
    let r2: SecretRef = r.to_column().parse().unwrap();
    assert_eq!(s.get(&c, &r2).await.unwrap().expose(), b"hunter2");
}

#[tokio::test]
async fn purpose_owner_and_org_mismatch_are_refused() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    let c = ctx(Uuid::now_v7());
    let p = managed_path(&c);
    let stored = json!({
        "value": "aHVudGVyMg==", "purpose": c.purpose.as_str(),
        "owner_id": c.owner_id.to_string(), "org_id": c.org_id.to_string(),
    });
    Mock::given(method("GET"))
        .and(path(format!("/v1/secret/data/{p}")))
        .respond_with(kv_body(stored))
        .mount(&server)
        .await;
    let s = build(&store_toml(&server, &dir.path().join("jwt"), ""));
    let r = SecretRef::ext("vault", &p, None, "1").unwrap();
    assert!(s.get(&c, &r).await.is_ok());

    // The ref names one path; any other context fails on the path binding before a read.
    let mut o = c;
    o.owner_id = Uuid::now_v7();
    assert_eq!(
        s.get(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    let mut o = c;
    o.org_id = Uuid::now_v7();
    assert_eq!(
        s.get(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    let mut o = c;
    o.purpose = Purpose::WebhookVerification;
    assert_eq!(
        s.get(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    assert_eq!(
        s.delete(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );

    // A forged ref to an arbitrary (non-managed) path with a numeric version is no way in.
    let forged = SecretRef::ext("vault", "teams/physics/key", None, "1").unwrap();
    assert_eq!(
        s.get(&c, &forged).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
}

#[tokio::test]
async fn context_stored_with_the_value_is_checked_too() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    let c = ctx(Uuid::now_v7());
    let p = managed_path(&c);
    // Right path, but the value carries another org's context (a row swapped inside Vault).
    Mock::given(method("GET"))
        .and(path(format!("/v1/secret/data/{p}")))
        .respond_with(kv_body(json!({
            "value": "eA==", "purpose": c.purpose.as_str(),
            "owner_id": c.owner_id.to_string(), "org_id": Uuid::now_v7().to_string(),
        })))
        .mount(&server)
        .await;
    let s = build(&store_toml(&server, &dir.path().join("jwt"), ""));
    let r = SecretRef::ext("vault", &p, None, "1").unwrap();
    assert_eq!(
        s.get(&c, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
}

#[tokio::test]
async fn delete_destroys_that_version_and_references_are_never_destroyed() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    let c = ctx(Uuid::now_v7());
    let p = managed_path(&c);
    Mock::given(method("POST"))
        .and(path(format!("/v1/secret/destroy/{p}")))
        .and(body_json(json!({ "versions": [4] })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let s = build(&store_toml(&server, &dir.path().join("jwt"), ""));
    s.delete(&c, &SecretRef::ext("vault", &p, None, "4").unwrap())
        .await
        .unwrap();
    // A reference ref: no request at all (the destroy mock allows exactly one call).
    s.delete(
        &c,
        &SecretRef::ext("vault", "teams/x", Some("value"), "latest").unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn reference_allowed_reads_latest_and_refusals_never_reach_vault() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    let org = Uuid::now_v7();
    let other = Uuid::now_v7();
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/teams/physics/openai"))
        .and(query_param_is_missing("version"))
        .respond_with(kv_body(json!({ "key": "sk-live", "n": 7 })))
        .mount(&server)
        .await;
    let toml = store_toml(
        &server,
        &dir.path().join("jwt"),
        &scopes_for(org, "\"teams/physics/*\", \"shared/exact\""),
    );
    let s = build(&toml);
    let c = ctx(org);

    let r = s
        .resolve_reference(&c, "teams/physics/openai", Some("key"))
        .await
        .unwrap();
    assert_eq!(r.to_column(), "ext:vault:teams/physics/openai#key@latest");
    assert_eq!(s.get(&c, &r).await.unwrap().expose(), b"sk-live");

    // Outside the scope, another org's scope, the managed prefix: refused with no Vault call.
    let reads = |reqs: &[wiremock::Request]| {
        reqs.iter()
            .filter(|r| r.url.path().starts_with("/v1/secret/data/"))
            .count()
    };
    assert_eq!(reads(&server.received_requests().await.unwrap()), 2);
    for (who, p) in [
        (org, "teams/chemistry/openai"),
        (org, "shared/exact/child"),
        (other, "teams/physics/openai"),
        (org, "aurora/anything"),
    ] {
        assert_eq!(
            s.resolve_reference(&ctx(who), p, Some("key"))
                .await
                .unwrap_err(),
            SecretError::ReferenceNotPermitted,
            "{p}"
        );
    }
    // The same refusal at open for a stored ref under another org.
    assert_eq!(
        s.get(&ctx(other), &r).await.unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
    assert_eq!(reads(&server.received_requests().await.unwrap()), 2);
    // A non-string field, a missing field and a missing secret look identical.
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/teams/physics/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    for (p, f) in [
        ("teams/physics/openai", "n"),
        ("teams/physics/openai", "nope"),
        ("teams/physics/missing", "key"),
    ] {
        assert_eq!(
            s.resolve_reference(&c, p, Some(f)).await.unwrap_err(),
            SecretError::ReferenceNotPermitted,
            "{p}#{f}"
        );
    }
}

#[tokio::test]
async fn reference_never_pins_a_version_and_defaults_the_field() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    let org = Uuid::now_v7();
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/a/b"))
        .and(query_param_is_missing("version"))
        .respond_with(kv_body(json!({ "value": "v1" })))
        .mount(&server)
        .await;
    let s = build(&store_toml(
        &server,
        &dir.path().join("jwt"),
        &scopes_for(org, "\"a/*\""),
    ));
    let c = ctx(org);
    let r = s.resolve_reference(&c, "a/b", None).await.unwrap();
    assert_eq!(r.to_column(), "ext:vault:a/b#value@latest");
    // A hand-made pinned reference is refused (SEC-61: latest only).
    let pinned = SecretRef::ext("vault", "a/b", Some("value"), "2").unwrap();
    assert!(matches!(
        s.get(&c, &pinned).await.unwrap_err(),
        SecretError::Malformed(_)
    ));
    // Malformed paths are rejected before anything else.
    for bad in ["", "/a/b", "a/b/", "a//b", "a/./b", "a/../b", "data\u{0}x"] {
        assert!(
            matches!(
                s.resolve_reference(&c, bad, None).await.unwrap_err(),
                SecretError::Malformed(_)
            ),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn token_is_renewed_at_two_thirds_and_login_repeats_when_renewal_fails() {
    let server = MockServer::start().await;
    // 3 s lease: renew at 2 s.
    mount_login(&server, 3, true).await;
    let dir = jwt_file();
    let org = Uuid::now_v7();
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/a/b"))
        .respond_with(kv_body(json!({ "value": "v" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/token/renew-self"))
        .and(header("X-Vault-Token", VT))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": { "client_token": VT, "lease_duration": 3, "renewable": true }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let s = build(&store_toml(
        &server,
        &dir.path().join("jwt"),
        &scopes_for(org, "\"a/*\""),
    ));
    let c = ctx(org);
    s.resolve_reference(&c, "a/b", None).await.unwrap();
    let count = |p: &'static str, reqs: &[wiremock::Request]| {
        reqs.iter().filter(|r| r.url.path() == p).count()
    };
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(count("/v1/auth/kubernetes/login", &reqs), 1);
    assert_eq!(count("/v1/auth/token/renew-self", &reqs), 0);

    tokio::time::sleep(Duration::from_millis(2300)).await;
    s.resolve_reference(&c, "a/b", None).await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(count("/v1/auth/token/renew-self", &reqs), 1);
    assert_eq!(count("/v1/auth/kubernetes/login", &reqs), 1);

    // Renewal now fails (the one-shot mock is used up, wiremock answers 404): log in again.
    tokio::time::sleep(Duration::from_millis(2300)).await;
    s.resolve_reference(&c, "a/b", None).await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(count("/v1/auth/kubernetes/login", &reqs), 2);
}

#[tokio::test]
async fn a_revoked_token_logs_in_again_once() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    let org = Uuid::now_v7();
    // lookup-self fails: the token is dead, so the 403 triggers a fresh login and one retry.
    Mock::given(method("GET"))
        .and(path("/v1/auth/token/lookup-self"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/a/b"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/a/b"))
        .respond_with(kv_body(json!({ "value": "v" })))
        .mount(&server)
        .await;
    let s = build(&store_toml(
        &server,
        &dir.path().join("jwt"),
        &scopes_for(org, "\"a/*\""),
    ));
    s.resolve_reference(&ctx(org), "a/b", None).await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs.iter()
            .filter(|r| r.url.path() == "/v1/auth/kubernetes/login")
            .count(),
        2
    );
}

#[tokio::test]
async fn approle_logs_in_with_the_environment_credentials() {
    let server = MockServer::start().await;
    // Cargo sets these for every test process; any non-secret variables would do.
    let role_id = std::env::var("CARGO_PKG_NAME").unwrap();
    let secret_id = std::env::var("CARGO_PKG_VERSION").unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/auth/ar/login"))
        .and(body_json(
            json!({ "role_id": role_id, "secret_id": secret_id }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": { "client_token": VT, "lease_duration": 3600, "renewable": true }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let org = Uuid::now_v7();
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/a/b"))
        .and(header("X-Vault-Token", VT))
        .respond_with(kv_body(json!({ "value": "v" })))
        .mount(&server)
        .await;
    let s = build(&format!(
        "id = \"vault\"\nkind = \"hashicorp-vault\"\naddress = \"{}\"\nauth = {{ method = \"approle\", mount = \"ar\", role_id_env = \"CARGO_PKG_NAME\", secret_id_env = \"CARGO_PKG_VERSION\" }}\n{}",
        server.uri(),
        scopes_for(org, "\"a/*\"")
    ));
    s.resolve_reference(&ctx(org), "a/b", None).await.unwrap();
}

#[tokio::test]
async fn unavailable_store_fails_closed_without_leaking() {
    let dir = jwt_file();
    let jwt = dir.path().join("jwt");
    let org = Uuid::now_v7();
    let scopes = scopes_for(org, "\"a/*\"");
    let c = ctx(org);

    // Login rejected.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/kubernetes/login"))
        .respond_with(ResponseTemplate::new(500).set_body_string(format!("boom {JWT}")))
        .mount(&server)
        .await;
    let s = build(&store_toml(&server, &jwt, &scopes));
    let e = s.resolve_reference(&c, "a/b", None).await.unwrap_err();
    assert_eq!(e, SecretError::StoreUnavailable);
    assert!(!format!("{e} {e:?} {s:?}").contains(JWT));
    assert_eq!(
        s.put(&c, &pt("x")).await.unwrap_err(),
        SecretError::StoreUnavailable
    );
    assert_eq!(s.health().await, StoreHealth::Unavailable);

    // JWT file missing.
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let s = build(&store_toml(&server, &dir.path().join("absent"), &scopes));
    assert_eq!(
        s.resolve_reference(&c, "a/b", None).await.unwrap_err(),
        SecretError::StoreUnavailable
    );

    // KV answers 503 (sealed / overloaded): unavailable, not "not permitted".
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/a/b"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let s = build(&store_toml(&server, &jwt, &scopes));
    assert_eq!(
        s.resolve_reference(&c, "a/b", None).await.unwrap_err(),
        SecretError::StoreUnavailable
    );

    // Nothing listens.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let s = build(&format!(
        "id = \"vault\"\nkind = \"hashicorp-vault\"\naddress = \"http://127.0.0.1:{dead}\"\nauth = {{ method = \"kubernetes\", role = \"aurora\", jwt_path = \"{}\" }}\n{scopes}",
        jwt.display()
    ));
    assert_eq!(
        s.put(&c, &pt("x")).await.unwrap_err(),
        SecretError::StoreUnavailable
    );
    assert_eq!(s.health().await, StoreHealth::Unavailable);
}

#[tokio::test]
async fn health_is_healthy_when_vault_and_login_work() {
    let server = MockServer::start().await;
    mount_login(&server, 3600, true).await;
    let dir = jwt_file();
    Mock::given(method("GET"))
        .and(path("/v1/sys/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "sealed": false })))
        .mount(&server)
        .await;
    let s = build(&store_toml(&server, &dir.path().join("jwt"), ""));
    assert_eq!(s.health().await, StoreHealth::Healthy);
    assert_eq!(s.kind(), "hashicorp-vault");
    assert!(s.capabilities().managed && s.capabilities().references);
}

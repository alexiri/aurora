#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-MCON-1 (D-128; docs/13 §3.1; C-450..C-459; `18 §Managed connectors`): the connector host
//! plane, the managed-connector admin API, sealed write-only settings and delegated connector
//! authentication. Test names `sec69_*` and `sec70_*` are listed in `tests/security-coverage.md`.
//! (The "captured log" half of SEC-70 lives in `tests/managed_logs.rs`: it needs its own binary.)

use crate::common::*;
use crate::managed_support::*;
use http::Method;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use uuid::Uuid;

async fn all_audit_text(m: &M) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT coalesce(string_agg(row_to_json(a)::text, E'\\n'), '') FROM audit_log a",
    )
    .fetch_one(m.h.pool())
    .await
    .unwrap()
}

async fn connector_row_text(m: &M, id: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT row_to_json(c)::text FROM connectors c WHERE id=$1")
        .bind(id)
        .fetch_one(m.h.pool())
        .await
        .unwrap()
}

fn field_paths(v: &Value) -> Vec<String> {
    v["error"]["details"]["fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| f["path"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Request on the connector plane with the host token and `Aurora-Connector: id`.
async fn delegated(
    m: &M,
    id: Option<&str>,
    tok: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let b = bearer(tok);
    let mut h = vec![("authorization", b.as_str())];
    if let Some(id) = id {
        h.push(("aurora-connector", id));
    }
    let (s, _, v) =
        m.h.send(method, &format!("/v1/connector{path}"), &h, body)
            .await;
    (s.as_u16(), v)
}

async fn delegated_register(m: &M, id: &str, manifest: &Value) -> (u16, Value) {
    delegated(
        m,
        Some(id),
        &m.host_token,
        Method::POST,
        "/register",
        Some(json!({"manifest": manifest, "signature": m.sign(manifest), "version": "1.0.0"})),
    )
    .await
}

// ------------------------------------------------------------------- catalogue

#[tokio::test]
async fn host_plane_requires_the_auh_token() {
    let m = M::new().await;
    let agent = aurora_core::token::generate(aurora_core::token::TokenPrefix::Connector);
    for tok in [None, Some("auh_unknown"), Some(agent.expose().as_str())] {
        let b = tok.map(bearer);
        let h: Vec<(&str, &str)> = b
            .as_deref()
            .map(|b| vec![("authorization", b)])
            .unwrap_or_default();
        for (method, path) in [
            (Method::POST, "/v1/connector-host/register"),
            (Method::GET, "/v1/connector-host/assignments"),
            (Method::POST, "/v1/connector-host/status"),
        ] {
            let (s, _, v) = m.h.send(method, path, &h, Some(json!({}))).await;
            assert_eq!(s, 401, "{path} with {tok:?}: {v}");
        }
    }
    // A valid token works.
    let (s, _, _) = m.assignments(None, None, 0).await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn catalogue_lists_verified_types_and_drops_stale_hosts() {
    let m = M::new().await;
    // Nothing announced: no host running.
    let (s, v) = m
        .admin(Method::GET, "/v1/admin/connector-types", None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["host_running"], false);
    assert_eq!(v["items"], json!([]));

    let host = Uuid::now_v7();
    let good = manifest("alpha");
    let bad_sig = manifest("bravo");
    let mismatch = manifest("charlie");
    let invalid = {
        let mut x = manifest("delta");
        x["instance_setup"] = json!({"fields": [{"name": "x", "type": "json_file"}]});
        x
    };
    let types = json!([
        {"type_id": "alpha", "manifest": good, "manifest_signature": m.sign(&good)},
        {"type_id": "bravo", "manifest": bad_sig, "manifest_signature": m.sign(&good)},
        {"type_id": "other", "manifest": mismatch, "manifest_signature": m.sign(&mismatch)},
        {"type_id": "delta", "manifest": invalid, "manifest_signature": m.sign(&invalid)},
    ]);
    let (s, v) = m.host_register_raw(host, "1.0.0", types).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["poll_interval_secs"], 30);
    let mut rej: Vec<(String, String)> = v["rejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["type_id"].as_str().unwrap().to_owned(),
                r["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    rej.sort();
    assert_eq!(
        rej,
        [
            ("bravo".into(), "signature".into()),
            ("delta".into(), "invalid_manifest".into()),
            ("other".into(), "id_mismatch".into()),
        ]
    );

    let (_, v) = m
        .admin(Method::GET, "/v1/admin/connector-types", None)
        .await;
    assert_eq!(v["host_running"], true);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only the verified type: {v}");
    let t = &items[0];
    assert_eq!(t["type_id"], "alpha");
    assert_eq!(t["name"], "Type alpha");
    assert_eq!(t["version"], "1.0.0");
    assert_eq!(t["publisher"], "acme");
    assert_eq!(t["manifest_digest"], m.digest(&good));
    assert_eq!(t["hosts"], 1);
    assert_eq!(t["added"], false);
    assert_eq!(t["instance_setup"]["fields"].as_array().unwrap().len(), 6);
    assert!(t["manifest"]["about"].get("icon_png_b64").is_none());

    // A host older than CONNECTOR_HOST_STALE drops out.
    sqlx::query("UPDATE connector_hosts SET last_seen_at = now() - interval '91 seconds'")
        .execute(m.h.pool())
        .await
        .unwrap();
    let (_, v) = m
        .admin(Method::GET, "/v1/admin/connector-types", None)
        .await;
    assert_eq!(v["host_running"], false);
    assert_eq!(v["items"], json!([]));
}

#[tokio::test]
async fn catalogue_newest_host_version_wins_and_counts_only_its_digest() {
    let m = M::new().await;
    let old = manifest("alpha");
    let mut newer = manifest("alpha");
    newer["package"]["version"] = json!("1.1.0");
    for (ver, mf) in [("1.0.0", &old), ("1.0.0", &old), ("1.2.0", &newer)] {
        let (s, v) = m.host_register(Uuid::now_v7(), ver, &[mf]).await;
        assert_eq!(s, 200, "{v}");
    }
    let (_, v) = m
        .admin(Method::GET, "/v1/admin/connector-types", None)
        .await;
    let t = &v["items"][0];
    assert_eq!(t["version"], "1.1.0");
    assert_eq!(t["manifest_digest"], m.digest(&newer));
    assert_eq!(t["hosts"], 1);
}

#[tokio::test]
async fn catalogue_and_overview_are_for_instance_admins_only() {
    let m = M::new().await;
    let user = m.h.user("plain@x.io", false).await;
    let c = cookie(&m.h.good_session(user).await.1);
    for path in ["/v1/admin/connector-types", "/v1/admin/overview"] {
        let (s, _, _) = m.h.send(Method::GET, path, &hd(&c), None).await;
        assert_eq!(s, 403, "{path}");
    }
    let (s, v) =
        m.h.send(
            Method::POST,
            "/v1/admin/connectors",
            &hd(&c),
            Some(json!({"id": "alpha", "managed": true})),
        )
        .await
        .into_status_body();
    assert_eq!(s, 403, "{v}");
}

trait IntoStatusBody {
    fn into_status_body(self) -> (u16, Value);
}
impl IntoStatusBody for (http::StatusCode, http::HeaderMap, Value) {
    fn into_status_body(self) -> (u16, Value) {
        (self.0.as_u16(), self.2)
    }
}

#[tokio::test]
async fn overview_lists_hosts_seen_within_a_day() {
    let m = M::new().await;
    let h1 = Uuid::now_v7();
    let h2 = Uuid::now_v7();
    m.host_register(h1, "1.0.0", &[&manifest("alpha"), &manifest("bravo")])
        .await;
    m.host_register(h2, "1.0.0", &[&manifest("alpha")]).await;
    sqlx::query("UPDATE connector_hosts SET last_seen_at = now() - interval '25 hours' WHERE instance_id=$1")
        .bind(h2)
        .execute(m.h.pool())
        .await
        .unwrap();
    let (s, v) = m.admin(Method::GET, "/v1/admin/overview", None).await;
    assert_eq!(s, 200, "{v}");
    let hosts = v["connector_hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 1, "{v}");
    assert_eq!(hosts[0]["instance_id"], h1.to_string());
    assert_eq!(hosts[0]["version"], "1.0.0");
    assert_eq!(hosts[0]["types"], json!(["alpha", "bravo"]));
}

#[tokio::test]
async fn bootstrap_applies_the_host_token_hash_only() {
    let tok = aurora_core::token::generate(aurora_core::token::TokenPrefix::ConnectorHost)
        .expose()
        .clone();
    let t2 = tok.clone();
    let h = Harness::with_config(move |c| {
        c.bootstrap_connector_host_token = Some(aurora_config::Redacted::new(t2));
    })
    .await;
    aurora_api::domains::connectors::bootstrap_host_token(&h.state)
        .await
        .unwrap();
    // Idempotent.
    aurora_api::domains::connectors::bootstrap_host_token(&h.state)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connector_host_tokens")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM connector_host_tokens")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(stored, aurora_core::token::hash(&tok).to_vec());
    let b = bearer(&tok);
    let (s, _, v) = h
        .send(
            Method::GET,
            "/v1/connector-host/assignments?wait=0",
            &[("authorization", &b)],
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

// ------------------------------------------------------------------------- add

#[tokio::test]
async fn managed_create_awaits_registration_without_a_token() {
    let m = M::new().await;
    let host = Uuid::now_v7();
    m.host_register(host, "1.0.0", &[&manifest("alpha"), &manifest("bravo")])
        .await;
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(m.create_body("alpha")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert!(
        v.get("token").is_none(),
        "no token for a managed connector: {v}"
    );
    let c = &v["connector"];
    assert_eq!(c["id"], "alpha");
    assert_eq!(c["managed"], true);
    assert_eq!(c["type_id"], "alpha");
    assert_eq!(
        c["display_name"], "Type alpha",
        "defaults to the manifest name"
    );
    assert_eq!(c["status"], "awaiting_registration");
    assert_eq!(c["approved_manifest_digest"], m.digest(&manifest("alpha")));
    // Defaults applied; stored values kept.
    assert_eq!(c["settings"]["catchup_window_secs"], 300);
    assert_eq!(c["settings"]["region"], "eu");
    assert_eq!(c["secrets"]["api_key"]["set"], true);
    assert_eq!(c["secrets"]["api_key"]["generated"], false);
    assert!(c["secrets"]["api_key"]["updated_at"].is_string());
    assert_eq!(c["secrets"]["extra_token"]["set"], false);
    assert_eq!(c["secrets"]["callback_hmac_key"]["set"], true);
    assert_eq!(c["secrets"]["callback_hmac_key"]["generated"], true);
    assert_eq!(c["host_status"], json!([]));

    // The type now shows as added; a self-hosted connector is unchanged.
    let (_, t) = m
        .admin(Method::GET, "/v1/admin/connector-types", None)
        .await;
    let added: Vec<(&str, bool)> = t["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["type_id"].as_str().unwrap(),
                i["added"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(added, [("alpha", true), ("bravo", false)]);
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "selfhosted", "display_name": "Self"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert!(v["token"].as_str().unwrap().starts_with("aun_"));
    assert_eq!(v["connector"]["managed"], false);
    assert_eq!(v["connector"]["settings"], Value::Null);
    assert_eq!(v["connector"]["secrets"], json!({}));
    assert_eq!(v["connector"]["host_status"], json!([]));
    assert_eq!(v["connector"]["type_id"], Value::Null);

    let (_, list) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn managed_create_validation() {
    let m = M::new().await;
    m.host_register(Uuid::now_v7(), "1.0.0", &[&manifest("alpha")])
        .await;
    let post = |b: Value| m.admin(Method::POST, "/v1/admin/connectors", Some(b));

    // Unknown type → 422 unknown_connector_type.
    let mut b = m.create_body("alpha");
    b["id"] = json!("nosuch");
    let (s, v) = post(b).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("unknown_connector_type")),
        "{v}"
    );

    // Missing digest → 422 invalid_request; wrong digest → 409 stale_manifest.
    let mut b = m.create_body("alpha");
    b.as_object_mut().unwrap().remove("approve_manifest_digest");
    let (s, v) = post(b).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("invalid_request")),
        "{v}"
    );
    assert_eq!(field_paths(&v), ["/approve_manifest_digest"]);
    let mut b = m.create_body("alpha");
    b["approve_manifest_digest"] = json!("0".repeat(64));
    let (s, v) = post(b).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_manifest")),
        "{v}"
    );

    // Per-field errors: missing required secret, supplied generated secret, unknown keys, bad
    // values, a secret in `settings` and a setting in `secrets`.
    let mut b = m.create_body("alpha");
    b["secrets"] = json!({"callback_hmac_key": "x", "nope": "x", "public_url": "x"});
    b["settings"] =
        json!({"catchup_window_secs": 0, "region": "mars", "mystery": 1, "extra_token": "s"});
    let (s, v) = post(b).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "invalid_request");
    let mut paths = field_paths(&v);
    paths.sort();
    assert_eq!(
        paths,
        [
            "/secrets/api_key",
            "/secrets/callback_hmac_key",
            "/secrets/nope",
            "/secrets/public_url",
            "/settings/catchup_window_secs",
            "/settings/extra_token",
            "/settings/mystery",
            "/settings/region",
        ]
    );
    // A supplied secret value is never echoed in an error.
    let mut b = m.create_body("alpha");
    b["secrets"] = json!({"api_key": SECRET_VALUE, "callback_hmac_key": SECRET_VALUE_2});
    let (s, v) = post(b).await;
    assert_eq!(s, 422, "{v}");
    assert!(!v.to_string().contains(SECRET_VALUE) && !v.to_string().contains(SECRET_VALUE_2));

    // URL and number validation; oversize secret.
    for (settings, secrets) in [
        (json!({"public_url": "ftp://x"}), json!({"api_key": "k"})),
        (
            json!({"public_url": "https://u:p@x"}),
            json!({"api_key": "k"}),
        ),
        (json!({"catchup_window_secs": "5"}), json!({"api_key": "k"})),
        (
            json!({"catchup_window_secs": 86401}),
            json!({"api_key": "k"}),
        ),
        (json!({}), json!({"api_key": "x".repeat(8 * 1024 + 1)})),
    ] {
        let mut b = m.create_body("alpha");
        b["settings"] = settings.clone();
        b["secrets"] = secrets;
        let (s, v) = post(b).await;
        assert_eq!(s, 422, "{settings}: {v}");
    }
    // http is allowed (kind and dev setups).
    let mut b = m.create_body("alpha");
    b["settings"] = json!({"public_url": "http://aurora.test:4100/connectors/alpha"});
    let (s, v) = post(b).await;
    assert_eq!(s, 201, "{v}");

    // Existing id → 409 connector_exists (managed and self-hosted alike).
    let (s, v) = post(m.create_body("alpha")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("connector_exists")),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connectors")
        .fetch_one(m.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1, "failed creates leave nothing behind");
}

#[tokio::test]
async fn generated_secrets_exist_and_differ_per_connector() {
    let m = M::new().await;
    let host = Uuid::now_v7();
    m.host_register(host, "1.0.0", &[&manifest("alpha"), &manifest("bravo")])
        .await;
    for id in ["alpha", "bravo"] {
        let (s, v) = m
            .admin(
                Method::POST,
                "/v1/admin/connectors",
                Some(m.create_body(id)),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
    let (s, _, v) = m.assignments(Some(host), None, 0).await;
    assert_eq!(s, 200, "{v}");
    let cs = v["connectors"].as_array().unwrap();
    assert_eq!(cs.len(), 2);
    let keys: Vec<&str> = cs
        .iter()
        .map(|c| c["secrets"]["callback_hmac_key"].as_str().unwrap())
        .collect();
    assert_ne!(keys[0], keys[1]);
    for k in keys {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD.decode(k).unwrap();
        assert_eq!(raw.len(), 32, "32 random bytes, base64");
    }
}

// ----------------------------------------------------------------- SEC-70

#[tokio::test]
async fn sec70_secret_values_never_in_responses_audit_or_stored_rows() {
    let m = M::new().await;
    let (host, created) = m.add_managed("alpha").await;
    let mut bodies = vec![created.to_string()];
    let mut note = |v: &Value| bodies.push(v.to_string());

    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(
                json!({"secrets": {"api_key": SECRET_VALUE_2, "extra_token": SECRET_VALUE},
                        "regenerate": ["callback_hmac_key"], "settings": {"region": "us"}}),
            ),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    note(&v);
    let (_, v) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    note(&v);
    // Failed writes echo nothing either.
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"secrets": {"callback_hmac_key": SECRET_VALUE}})),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    note(&v);
    let (s, v) = m
        .admin(Method::POST, "/v1/admin/connectors/alpha/token", None)
        .await;
    assert_eq!(s, 409, "{v}");
    note(&v);

    // The generated value, as the host sees it (the only place plaintext is served).
    let (s, _, a) = m.assignments(Some(host), None, 0).await;
    assert_eq!(s, 200, "{a}");
    let c = &a["connectors"][0];
    assert_eq!(c["secrets"]["api_key"], SECRET_VALUE_2);
    assert_eq!(c["secrets"]["extra_token"], SECRET_VALUE);
    let generated = c["secrets"]["callback_hmac_key"]
        .as_str()
        .unwrap()
        .to_owned();

    let audit = all_audit_text(&m).await;
    let row = connector_row_text(&m, "alpha").await;
    for needle in [SECRET_VALUE, SECRET_VALUE_2, generated.as_str()] {
        for b in &bodies {
            assert!(
                !b.contains(needle),
                "secret value in an admin response: {b}"
            );
        }
        assert!(!audit.contains(needle), "secret value in an audit row");
        assert!(
            !row.contains(needle),
            "plaintext stored in the connector row"
        );
    }
    // The audit rows exist and carry names only.
    let created_ev: String =
        sqlx::query_scalar("SELECT metadata::text FROM audit_log WHERE action='connector.created'")
            .fetch_one(m.h.pool())
            .await
            .unwrap();
    assert!(created_ev.contains("api_key") && created_ev.contains("callback_hmac_key"));
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='connector.updated'")
            .fetch_one(m.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn sec70_patch_replaces_clears_and_regenerates_and_bumps_the_revision() {
    let m = M::new().await;
    let (host, _) = m.add_managed("alpha").await;
    let patch = |b: Value| m.admin(Method::PATCH, "/v1/admin/connectors/alpha", Some(b));
    let mut rev = m.revision().await;
    let secrets_of = |v: &Value| v["connectors"][0]["secrets"].clone();

    // Replace.
    let (s, v) = patch(json!({"secrets": {"api_key": SECRET_VALUE_2, "extra_token": "t1"}})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["secrets"]["extra_token"]["set"], true);
    let now = m.revision().await;
    assert!(now > rev, "replace bumps the revision");
    rev = now;
    let (_, _, a) = m.assignments(Some(host), None, 0).await;
    assert_eq!(secrets_of(&a)["api_key"], SECRET_VALUE_2);
    assert_eq!(secrets_of(&a)["extra_token"], "t1");

    // Clear an optional secret; a required one → 422; a generated one cannot be cleared.
    let (s, v) = patch(json!({"secrets": {"extra_token": null}})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["secrets"]["extra_token"]["set"], false);
    assert_eq!(v["secrets"]["extra_token"]["updated_at"], Value::Null);
    let now = m.revision().await;
    assert!(now > rev, "clear bumps the revision");
    rev = now;
    let (s, v) = patch(json!({"secrets": {"api_key": null}})).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(field_paths(&v), ["/secrets/api_key"]);
    let (s, v) = patch(json!({"secrets": {"callback_hmac_key": null}})).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(m.revision().await, rev, "a refused change does not bump");

    // Regenerate: a new value; non-generated names → 422.
    let before =
        secrets_of(&m.assignments(Some(host), None, 0).await.2)["callback_hmac_key"].clone();
    let (s, v) = patch(json!({"regenerate": ["callback_hmac_key"]})).await;
    assert_eq!(s, 200, "{v}");
    let after =
        secrets_of(&m.assignments(Some(host), None, 0).await.2)["callback_hmac_key"].clone();
    assert_ne!(before, after);
    let now = m.revision().await;
    assert!(now > rev, "regenerate bumps the revision");
    let (s, v) = patch(json!({"regenerate": ["api_key"]})).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(field_paths(&v), ["/regenerate/api_key"]);
    let (s, _) = patch(json!({"regenerate": ["nosuch"]})).await;
    assert_eq!(s, 422);

    // Settings merge per key; null restores the default; validation as on create.
    let (s, v) =
        patch(json!({"settings": {"region": "us", "public_url": "https://x.example"}})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["settings"]["region"], "us");
    assert_eq!(
        v["settings"]["catchup_window_secs"], 300,
        "untouched keys stay"
    );
    let (s, v) = patch(json!({"settings": {"region": null, "catchup_window_secs": null}})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["settings"]["region"], "eu");
    assert_eq!(v["settings"]["catchup_window_secs"], 600);
    assert_eq!(v["settings"]["public_url"], "https://x.example");
    for bad in [
        json!({"settings": {"region": "mars"}}),
        json!({"settings": {"api_key": "x"}}),
        json!({"settings": {"unknown": 1}}),
        json!({"secrets": {"nosuch": "x"}}),
        json!({"secrets": {"api_key": ""}}),
    ] {
        let (s, v) = patch(bad.clone()).await;
        assert_eq!(s, 422, "{bad}: {v}");
    }
    let (s, _) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/nosuch",
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn patch_on_a_self_hosted_connector_is_limited_to_name_and_enabled() {
    let m = M::new().await;
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "selfhosted", "display_name": "Self"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let patch = |b: Value| m.admin(Method::PATCH, "/v1/admin/connectors/selfhosted", Some(b));
    for b in [
        json!({"settings": {"a": 1}}),
        json!({"secrets": {"a": "x"}}),
        json!({"regenerate": []}),
    ] {
        let (s, v) = patch(b).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("connector_not_managed")),
            "{v}"
        );
    }
    let (s, v) = patch(json!({"display_name": "Renamed", "enabled": false})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["display_name"], "Renamed");
    assert_eq!(v["status"], "disabled");
    // Never registered: enabling goes back to awaiting_registration.
    let (s, v) = patch(json!({"enabled": true})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "awaiting_registration");
    let (s, v) = patch(json!({"display_name": "  "})).await;
    assert_eq!(s, 422, "{v}");
}

#[tokio::test]
async fn sec70_store_failure_refuses_the_write_and_changes_nothing() {
    let m = M::with_memory_store().await;
    let mem = m.mem.clone().unwrap();
    m.host_register(
        Uuid::now_v7(),
        "1.0.0",
        &[&manifest("alpha"), &manifest("bravo")],
    )
    .await;

    // Create with the store down: 503, nothing created, nothing sealed.
    mem.set_unavailable(true);
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(m.create_body("alpha")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (503, Some("secret_store_unavailable")),
        "{v}"
    );
    assert!(!v.to_string().contains(SECRET_VALUE));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connectors")
        .fetch_one(m.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    mem.set_unavailable(false);
    assert_eq!(mem.managed_count(), 0);

    // PATCH with the store down: 503 and the connector is unchanged.
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(m.create_body("alpha")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sealed_before = mem.managed_count();
    assert_eq!(sealed_before, 2, "api_key and the generated key");
    let before = connector_row_text(&m, "alpha").await;
    let rev = m.revision().await;
    mem.set_unavailable(true);
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(
                json!({"display_name": "Changed", "secrets": {"api_key": SECRET_VALUE_2},
                        "regenerate": ["callback_hmac_key"], "enabled": false}),
            ),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (503, Some("secret_store_unavailable")),
        "{v}"
    );
    assert!(!v.to_string().contains(SECRET_VALUE_2));
    // Assignments cannot decrypt either: 503, the host keeps its running set.
    let (s, _, v) = m.assignments(None, None, 0).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (503, Some("secret_store_unavailable")),
        "{v}"
    );
    mem.set_unavailable(false);
    assert_eq!(
        connector_row_text(&m, "alpha").await,
        before,
        "nothing changed"
    );
    assert_eq!(
        mem.managed_count(),
        sealed_before,
        "nothing sealed or discarded"
    );
    assert_eq!(m.revision().await, rev);
}

#[tokio::test]
async fn sec70_delete_discards_the_sealed_values_and_is_blocked_while_in_use() {
    let m = M::with_memory_store().await;
    let mem = m.mem.clone().unwrap();
    m.add_managed("alpha").await;
    assert_eq!(mem.managed_count(), 2);
    // Replacing a secret discards the old value.
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"secrets": {"api_key": SECRET_VALUE_2}, "regenerate": ["callback_hmac_key"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        mem.managed_count(),
        2,
        "old values discarded after the commit"
    );

    // An integration pins the connector (409 connector_in_use, secrets kept).
    let org = m.h.org().await;
    sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,'alpha','T','T','{}','x','active')")
        .bind(Uuid::now_v7())
        .bind(org)
        .execute(m.h.pool())
        .await
        .unwrap();
    let (s, v) = m
        .admin(Method::DELETE, "/v1/admin/connectors/alpha", None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("connector_in_use")),
        "{v}"
    );
    assert_eq!(mem.managed_count(), 2);
    sqlx::query("DELETE FROM channel_integrations")
        .execute(m.h.pool())
        .await
        .unwrap();

    let rev = m.revision().await;
    let (s, v) = m
        .admin(Method::DELETE, "/v1/admin/connectors/alpha", None)
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(mem.managed_count(), 0, "sealed values discarded");
    assert!(m.revision().await > rev, "delete bumps the revision");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connectors")
        .fetch_one(m.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn token_rotation_on_a_managed_connector_is_refused() {
    let m = M::new().await;
    m.add_managed("alpha").await;
    let (s, v) = m
        .admin(Method::POST, "/v1/admin/connectors/alpha/token", None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("connector_managed")),
        "{v}"
    );
}

// ----------------------------------------------------------------- SEC-69

#[tokio::test]
async fn sec69_delegated_auth_matrix() {
    let m = M::new().await;
    let (_host, _) = m.add_managed("alpha").await;
    m.host_register(Uuid::now_v7(), "1.0.0", &[&manifest("bravo")])
        .await;
    // A second managed connector that gets disabled, and a self-hosted one.
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(m.create_body("bravo")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/bravo",
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "selfhosted", "display_name": "Self"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let aun = v["token"].as_str().unwrap().to_owned();
    let alpha = manifest("alpha");

    // Accepted: managed, enabled. Registers, becomes active (the reviewed digest is approved).
    let (s, v) = delegated_register(&m, "alpha", &alpha).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connector_id"], "alpha");
    assert_eq!(v["status"], "active");
    let replica = v["replica_id"].as_str().unwrap().to_owned();
    let b = bearer(&m.host_token);
    let (s, _, v) =
        m.h.send(
            Method::GET,
            "/v1/connector/config",
            &[
                ("authorization", &b),
                ("aurora-connector", "alpha"),
                ("x-aurora-replica", &replica),
            ],
            None,
        )
        .await;
    assert_eq!(s, 200, "ConnectorCtx accepts delegated auth: {v}");

    // Refused with the same 401 body as a bad aun_ token.
    let (_, bad) = {
        let (s, _, v) =
            m.h.send(
                Method::GET,
                "/v1/connector/config",
                &[("authorization", &bearer("aun_wrong"))],
                None,
            )
            .await;
        (s, v)
    };
    for (what, id, tok) in [
        ("self-hosted", Some("selfhosted"), m.host_token.as_str()),
        ("disabled", Some("bravo"), m.host_token.as_str()),
        ("unknown id", Some("nosuch"), m.host_token.as_str()),
        ("missing header", None, m.host_token.as_str()),
        ("unknown auh token", Some("alpha"), "auh_notarealtoken"),
    ] {
        let (s, v) = delegated(&m, id, tok, Method::POST, "/register", Some(json!({}))).await;
        assert_eq!(s, 401, "{what}: {v}");
        assert_eq!(v, bad, "{what}: same body as a bad aun_ token");
        let (s, v) = delegated(&m, id, tok, Method::GET, "/config", None).await;
        assert_eq!(s, 401, "{what} (config): {v}");
    }

    // `aun_` + the header: the header is ignored; the call is the self-hosted connector's own.
    let sh = {
        let mut x = manifest("selfhosted");
        x["instance_setup"] = Value::Null;
        x.as_object_mut().unwrap().remove("instance_setup");
        x
    };
    let (s, _, v) =
        m.h.send(
            Method::POST,
            "/v1/connector/register",
            &[
                ("authorization", &bearer(&aun)),
                ("aurora-connector", "alpha"),
            ],
            Some(json!({"manifest": sh, "signature": m.sign(&sh), "version": "1"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["connector_id"], "selfhosted",
        "aun_ authenticates as its own connector"
    );
}

#[tokio::test]
async fn sec69_widened_manifest_goes_to_pending_approval_for_managed_connectors() {
    let m = M::new().await;
    m.add_managed("alpha").await;
    let alpha = manifest("alpha");
    let (s, v) = delegated_register(&m, "alpha", &alpha).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");

    // A new instance_setup field is a widening.
    let mut wider = manifest("alpha");
    wider["package"]["version"] = json!("1.1.0");
    wider["instance_setup"]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "new_setting", "label": "New", "type": "text"}));
    let (s, v) = delegated_register(&m, "alpha", &wider).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "pending_approval");
    let (_, list) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    let c = &list["items"][0];
    assert_eq!(c["status"], "pending_approval");
    assert!(
        c["widening"][0].as_str().unwrap().contains("new_setting"),
        "{c}"
    );
    // The settings form still follows the approved manifest.
    assert!(c["secrets"].get("callback_hmac_key").is_some());
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"settings": {"new_setting": "x"}})),
        )
        .await;
    assert_eq!(s, 422, "{v}");

    // Approval is the unchanged route and bumps the assignments revision.
    let rev = m.revision().await;
    let digest = m.digest(&wider);
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors/alpha/approve",
            Some(json!({"manifest_digest": digest})),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");
    assert!(m.revision().await > rev);
    let (_, list) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    assert!(
        list["items"][0]["secrets"]
            .get("callback_hmac_key")
            .is_some()
    );
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"settings": {"new_setting": "x"}})),
        )
        .await;
    assert_eq!(s, 200, "the approved field is now accepted: {v}");

    // A self-hosted connector's widening works the same way (unchanged behaviour).
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "sh", "display_name": "Sh"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let aun = v["token"].as_str().unwrap().to_owned();
    let mut base = manifest("sh");
    base.as_object_mut().unwrap().remove("instance_setup");
    let reg = |mf: Value| {
        let sig = m.sign(&mf);
        let b = bearer(&aun);
        let m = &m;
        async move {
            let (s, _, v) =
                m.h.send(
                    Method::POST,
                    "/v1/connector/register",
                    &[("authorization", &b)],
                    Some(json!({"manifest": mf, "signature": sig, "version": "1"})),
                )
                .await;
            assert_eq!(s, 200, "{v}");
            v["status"].as_str().unwrap().to_owned()
        }
    };
    assert_eq!(reg(base.clone()).await, "pending_approval");
}

#[tokio::test]
async fn sec69_disable_stops_serving_and_enable_restores_the_status() {
    let m = M::new().await;
    let (host, _) = m.add_managed("alpha").await;
    let alpha = manifest("alpha");
    let (s, v) = delegated_register(&m, "alpha", &alpha).await;
    assert_eq!(s, 200, "{v}");
    let (_, _, a) = m.assignments(Some(host), None, 0).await;
    assert_eq!(a["connectors"].as_array().unwrap().len(), 1);
    assert_eq!(a["connectors"][0]["status"], "active");
    let rev = a["revision"].as_i64().unwrap();

    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("disabled")), "{v}");
    let (_, _, a) = m.assignments(Some(host), None, 0).await;
    assert_eq!(
        a["connectors"],
        json!([]),
        "a disabled connector is not assigned"
    );
    assert!(a["revision"].as_i64().unwrap() > rev);
    let (s, _) = delegated(
        &m,
        Some("alpha"),
        &m.host_token,
        Method::GET,
        "/config",
        None,
    )
    .await;
    assert_eq!(s, 401, "a disabled connector is not served");

    // Enabling restores `active` (registered, digests equal).
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"enabled": true})),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");
    let (_, _, a) = m.assignments(Some(host), None, 0).await;
    assert_eq!(a["connectors"].as_array().unwrap().len(), 1);

    // Never registered + disabled + enabled → awaiting_registration.
    let (_, _) = m.add_managed("bravo").await;
    m.admin(
        Method::PATCH,
        "/v1/admin/connectors/bravo",
        Some(json!({"enabled": false})),
    )
    .await;
    let (_, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/bravo",
            Some(json!({"enabled": true})),
        )
        .await;
    assert_eq!(v["status"], "awaiting_registration");

    // Registered with a digest that is not the approved one → pending_approval.
    let mut wider = manifest("alpha");
    wider["capabilities"]["buttons"] = json!(true);
    delegated_register(&m, "alpha", &wider).await;
    m.admin(
        Method::PATCH,
        "/v1/admin/connectors/alpha",
        Some(json!({"enabled": false})),
    )
    .await;
    let (_, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"enabled": true})),
        )
        .await;
    assert_eq!(v["status"], "pending_approval");
}

// ------------------------------------------------------------- assignments

#[tokio::test]
async fn assignments_apply_defaults_decrypt_secrets_and_are_not_cached() {
    let m = M::new().await;
    let (host, _) = m.add_managed("alpha").await;
    let (s, hm, v) = m.assignments(Some(host), None, 0).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(hm.get("cache-control").unwrap(), "no-store");
    let c = &v["connectors"][0];
    assert_eq!(c["id"], "alpha");
    assert_eq!(c["type_id"], "alpha");
    assert_eq!(c["display_name"], "Type alpha");
    assert_eq!(c["status"], "awaiting_registration");
    let base =
        m.h.state
            .config
            .external_url
            .trim_end_matches('/')
            .to_owned();
    assert_eq!(
        c["settings"]["public_url"],
        format!("{base}/connectors/alpha")
    );
    assert_eq!(c["settings"]["catchup_window_secs"], 300);
    assert_eq!(c["settings"]["region"], "eu");
    assert_eq!(
        c["secrets"]["api_key"], SECRET_VALUE,
        "decrypted for the host only"
    );
    assert!(
        c["secrets"].get("extra_token").is_none(),
        "unset secrets are absent"
    );

    // An explicit public_url wins.
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"settings": {"public_url": "https://chat.example/aurora"}})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, _, v) = m.assignments(Some(host), None, 0).await;
    assert_eq!(
        v["connectors"][0]["settings"]["public_url"],
        "https://chat.example/aurora"
    );
}

#[tokio::test]
async fn assignments_serve_only_types_the_calling_host_announced() {
    let m = M::new().await;
    let (h1, _) = m.add_managed("alpha").await;
    let h2 = Uuid::now_v7();
    m.host_register(h2, "1.0.0", &[&manifest("bravo")]).await;
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(m.create_body("bravo")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let ids = |v: &Value| -> Vec<String> {
        v["connectors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&m.assignments(Some(h1), None, 0).await.2), ["alpha"]);
    assert_eq!(ids(&m.assignments(Some(h2), None, 0).await.2), ["bravo"]);
    assert_eq!(
        ids(&m.assignments(None, None, 0).await.2),
        ["alpha", "bravo"]
    );
    assert_eq!(
        ids(&m.assignments(Some(Uuid::now_v7()), None, 0).await.2),
        Vec::<String>::new()
    );
    // `wait` is 0-30.
    let (s, _, v) = m.assignments(Some(h1), None, 31).await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn assignments_long_poll_returns_on_change_and_on_timeout() {
    let m = M::new().await;
    let (host, _) = m.add_managed("alpha").await;
    let rev = m.revision().await;

    // Same revision: waits for the full `wait`, then answers with the same revision.
    let t = Instant::now();
    let (s, _, v) = m.assignments(Some(host), Some(rev), 1).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["revision"], rev);
    assert!(
        t.elapsed() >= Duration::from_millis(900),
        "waited: {:?}",
        t.elapsed()
    );
    assert_eq!(v["connectors"].as_array().unwrap().len(), 1);

    // A different (older) revision returns at once.
    let t = Instant::now();
    let (_, _, v) = m.assignments(Some(host), Some(rev - 1), 20).await;
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(v["revision"], rev);

    // A change wakes a waiting poll well before its timeout.
    let t = Instant::now();
    let poll = m.assignments(Some(host), Some(rev), 20);
    let change = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        m.admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"display_name": "Renamed"})),
        )
        .await
    };
    let ((s, _, v), (cs, cv)) = tokio::join!(poll, change);
    assert_eq!(cs, 200, "{cv}");
    assert_eq!(s, 200, "{v}");
    assert!(
        t.elapsed() < Duration::from_secs(8),
        "woken by the change: {:?}",
        t.elapsed()
    );
    assert!(v["revision"].as_i64().unwrap() > rev);
    assert_eq!(v["connectors"][0]["display_name"], "Renamed");
}

// ------------------------------------------------------------------ status

#[tokio::test]
async fn host_status_is_sanitized_shown_per_host_and_ignores_unknown_ids() {
    let m = M::new().await;
    let (host, _) = m.add_managed("alpha").await;
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "selfhosted", "display_name": "Self"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let long = format!("boom\u{0}\u{7}\n{}", "x".repeat(400));
    let (s, _, v) = m
        .host_req(
            None,
            Method::POST,
            "/status",
            Some(json!({"instance_id": host, "connectors": [
                {"id": "alpha", "state": "error", "message": long},
                {"id": "selfhosted", "state": "running"},
                {"id": "nosuch", "state": "running"},
            ]})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let (_, list) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    let by = |id: &str| {
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()
            .clone()
    };
    let st = by("alpha")["host_status"].clone();
    assert_eq!(st.as_array().unwrap().len(), 1, "{st}");
    assert_eq!(st[0]["host_instance"], host.to_string());
    assert_eq!(st[0]["state"], "error");
    let msg = st[0]["message"].as_str().unwrap();
    assert!(msg.chars().count() <= 300);
    assert!(
        !msg.chars().any(char::is_control),
        "control characters stripped"
    );
    assert!(msg.starts_with("boom"));
    assert_eq!(
        by("selfhosted")["host_status"],
        json!([]),
        "non-managed ids are ignored"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connector_host_status")
        .fetch_one(m.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);

    // A newer report replaces the old one; a host that stops reporting a connector loses its row.
    m.host_req(
        None,
        Method::POST,
        "/status",
        Some(json!({"instance_id": host, "connectors": [{"id": "alpha", "state": "running"}]})),
    )
    .await;
    let (_, list) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    let st = &list["items"][0]["host_status"];
    assert_eq!(
        (st[0]["state"].as_str(), st[0]["message"].clone()),
        (Some("running"), Value::Null)
    );
    m.host_req(
        None,
        Method::POST,
        "/status",
        Some(json!({"instance_id": host, "connectors": []})),
    )
    .await;
    let (_, list) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    assert_eq!(list["items"][0]["host_status"], json!([]));

    // Reports of a host not seen recently are not shown.
    m.host_req(
        None,
        Method::POST,
        "/status",
        Some(json!({"instance_id": host, "connectors": [{"id": "alpha", "state": "running"}]})),
    )
    .await;
    sqlx::query("UPDATE connector_hosts SET last_seen_at = now() - interval '2 minutes'")
        .execute(m.h.pool())
        .await
        .unwrap();
    let (_, list) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    assert_eq!(list["items"][0]["host_status"], json!([]));
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-107 (REVIEW-5 PLUG-1..PLUG-5): what an upgrade may do without an instance admin looking.
//! Publisher changes, built-in ids, downgrades, token endpoints, non-canonical key ids.

use crate::common::*;
use crate::plugins_lifecycle::{Admin, setup};
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use base64::Engine;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "api.trust.example";

fn yaml(publisher: &str, version: &str, token_url: Option<&str>) -> String {
    let (oauth, connect, cred) = match token_url {
        Some(t) => (
            format!(
                "oauth_clients:\n  - id: tclient\n    authorize_url: https://{HOST}/authorize\n    token_url: {t}\n    fields:\n      - {{ name: client_id, label: Client ID }}\n      - {{ name: client_secret, label: Secret, secret: true }}\n"
            ),
            "      - id: oauth\n        kind: oauth2\n        client: tclient\n        scopes:\n          - { scope: read, name: Read, access: read }\n        default_scopes: [read]\n        callback: query\n        token_response:\n          access_token: /access_token\n          refresh_token: /refresh_token\n          expires_in: /expires_in\n",
            "    credentials: [ { name: access_token, secret: true }, { name: refresh_token, secret: true, optional: true } ]\n",
        ),
        None => (
            String::new(),
            "      - id: api_key\n        kind: fields\n        fields: [ { name: api_token, label: T, type: secret, required: true } ]\n",
            "    credentials: [ { name: api_token, secret: true } ]\n",
        ),
    };
    let bearer = if token_url.is_some() {
        "access_token"
    } else {
        "api_token"
    };
    format!(
        "package: {{ id: trusted, version: {version}, contract: 1, min_core: 0.1.0, publisher: {publisher}, name: T }}\n{oauth}apps:\n  - about: {{ id: trusted_app, name: A }}\n{cred}    connect:\n{connect}    hosts:\n      - match: {{ host: \"{HOST}\" }}\n        inject:\n          - bearer: \"{{credentials.{bearer}}}\"\n"
    )
}

/// Upload `bytes` as the admin (any signer); the raw status and body.
async fn upload_bytes(a: &Admin<'_>, bytes: &[u8]) -> (u16, Value) {
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
        .header("cookie", &a.cookie)
        .header("origin", ORIGIN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = a.h.app.clone().oneshot(req).await.unwrap();
    let s = res.status().as_u16();
    let b = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

fn signed_with(y: &str, k: &SigningKey, key_id: &str) -> Vec<u8> {
    let mut b = PackageBuilder::new(y);
    b.sign(k, key_id);
    b.build().unwrap()
}

async fn trust(a: &Admin<'_>, publisher: &str, k: &SigningKey) {
    let (s, v) = a
        .json(
            Method::POST,
            "/v1/admin/trusted-keys",
            Some(json!({"publisher": publisher, "name": "k2",
                "public_key": PackageBuilder::key_id(k)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
}

/// Install `trusted` 1.0.0 (publisher `acme`, no oauth) and approve it.
async fn install(a: &Admin<'_>, token_url: Option<&str>) -> Value {
    let v = a.upload(&yaml("acme", "1.0.0", token_url)).await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    let (s, b) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{b}");
    v
}

#[tokio::test]
async fn another_publishers_key_cannot_upgrade_a_package_unseen() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, None).await;
    let other = generate_signing_key().unwrap();
    trust(&a, "mallory", &other).await;
    let (s, v) = upload_bytes(
        &a,
        &signed_with(
            &yaml("mallory", "1.1.0", None),
            &other,
            &PackageBuilder::key_id(&other),
        ),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "pending_approval", "{v}");
    assert_eq!(v["status_reason"], "publisher_changed", "{v}");

    // The same id from the built-in source: only a built-in version may follow it.
    sqlx::query("UPDATE plugin_packages SET source = 'builtin' WHERE package_id = 'trusted'")
        .execute(h.pool())
        .await
        .unwrap();
    let v = a.upload(&yaml("acme", "1.2.0", None)).await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    assert_eq!(v["status_reason"], "builtin_id_reserved", "{v}");
    let (s, v) = upload_bytes(
        &a,
        &signed_with(
            &yaml("mallory", "1.3.0", None),
            &other,
            &PackageBuilder::key_id(&other),
        ),
    )
    .await;
    assert_eq!(
        (s, v["status_reason"].as_str()),
        (201, Some("publisher_changed"))
    );
    // Nothing was activated by any of them.
    let active: Vec<String> = sqlx::query_scalar(
        "SELECT version FROM plugin_packages WHERE package_id = 'trusted' AND status = 'active'",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(active, ["1.0.0"]);
}

#[tokio::test]
async fn an_older_signed_version_waits_for_approval() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let v1 = a.upload(&yaml("acme", "1.0.0", None)).await;
    assert_eq!(a.act(&v1["id"], "approve").await.0, 200);
    let v2 = a.upload(&yaml("acme", "2.0.0", None)).await;
    assert_eq!(v2["status"], "active", "{v2}");
    // 1.9.0 is newer than the installed 1.0.0 but older than the approved 2.0.0.
    let old = a.upload(&yaml("acme", "1.9.0", None)).await;
    assert_eq!(old["status"], "pending_approval", "{old}");
    assert_eq!(old["status_reason"], "version_downgrade", "{old}");
    let active: Vec<String> = sqlx::query_scalar(
        "SELECT version FROM plugin_packages WHERE package_id = 'trusted' AND status = 'active'",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(active, ["2.0.0"]);
    // A newer one still upgrades automatically.
    let v3 = a.upload(&yaml("acme", "2.0.1", None)).await;
    assert_eq!(v3["status"], "active", "{v3}");
}

#[tokio::test]
async fn a_new_token_endpoint_host_is_a_widening_and_reserved_hosts_are_invalid() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, Some(&format!("https://{HOST}/token"))).await;
    // Same host, different path: still automatic.
    let same = a
        .upload(&yaml(
            "acme",
            "1.0.1",
            Some(&format!("https://{HOST}/oauth/token")),
        ))
        .await;
    assert_eq!(same["status"], "active", "{same}");
    // Only the token host changes (and it is listed): held, with the host shown.
    let moved = yaml(
        "acme",
        "1.0.2",
        Some("https://collector.evil.example/token"),
    )
    .replacen(
        "apps:",
        "permissions:\n  token_hosts: [collector.evil.example]\napps:",
        1,
    );
    let v = a.upload(&moved).await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    assert_eq!(v["status_reason"], "permissions_widened", "{v}");
    assert!(
        v["widenings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["kind"] == "token_host" && w["detail"] == "collector.evil.example"),
        "{v}"
    );
    // Unlisted and unclaimed: not a package at all.
    let bad = a
        .upload(&yaml(
            "acme",
            "1.0.3",
            Some("https://collector.evil.example/token"),
        ))
        .await;
    assert_eq!(bad["status"], "invalid", "{bad}");
    assert!(
        bad["violations"]
            .to_string()
            .contains("token_host_unclaimed")
    );
    // A reserved host cannot be listed either.
    let reserved = yaml("acme", "1.0.4", Some("https://aurora-api/token")).replacen(
        "apps:",
        "permissions:\n  token_hosts: [aurora-api]\napps:",
        1,
    );
    let bad = a.upload(&reserved).await;
    assert_eq!(bad["status"], "invalid", "{bad}");
    assert!(
        bad["violations"]
            .to_string()
            .contains("token_host_reserved")
    );
}

#[tokio::test]
async fn key_ids_are_canonical_and_revocation_finds_legacy_spellings() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    // A key whose canonical encoding contains `+` or `/`.
    let k = loop {
        let k = generate_signing_key().unwrap();
        if PackageBuilder::key_id(&k).contains(['+', '/']) {
            break k;
        }
    };
    let canonical = PackageBuilder::key_id(&k);
    let urlsafe = base64::engine::general_purpose::URL_SAFE.encode(k.verifying_key().to_bytes());
    assert_ne!(urlsafe, canonical);
    // Trusting the key by its URL-safe spelling stores the canonical one.
    let (s, row) = a
        .json(
            Method::POST,
            "/v1/admin/trusted-keys",
            Some(json!({"publisher": "kpub", "name": "k", "public_key": urlsafe})),
        )
        .await;
    assert_eq!(s, 201, "{row}");
    assert_eq!(row["public_key"], canonical, "{row}");

    // Signed with a non-canonical spelling: refused outright, no row.
    for spelling in [&urlsafe, &canonical.trim_end_matches('=').to_owned()] {
        let (s, v) =
            upload_bytes(&a, &signed_with(&yaml("kpub", "1.0.0", None), &k, spelling)).await;
        assert_eq!(s, 422, "{spelling}: {v}");
        assert_eq!(
            v["error"]["details"]["violations"][0]["code"], "signature_malformed",
            "{v}"
        );
    }
    let (s, ok) = upload_bytes(
        &a,
        &signed_with(&yaml("kpub", "1.0.0", None), &k, &canonical),
    )
    .await;
    assert_eq!(s, 201, "{ok}");
    let id = ok["id"].as_str().unwrap().to_owned();
    assert_eq!(a.act(&ok["id"], "approve").await.0, 200);

    // A row written before ids were canonical carries the URL-safe spelling: revocation still
    // disables it (and `enable` refuses it afterwards).
    sqlx::query("UPDATE plugin_packages SET signer_key_id = $2 WHERE id = $1::uuid")
        .bind(&id)
        .bind(&urlsafe)
        .execute(h.pool())
        .await
        .unwrap();
    let (_, keys) = a.json(Method::GET, "/v1/admin/trusted-keys", None).await;
    let kid = keys["items"][0]["id"].as_str().unwrap().to_owned();
    let (s, _) = a
        .json(
            Method::DELETE,
            &format!("/v1/admin/trusted-keys/{kid}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let got = a.status(&ok["id"]).await;
    assert_eq!(got["status"], "disabled", "{got}");
    assert_eq!(got["status_reason"], "signing_key_revoked");
    let (s, e) = a.act(&ok["id"], "enable").await;
    assert_eq!(s, 409, "{e}");
}

/// The data fix: a stored non-canonical `signer_key_id` is rewritten by the migration's SQL.
#[tokio::test]
async fn migration_canonicalises_stored_signer_key_ids() {
    let h = Harness::new().await;
    let k = generate_signing_key().unwrap();
    let canonical = PackageBuilder::key_id(&k);
    let urlsafe = base64::engine::general_purpose::URL_SAFE.encode(k.verifying_key().to_bytes());
    let sql = include_str!(
        "../../../../crates/aurora-db/migrations/20261201000070_plugin_signer_key_canonical.sql"
    );
    for (i, spelling) in [urlsafe.as_str(), canonical.trim_end_matches('='), "builtin"]
        .iter()
        .enumerate()
    {
        sqlx::query("INSERT INTO plugin_packages (id, package_id, version, contract, publisher, digest, signer_key_id, manifest, permissions, source, status) VALUES ($1,$2,'1.0.0',1,'p',$2,$3,'{}','{}','upload','disabled')")
            .bind(uuid::Uuid::now_v7())
            .bind(format!("legacy{i}"))
            .bind(spelling)
            .execute(h.pool())
            .await
            .unwrap();
    }
    sqlx::raw_sql(sql).execute(h.pool()).await.unwrap();
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT signer_key_id FROM plugin_packages WHERE package_id LIKE 'legacy%' ORDER BY package_id",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(ids, [canonical.clone(), canonical, "builtin".to_owned()]);
}

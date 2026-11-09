#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P3-022 (RFE-022, D-65, SEC-57; C-340..C-343): plugin major upgrades and live connections.

use crate::common::*;
use crate::plugins_lifecycle::{Admin, setup};
use http::Method;
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use uuid::Uuid;

/// Package `up` with one app `up_app`; `creds` = `(name, optional)`.
fn yaml(version: &str, creds: &[(&str, bool)], host: &str) -> String {
    let decl: Vec<String> = creds
        .iter()
        .map(|(n, opt)| format!("{{ name: {n}, secret: true, optional: {opt} }}"))
        .collect();
    let fields: Vec<String> = creds
        .iter()
        .filter(|(_, opt)| !opt)
        .map(|(n, _)| {
            format!("{{ name: {n}, label: {n}, type: secret, store: credentials, required: true }}")
        })
        .collect();
    format!(
        "package: {{ id: up, version: {version}, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}\napps:\n  - about: {{ id: up_app, name: A }}\n    credentials: [ {} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {} ]\n    hosts:\n      - match: {{ host: \"{host}\" }}\n        inject:\n          - bearer: \"{{credentials.{}}}\"\n",
        decl.join(", "),
        fields.join(", "),
        creds[0].0
    )
}

const HOST: &str = "api.up.example";

async fn seed(
    h: &Harness,
    org: Uuid,
    status: &str,
    keys: Option<&[&str]>,
    app_version: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    let keys: Option<Vec<String>> = keys.map(|k| k.iter().map(|s| (*s).to_owned()).collect());
    sqlx::query("INSERT INTO app_connections (id, org_id, app_id, package_id, app_version, label, status, credentials_ref, credential_keys) VALUES ($1,$2,'up_app','up',$3,'c',$4,'ref',$5)")
        .bind(id)
        .bind(org)
        .bind(app_version)
        .bind(status)
        .bind(keys)
        .execute(h.pool())
        .await
        .unwrap();
    id
}

async fn row(h: &Harness, id: Uuid) -> (String, Option<String>, bool) {
    sqlx::query_as("SELECT status, status_reason, reauth_required_at IS NOT NULL FROM app_connections WHERE id = $1")
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

/// Install and approve `up` 1.0.0 (required `api_token`).
async fn install_v1(a: &Admin<'_>) {
    let v = a
        .upload(&yaml("1.0.0", &[("api_token", false)], HOST))
        .await;
    assert_eq!(v["status"], "pending_approval");
    let (s, b) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{b}");
    assert_eq!(b["connections_reauth"], 0);
}

struct Seeded {
    kept: Uuid,
    reauth: Uuid,
    disabled: Uuid,
    orphaned: Uuid,
    legacy_known: Uuid,
    legacy_unknown: Uuid,
}

/// One connection per status / key-knowledge case.
async fn seed_matrix(h: &Harness) -> Seeded {
    let org = h.org().await;
    Seeded {
        kept: seed(h, org, "active", Some(&["api_token"]), "1.0.0").await,
        reauth: seed(h, org, "reauth_required", Some(&["api_token"]), "1.0.0").await,
        disabled: seed(h, org, "disabled", Some(&["api_token"]), "1.0.0").await,
        orphaned: seed(h, org, "orphaned", Some(&["api_token"]), "1.0.0").await,
        legacy_known: seed(h, org, "active", None, "1.0.0").await,
        legacy_unknown: seed(h, org, "active", None, "0.0.1").await,
    }
}

async fn notifications(l: &mut PgListener) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(Ok(Some(n))) =
        tokio::time::timeout(std::time::Duration::from_millis(300), l.try_recv()).await
    {
        if let Ok(v) = serde_json::from_str::<Value>(n.payload()) {
            out.push(v);
        }
    }
    out
}

async fn audit_meta(h: &Harness, action: &str) -> Vec<Value> {
    sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action = $1 ORDER BY id")
        .bind(action)
        .fetch_all(h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn renamed_required_field_flips_exactly_the_failing_active_connections() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install_v1(&a).await;
    let s = seed_matrix(&h).await;

    // v2 renames the required field: the upgrade is held, never activated automatically.
    let v2 = a.upload(&yaml("2.0.0", &[("token_v2", false)], HOST)).await;
    assert_eq!(
        (v2["status"].as_str(), v2["status_reason"].as_str()),
        (Some("pending_approval"), Some("breaks_connections")),
        "{v2}"
    );
    // The list never carries the count; the detail does (C-340).
    let (_, list) = a
        .json(
            Method::GET,
            "/v1/admin/plugins?status=pending_approval",
            None,
        )
        .await;
    assert!(list["items"][0]["impact"].is_null(), "{list}");
    let det = a.status(&v2["id"]).await;
    let imp = &det["impact"];
    assert_eq!(imp["major_upgrade"], true);
    assert_eq!(imp["from_version"], "1.0.0");
    // Live = active + reauth_required: kept, reauth, legacy_known, legacy_unknown.
    assert_eq!(imp["connections_total"], 4, "{imp}");
    assert_eq!(imp["connections_needing_reauth"], 4);
    assert_eq!(
        imp["by_app"],
        json!([{"app_id": "up_app", "connections": 4, "needing_reauth": 4}])
    );
    // Counts only: no tenant identifier anywhere in the response.
    let text = det.to_string();
    for id in [s.kept, s.reauth, s.legacy_known, s.legacy_unknown] {
        assert!(!text.contains(&id.to_string()));
    }
    assert!(!text.contains(&h.org().await.to_string()));

    let mut l = PgListener::connect_with(h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (st, ap) = a.act(&v2["id"], "approve").await;
    assert_eq!(st, 200, "{ap}");
    // The already-`reauth_required` row is counted in the impact, not flipped again.
    assert_eq!(ap["connections_reauth"], 3);
    assert_eq!(ap["status"], "active");
    assert!(ap["impact"].is_null());

    let reason = "plugin_major_upgrade: up 1.0.0 -> 2.0.0; missing: token_v2";
    for id in [s.kept, s.legacy_known, s.legacy_unknown] {
        let (status, why, at) = row(&h, id).await;
        assert_eq!(
            (status.as_str(), why.as_deref(), at),
            ("reauth_required", Some(reason), true)
        );
    }
    assert_eq!(row(&h, s.reauth).await.0, "reauth_required");
    assert_eq!(row(&h, s.reauth).await.1, None, "not rewritten");
    assert_eq!(row(&h, s.disabled).await.0, "disabled");
    assert_eq!(row(&h, s.orphaned).await.0, "orphaned");
    assert!(!row(&h, s.disabled).await.2);

    // `connection` invalidation reaches the gateways (delivered at commit).
    let got = notifications(&mut l).await;
    for id in [s.kept, s.legacy_known, s.legacy_unknown] {
        assert!(
            got.iter()
                .any(|n| n["scope"] == "connection" && n["id"] == id.to_string()),
            "no invalidation for {id}: {got:?}"
        );
    }
    // The audit row of the (critical) approval carries the counts, never an identity.
    let meta = audit_meta(&h, "plugin.approve").await;
    let last = meta.last().unwrap();
    assert_eq!(last["connections_reauth"], 3);
    assert_eq!(last["by_app"], json!({"up_app": 3}));
}

#[tokio::test]
async fn kept_fields_and_optional_additions_flip_nothing_but_unknown_legacy_fails_closed() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install_v1(&a).await;
    let s = seed_matrix(&h).await;

    // 1.1: an added optional field (minor): auto-activates, nothing changes.
    let v11 = a
        .upload(&yaml(
            "1.1.0",
            &[("api_token", false), ("extra", true)],
            HOST,
        ))
        .await;
    assert_eq!(v11["status"], "active", "{v11}");
    for id in [s.kept, s.legacy_known, s.legacy_unknown] {
        assert_eq!(row(&h, id).await.0, "active");
    }

    // 2.0: required `api_token` kept, optional added: a major upgrade, but only the legacy
    // row whose keys cannot be derived (no stored manifest for its version) fails (closed).
    let v2 = a
        .upload(&yaml(
            "2.0.0",
            &[("api_token", false), ("extra", true), ("more", true)],
            HOST,
        ))
        .await;
    assert_eq!(
        (v2["status"].as_str(), v2["status_reason"].as_str()),
        (Some("pending_approval"), Some("breaks_connections")),
        "{v2}"
    );
    let imp = a.status(&v2["id"]).await["impact"].clone();
    assert_eq!(imp["major_upgrade"], true);
    assert_eq!(imp["connections_needing_reauth"], 1, "{imp}");
    let (_, ap) = a.act(&v2["id"], "approve").await;
    assert_eq!(ap["connections_reauth"], 1);
    assert_eq!(row(&h, s.legacy_unknown).await.0, "reauth_required");
    assert!(
        row(&h, s.legacy_unknown)
            .await
            .1
            .unwrap()
            .ends_with("missing: api_token")
    );
    assert_eq!(row(&h, s.legacy_known).await.0, "active");
    assert_eq!(row(&h, s.kept).await.0, "active");
}

#[tokio::test]
async fn added_required_field_flips_and_minor_renames_never_do() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install_v1(&a).await;
    let org = h.org().await;
    let c = seed(&h, org, "active", Some(&["api_token"]), "1.0.0").await;

    // A minor version may rename a required field: only majors are checked (09 §10).
    let v11 = a.upload(&yaml("1.1.0", &[("renamed", false)], HOST)).await;
    assert_eq!(v11["status"], "active", "{v11}");
    assert_eq!(row(&h, c).await.0, "active");

    // A major version adding a required field breaks every connection not holding it.
    let v2 = a
        .upload(&yaml(
            "2.0.0",
            &[("renamed", false), ("must_have", false)],
            HOST,
        ))
        .await;
    assert_eq!(v2["status_reason"], "breaks_connections");
    let (_, ap) = a.act(&v2["id"], "approve").await;
    assert_eq!(ap["connections_reauth"], 1);
    let (_, why, _) = row(&h, c).await;
    assert_eq!(
        why.unwrap(),
        "plugin_major_upgrade: up 1.1.0 -> 2.0.0; missing: must_have, renamed"
    );
}

#[tokio::test]
async fn major_without_impact_activates_automatically_with_an_audit_row() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install_v1(&a).await;
    let org = h.org().await;
    let c = seed(&h, org, "active", Some(&["api_token"]), "1.0.0").await;
    let v2 = a
        .upload(&yaml("2.0.0", &[("api_token", false), ("n", true)], HOST))
        .await;
    assert_eq!(v2["status"], "active", "{v2}");
    assert_eq!(row(&h, c).await.0, "active");
    let meta = audit_meta(&h, "plugin.activate").await;
    assert_eq!(meta.last().unwrap()["connections_reauth"], 0);
    assert_eq!(meta.last().unwrap()["auto"], true);
}

#[tokio::test]
async fn pending_first_install_has_a_non_major_impact_and_other_statuses_none() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let v = a
        .upload(&yaml("1.0.0", &[("api_token", false)], HOST))
        .await;
    let imp = a.status(&v["id"]).await["impact"].clone();
    assert_eq!(
        imp,
        json!({"major_upgrade": false, "from_version": null, "connections_total": 0,
            "connections_needing_reauth": 0,
            "by_app": [{"app_id": "up_app", "connections": 0, "needing_reauth": 0}]})
    );
    let (_, ap) = a.act(&v["id"], "approve").await;
    assert!(ap["impact"].is_null());
    assert!(a.status(&v["id"]).await["impact"].is_null());
}

#[tokio::test]
async fn failure_after_the_flips_rolls_back_the_activation_and_every_flip() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    // `other` owns `shared.example`; `up` 2.0 claims it too and renames its required field.
    let other = "package: { id: other, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: O }\napps:\n  - about: { id: other_app, name: O }\n    credentials: [ { name: k, secret: true } ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ { name: k, label: k, type: secret, store: credentials, required: true } ]\n    hosts:\n      - match: { host: shared.example }\n        inject:\n          - bearer: \"{credentials.k}\"\n";
    let o = a.upload(other).await;
    assert_eq!(a.act(&o["id"], "approve").await.0, 200);
    install_v1(&a).await;
    let org = h.org().await;
    let c = seed(&h, org, "active", Some(&["api_token"]), "1.0.0").await;
    let v2 = a
        .upload(&yaml("2.0.0", &[("token_v2", false)], "shared.example"))
        .await;
    assert_eq!(v2["status_reason"], "claim_conflict", "{v2}");
    let (s, e) = a.act(&v2["id"], "approve").await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("claim_conflict_unresolved")),
        "{e}"
    );
    // The flip was applied before the compile failed: it must be gone with the activation.
    assert_eq!(row(&h, c).await, ("active".to_owned(), None, false));
    let active: Vec<String> = sqlx::query_scalar(
        "SELECT version FROM plugin_packages WHERE package_id = 'up' AND status = 'active'",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(active, ["1.0.0"]);
    let v2s = a.status(&v2["id"]).await;
    assert_eq!(v2s["status"], "pending_approval");
}

#[tokio::test]
async fn approval_needs_an_instance_admin() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let v = a
        .upload(&yaml("1.0.0", &[("api_token", false)], HOST))
        .await;
    let u = h.user("plain@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let c = cookie(&tok);
    let id = v["id"].as_str().unwrap();
    let (s, _, _) = h
        .send(
            Method::POST,
            &format!("/v1/admin/plugins/{id}/approve"),
            &[("cookie", &c), ("origin", ORIGIN)],
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _, _) = h
        .send(
            Method::GET,
            &format!("/v1/admin/plugins/{id}"),
            &[("cookie", &c)],
            None,
        )
        .await;
    assert_eq!(s, 403);
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-123 (D-195(a), D-215, 09 §10 *Superseded apps*): `github-app` / `github-app-ghes` /
//! `aws-role` became connect methods of `github` / `github-ghes` / `aws-keys`. Activating the
//! version that has the method moves the old apps' connections (ids, credentials, grants kept),
//! instances, skill libraries and availability, deletes the old pair's host-claim decisions and
//! retires the old package; rerunning is a no-op.

use crate::common::*;
use crate::plugins_lifecycle::{Admin, setup};
use aurora_api::domains::plugins::supersede;
use aurora_registry::Manifest;
use http::Method;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const HOOKLESS: &str =
    "package: { id: PKG, version: VER, contract: 1, min_core: 0.1.0, publisher: acme, name: P }\n";

/// The retired `github-app` package: two apps, the second instance-bound.
fn old_github_app() -> String {
    HOOKLESS
        .replace("PKG", "github-app")
        .replace("VER", "1.0.1")
        + r#"apps:
  - about: { id: github-app, name: GitHub App }
    credentials: [ { name: access_token, secret: true }, { name: private_key, secret: true } ]
    connect:
      - id: installation
        kind: fields
        fields: [ { name: private_key, label: K, type: secret, required: true } ]
    hosts:
      - match: { host: api.github.com }
        inject: [ { bearer: "{credentials.access_token}" } ]
  - about: { id: github-app-ghes, name: GitHub App on GHES }
    instances: { label: GHES host }
    credentials: [ { name: access_token, secret: true }, { name: private_key, secret: true } ]
    connect:
      - id: installation
        kind: fields
        fields: [ { name: private_key, label: K, type: secret, required: true } ]
    hosts:
      - match: { host: "{instance}" }
        inject: [ { bearer: "{credentials.access_token}" } ]
"#
}

/// `github` before the merge (1.1.1) and after (2.0.0: the `github_app` method, a new claim so the
/// version is a widening that waits for approval).
fn github(version: &str, merged: bool) -> String {
    let (extra_cred, method, extra_host) = if merged {
        (
            ", { name: private_key, secret: true, optional: true }",
            "      - id: github_app\n        kind: fields\n        label: GitHub App\n        description: An installed app.\n        recommended: true\n        fields: [ { name: private_key, label: K, type: secret, required: true } ]\n",
            "      - match: { host: github.com }\n        inject: [ { bearer: \"{credentials.access_token}\" } ]\n",
        )
    } else {
        ("", "", "")
    };
    HOOKLESS.replace("PKG", "github").replace("VER", version)
        + &format!(
            r#"apps:
  - about: {{ id: github, name: GitHub }}
    credentials: [ {{ name: access_token, secret: true }}{extra_cred} ]
    connect:
{method}      - id: oauth
        kind: fields
        fields: [ {{ name: access_token, label: T, type: secret, required: true }} ]
    hosts:
      - match: {{ host: api.github.com }}
        inject: [ {{ bearer: "{{credentials.access_token}}" }} ]
{extra_host}  - about: {{ id: github-ghes, name: GitHub Enterprise Server }}
    instances: {{ label: GHES host }}
    credentials: [ {{ name: access_token, secret: true }}{extra_cred} ]
    connect:
{method}      - id: oauth
        kind: fields
        fields: [ {{ name: access_token, label: T, type: secret, required: true }} ]
    hosts:
      - match: {{ host: "{{instance}}" }}
        inject: [ {{ bearer: "{{credentials.access_token}}" }} ]
"#
        )
}

async fn force_active(h: &Harness, id: &Value) {
    sqlx::query("UPDATE plugin_packages SET status='active', approved_at=now() WHERE id=$1::uuid")
        .bind(id.as_str().unwrap())
        .execute(h.pool())
        .await
        .unwrap();
}

struct Seed {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    c_app: Uuid,
    c_ghes: Uuid,
    c_ghes_other: Uuid,
    c_plain: Uuid,
    i_old_same: Uuid,
    i_new_same: Uuid,
    i_old_other: Uuid,
}

#[allow(clippy::too_many_arguments)]
async fn conn(
    pool: &PgPool,
    org: Uuid,
    ws: Option<Uuid>,
    (app, pkg, ver): (&str, &str, &str),
    status: &str,
    instance: Option<Uuid>,
    method: Option<&str>,
    keys: &[&str],
) -> Uuid {
    let id = Uuid::now_v7();
    let keys: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label,
            status, credentials_ref, instance_id, connect_method, credential_keys, scopes)
         VALUES ($1,$2,$3,$4,$5,$6,'c',$7,'ref-'||$1::text,$8,$9,$10,'{read}')",
    )
    .bind(id)
    .bind(org)
    .bind(ws)
    .bind(app)
    .bind(pkg)
    .bind(ver)
    .bind(status)
    .bind(instance)
    .bind(method)
    .bind(&keys)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn instance(pool: &PgPool, org: Uuid, pkg: &str, app: &str, host: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO app_instances (id, org_id, package_id, app_id, host, label) VALUES ($1,$2,$3,$4,$5,'GHES')",
    )
    .bind(id)
    .bind(org)
    .bind(pkg)
    .bind(app)
    .bind(host)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed(h: &Harness) -> Seed {
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let p = h.pool();
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'a','A','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(p).await.unwrap();
    let i_old_same = instance(p, org, "github-app", "github-app-ghes", "ghes.example.com").await;
    let i_new_same = instance(p, org, "github", "github-ghes", "ghes.example.com").await;
    let i_old_other = instance(p, org, "github-app", "github-app-ghes", "other.example.com").await;
    let both = &["access_token", "private_key"][..];
    let c_app = conn(
        p,
        org,
        Some(ws),
        ("github-app", "github-app", "1.0.1"),
        "active",
        None,
        None,
        both,
    )
    .await;
    let c_ghes = conn(
        p,
        org,
        None,
        ("github-app-ghes", "github-app", "1.0.1"),
        "reauth_required",
        Some(i_old_same),
        None,
        both,
    )
    .await;
    let c_ghes_other = conn(
        p,
        org,
        None,
        ("github-app-ghes", "github-app", "1.0.1"),
        "active",
        Some(i_old_other),
        None,
        both,
    )
    .await;
    let c_plain = conn(
        p,
        org,
        None,
        ("github", "github", "1.1.1"),
        "active",
        None,
        Some("oauth"),
        &["access_token"],
    )
    .await;
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode, allow_tools, ask_tools) VALUES ($1,$2,$3,$4,'custom','{get_repo,list_issues}','{}')")
        .bind(Uuid::now_v7()).bind(ws).bind(agent).bind(c_app).execute(p).await.unwrap();
    sqlx::query("INSERT INTO skill_libraries (id, org_id, label, connection_id, app_id, repo) VALUES ($1,$2,'lib',$3,'github-app','example/skills')")
        .bind(Uuid::now_v7()).bind(org).bind(c_app).execute(p).await.unwrap();
    sqlx::query("INSERT INTO org_app_availability (org_id, app_id, workspace_id) VALUES ($1,'github-app',NULL)")
        .bind(org).execute(p).await.unwrap();
    sqlx::query("INSERT INTO host_claim_decisions (id, claim_key, claim_fingerprint, winner_package_id, winner_app_id) VALUES ($1,'api.github.com|/','f','github-app','github-app')")
        .bind(Uuid::now_v7()).execute(p).await.unwrap();
    Seed {
        org,
        ws,
        agent,
        c_app,
        c_ghes,
        c_ghes_other,
        c_plain,
        i_old_same,
        i_new_same,
        i_old_other,
    }
}

type Row = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<Uuid>,
);

async fn row(h: &Harness, id: Uuid) -> Row {
    sqlx::query_as(
        "SELECT app_id, package_id, app_version, connect_method, status, credentials_ref, instance_id
           FROM app_connections WHERE id = $1",
    )
    .bind(id)
    .fetch_one(h.pool())
    .await
    .unwrap()
}

async fn package_status(h: &Harness, pkg: &str) -> Vec<(String, Option<String>)> {
    sqlx::query_as("SELECT status, status_reason FROM plugin_packages WHERE package_id = $1 ORDER BY created_at")
        .bind(pkg)
        .fetch_all(h.pool())
        .await
        .unwrap()
}

/// The activation of `github` 2.0.0 moves everything, keeping ids, credentials and grants.
#[tokio::test]
async fn approving_github_moves_the_github_app_connections_and_retires_the_package() {
    let h = Harness::new().await;
    let a: Admin<'_> = setup(&h).await;
    // Before: github 1.1.1 and github-app 1.0.1 are both active and claim api.github.com (the
    // conflict a fresh instance used to start with); activated directly, as an older Aurora did.
    let g1 = a.upload(&github("1.1.1", false)).await;
    let old = a.upload(&old_github_app()).await;
    force_active(&h, &g1["id"]).await;
    force_active(&h, &old["id"]).await;
    let s = seed(&h).await;
    let before = row(&h, s.c_app).await;

    let g2 = a.upload(&github("2.0.0", true)).await;
    // The retired package is not a conflict for the admin to decide.
    assert_eq!(g2["status"], "pending_approval", "{g2}");
    assert_ne!(g2["status_reason"], "claim_conflict", "{g2}");
    let (st, conflicts) = a
        .json(http::Method::GET, "/v1/admin/host-claims", None)
        .await;
    assert_eq!(st, 200, "{conflicts}");
    assert!(
        conflicts.to_string().find("github-app").is_none(),
        "no conflict names the retired package: {conflicts}"
    );

    let (st, v) = a.act(&g2["id"], "approve").await;
    assert_eq!(st, 200, "{v}");

    // Connections: same id, credentials, status and scope; new app, package, version and method.
    let after = row(&h, s.c_app).await;
    assert_eq!(
        after,
        (
            "github".into(),
            "github".into(),
            "2.0.0".into(),
            Some("github_app".into()),
            "active".into(),
            before.5.clone(),
            None
        )
    );
    let ghes = row(&h, s.c_ghes).await;
    assert_eq!(
        (ghes.0.as_str(), ghes.3.as_deref(), ghes.4.as_str()),
        ("github-ghes", Some("github_app"), "reauth_required")
    );
    // The registration that existed on both sides merged into the new app's; the other moved.
    assert_eq!(ghes.6, Some(s.i_new_same));
    let other = row(&h, s.c_ghes_other).await;
    assert_eq!(
        (other.0.as_str(), other.6),
        ("github-ghes", Some(s.i_old_other))
    );
    let (n_old,): (i64,) = sqlx::query_as("SELECT count(*) FROM app_instances WHERE id = $1")
        .bind(s.i_old_same)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(n_old, 0);
    let (iapp, ipkg): (String, String) =
        sqlx::query_as("SELECT app_id, package_id FROM app_instances WHERE id = $1")
            .bind(s.i_old_other)
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!((iapp.as_str(), ipkg.as_str()), ("github-ghes", "github"));
    // A connection of the surviving app is untouched.
    let plain = row(&h, s.c_plain).await;
    assert_eq!(
        (plain.1.as_str(), plain.3.as_deref()),
        ("github", Some("oauth"))
    );
    // The grant still points at the same connection with the same tools.
    let grant: (Uuid, String, Vec<String>) =
        sqlx::query_as("SELECT agent_id, mode, allow_tools FROM grants WHERE connection_id = $1")
            .bind(s.c_app)
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(
        grant,
        (
            s.agent,
            "custom".into(),
            vec!["get_repo".into(), "list_issues".into()]
        )
    );
    // Libraries and availability follow the app; the decision between the old pair is gone.
    let lib: String = sqlx::query_scalar("SELECT app_id FROM skill_libraries WHERE org_id = $1")
        .bind(s.org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(lib, "github");
    let avail: Vec<String> =
        sqlx::query_scalar("SELECT app_id FROM org_app_availability WHERE org_id = $1")
            .bind(s.org)
            .fetch_all(h.pool())
            .await
            .unwrap();
    assert_eq!(avail, vec!["github".to_owned()]);
    let decisions: i64 = sqlx::query_scalar("SELECT count(*) FROM host_claim_decisions")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(decisions, 0);
    // The retired package ends disabled with no connection left; the new one is active and the
    // snapshot compiled (no conflict) without the old apps.
    assert_eq!(
        package_status(&h, "github-app").await,
        vec![("disabled".into(), Some("superseded_by_github".into()))]
    );
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_connections WHERE app_id LIKE 'github-app%'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(left, 0);
    let snap: Value = sqlx::query_scalar(
        "SELECT snapshot FROM registry_snapshots ORDER BY activated_at DESC LIMIT 1",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    let ids: Vec<&str> = snap["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["app_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["github", "github-ghes"]);
    // One audit row with the counts, names and ids only.
    let audit: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'plugin.supersede' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(audit["result"]["connections_moved"], 3);
    assert_eq!(
        audit["result"]["by_app"],
        json!({"github-app": 1, "github-app-ghes": 2})
    );
    assert_eq!(
        audit["result"]["retired_packages"],
        json!({"github-app": 1})
    );

    // The retired package cannot be enabled again.
    let (st, v) = a.act(&old["id"], "enable").await;
    assert_eq!(st, 409, "{v}");
    assert_eq!(v["error"]["code"], "superseded");

    // Rerunning the step is a no-op.
    let stored: Value = sqlx::query_scalar(
        "SELECT manifest FROM plugin_packages WHERE package_id = 'github' AND version = '2.0.0'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    let manifest: Manifest = serde_json::from_value(stored).unwrap();
    let mut tx = h.pool().begin().await.unwrap();
    let again = supersede::apply(&mut tx, &manifest, "2.0.0").await.unwrap();
    tx.commit().await.unwrap();
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(row(&h, s.c_app).await, after);
    let _ = s.ws;
}

/// A version that does not widen activates at upload; the step runs in that path too (`aws-role`).
#[tokio::test]
async fn an_auto_activated_aws_keys_takes_over_the_aws_role_connections() {
    let h = Harness::new().await;
    let a: Admin<'_> = setup(&h).await;
    let aws = |pkg: &str, ver: &str, app: &str, method: &str| {
        HOOKLESS.replace("PKG", pkg).replace("VER", ver)
            + &format!(
                r#"apps:
  - about: {{ id: {app}, name: AWS }}
    credentials: [ {{ name: access_key_id, secret: true }}, {{ name: secret_access_key, secret: true }} ]
    connect:
      - id: {method}
        kind: fields
        fields: [ {{ name: access_key_id, label: A, type: secret, required: true }}, {{ name: secret_access_key, label: S, type: secret, required: true }} ]
    hosts:
      - match: {{ host: "s3.*.amazonaws.com" }}
        inject: [ {{ bearer: "{{credentials.access_key_id}}" }} ]
"#
            )
    };
    // aws-keys 1.1.0 and aws-role 1.1.0, active, claiming the same host.
    let keys1 = a
        .upload(&aws("aws-keys", "1.1.0", "aws-keys", "keys"))
        .await;
    let role = a
        .upload(&aws("aws-role", "1.1.0", "aws-role", "role"))
        .await;
    force_active(&h, &keys1["id"]).await;
    force_active(&h, &role["id"]).await;
    let org = h.org().await;
    let c = conn(
        h.pool(),
        org,
        None,
        ("aws-role", "aws-role", "1.1.0"),
        "active",
        None,
        None,
        &["access_key_id", "secret_access_key", "session_token"],
    )
    .await;
    // aws-keys 2.0.0: both methods, identical claim: no widening, so it activates at upload.
    let merged = aws("aws-keys", "2.0.0", "aws-keys", "keys").replace(
        "    connect:\n",
        "    connect:\n      - id: assume_role\n        kind: fields\n        fields: [ { name: access_key_id, label: A, type: secret, required: true }, { name: secret_access_key, label: S, type: secret, required: true } ]\n",
    );
    let v = a.upload(&merged).await;
    assert_eq!(v["status"], "active", "{v}");
    let r = row(&h, c).await;
    assert_eq!(
        (r.0.as_str(), r.1.as_str(), r.2.as_str(), r.3.as_deref()),
        ("aws-keys", "aws-keys", "2.0.0", Some("assume_role"))
    );
    assert_eq!(
        package_status(&h, "aws-role").await,
        vec![("disabled".into(), Some("superseded_by_aws-keys".into()))]
    );
}

/// FIX-135 (D-242, C-744): the catalogue DTOs carry `family` (id, name, variant label, whether the
/// member is instance-bound); an app without a family has `null`. The shipped GitHub and Jira
/// families are declared by the real manifests (registry `builtin_versions`).
#[tokio::test]
async fn the_catalogue_carries_the_app_family() {
    let h = Harness::new().await;
    let a: Admin<'_> = setup(&h).await;
    let y = HOOKLESS.replace("PKG", "fam").replace("VER", "1.0.0")
        + r#"apps:
  - about: { id: desk, name: Desk, family: { id: desk, name: Desk, variant_label: desk.example } }
    credentials: [ { name: access_token, secret: true } ]
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: T, type: secret, store: credentials, required: true } ]
    hosts:
      - match: { host: api.desk.example }
        inject: [ { bearer: "{credentials.access_token}" } ]
  - about: { id: desk-self, name: Desk Server, family: { id: desk, name: Desk, variant_label: Server } }
    instances: { label: Desk server }
    credentials: [ { name: access_token, secret: true } ]
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: T, type: secret, store: credentials, required: true } ]
    hosts:
      - match: { host: "{instance}" }
        inject: [ { bearer: "{credentials.access_token}" } ]
  - about: { id: solo, name: Solo }
    credentials: [ { name: access_token, secret: true } ]
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: T, type: secret, store: credentials, required: true } ]
    hosts:
      - match: { host: api.solo.example }
        inject: [ { bearer: "{credentials.access_token}" } ]
"#;
    let v = a.upload(&y).await;
    if v["status"] == "pending_approval" {
        let (st, b) = a.act(&v["id"], "approve").await;
        assert_eq!(st, 200, "{b}");
    }
    let (st, list) = a.json(Method::GET, "/v1/apps", None).await;
    assert_eq!(st, 200, "{list}");
    let fam = |id: &str| {
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["app_id"] == id)
            .unwrap_or_else(|| panic!("{id} in {list}"))["family"]
            .clone()
    };
    assert_eq!(
        fam("desk"),
        json!({"id": "desk", "name": "Desk", "variant_label": "desk.example", "instance_bound": false})
    );
    assert_eq!(
        fam("desk-self"),
        json!({"id": "desk", "name": "Desk", "variant_label": "Server", "instance_bound": true})
    );
    assert!(fam("solo").is_null());
    let (st, d) = a.json(Method::GET, "/v1/apps/desk-self", None).await;
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["family"]["variant_label"], "Server");
}

/// FIX-134 (D-243): one Sentry app. `sentry` 1.0.2 has four apps; 2.0.0 is one app with two preset
/// instances. Activating it seeds the presets, moves the region apps' connections to the matching
/// preset instance and the self-hosted app's to the same registration, keeps ids and grants, and
/// retires no package (the old apps live in the package itself). Rerunning is a no-op; a preset
/// instance cannot be renamed, disabled, promoted or deleted.
#[tokio::test]
async fn one_sentry_app_takes_over_the_region_and_self_hosted_connections() {
    let root = env!("CARGO_MANIFEST_DIR");
    let load = |p: &str| {
        std::fs::read_to_string(format!("{root}/{p}"))
            .unwrap()
            .replace("publisher: aurora", "publisher: acme")
    };
    let previous = load("../../crates/aurora-registry/tests/fixtures/previous/sentry-1.0.2.yaml");
    let shipped = load("../../plugins/builtin/sentry/plugin.yaml");
    let h = Harness::new().await;
    let a: Admin<'_> = setup(&h).await;
    let v1 = a.upload(&previous).await;
    let (st, b) = a.act(&v1["id"], "approve").await;
    assert_eq!(st, 200, "{b}");

    let org = h.org().await;
    let ws = h.workspace(org).await;
    let p = h.pool();
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'a','A','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(p).await.unwrap();
    let key = &["access_token"][..];
    let sentry = |app: &'static str| (app, "sentry", "1.0.2");
    let c_default = conn(
        p,
        org,
        None,
        sentry("sentry"),
        "active",
        None,
        Some("token"),
        key,
    )
    .await;
    let c_us = conn(
        p,
        org,
        Some(ws),
        sentry("sentry-us"),
        "active",
        None,
        Some("token"),
        key,
    )
    .await;
    let c_de = conn(
        p,
        org,
        None,
        sentry("sentry-de"),
        "active",
        None,
        Some("token"),
        key,
    )
    .await;
    let inst = instance(p, org, "sentry", "sentry-self-hosted", "sentry.example.com").await;
    let c_self = conn(
        p,
        org,
        None,
        sentry("sentry-self-hosted"),
        "reauth_required",
        Some(inst),
        Some("token"),
        key,
    )
    .await;
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode, allow_tools, ask_tools) VALUES ($1,$2,$3,$4,'custom','{get_issue,list_issues}','{update_issue}')")
        .bind(Uuid::now_v7()).bind(ws).bind(agent).bind(c_us).execute(p).await.unwrap();

    // (P5-GUIDE-4) The shipped package lists an agent guide file: it travels in the archive.
    let notes = std::fs::read(format!(
        "{}/../../plugins/builtin/sentry/agent/sentry/api-notes.md",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let v2 = a
        .upload_with(&shipped, &[("agent/sentry/api-notes.md", notes.as_slice())])
        .await;
    assert_eq!(v2["version"], "2.3.1", "{v2}");
    if v2["status"] == "pending_approval" {
        let (st, b) = a.act(&v2["id"], "approve").await;
        assert_eq!(st, 200, "{b}");
    }
    assert_eq!(a.status(&v2["id"]).await["status"], "active");
    assert_eq!(a.status(&v1["id"]).await["status"], "superseded");

    let preset = |host: &'static str| {
        let pool = p.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM app_instances WHERE builtin AND app_id = 'sentry' AND host = $1 AND org_id IS NULL AND status = 'active'",
            )
            .bind(host)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let us = preset("us.sentry.io").await;
    let de = preset("de.sentry.io").await;
    let labels: Vec<String> =
        sqlx::query_scalar("SELECT label FROM app_instances WHERE builtin ORDER BY host")
            .fetch_all(p)
            .await
            .unwrap();
    assert_eq!(labels, ["EU region (DE)", "US region"]);

    let tail = |r: &Row| {
        (
            r.0.clone(),
            r.1.clone(),
            r.2.clone(),
            r.3.clone(),
            r.4.clone(),
            r.6,
        )
    };
    let moved = |inst| {
        (
            "sentry".to_owned(),
            "sentry".to_owned(),
            "2.3.1".to_owned(),
            Some("token".to_owned()),
            "active".to_owned(),
            inst,
        )
    };
    // The sentry.io connection is untouched (it keeps the version it was made with); the region ones are bound to
    // their preset; the self-hosted one keeps its registration and its status.
    let mut untouched = moved(None);
    untouched.2 = "1.0.2".to_owned();
    assert_eq!(tail(&row(&h, c_default).await), untouched);
    assert_eq!(tail(&row(&h, c_us).await), moved(Some(us)));
    assert_eq!(tail(&row(&h, c_de).await), moved(Some(de)));
    let r = row(&h, c_self).await;
    assert_eq!(
        (r.0.as_str(), r.4.as_str(), r.6),
        ("sentry", "reauth_required", Some(inst))
    );
    let (iapp, ipkg, ibuiltin): (String, String, bool) =
        sqlx::query_as("SELECT app_id, package_id, builtin FROM app_instances WHERE id = $1")
            .bind(inst)
            .fetch_one(p)
            .await
            .unwrap();
    assert_eq!(
        (iapp.as_str(), ipkg.as_str(), ibuiltin),
        ("sentry", "sentry", false)
    );
    // The grant is the same row on the same connection with the same tools.
    let grant: (String, Vec<String>, Vec<String>) =
        sqlx::query_as("SELECT mode, allow_tools, ask_tools FROM grants WHERE connection_id = $1")
            .bind(c_us)
            .fetch_one(p)
            .await
            .unwrap();
    assert_eq!(
        grant,
        (
            "custom".into(),
            vec!["get_issue".into(), "list_issues".into()],
            vec!["update_issue".into()]
        )
    );
    // No old app is left anywhere and the package itself was not retired.
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app_connections WHERE app_id IN ('sentry-us','sentry-de','sentry-self-hosted')",
    ).fetch_one(p).await.unwrap();
    assert_eq!(left, 0);
    assert_eq!(
        package_status(&h, "sentry").await,
        vec![("superseded".into(), None), ("active".into(), None)]
    );
    let audit: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'plugin.supersede' ORDER BY created_at DESC LIMIT 1",
    ).fetch_one(p).await.unwrap();
    assert_eq!(audit["result"]["connections_moved"], 3);
    assert_eq!(
        audit["result"]["by_app"],
        json!({"sentry-us": 1, "sentry-de": 1, "sentry-self-hosted": 1})
    );
    assert_eq!(audit["result"]["retired_packages"], json!({}));

    // Rerunning the step is a no-op (the presets are refreshed, not duplicated).
    let stored: Value = sqlx::query_scalar(
        "SELECT manifest FROM plugin_packages WHERE package_id = 'sentry' AND version = '2.3.1'",
    )
    .fetch_one(p)
    .await
    .unwrap();
    let manifest: Manifest = serde_json::from_value(stored).unwrap();
    let mut tx = p.begin().await.unwrap();
    let again = supersede::apply(&mut tx, &manifest, "2.3.1").await.unwrap();
    tx.commit().await.unwrap();
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(again.presets, 2);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_instances WHERE builtin")
        .fetch_one(p)
        .await
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(tail(&row(&h, c_us).await), moved(Some(us)));

    // A preset instance is read-only to everyone.
    let (st, v) = a
        .json(
            Method::PATCH,
            &format!("/v1/app-instances/{us}"),
            Some(json!({"label": "x"})),
        )
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (409, Some("instance_builtin")),
        "{v}"
    );
    let (st, v) = a
        .json(Method::DELETE, &format!("/v1/app-instances/{us}"), None)
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (409, Some("instance_builtin")),
        "{v}"
    );
    let (st, v) = a
        .json(
            Method::POST,
            &format!("/v1/admin/app-instances/{us}/promote"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (409, Some("instance_builtin")),
        "{v}"
    );
    // ... and the admin list shows it as built in.
    let (st, v) = a
        .json(Method::GET, "/v1/admin/app-instances?app_id=sentry", None)
        .await;
    assert_eq!(st, 200, "{v}");
    let built: Vec<bool> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["builtin"].as_bool().unwrap())
        .collect();
    assert_eq!(built.iter().filter(|b| **b).count(), 2, "{v}");
}

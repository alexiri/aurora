#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-124 (D-195, D-236, C-746..C-748): Admin › Secret stores. Store configuration is the
//! `secrets` settings section (SET-1's service: locks, CAS, critical audit, re-authentication);
//! the `memory` store stands in for Vault here, the Vault code path is covered by
//! `aurora-secrets`' suites (`AURORA_TEST_VAULT=1`) and by the config-level tests below.

use crate::common::*;
use aurora_api::secrets_migrate::{Options, run};
use aurora_core::Secret;
use aurora_secrets::{MemoryStore, Purpose, SealContext, SecretRef, SecretVault, StoreRegistry};
use http::Method;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const CANARY: &str = "canary-approle-secret-id-9f3a";

/// A harness with the deployment configuration attached, so writes are validated on the merged
/// view as on a real instance (`validate` is on only when the configuration is known).
async fn harness() -> Harness {
    Harness::new().await.with_helm("")
}

struct Admin {
    cookie: String,
    session: Uuid,
}

async fn admin(h: &Harness, email: &str, reauth: bool) -> Admin {
    let user = h.user(email, true).await;
    let (session, tok) = h.good_session(user).await;
    if reauth {
        h.reauth(session).await;
    }
    Admin {
        cookie: cookie(&tok),
        session,
    }
}

async fn call(h: &Harness, a: &Admin, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &a.cookie), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

async fn status(h: &Harness, a: &Admin) -> Value {
    let (s, v) = call(h, a, Method::GET, "/v1/admin/secret-stores", None).await;
    assert_eq!(s, 200, "{v}");
    v
}

fn store<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["stores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("no store {id} in {v}"))
}

async fn put_memory_store(h: &Harness, a: &Admin, id: &str, generation: i64) -> (u16, Value) {
    call(
        h,
        a,
        Method::PUT,
        &format!("/v1/admin/secret-stores/{id}"),
        Some(json!({ "generation": generation, "store": { "kind": "memory" } })),
    )
    .await
}

fn memory_vault() -> (Arc<SecretVault>, Arc<MemoryStore>) {
    let k = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [b'k'; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{k}"), "k1").unwrap());
    let mut reg =
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap();
    let mem = Arc::new(MemoryStore::new("mem"));
    reg.register(mem.clone());
    (Arc::new(SecretVault::new(reg)), mem)
}

fn ctx(owner: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::InstanceSetting,
        owner_id: owner,
        org_id: Uuid::nil(),
        workspace_id: None,
    }
}

#[tokio::test]
async fn status_lists_the_inline_store_and_refuses_non_admins() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let v = status(&h, &a).await;
    assert_eq!(v["default_store"]["value"], "inline");
    assert_eq!(v["default_store"]["source"], "default");
    assert_eq!(v["effective_default"], "inline");
    let inline = store(&v, "inline");
    assert_eq!(
        (inline["source"].clone(), inline["default"].clone()),
        (json!("builtin"), json!(true))
    );
    assert_eq!(inline["health"]["ok"], true);
    assert_eq!(v["migration"]["state"], "none");

    let user = h.user("member@example.com", false).await;
    let (_, tok) = h.good_session(user).await;
    let (s, _, _) = h
        .send(
            Method::GET,
            "/v1/admin/secret-stores",
            &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 403);
}

#[tokio::test]
async fn adding_a_store_and_making_it_default_takes_effect_without_a_restart() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let g0 = status(&h, &a).await["generation"].as_i64().unwrap();

    // Test connection first: nothing is saved by it.
    let (s, t) = call(
        &h,
        &a,
        Method::POST,
        "/v1/admin/secret-store-test",
        Some(json!({ "id": "mem", "store": { "kind": "memory" } })),
    )
    .await;
    assert_eq!(s, 200, "{t}");
    assert_eq!(t["ok"], true, "{t}");
    let names: Vec<&str> = t["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["configuration", "authenticate", "write", "read", "delete"]
    );
    assert_eq!(status(&h, &a).await["generation"], g0);

    let (s, v) = put_memory_store(&h, &a, "mem", g0).await;
    assert_eq!(s, 200, "{v}");
    let mem = store(&v, "mem");
    assert_eq!(mem["source"], "dashboard");
    assert_eq!(mem["health"]["ok"], true);
    assert_eq!(mem["default"], false);
    let g1 = v["generation"].as_i64().unwrap();
    assert!(g1 > g0);

    // Still inline until the default changes.
    let r = h
        .state
        .vault
        .seal(&ctx(Uuid::now_v7()), Secret::new(b"one".to_vec()))
        .await
        .unwrap();
    assert!(r.to_column().starts_with("inline:"));

    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g1, "store_id": "mem" })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["default_store"]["value"], "mem");
    assert_eq!(v["default_store"]["source"], "dashboard");
    assert_eq!(v["effective_default"], "mem");
    assert_eq!(store(&v, "mem")["default"], true);

    // New managed values land in the new default, in this process, with no restart.
    let owner = Uuid::now_v7();
    let r = h
        .state
        .vault
        .seal(&ctx(owner), Secret::new(b"two".to_vec()))
        .await
        .unwrap();
    assert_eq!(r.store_id(), Some("mem"));
    let opened = h.state.vault.open(&ctx(owner), &r).await.unwrap();
    assert_eq!(opened.expose().as_slice(), b"two");

    // A store that is not usable cannot become the default; the default cannot be removed.
    let g2 = v["generation"].as_i64().unwrap();
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g2, "store_id": "ghost" })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("store_not_usable"))
    );
    let (s, e) = call(
        &h,
        &a,
        Method::DELETE,
        &format!("/v1/admin/secret-stores/mem?generation={g2}"),
        None,
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("store_is_default"))
    );
}

#[tokio::test]
async fn a_failed_store_stays_listed_with_its_error_and_is_never_the_default() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    // A valid Vault definition (kubernetes auth builds without contacting Vault) whose CA is
    // not a certificate the client can load.
    let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault",
        Some(json!({ "generation": g, "store": {
            "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
            "auth": { "method": "kubernetes", "role": "aurora" }, "ca_pem": pem } })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let vs = store(&v, "vault");
    assert_eq!(vs["health"]["ok"], false);
    assert_eq!(vs["health"]["error_code"], "init_failed");
    assert!(
        vs["error"]
            .as_str()
            .is_some_and(|e| e.contains("secret store 'vault'")),
        "{vs}"
    );
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": v["generation"], "store_id": "vault" })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("store_not_usable"))
    );
    assert_eq!(v["effective_default"], "inline");
}

#[tokio::test]
async fn helm_set_keys_are_locked_and_every_write_is_audited_with_re_authentication() {
    let h = Harness::new()
        .await
        .with_helm(
            "[secrets]\ndefault_store = \"inline\"\n[[secrets.stores]]\nid = \"helmvault\"\nkind = \"memory\"\n",
        );
    let a = admin(&h, "root@example.com", true).await;
    let v = status(&h, &a).await;
    let hv = store(&v, "helmvault");
    assert_eq!(hv["source"], "helm");
    assert_eq!(hv["helm_key"], "vault.*");
    assert_eq!(v["default_store"]["source"], "helm");
    let g = v["generation"].as_i64().unwrap();

    // The default is Helm's: refused.
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g, "store_id": "helmvault" })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("setting_locked")),
        "{e}"
    );
    // A Helm store cannot be removed or have its scopes changed from the dashboard.
    let (s, e) = call(
        &h,
        &a,
        Method::DELETE,
        &format!("/v1/admin/secret-stores/helmvault?generation={g}"),
        None,
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("setting_locked"))
    );
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/helmvault/reference-scopes",
        Some(json!({ "generation": g, "scopes": [] })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("setting_locked"))
    );
    // A dashboard store reusing a Helm store's id: 422 entry_id_in_helm.
    let (s, e) = put_memory_store(&h, &a, "helmvault", g).await;
    assert_eq!(s, 422, "{e}");
    assert!(e.to_string().contains("entry_id_in_helm"), "{e}");

    // A dashboard store beside it works, and is audited with the change.
    let (s, v) = put_memory_store(&h, &a, "mem", g).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(store(&v, "mem")["source"], "dashboard");
    assert_eq!(store(&v, "helmvault")["source"], "helm");
    let rows: Vec<(Value,)> = sqlx::query_as(
        "SELECT metadata FROM audit_log WHERE action = 'instance_settings.update' \
         AND target_id = 'secrets'",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].0["change"].to_string().contains("mem"),
        "{}",
        rows[0].0
    );

    // No re-authentication within the window → 412 and nothing changes.
    sqlx::query("UPDATE sessions SET reauth_at = now() - interval '2 hours' WHERE id = $1")
        .bind(a.session)
        .execute(h.pool())
        .await
        .unwrap();
    let g2 = v["generation"].as_i64().unwrap();
    let (s, e) = call(
        &h,
        &a,
        Method::DELETE,
        &format!("/v1/admin/secret-stores/mem?generation={g2}"),
        None,
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (412, Some("reauth_required"))
    );
    assert_eq!(status(&h, &a).await["generation"], g2);
}

#[tokio::test]
async fn no_secret_material_reaches_any_response_or_audit_row() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let entry = json!({ "generation": g, "store": {
        "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
        "auth": { "method": "approle", "role_id": "role-1", "secret_id": { "$secret": CANARY } } } });
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault",
        Some(entry),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let vs = store(&v, "vault");
    // The sealed secret id is in the inline store and the store was built from it.
    assert_eq!(vs["settings"]["auth"]["secret_id"]["$secret"]["set"], true);
    assert_eq!(vs["error"], Value::Null, "{vs}");
    let (_, section) = call(&h, &a, Method::GET, "/v1/admin/settings/secrets", None).await;
    let audit: Vec<(Value,)> =
        sqlx::query_as("SELECT metadata FROM audit_log WHERE action LIKE 'instance_settings.%'")
            .fetch_all(h.pool())
            .await
            .unwrap();
    let stored: Value =
        sqlx::query_scalar("SELECT value FROM instance_settings WHERE section = 'secrets'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    for (name, text) in [
        ("status", v.to_string()),
        ("section", section.to_string()),
        ("audit", format!("{audit:?}")),
        (
            "mail",
            format!(
                "{:?}",
                h.mail
                    .sent()
                    .iter()
                    .map(|m| format!("{m:?}"))
                    .collect::<Vec<_>>()
            ),
        ),
    ] {
        assert!(!text.contains(CANARY), "{name} leaks the secret id");
    }
    // Stored sealed: the row holds a ciphertext, never the value.
    assert!(!stored.to_string().contains(CANARY));
    assert!(stored.to_string().contains("inline:v1:"));
    // Keeping it on an edit keeps the sealed value; the store still builds.
    let g1 = v["generation"].as_i64().unwrap();
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault",
        Some(json!({ "generation": g1, "store": {
            "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
            "managed_prefix": "other",
            "auth": { "method": "approle", "role_id": "role-1", "secret_id": { "$secret": "keep" } } } })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(store(&v, "vault")["error"], Value::Null);
    // A dashboard store may only be a Vault store.
    let g2 = v["generation"].as_i64().unwrap();
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/other",
        Some(json!({ "generation": g2, "store": { "kind": "inline" } })),
    )
    .await;
    assert_eq!(s, 422, "{e}");
    // Test connection with a sealed value kept: never echoes it.
    let (s, t) = call(
        &h,
        &a,
        Method::POST,
        "/v1/admin/secret-store-test",
        Some(json!({ "id": "vault", "store": {
            "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
            "auth": { "method": "approle", "role_id": "role-1", "secret_id": { "$secret": "keep" } } } })),
    )
    .await;
    assert_eq!(s, 200, "{t}");
    assert_eq!(t["ok"], false);
    assert!(!t.to_string().contains(CANARY));
    assert_eq!(t["steps"][0]["name"], "configuration");
    assert_eq!(t["steps"][0]["ok"], true);
}

#[tokio::test]
async fn health_is_probed_at_most_once_per_interval() {
    let h = Harness::new()
        .await
        .with_helm("[[secrets.stores]]\nid = \"mem\"\nkind = \"memory\"\n");
    let (vault, mem) = memory_vault();
    let state = h.state.clone().with_vault(vault);
    let mut h = h;
    h.state = state;
    h.app = test_router(h.state.clone());
    let a = admin(&h, "root@example.com", true).await;
    for _ in 0..5 {
        let v = status(&h, &a).await;
        assert_eq!(store(&v, "mem")["health"]["ok"], true);
    }
    assert_eq!(mem.health_calls(), 1, "five page loads, one probe");
}

#[tokio::test]
async fn deleting_a_store_that_still_holds_values_is_refused_with_the_count() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, v) = put_memory_store(&h, &a, "mem", g).await;
    assert_eq!(s, 200, "{v}");
    seed_secret(&h, "mem").await;
    let g1 = v["generation"].as_i64().unwrap();
    let (s, e) = call(
        &h,
        &a,
        Method::DELETE,
        &format!("/v1/admin/secret-stores/mem?generation={g1}"),
        None,
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("store_not_empty")),
        "{e}"
    );
    assert_eq!(e["error"]["details"]["managed_values"], 1);
    // The page counts it (after the cache interval is moot: a fresh process state).
    let st = status(&h, &a).await;
    assert_eq!(store(&st, "mem")["managed_values"], 1);
}

/// One managed `secrets` row whose value lives in `store`; returns its id.
async fn seed_secret(h: &Harness, store: &str) -> Uuid {
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let id = Uuid::now_v7();
    let c = SealContext {
        purpose: Purpose::SecretValue,
        owner_id: id,
        org_id: org,
        workspace_id: Some(ws),
    };
    let reg = h.state.vault.registry();
    let r = reg
        .get(store)
        .unwrap()
        .put(&c, &Secret::new(format!("value-{id}").into_bytes()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'k','generic','a.example.com','{}','managed',$4)")
        .bind(id)
        .bind(org)
        .bind(ws)
        .bind(r.to_column())
        .execute(h.pool())
        .await
        .unwrap();
    id
}

async fn ref_of(h: &Harness, id: Uuid) -> SecretRef {
    sqlx::query_scalar::<_, String>("SELECT value_ref FROM secrets WHERE id = $1")
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap()
        .parse()
        .unwrap()
}

async fn wait_state(h: &Harness, a: &Admin, want: &str) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (s, v) = call(
            h,
            a,
            Method::GET,
            "/v1/admin/secret-stores/migrations/current",
            None,
        )
        .await;
        assert_eq!(s, 200, "{v}");
        if v["state"] == want {
            return v;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "migration never reached {want}: {v}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn migration_dry_run_moves_every_managed_value_and_discards_the_old_ones() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, _) = put_memory_store(&h, &a, "mem", g).await;
    assert_eq!(s, 200);
    let ids = [
        seed_secret(&h, "inline").await,
        seed_secret(&h, "inline").await,
        seed_secret(&h, "inline").await,
    ];

    let (s, dry) = call(
        &h,
        &a,
        Method::POST,
        "/v1/admin/secret-stores/migrations",
        Some(json!({ "target": "mem", "dry_run": true })),
    )
    .await;
    assert_eq!(s, 200, "{dry}");
    assert_eq!(dry["state"], "dry_run");
    let sv = dry["per_purpose"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["purpose"] == "secret.value")
        .unwrap();
    assert_eq!(sv["moved"], 3);
    for id in ids {
        assert!(
            ref_of(&h, id).await.to_column().starts_with("inline:"),
            "dry run wrote"
        );
    }

    // Starting needs a recent re-authentication.
    let (s, e) = call(
        &h,
        &admin(&h, "other@example.com", false).await,
        Method::POST,
        "/v1/admin/secret-stores/migrations",
        Some(json!({ "target": "mem" })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (412, Some("reauth_required"))
    );

    let (s, job) = call(
        &h,
        &a,
        Method::POST,
        "/v1/admin/secret-stores/migrations",
        Some(json!({ "target": "mem" })),
    )
    .await;
    assert_eq!(s, 202, "{job}");
    let done = wait_state(&h, &a, "succeeded").await;
    let sv = done["per_purpose"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["purpose"] == "secret.value")
        .unwrap();
    assert_eq!(
        (
            sv["moved"].as_u64(),
            sv["failed"].as_u64(),
            sv["total"].as_u64()
        ),
        (Some(3), Some(0), Some(3))
    );
    for id in ids {
        let r = ref_of(&h, id).await;
        assert_eq!(r.store_id(), Some("mem"));
    }
    // Both ends of the job are in the audit log.
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action LIKE 'secret_stores.migrate_%' ORDER BY created_at",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(
        actions,
        [
            "secret_stores.migrate_start",
            "secret_stores.migrate_finish"
        ]
    );
    // The page's counts follow (the job cleared the cache).
    let st = status(&h, &a).await;
    assert_eq!(store(&st, "mem")["managed_values"], 3);
    assert_eq!(st["migration"]["state"], "succeeded");
}

#[tokio::test]
async fn a_running_migration_excludes_a_second_one_and_the_cli_run() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, _) = put_memory_store(&h, &a, "mem", g).await;
    assert_eq!(s, 200);
    seed_secret(&h, "inline").await;

    // Hold the migration lock the way a running job does.
    let mut tx = h.pool().begin().await.unwrap();
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(aurora_api::secrets_migrate::MIGRATION_LOCK_KEY)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(got);

    let (s, e) = call(
        &h,
        &a,
        Method::POST,
        "/v1/admin/secret-stores/migrations",
        Some(json!({ "target": "mem" })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("migration_running")),
        "{e}"
    );
    let err = run(h.pool(), &h.state.vault, &Options::new("mem"))
        .await
        .unwrap_err();
    assert!(
        err.contains("another secrets migration is running"),
        "{err}"
    );
    let (_, cur) = call(
        &h,
        &a,
        Method::GET,
        "/v1/admin/secret-stores/migrations/current",
        None,
    )
    .await;
    assert_eq!(cur["running_elsewhere"], true);

    // Released: the same run goes through, and it is resumable (a second run moves nothing).
    tx.rollback().await.unwrap();
    let mut o = Options::new("mem");
    o.discard_grace = Duration::ZERO;
    let s = run(h.pool(), &h.state.vault, &o).await.unwrap();
    assert_eq!(s.moved(), 1);
    let s = run(h.pool(), &h.state.vault, &o).await.unwrap();
    assert_eq!(s.moved(), 0);
}

#[tokio::test]
async fn instance_setting_secrets_stay_inline_when_values_move_to_another_store() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, _) = put_memory_store(&h, &a, "mem", g).await;
    assert_eq!(s, 200);
    let (vault, _) = (h.state.vault.clone(), ());
    let owner = Uuid::now_v7();
    let r = vault
        .seal_inline(&ctx(owner), Secret::new(b"keep-me-inline".to_vec()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO instance_settings (section, value, generation) VALUES ('settings_test', $1, 1)")
        .bind(json!({"settings_test.token": {"$secret": {"ref": r.to_column(), "owner": owner, "updated_at": "2026-11-15T00:00:00Z"}}}))
        .execute(h.pool())
        .await
        .unwrap();
    let mut o = Options::new("mem");
    o.keep_instance_settings_inline = true;
    o.discard_grace = Duration::ZERO;
    let s = run(h.pool(), &vault, &o).await.unwrap();
    assert!(!s.per_purpose.contains_key("instance_setting"));
    let col: String = sqlx::query_scalar(
        "SELECT value->'settings_test.token'->'$secret'->>'ref' FROM instance_settings WHERE section = 'settings_test'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert!(col.starts_with("inline:"));
}

#[tokio::test]
async fn reference_scopes_are_edited_per_organization_and_validated() {
    let h = harness().await;
    let a = admin(&h, "root@example.com", true).await;
    let org = h.org().await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault",
        Some(json!({ "generation": g, "store": {
            "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
            "auth": { "method": "kubernetes", "role": "aurora" } } })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let g1 = v["generation"].as_i64().unwrap();
    // A glob that reaches the managed prefix is refused (SEC-61).
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault/reference-scopes",
        Some(json!({ "generation": g1, "scopes": [{ "org_id": org, "paths": ["aurora/*"] }] })),
    )
    .await;
    assert_eq!(s, 422, "{e}");
    // An unknown organization is refused.
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault/reference-scopes",
        Some(json!({ "generation": g1, "scopes": [{ "org_id": Uuid::now_v7(), "paths": ["teams/*"] }] })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("org_not_found"))
    );
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault/reference-scopes",
        Some(json!({ "generation": g1, "scopes": [{ "org_id": org, "paths": ["teams/acme/*"] }] })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let scopes = store(&v, "vault")["reference_scopes"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0]["org_id"], json!(org.to_string()));
    assert_eq!(scopes[0]["paths"], json!(["teams/acme/*"]));
    assert!(scopes[0]["org_slug"].is_string());
}

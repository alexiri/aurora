#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-124 (D-195, D-236, C-746..C-748): Admin › Secret stores. Store configuration is the
//! `secrets` settings section (SET-1's service: locks, CAS, critical audit, re-authentication);
//! the `memory` store stands in for Vault here, the Vault code path is covered by
//! `aurora-secrets`' suites (`AURORA_TEST_VAULT=1`) and by the config-level tests below.

use crate::common::*;
use aurora_api::domains::auth::mail::{Mail, MailError, Mailer};
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

/// An instance admin's session. FIX-157 (D-317): it never re-authenticated.
async fn admin(h: &Harness, email: &str) -> Admin {
    let user = h.user(email, true).await;
    let (session, tok) = h.good_session(user).await;
    Admin {
        cookie: cookie(&tok),
        session,
    }
}

async fn call(h: &Harness, a: &Admin, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    h.call(&a.cookie, m, uri, body).await
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
    let a = admin(&h, "root@example.com").await;
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
    let a = admin(&h, "root@example.com").await;
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
    let a = admin(&h, "root@example.com").await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    // A valid Vault definition (approle auth builds without contacting Vault) whose CA is
    // not a certificate the client can load.
    let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault",
        Some(json!({ "generation": g, "store": {
            "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
            "auth": { "method": "approle", "role_id": "aurora", "secret_id": { "$secret": "s" } }, "ca_pem": pem } })),
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
    let a = admin(&h, "root@example.com").await;
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

    // FIX-157 (D-317): the admin session never re-authenticated, and the writes above worked.
    let reauth_stamped: bool =
        sqlx::query_scalar("SELECT reauth_at IS NOT NULL FROM sessions WHERE id = $1")
            .bind(a.session)
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert!(!reauth_stamped);
    // A secret-store write stays session-only: an API key is refused and nothing changes.
    let g2 = v["generation"].as_i64().unwrap();
    let org = h.org().await;
    let user: Uuid = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(a.session)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (_, key) = h.api_key(user, org, None, false).await;
    let (s, _, _) = h
        .send(
            Method::DELETE,
            &format!("/v1/admin/secret-stores/mem?generation={g2}"),
            &[
                ("authorization", &format!("Bearer {key}")),
                ("origin", ORIGIN),
            ],
            None,
        )
        .await;
    assert!(s == 401 || s == 403, "api key got {s}");
    assert_eq!(status(&h, &a).await["generation"], g2);
    // The session deletes it with no re-authentication.
    let (s, e) = call(
        &h,
        &a,
        Method::DELETE,
        &format!("/v1/admin/secret-stores/mem?generation={g2}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{e}");
    assert!(
        status(&h, &a).await["stores"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["id"] != "mem")
    );
}

#[tokio::test]
async fn no_secret_material_reaches_any_response_or_audit_row() {
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;
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
    let a = admin(&h, "root@example.com").await;
    for _ in 0..5 {
        let v = status(&h, &a).await;
        assert_eq!(store(&v, "mem")["health"]["ok"], true);
    }
    assert_eq!(mem.health_calls(), 1, "five page loads, one probe");
}

#[tokio::test]
async fn deleting_a_store_that_still_holds_values_is_refused_with_the_count() {
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;
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
    let a = admin(&h, "root@example.com").await;
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

    // Starting is session-only (an API key is refused, nothing starts); it needs no
    // re-authentication (FIX-157, D-317): `a` never re-authenticated and starts it below.
    let org = h.org().await;
    let user: Uuid = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(a.session)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (_, key) = h.api_key(user, org, None, false).await;
    let (s, _, _) = h
        .send(
            Method::POST,
            "/v1/admin/secret-stores/migrations",
            &[
                ("authorization", &format!("Bearer {key}")),
                ("origin", ORIGIN),
            ],
            Some(json!({ "target": "mem" })),
        )
        .await;
    assert!(s == 401 || s == 403, "api key got {s}");

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
    let a = admin(&h, "root@example.com").await;
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
    let a = admin(&h, "root@example.com").await;
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
    let a = admin(&h, "root@example.com").await;
    let org = h.org().await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/vault",
        Some(json!({ "generation": g, "store": {
            "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
            "auth": { "method": "approle", "role_id": "aurora", "secret_id": { "$secret": "s" } } } })),
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

// ---------------------------------------------------------------------- review fixes (SEC-89/90)

/// A listener that records every connection it gets: nothing may be sent to it.
struct Tripwire {
    port: u16,
    hits: Arc<std::sync::atomic::AtomicUsize>,
}

async fn tripwire() -> Tripwire {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n = hits.clone();
    tokio::spawn(async move {
        while l.accept().await.is_ok() {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });
    Tripwire { port, hits }
}

impl Tripwire {
    fn hits(&self) -> usize {
        self.hits.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn field_paths(e: &Value) -> Vec<String> {
    e["error"]["details"]["fields"]
        .as_array()
        .map(|f| {
            f.iter()
                .map(|x| x["path"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_dashboard_store_cannot_name_an_environment_variable_or_a_file() {
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let t = tripwire().await;
    let addr = format!("http://127.0.0.1:{}", t.port);
    for (auth, forbidden) in [
        (
            json!({ "method": "approle", "role_id": "r", "secret_id_env": "AURORA_DATABASE_URL" }),
            "secret_id_env",
        ),
        (
            json!({ "method": "approle", "role_id_env": "HOME", "secret_id": { "$secret": "x" } }),
            "role_id_env",
        ),
        (
            json!({ "method": "kubernetes", "role": "r", "jwt_path": "/etc/passwd" }),
            "jwt_path",
        ),
        (
            json!({ "method": "token", "token_env": "HOME" }),
            "token_env",
        ),
    ] {
        let (s, e) = call(
            &h,
            &a,
            Method::PUT,
            "/v1/admin/secret-stores/v",
            Some(json!({ "generation": g, "store": {
                "kind": "hashicorp-vault", "address": "https://vault.invalid:8200", "auth": auth } })),
        )
        .await;
        assert_eq!(s, 422, "{forbidden}: {e}");
        assert!(
            field_paths(&e)
                .iter()
                .any(|p| p.ends_with(&format!("auth/{forbidden}"))),
            "{forbidden}: {e}"
        );
        // Test connection refuses the same, without dialling.
        let (s, t2) = call(
            &h,
            &a,
            Method::POST,
            "/v1/admin/secret-store-test",
            Some(json!({ "id": "v", "store": {
                "kind": "hashicorp-vault", "address": addr, "auth": auth } })),
        )
        .await;
        assert_eq!(s, 200, "{t2}");
        assert_eq!(t2["ok"], false);
        assert!(
            t2["steps"][0]["error_code"]
                .as_str()
                .unwrap()
                .contains(forbidden),
            "{t2}"
        );
    }
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/v",
        Some(json!({ "generation": g, "store": {
            "kind": "hashicorp-vault", "address": "https://vault.invalid:8200",
            "ca_file": "/etc/ssl/cert.pem",
            "auth": { "method": "kubernetes", "role": "r" } } })),
    )
    .await;
    assert_eq!(s, 422, "{e}");
    assert!(
        field_paths(&e).iter().any(|p| p.ends_with("/ca_file")),
        "{e}"
    );
    assert_eq!(t.hits(), 0, "nothing was sent anywhere");
}

#[tokio::test]
async fn keeping_a_secret_while_the_destination_changes_needs_the_secret_again() {
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let store_body = |address: &str, role_id: &str, secret: Value| {
        json!({ "kind": "hashicorp-vault", "address": address,
            "auth": { "method": "approle", "role_id": role_id, "secret_id": secret } })
    };
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/v",
        Some(json!({ "generation": g, "store":
            store_body("https://vault.invalid:8200", "r1", json!({ "$secret": CANARY })) })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let g1 = v["generation"].as_i64().unwrap();
    let t = tripwire().await;
    let keep = json!({ "$secret": "keep" });
    for (address, role) in [
        (format!("http://127.0.0.1:{}", t.port), "r1"),
        ("https://other.invalid:8200".to_owned(), "r1"),
        ("https://vault.invalid:8200".to_owned(), "r2"),
    ] {
        let body = store_body(&address, role, keep.clone());
        let (s, e) = call(
            &h,
            &a,
            Method::PUT,
            "/v1/admin/secret-stores/v",
            Some(json!({ "generation": g1, "store": body.clone() })),
        )
        .await;
        assert_eq!(s, 422, "{address} {role}: {e}");
        // The loopback address is also refused by the destination guard, which runs first.
        let loopback = address.starts_with("http://127.");
        if !loopback {
            assert_eq!(e["error"]["code"], "secret_reentry_required", "{e}");
            assert!(
                e["error"]["details"]["fields"][0]["code"] == "secret_reentry_required",
                "{e}"
            );
        }
        let (s, r) = call(
            &h,
            &a,
            Method::POST,
            "/v1/admin/secret-store-test",
            Some(json!({ "id": "v", "store": body })),
        )
        .await;
        assert_eq!(s, 200, "{r}");
        assert_eq!(r["ok"], false);
        if !loopback {
            assert_eq!(
                r["steps"][0]["error_code"], "secret_reentry_required",
                "{r}"
            );
        }
    }
    assert_eq!(t.hits(), 0, "the kept secret was never sent anywhere");
    // Entering it again is allowed.
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/v",
        Some(json!({ "generation": g1, "store":
            store_body("https://other.invalid:8200", "r1", json!({ "$secret": "fresh" })) })),
    )
    .await;
    assert_eq!(s, 200, "{e}");
}

#[tokio::test]
async fn a_dashboard_vault_address_may_not_be_loopback_private_or_metadata() {
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let t = tripwire().await;
    for address in [
        "http://169.254.169.254".to_owned(),
        format!("http://127.0.0.1:{}", t.port),
        "https://10.1.2.3:8200".to_owned(),
        "https://[fd00::1]:8200".to_owned(),
    ] {
        let store = json!({ "kind": "hashicorp-vault", "address": address,
            "auth": { "method": "approle", "role_id": "r", "secret_id": { "$secret": "s" } } });
        let (s, e) = call(
            &h,
            &a,
            Method::PUT,
            "/v1/admin/secret-stores/v",
            Some(json!({ "generation": g, "store": store.clone() })),
        )
        .await;
        assert_eq!(s, 422, "{address}: {e}");
        assert!(
            e["error"]["details"]["fields"][0]["code"] == "destination_refused",
            "{address}: {e}"
        );
        let (s, r) = call(
            &h,
            &a,
            Method::POST,
            "/v1/admin/secret-store-test",
            Some(json!({ "id": "v", "store": store })),
        )
        .await;
        assert_eq!(s, 200, "{r}");
        assert_eq!(
            r["steps"][0]["error_code"], "destination_refused",
            "{address}: {r}"
        );
    }
    assert_eq!(t.hits(), 0);
}

#[tokio::test]
async fn a_dashboard_store_cannot_use_the_kubernetes_login() {
    // D-307: the login would send the pod's service-account token to the admin-chosen address.
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;
    let g = status(&h, &a).await["generation"].as_i64().unwrap();
    let t = tripwire().await;
    let body = |addr: &str| {
        json!({ "kind": "hashicorp-vault", "address": addr,
        "auth": { "method": "kubernetes", "role": "r" } })
    };
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-stores/k",
        Some(json!({ "generation": g, "store": body("https://vault.invalid:8200") })),
    )
    .await;
    assert_eq!(s, 422, "{e}");
    assert!(
        field_paths(&e).iter().any(|p| p.ends_with("auth/method")),
        "{e}"
    );
    let (s, r) = call(
        &h,
        &a,
        Method::POST,
        "/v1/admin/secret-store-test",
        Some(json!({ "id": "k", "store": body(&format!("http://127.0.0.1:{}", t.port)) })),
    )
    .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(
        r["steps"][0]["error_code"], "dashboard_reference_refused:auth.method",
        "{r}"
    );
    assert_eq!(t.hits(), 0);
}

/// A mailer that records every attempt and fails them all (the freeze suite's pattern).
struct RecordingFailMailer(std::sync::Mutex<Vec<String>>);

#[async_trait::async_trait]
impl Mailer for RecordingFailMailer {
    async fn send(&self, m: Mail) -> Result<(), MailError> {
        self.0.lock().unwrap().push(m.to);
        Err(MailError("smtp down".into()))
    }
}

/// (FIX-244, D-486) The default-secret-store change tells every organization's effective owners —
/// once each, not once per organization — never the actor (an instance administrator is never
/// named), never an ordinary member, never a disabled user. The body says who, when, and from
/// which store to which; no e-mail address and no secret appear in it. Adding a store without
/// making it the default tells nobody.
#[tokio::test]
async fn the_default_store_change_tells_the_org_owners_and_only_them() {
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;

    // Pia owns two organizations (one notice, not two), Olive owns the second; an ordinary
    // member and a disabled owner of the first get nothing.
    let pia = h.user("pia@x.io", false).await;
    let olive = h.user("olive@x.io", false).await;
    let member = h.user("mem@x.io", false).await;
    let sleepy = h.user("sleepy@x.io", false).await;
    let org1 = h.org().await;
    let org2 = h.org().await;
    h.member(org1, pia, "owner", "active").await;
    h.member(org1, member, "member", "active").await;
    h.member(org1, sleepy, "owner", "active").await;
    h.disable_user(sleepy).await;
    h.member(org2, pia, "owner", "active").await;
    h.member(org2, olive, "owner", "active").await;

    let g0 = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, v) = put_memory_store(&h, &a, "mem", g0).await;
    assert_eq!(s, 200, "{v}");
    let owner_mail = |h: &Harness| {
        h.mail
            .sent()
            .into_iter()
            .filter(|m| m.to == "pia@x.io" || m.to == "olive@x.io")
            .collect::<Vec<_>>()
    };
    assert!(
        owner_mail(&h).is_empty(),
        "adding a store without making it the default tells nobody"
    );

    let g1 = v["generation"].as_i64().unwrap();
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g1, "store_id": "mem" })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let mail = owner_mail(&h);
    let mut tos: Vec<&str> = mail.iter().map(|m| m.to.as_str()).collect();
    tos.sort_unstable();
    tos.dedup();
    assert_eq!(
        tos,
        ["olive@x.io", "pia@x.io"],
        "every organization's owner, once each, nobody else: {tos:?}"
    );
    assert!(
        !h.mail
            .sent()
            .iter()
            .any(|m| m.to == "mem@x.io" || m.to == "sleepy@x.io"),
        "an ordinary member and a disabled owner get nothing"
    );
    assert_eq!(
        mail.iter().filter(|m| m.to == "pia@x.io").count(),
        1,
        "an owner of two organizations gets one notice: {mail:?}"
    );
    let m = &mail[0];
    assert!(
        m.subject.contains("default secret store"),
        "the subject says what happened: {m:?}"
    );
    assert!(
        m.body
            .contains("an instance administrator changed the instance's default secret store")
            && m.body.contains("go to mem instead of the built-in store"),
        "who (the actor never named), from which store to which: {m:?}"
    );
    assert!(
        !m.body.contains('@'),
        "no e-mail address in the body: {m:?}"
    );

    // Only a real change tells: the same default again sends nothing.
    let g2 = v["generation"].as_i64().unwrap();
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g2, "store_id": "mem" })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(owner_mail(&h).len(), mail.len(), "no second notice");
}

/// (FIX-244, D-486) A refused default change rolls back and sends nothing: a stale generation
/// and an unknown store.
#[tokio::test]
async fn a_refused_default_change_sends_nothing() {
    let h = harness().await;
    let a = admin(&h, "root@example.com").await;
    let pia = h.user("pia@x.io", false).await;
    let org1 = h.org().await;
    h.member(org1, pia, "owner", "active").await;

    let g0 = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, v) = put_memory_store(&h, &a, "mem", g0).await;
    assert_eq!(s, 200, "{v}");
    let g1 = v["generation"].as_i64().unwrap();

    // A stale generation: the save is refused, the transaction rolls back.
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g1 + 5, "store_id": "mem" })),
    )
    .await;
    assert_eq!(s, 409, "{e}");
    // An unknown store: refused too.
    let (s, e) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g1, "store_id": "ghost" })),
    )
    .await;
    assert_ne!(s, 200, "{e}");
    assert!(
        !h.mail.sent().iter().any(|m| m.to == "pia@x.io"),
        "a rolled-back change sends nothing"
    );
}

/// (FIX-244, D-486) A failing recipient does not stop the others and never fails the save:
/// every recipient's send is attempted and the change commits.
#[tokio::test]
async fn a_failing_recipient_does_not_stop_the_others_and_the_save_still_commits() {
    let mut h = harness().await;
    let a = admin(&h, "root@example.com").await;
    let pia = h.user("pia@x.io", false).await;
    let olive = h.user("olive@x.io", false).await;
    let org1 = h.org().await;
    let org2 = h.org().await;
    h.member(org1, pia, "owner", "active").await;
    h.member(org2, olive, "owner", "active").await;

    let attempts = Arc::new(RecordingFailMailer(std::sync::Mutex::new(Vec::new())));
    h.state = h
        .state
        .clone()
        .with_mailer(Arc::clone(&attempts) as Arc<dyn Mailer>);
    h.app = test_router(h.state.clone());

    let g0 = status(&h, &a).await["generation"].as_i64().unwrap();
    let (s, v) = put_memory_store(&h, &a, "mem", g0).await;
    assert_eq!(s, 200, "{v}");
    let g1 = v["generation"].as_i64().unwrap();
    let (s, v) = call(
        &h,
        &a,
        Method::PUT,
        "/v1/admin/secret-store-default",
        Some(json!({ "generation": g1, "store_id": "mem" })),
    )
    .await;
    assert_eq!(s, 200, "{v}: the save stands, the failures are logged");
    // The change committed.
    assert_eq!(status(&h, &a).await["effective_default"], json!("mem"));
    // Every recipient was attempted (one failing recipient did not stop the others); the
    // actor is not among them (the admin notification of the settings change fails too and
    // is logged the same way).
    let mut tos = attempts.0.lock().unwrap().clone();
    tos.retain(|t| t == "pia@x.io" || t == "olive@x.io");
    tos.sort_unstable();
    tos.dedup();
    assert_eq!(tos, ["olive@x.io", "pia@x.io"], "{tos:?}");
}

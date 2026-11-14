#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `aurora-api secrets migrate` (C-375): inline → store → inline over every purpose, dry run,
//! filters, idempotence, the CAS loss, a failure between put and commit, a context mismatch,
//! and the live-traffic window. The Vault round trip runs against a dev server when
//! `AURORA_TEST_VAULT=1` (see `crates/aurora-secrets/tests/it/support.rs`).

use crate::common::*;
use async_trait::async_trait;
use aurora_api::secrets_migrate::{Options, Summary, run};
use aurora_config::{SecretStoreConfig, SecretsSection};
use aurora_core::Secret;
use aurora_secrets::{
    MemoryStore, Purpose, SealContext, SecretRef, SecretStore, SecretVault, StoreCapabilities,
    StoreHealth, StoreRegistry,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Where each purpose's ref lives: `(purpose, "SELECT <ref> FROM … WHERE id = $1")`.
const REF_SQL: [(Purpose, &str); 8] = [
    (
        Purpose::SecretValue,
        "SELECT value_ref FROM secrets WHERE id = $1",
    ),
    (
        Purpose::ConnectionCredentials,
        "SELECT credentials_ref FROM app_connections WHERE id = $1",
    ),
    (
        Purpose::OauthClientSecret,
        "SELECT secret_ref FROM app_oauth_clients WHERE id = $1",
    ),
    (
        Purpose::IntegrationCredentials,
        "SELECT credentials_ref FROM channel_integrations WHERE id = $1",
    ),
    (
        Purpose::PresenceCredentials,
        "SELECT credentials_ref FROM channel_presences WHERE id = $1",
    ),
    (
        Purpose::WebhookVerification,
        "SELECT verification->>'secret_ref' FROM webhooks WHERE id = $1",
    ),
    (
        Purpose::ChannelOpDeprovision,
        "SELECT payload->>'credentials_ref' FROM channel_ops WHERE id = $1",
    ),
    (
        Purpose::ConnectorSettings,
        "SELECT secret_refs->>'key' FROM connectors WHERE secret_owner = $1",
    ),
];

struct Seed {
    ctx: SealContext,
    value: String,
}

struct World {
    h: Harness,
    seeds: Vec<Seed>,
    vault: Arc<SecretVault>,
}

fn keyring() -> Arc<aurora_crypto::Keyring> {
    let k = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [b'k'; 32]);
    Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{k}"), "k1").unwrap())
}

fn store_cfg(v: serde_json::Value) -> SecretStoreConfig {
    serde_json::from_value(v).unwrap()
}

fn vault_with(stores: Vec<SecretStoreConfig>) -> SecretVault {
    let cfg = SecretsSection {
        stores,
        ..SecretsSection::default()
    };
    SecretVault::new(StoreRegistry::from_config(&cfg, keyring()).unwrap())
}

fn memory_cfg() -> SecretStoreConfig {
    store_cfg(json!({"id": "memory", "kind": "memory"}))
}

/// Seed one row per purpose, sealed inline under the right context.
async fn world(vault: SecretVault) -> World {
    let h = Harness::new().await;
    let vault = Arc::new(vault);
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('chat', $1, 'chat', '{}', 'active')")
        .bind(vec![7u8; 32]).execute(h.pool()).await.unwrap();
    let integration = Uuid::now_v7();
    sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,'chat','T','T','{}','x','active')")
        .bind(integration).bind(org).execute(h.pool()).await.unwrap();
    let inline = vault.registry().get("inline").unwrap().clone();

    let mut seeds = Vec::new();
    for p in Purpose::ALL {
        let id = if p == Purpose::IntegrationCredentials {
            integration
        } else {
            Uuid::now_v7()
        };
        let (ctx_ws, ctx_org) = match p {
            Purpose::SecretValue | Purpose::ConnectionCredentials => (Some(ws), org),
            Purpose::OauthClientSecret => (None, org),
            Purpose::WebhookVerification => (Some(ws), org),
            Purpose::ConnectorSettings => (None, Uuid::nil()),
            _ => (None, org),
        };
        let ctx = SealContext {
            purpose: p,
            owner_id: id,
            org_id: ctx_org,
            workspace_id: ctx_ws,
        };
        let value = format!("plain-{}-{id}", p.as_str());
        let r = inline
            .put(&ctx, &Secret::new(value.clone().into_bytes()))
            .await
            .unwrap()
            .to_column();
        let pool = h.pool();
        match p {
            Purpose::SecretValue => {
                sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'k','generic','a.example.com','{}','managed',$4)")
                    .bind(id).bind(org).bind(ws).bind(&r).execute(pool).await.unwrap();
            }
            Purpose::ConnectionCredentials => {
                sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'a','p','1','l','active',$4)")
                    .bind(id).bind(org).bind(ws).bind(&r).execute(pool).await.unwrap();
            }
            Purpose::OauthClientSecret => {
                sqlx::query("INSERT INTO app_oauth_clients (id, org_id, package_id, client_key, fields, secret_ref) VALUES ($1,$2,'p','c','{}',$3)")
                    .bind(id).bind(org).bind(&r).execute(pool).await.unwrap();
            }
            Purpose::IntegrationCredentials => {
                sqlx::query("UPDATE channel_integrations SET credentials_ref = $2 WHERE id = $1")
                    .bind(id)
                    .bind(&r)
                    .execute(pool)
                    .await
                    .unwrap();
            }
            Purpose::PresenceCredentials => {
                sqlx::query("INSERT INTO channel_presences (id, agent_id, integration_id, mode, credentials_ref, status) VALUES ($1,$2,$3,'dedicated',$4,'active')")
                    .bind(id).bind(agent).bind(integration).bind(&r).execute(pool).await.unwrap();
            }
            Purpose::ChannelOpDeprovision => {
                sqlx::query("INSERT INTO channel_ops (id, connector_id, kind, payload, status) VALUES ($1,'chat','presence.deprovision',$2,'pending')")
                    .bind(id)
                    .bind(json!({"credentials_ref": r, "org_id": org}))
                    .execute(pool).await.unwrap();
            }
            Purpose::ConnectorSettings => {
                sqlx::query("INSERT INTO connectors (id, display_name, status, managed, type_id, settings, secret_owner, secret_refs) VALUES ('managed-one','m','active',true,'managed-one','{}',$1,$2)")
                    .bind(id)
                    .bind(json!({"key": r}))
                    .execute(pool).await.unwrap();
            }
            Purpose::WebhookVerification => {
                sqlx::query("INSERT INTO webhooks (id, agent_id, name, instructions, token_hash, display_prefix, verification) VALUES ($1,$2,'w','i',$3,'awh_',$4)")
                    .bind(id).bind(agent).bind(vec![9u8; 32])
                    .bind(json!({"kind": "hmac", "header": "x-sig", "secret_ref": r}))
                    .execute(pool).await.unwrap();
            }
        }
        seeds.push(Seed { ctx, value });
    }
    World { h, seeds, vault }
}

impl World {
    async fn col(&self, s: &Seed) -> String {
        let sql = REF_SQL.iter().find(|(p, _)| *p == s.ctx.purpose).unwrap().1;
        sqlx::query_scalar::<_, String>(sql)
            .bind(s.ctx.owner_id)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// Every row's ref is in `store`, parses, and opens to the seeded value under its context.
    async fn assert_all_in(&self, store: &str) -> Vec<String> {
        let mut cols = Vec::new();
        for s in &self.seeds {
            let col = self.col(s).await;
            let r: SecretRef = col.parse().unwrap();
            assert_eq!(
                r.store_id().unwrap_or("inline"),
                store,
                "{:?}",
                s.ctx.purpose
            );
            let v = self.vault.open(&s.ctx, &r).await.unwrap();
            assert_eq!(
                v.expose().as_slice(),
                s.value.as_bytes(),
                "{:?}",
                s.ctx.purpose
            );
            cols.push(col);
        }
        cols
    }

    fn quick(to: &str) -> Options {
        let mut o = Options::new(to);
        o.discard_grace = Duration::ZERO;
        o
    }
}

fn counts(s: &Summary, p: Purpose) -> (u64, u64, u64, u64) {
    let c = s.per_purpose[p.as_str()];
    (c.moved, c.skipped, c.failed, c.already)
}

/// inline → `store` → inline, checking values, refs and the store's contents at each step.
async fn round_trip(w: &World, store: &str) {
    let before = w.assert_all_in("inline").await;

    // Dry run: reports, writes nothing (not even to the store).
    let mut o = World::quick(store);
    o.dry_run = true;
    let s = run(w.h.pool(), &w.vault, &o).await.unwrap();
    for p in Purpose::ALL {
        assert_eq!(counts(&s, p), (1, 0, 0, 0));
    }
    assert_eq!(
        w.assert_all_in("inline").await,
        before,
        "dry run changed a ref"
    );

    // Purpose filter: only that purpose moves.
    let mut o = World::quick(store);
    o.purpose = Some(Purpose::SecretValue);
    let s = run(w.h.pool(), &w.vault, &o).await.unwrap();
    assert_eq!(s.per_purpose.len(), 1);
    assert_eq!(counts(&s, Purpose::SecretValue), (1, 0, 0, 0));
    let sv = &w.seeds[0];
    assert_eq!(
        w.col(sv).await.parse::<SecretRef>().unwrap().store_id(),
        Some(store)
    );
    for other in &w.seeds[1..] {
        assert!(w.col(other).await.starts_with("inline:"));
    }

    // The rest → store; values and refs verified; the gateway-facing open works.
    let s = run(w.h.pool(), &w.vault, &World::quick(store))
        .await
        .unwrap();
    assert_eq!(counts(&s, Purpose::SecretValue), (0, 0, 0, 1));
    for p in &Purpose::ALL[1..] {
        assert_eq!(counts(&s, *p), (1, 0, 0, 0));
    }
    assert_eq!(s.failed(), 0);
    let in_store = w.assert_all_in(store).await;

    // Idempotent: a second run moves nothing and writes nothing new.
    let s = run(w.h.pool(), &w.vault, &World::quick(store))
        .await
        .unwrap();
    assert_eq!(s.moved(), 0);
    assert_eq!(w.assert_all_in(store).await, in_store);

    // Back to inline.
    let s = run(w.h.pool(), &w.vault, &World::quick("inline"))
        .await
        .unwrap();
    for p in Purpose::ALL {
        assert_eq!(counts(&s, p), (1, 0, 0, 0));
    }
    w.assert_all_in("inline").await;
    let s = run(w.h.pool(), &w.vault, &World::quick("inline"))
        .await
        .unwrap();
    assert_eq!(s.moved(), 0);

    // Nothing left behind: every value written to the store was discarded.
    for (s, col) in w.seeds.iter().zip(&in_store) {
        let r: SecretRef = col.parse().unwrap();
        assert!(
            w.vault.open(&s.ctx, &r).await.is_err(),
            "{:?}: old value still readable",
            s.ctx.purpose
        );
    }
}

#[tokio::test]
async fn round_trip_through_the_memory_store() {
    let mem = Arc::new(MemoryStore::new("memory"));
    let mut sr = StoreRegistry::from_config(&SecretsSection::default(), keyring()).unwrap();
    sr.register(mem.clone());
    let w = world(SecretVault::new(sr)).await;
    round_trip(&w, "memory").await;
    assert_eq!(
        mem.managed_count(),
        0,
        "memory store not empty after the way back"
    );
}

/// (P4-MCON) A managed connector seals several fields under one owner; paging one row at a time
/// must still move every field of every connector exactly once.
#[tokio::test]
async fn connector_settings_with_several_fields_move_whole_with_small_pages() {
    let mem = Arc::new(MemoryStore::new("memory"));
    let mut sr = StoreRegistry::from_config(&SecretsSection::default(), keyring()).unwrap();
    sr.register(mem.clone());
    let vault = SecretVault::new(sr);
    let h = Harness::new().await;
    let inline = vault.registry().get("inline").unwrap().clone();
    let mut expect = Vec::new();
    for c in ["mc-a", "mc-b"] {
        let owner = Uuid::now_v7();
        let ctx = SealContext {
            purpose: Purpose::ConnectorSettings,
            owner_id: owner,
            org_id: Uuid::nil(),
            workspace_id: None,
        };
        let mut refs = serde_json::Map::new();
        for f in ["k1", "k2", "k3"] {
            let v = format!("{c}-{f}-value");
            let r = inline
                .put(&ctx, &Secret::new(v.clone().into_bytes()))
                .await
                .unwrap();
            refs.insert(f.to_owned(), json!(r.to_column()));
            expect.push((c, f, v, ctx));
        }
        sqlx::query("INSERT INTO connectors (id, display_name, status, managed, type_id, settings, secret_owner, secret_refs) VALUES ($1,$1,'active',true,$1,'{}',$2,$3)")
            .bind(c)
            .bind(owner)
            .bind(serde_json::Value::Object(refs))
            .execute(h.pool())
            .await
            .unwrap();
    }
    let mut o = World::quick("memory");
    o.purpose = Some(Purpose::ConnectorSettings);
    o.batch = 2;
    let s = run(h.pool(), &vault, &o).await.unwrap();
    assert_eq!(counts(&s, Purpose::ConnectorSettings), (6, 0, 0, 0));
    for (c, f, v, ctx) in expect {
        let col: String = sqlx::query_scalar("SELECT secret_refs->>$2 FROM connectors WHERE id=$1")
            .bind(c)
            .bind(f)
            .fetch_one(h.pool())
            .await
            .unwrap();
        let r: SecretRef = col.parse().unwrap();
        assert_eq!(r.store_id(), Some("memory"), "{c}/{f}");
        assert_eq!(
            vault.open(&ctx, &r).await.unwrap().expose().as_slice(),
            v.as_bytes()
        );
    }
    let s = run(h.pool(), &vault, &o).await.unwrap();
    assert_eq!(
        counts(&s, Purpose::ConnectorSettings),
        (0, 0, 0, 6),
        "idempotent"
    );
}

#[tokio::test]
async fn round_trip_through_vault() {
    let Some(v) = vault_env() else { return };
    let unique = format!("mig-{}", Uuid::now_v7().simple());
    let w = world(vault_with(vec![store_cfg(vault_json(&v, &unique))])).await;
    round_trip(&w, "vault").await;
}

/// A different id than the memory test's so both can run in one process.
struct VaultEnv {
    addr: String,
}

fn vault_env() -> Option<VaultEnv> {
    // (FIX-140, D-198) unset: SKIPPED outside CI, a panic under CI=true.
    if !aurora_testkit::require_or_skip("AURORA_TEST_VAULT") {
        return None;
    }
    std::env::var("AURORA_TEST_VAULT_TOKEN").expect("AURORA_TEST_VAULT_TOKEN must be set");
    Some(VaultEnv {
        addr: std::env::var("AURORA_TEST_VAULT_ADDR")
            .unwrap_or_else(|_| "http://127.0.0.1:18200".into()),
    })
}

fn vault_json(v: &VaultEnv, unique: &str) -> serde_json::Value {
    json!({
        "id": "vault", "kind": "hashicorp-vault", "address": v.addr,
        "managed_prefix": format!("{unique}-managed"),
        "auth": {"method": "token", "token_env": "AURORA_TEST_VAULT_TOKEN"},
    })
}

/// A Vault outage at startup must not stop the process: the store is built without contacting
/// Vault, answers `StoreUnavailable` (503 `secret_store_unavailable`) and reports unhealthy;
/// inline values keep working. (`127.0.0.1:1` refuses connections.)
#[tokio::test]
async fn vault_outage_at_startup_does_not_prevent_building_the_registry() {
    let vault = vault_with(vec![store_cfg(json!({
        "id": "vault", "kind": "hashicorp-vault", "address": "http://127.0.0.1:1",
        // Any always-set variable: only its presence is checked at startup.
        "auth": {"method": "token", "token_env": "PATH"},
    }))]);
    let store = vault.registry().get("vault").unwrap();
    assert_eq!(store.health().await, StoreHealth::Unavailable);
    let ctx = SealContext {
        purpose: Purpose::SecretValue,
        owner_id: Uuid::now_v7(),
        org_id: Uuid::now_v7(),
        workspace_id: None,
    };
    let path = format!("aurora/{}/secret.value/{}", ctx.org_id, ctx.owner_id);
    let r = SecretRef::ext("vault", &path, None, "1").unwrap();
    assert_eq!(
        vault.open(&ctx, &r).await.unwrap_err(),
        aurora_secrets::SecretError::StoreUnavailable
    );
    // Inline values are unaffected.
    let inline = vault.registry().get("inline").unwrap();
    let r = inline.put(&ctx, &Secret::new(b"v".to_vec())).await.unwrap();
    assert_eq!(
        vault.open(&ctx, &r).await.unwrap().expose().as_slice(),
        b"v"
    );
}

#[tokio::test]
async fn refuses_unknown_and_non_managed_targets() {
    let w = world(vault_with(vec![memory_cfg()])).await;
    assert!(
        run(w.h.pool(), &w.vault, &Options::new("nope"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn context_mismatch_fails_that_row_only() {
    let w = world(vault_with(vec![memory_cfg()])).await;
    // Re-seal the oauth client's value under another owner: opening it under the row's own
    // context must fail the AAD check.
    let inline = w.vault.registry().get("inline").unwrap().clone();
    let bad = inline
        .put(
            &SealContext {
                owner_id: Uuid::now_v7(),
                ..w.seeds[2].ctx
            },
            &Secret::new(b"x".to_vec()),
        )
        .await
        .unwrap()
        .to_column();
    sqlx::query("UPDATE app_oauth_clients SET secret_ref = $2 WHERE id = $1")
        .bind(w.seeds[2].ctx.owner_id)
        .bind(&bad)
        .execute(w.h.pool())
        .await
        .unwrap();
    let s = run(w.h.pool(), &w.vault, &World::quick("memory"))
        .await
        .unwrap();
    assert_eq!(counts(&s, Purpose::OauthClientSecret), (0, 0, 1, 0));
    assert_eq!(s.failed(), 1);
    assert_eq!(s.moved(), 7);
    assert_eq!(
        w.col(&w.seeds[2]).await,
        bad,
        "failed row keeps its old ref"
    );
    let out = s.render(false);
    assert!(!out.contains("plain-") && !out.contains(&bad));
}

/// Fails between `put` and the commit (a trigger rejects the UPDATE): the old ref stays valid,
/// the new value is cleaned up, and a later run completes.
#[tokio::test]
async fn failure_between_put_and_commit_keeps_the_old_ref() {
    let mem = Arc::new(MemoryStore::new("memory"));
    let mut sr = StoreRegistry::from_config(&SecretsSection::default(), keyring()).unwrap();
    sr.register(mem.clone());
    let w = world(SecretVault::new(sr)).await;
    let pool = w.h.pool();
    sqlx::query("CREATE FUNCTION p3_boom() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'boom'; END $$")
        .execute(pool).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER p3_boom BEFORE UPDATE ON secrets FOR EACH ROW EXECUTE FUNCTION p3_boom()",
    )
    .execute(pool)
    .await
    .unwrap();
    let old = w.col(&w.seeds[0]).await;
    let mut o = World::quick("memory");
    o.purpose = Some(Purpose::SecretValue);
    let s = run(pool, &w.vault, &o).await.unwrap();
    assert_eq!(counts(&s, Purpose::SecretValue), (0, 0, 1, 0));
    assert_eq!(
        w.col(&w.seeds[0]).await,
        old,
        "ref changed although the commit failed"
    );
    assert_eq!(
        mem.managed_count(),
        0,
        "the uncommitted new value was left behind"
    );
    let v = w
        .vault
        .open(&w.seeds[0].ctx, &old.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(v.expose().as_slice(), w.seeds[0].value.as_bytes());
    sqlx::query("DROP TRIGGER p3_boom ON secrets")
        .execute(pool)
        .await
        .unwrap();
    let s = run(pool, &w.vault, &o).await.unwrap();
    assert_eq!(counts(&s, Purpose::SecretValue), (1, 0, 0, 0));
}

/// A store that, right after a `put`, changes the row like a concurrent writer would.
struct Racing {
    inner: Arc<MemoryStore>,
    inline: Arc<dyn SecretStore>,
    pool: sqlx::PgPool,
}

#[async_trait]
impl SecretStore for Racing {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn put(
        &self,
        ctx: &SealContext,
        plaintext: &Secret<Vec<u8>>,
    ) -> aurora_secrets::Result<SecretRef> {
        let r = self.inner.put(ctx, plaintext).await?;
        // The concurrent writer replaces the value with a fresh inline one.
        let theirs = self
            .inline
            .put(ctx, &Secret::new(b"concurrent".to_vec()))
            .await?;
        sqlx::query("UPDATE secrets SET value_ref = $2 WHERE id = $1")
            .bind(ctx.owner_id)
            .bind(theirs.to_column())
            .execute(&self.pool)
            .await
            .unwrap();
        Ok(r)
    }
    async fn get(
        &self,
        ctx: &SealContext,
        r: &SecretRef,
    ) -> aurora_secrets::Result<Secret<Vec<u8>>> {
        self.inner.get(ctx, r).await
    }
    async fn delete(&self, ctx: &SealContext, r: &SecretRef) -> aurora_secrets::Result<()> {
        self.inner.delete(ctx, r).await
    }
    async fn resolve_reference(
        &self,
        ctx: &SealContext,
        path: &str,
        field: Option<&str>,
    ) -> aurora_secrets::Result<SecretRef> {
        self.inner.resolve_reference(ctx, path, field).await
    }
    async fn health(&self) -> StoreHealth {
        self.inner.health().await
    }
}

#[tokio::test]
async fn cas_loss_skips_and_discards_the_new_value() {
    let mem = Arc::new(MemoryStore::new("memory"));
    let base = vault_with(vec![]);
    let inline = base.registry().get("inline").unwrap().clone();
    let w = world(base).await;
    let mut sr = StoreRegistry::from_config(&SecretsSection::default(), keyring()).unwrap();
    sr.register(Arc::new(Racing {
        inner: mem.clone(),
        inline,
        pool: w.h.pool().clone(),
    }));
    let vault = SecretVault::new(sr);
    let mut o = World::quick("memory");
    o.purpose = Some(Purpose::SecretValue);
    let s = run(w.h.pool(), &vault, &o).await.unwrap();
    assert_eq!(counts(&s, Purpose::SecretValue), (0, 1, 0, 0));
    assert_eq!(
        mem.managed_count(),
        0,
        "the losing new value must be discarded"
    );
    // The concurrent writer's value is what the row holds; nothing was overwritten.
    let col = w.col(&w.seeds[0]).await;
    let v = vault
        .open(&w.seeds[0].ctx, &col.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(v.expose().as_slice(), b"concurrent");
}

/// Live traffic: after the commit the new ref resolves, and the old ref stays readable until
/// the discard (which waits for the grace period).
#[tokio::test]
async fn old_ref_stays_readable_until_discard() {
    let mem = Arc::new(MemoryStore::new("memory"));
    let mut sr = StoreRegistry::from_config(&SecretsSection::default(), keyring()).unwrap();
    sr.register(mem.clone());
    let w = world(SecretVault::new(sr)).await;
    run(w.h.pool(), &w.vault, &World::quick("memory"))
        .await
        .unwrap();
    let old = w.col(&w.seeds[0]).await; // memory ref

    let pool = w.h.pool().clone();
    let vault = w.vault.clone();
    let mut o = Options::new("inline");
    o.discard_grace = Duration::from_millis(1500);
    let job = tokio::spawn(async move { run(&pool, &vault, &o).await.unwrap() });
    // Wait for the commit (the row now points at inline), then read both refs.
    let mut now = old.clone();
    for _ in 0..100 {
        now = w.col(&w.seeds[0]).await;
        if now != old {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(now, old, "the row was never swapped");
    let ctx = &w.seeds[0].ctx;
    let new_v = w.vault.open(ctx, &now.parse().unwrap()).await.unwrap();
    assert_eq!(new_v.expose().as_slice(), w.seeds[0].value.as_bytes());
    let old_v = w.vault.open(ctx, &old.parse().unwrap()).await.unwrap();
    assert_eq!(old_v.expose().as_slice(), w.seeds[0].value.as_bytes());
    let s = job.await.unwrap();
    assert_eq!(s.failed(), 0);
    assert!(w.vault.open(ctx, &old.parse().unwrap()).await.is_err());
}

fn ring_vault(keys: &[(&str, u8)], active: &str) -> SecretVault {
    let list = keys
        .iter()
        .map(|(id, b)| {
            let k = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [*b; 32]);
            format!("{id}:{k}")
        })
        .collect::<Vec<_>>()
        .join(",");
    let kr = Arc::new(aurora_crypto::Keyring::parse(&list, active).unwrap());
    SecretVault::new(StoreRegistry::from_config(&SecretsSection::default(), kr).unwrap())
}

fn key_of(col: &str) -> &str {
    col.strip_prefix("inline:v1:")
        .unwrap()
        .split(':')
        .next()
        .unwrap()
}

/// `secrets rekey`: values sealed under key A, the active key rotated to B, rekey re-seals every
/// value under B, a second run moves nothing, and A can leave the ring afterwards.
#[tokio::test]
async fn rekey_reseals_under_the_active_key() {
    let w = world(ring_vault(&[("k1", b'k')], "k1")).await;
    for s in &w.seeds {
        assert_eq!(key_of(&w.col(s).await), "k1");
    }
    let rotated = ring_vault(&[("k1", b'k'), ("k2", b'm')], "k2");
    let pool = w.h.pool();
    let mut o = Options::rekey("k2");
    o.discard_grace = Duration::ZERO;

    // Dry run changes nothing.
    o.dry_run = true;
    let s = run(pool, &rotated, &o).await.unwrap();
    for p in Purpose::ALL {
        assert_eq!(counts(&s, p), (1, 0, 0, 0));
    }
    for s in &w.seeds {
        assert_eq!(key_of(&w.col(s).await), "k1");
    }
    o.dry_run = false;

    // Filter: one purpose only.
    o.purpose = Some(Purpose::SecretValue);
    let s = run(pool, &rotated, &o).await.unwrap();
    assert_eq!(counts(&s, Purpose::SecretValue), (1, 0, 0, 0));
    assert_eq!(s.per_purpose.len(), 1);
    assert_eq!(key_of(&w.col(&w.seeds[1]).await), "k1");
    o.purpose = None;

    let s = run(pool, &rotated, &o).await.unwrap();
    assert_eq!(s.failed(), 0);
    assert_eq!(counts(&s, Purpose::SecretValue), (0, 0, 0, 1));
    for p in &Purpose::ALL[1..] {
        assert_eq!(counts(&s, *p), (1, 0, 0, 0));
    }
    let mut cols = Vec::new();
    for sd in &w.seeds {
        let col = w.col(sd).await;
        assert_eq!(key_of(&col), "k2");
        let v = rotated.open(&sd.ctx, &col.parse().unwrap()).await.unwrap();
        assert_eq!(v.expose().as_slice(), sd.value.as_bytes());
        cols.push(col);
    }
    // Idempotent.
    let s = run(pool, &rotated, &o).await.unwrap();
    assert_eq!(s.moved(), 0);
    for (sd, col) in w.seeds.iter().zip(&cols) {
        assert_eq!(&w.col(sd).await, col);
    }
    // The old key can leave the ring.
    let only_new = ring_vault(&[("k2", b'm')], "k2");
    for sd in &w.seeds {
        let v = only_new
            .open(&sd.ctx, &w.col(sd).await.parse().unwrap())
            .await
            .unwrap();
        assert_eq!(v.expose().as_slice(), sd.value.as_bytes());
    }
}

#[tokio::test]
async fn rekey_context_failure_fails_only_that_row_and_leaves_other_stores_alone() {
    let w = world(ring_vault(&[("k1", b'k')], "k1")).await;
    let rotated = ring_vault(&[("k1", b'k'), ("k2", b'm')], "k2");
    // A value sealed under another owner's context: it cannot be opened for its own row.
    let inline = w.vault.registry().get("inline").unwrap().clone();
    let bad = inline
        .put(
            &SealContext {
                owner_id: Uuid::now_v7(),
                ..w.seeds[2].ctx
            },
            &Secret::new(b"x".to_vec()),
        )
        .await
        .unwrap()
        .to_column();
    sqlx::query("UPDATE app_oauth_clients SET secret_ref = $2 WHERE id = $1")
        .bind(w.seeds[2].ctx.owner_id)
        .bind(&bad)
        .execute(w.h.pool())
        .await
        .unwrap();
    let mut o = Options::rekey("k2");
    o.discard_grace = Duration::ZERO;
    let s = run(w.h.pool(), &rotated, &o).await.unwrap();
    assert_eq!(counts(&s, Purpose::OauthClientSecret), (0, 0, 1, 0));
    assert_eq!(s.failed(), 1);
    assert_eq!(s.moved(), 7);
    assert_eq!(
        w.col(&w.seeds[2]).await,
        bad,
        "failed row keeps its old value"
    );
    let out = s.render(false);
    assert!(!out.contains("plain-") && !out.contains(&bad));
}

/// Two migrators over the same rows (an operator error, but it must stay safe): every row ends
/// up with exactly one valid ref in the target, the loser of each row's CAS counts a skip and
/// discards its own new value, and no value is lost or mixed.
async fn race(w: &World, store: &str) {
    let (a, b) = {
        let (p1, v1, p2, v2) = (
            w.h.pool().clone(),
            w.vault.clone(),
            w.h.pool().clone(),
            w.vault.clone(),
        );
        let (o1, o2) = (World::quick(store), World::quick(store));
        (
            tokio::spawn(async move { run(&p1, &v1, &o1).await.unwrap() }),
            tokio::spawn(async move { run(&p2, &v2, &o2).await.unwrap() }),
        )
    };
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    assert_eq!(a.failed() + b.failed(), 0);
    for p in Purpose::ALL {
        let (am, ask, _, aal) = counts(&a, p);
        let (bm, bsk, _, bal) = counts(&b, p);
        assert_eq!(am + bm, 1, "{p:?}: exactly one migrator wins the row");
        assert_eq!(
            ask + bsk + aal + bal,
            1,
            "{p:?}: the other skips or finds it moved"
        );
    }
    w.assert_all_in(store).await;
}

#[tokio::test]
async fn racing_migrators_leave_one_valid_ref_per_row_memory() {
    let mem = Arc::new(MemoryStore::new("memory"));
    let mut sr = StoreRegistry::from_config(&SecretsSection::default(), keyring()).unwrap();
    sr.register(mem.clone());
    let w = world(SecretVault::new(sr)).await;
    race(&w, "memory").await;
    assert_eq!(mem.managed_count(), 8, "a losing migrator leaked a value");
}

#[tokio::test]
async fn racing_migrators_leave_one_valid_ref_per_row_vault() {
    let Some(v) = vault_env() else { return };
    let unique = format!("race-{}", Uuid::now_v7().simple());
    let w = world(vault_with(vec![store_cfg(vault_json(&v, &unique))])).await;
    race(&w, "vault").await;
}

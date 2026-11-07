#![cfg(feature = "memory-store")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use aurora_config::{SecretStoreConfig, SecretsSection};
use aurora_core::Secret;
use aurora_crypto::Keyring;
use aurora_secrets::*;
use base64::Engine;
use uuid::Uuid;

fn key(b: u8) -> String {
    base64::engine::general_purpose::STANDARD.encode([b; 32])
}

fn keyring(active: &str) -> Arc<Keyring> {
    Arc::new(Keyring::parse(&format!("k1:{},k2:{}", key(1), key(2)), active).unwrap())
}

fn ctx() -> SealContext {
    SealContext {
        purpose: Purpose::SecretValue,
        owner_id: Uuid::now_v7(),
        org_id: Uuid::now_v7(),
        workspace_id: None,
    }
}

fn cfg(default: &str, stores: &[(&str, &str)]) -> SecretsSection {
    SecretsSection {
        default_store: default.into(),
        stores: stores
            .iter()
            .map(|(id, kind)| SecretStoreConfig {
                id: (*id).into(),
                kind: (*kind).into(),
                ..Default::default()
            })
            .collect(),
    }
}

fn vault() -> SecretVault {
    SecretVault::new(StoreRegistry::from_config(&cfg("inline", &[]), keyring("k1")).unwrap())
}

fn pt(s: &str) -> Secret<Vec<u8>> {
    Secret::new(s.as_bytes().to_vec())
}

#[tokio::test]
async fn inline_seal_open_and_mismatch() {
    let v = vault();
    let c = ctx();
    let r = v.seal(&c, pt("hunter2")).await.unwrap();
    assert!(r.to_column().starts_with("inline:v1:k1:"));
    assert_eq!(v.open(&c, &r).await.unwrap().expose(), b"hunter2");
    // Round trip through the column string.
    let r2: SecretRef = r.to_column().parse().unwrap();
    assert_eq!(v.open(&c, &r2).await.unwrap().expose(), b"hunter2");

    let mut o = c;
    o.owner_id = Uuid::now_v7();
    assert_eq!(
        v.open(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    let mut o = c;
    o.org_id = Uuid::now_v7();
    assert_eq!(
        v.open(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    let mut o = c;
    o.purpose = Purpose::WebhookVerification;
    assert_eq!(
        v.open(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
}

#[tokio::test]
async fn key_rotation_and_reseal() {
    let c = ctx();
    let old = vault().seal(&c, pt("v")).await.unwrap();
    let v2 =
        SecretVault::new(StoreRegistry::from_config(&cfg("inline", &[]), keyring("k2")).unwrap());
    assert_eq!(v2.open(&c, &old).await.unwrap().expose(), b"v");
    let new = v2.reseal(&c, &old, "inline").await.unwrap();
    assert!(new.to_column().starts_with("inline:v1:k2:"));
    assert_eq!(v2.open(&c, &new).await.unwrap().expose(), b"v");
}

fn mem_vault() -> (SecretVault, Arc<MemoryStore>) {
    let mem = Arc::new(MemoryStore::new("mem"));
    let mut reg = StoreRegistry::from_config(&cfg("inline", &[]), keyring("k1")).unwrap();
    reg.register(mem.clone());
    (SecretVault::new(reg), mem)
}

#[tokio::test]
async fn memory_reference_mode() {
    let (v, mem) = mem_vault();
    let c = ctx();
    mem.preload("kv/team/openai", Some("key"), b"sk-ref");
    mem.preload("kv/other/x", None, b"nope");
    mem.allow_reference(c.org_id, "kv/team/*");

    let r = v
        .check_reference(&c, "mem", "kv/team/openai", Some("key"))
        .await
        .unwrap();
    assert_eq!(v.open(&c, &r).await.unwrap().expose(), b"sk-ref");
    assert_eq!(
        v.check_reference(&c, "mem", "kv/other/x", None)
            .await
            .unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
    assert_eq!(
        v.check_reference(&c, "mem", "kv/team/missing", None)
            .await
            .unwrap_err(),
        SecretError::NotFound
    );
    assert_eq!(
        v.check_reference(&c, "inline", "p", None)
            .await
            .unwrap_err(),
        SecretError::ReferencesNotSupported
    );
    assert_eq!(
        v.check_reference(&c, "nope", "p", None).await.unwrap_err(),
        SecretError::StoreNotConfigured
    );
    // Discard of a reference never touches the store.
    v.discard(&c, &r, ValueSource::Reference).await.unwrap();
    assert_eq!(v.open(&c, &r).await.unwrap().expose(), b"sk-ref");
}

#[tokio::test]
async fn memory_managed_binding_discard_and_unavailable() {
    let (v, mem) = mem_vault();
    let c = ctx();
    let mut reg = StoreRegistry::from_config(&cfg("inline", &[]), keyring("k1")).unwrap();
    reg.register(mem.clone());
    reg.set_default("mem").unwrap();
    let v2 = SecretVault::new(reg);
    drop(v);

    let r = v2.seal(&c, pt("s")).await.unwrap();
    let r_b = v2.seal(&c, pt("s")).await.unwrap();
    assert_ne!(r, r_b, "put never overwrites");
    assert_eq!(v2.open(&c, &r).await.unwrap().expose(), b"s");
    let mut o = c;
    o.owner_id = Uuid::now_v7();
    assert_eq!(
        v2.open(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );

    let moved = v2.reseal(&c, &r, "inline").await.unwrap();
    assert_eq!(v2.open(&c, &moved).await.unwrap().expose(), b"s");

    assert_eq!(mem.managed_count(), 2);
    v2.discard(&c, &r, ValueSource::Managed).await.unwrap();
    assert_eq!(mem.managed_count(), 1);
    assert_eq!(v2.open(&c, &r).await.unwrap_err(), SecretError::NotFound);

    mem.set_unavailable(true);
    assert_eq!(mem.health().await, StoreHealth::Unavailable);
    assert_eq!(
        v2.open(&c, &r_b).await.unwrap_err(),
        SecretError::StoreUnavailable
    );
    assert_eq!(
        v2.seal(&c, pt("x")).await.unwrap_err(),
        SecretError::StoreUnavailable
    );
}

#[tokio::test]
async fn ext_ref_with_unconfigured_store() {
    let v = vault();
    let r = SecretRef::ext("ghost", "a/b", None, "1").unwrap();
    assert_eq!(
        v.open(&ctx(), &r).await.unwrap_err(),
        SecretError::StoreNotConfigured
    );
    assert_eq!(
        v.discard(&ctx(), &r, ValueSource::Managed)
            .await
            .unwrap_err(),
        SecretError::StoreNotConfigured
    );
}

#[test]
fn registry_validation() {
    let e = |c: SecretsSection| match StoreRegistry::from_config(&c, keyring("k1")) {
        Ok(_) => panic!("expected error"),
        Err(e) => e.to_string(),
    };
    assert!(e(cfg("inline", &[("v", "hashicorp-vault")])).contains("phase 3"));
    assert!(e(cfg("inline", &[("x", "bogus")])).contains("unknown kind"));
    assert!(e(cfg("missing", &[])).contains("not a configured store"));
    assert!(e(cfg("inline", &[("a", "inline"), ("a", "inline")])).contains("duplicate"));
    // Unmanaged default is rejected.
    let mut reg =
        StoreRegistry::from_config(&cfg("inline", &[("m", "memory")]), keyring("k1")).unwrap();
    assert!(reg.any_references());
    reg.set_default("m").unwrap();
    assert!(
        StoreRegistry::from_config(&cfg("inline", &[("inline", "inline")]), keyring("k1")).is_ok()
    );
}

#[tokio::test]
async fn no_plaintext_in_debug() {
    let v = vault();
    let c = ctx();
    let secret = pt("topsecretvalue");
    assert!(!format!("{secret:?}{secret}").contains("topsecretvalue"));
    let r = v.seal(&c, secret).await.unwrap();
    assert!(!format!("{r:?}{r}").contains("topsecretvalue"));
    let mut o = c;
    o.owner_id = Uuid::now_v7();
    let err = v.open(&o, &r).await.unwrap_err();
    assert!(!format!("{err:?}{err}").contains("topsecretvalue"));
}

struct Unmanaged;

#[async_trait::async_trait]
impl SecretStore for Unmanaged {
    fn id(&self) -> &str {
        "ro"
    }
    fn kind(&self) -> &'static str {
        "ro"
    }
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            managed: false,
            references: true,
        }
    }
    async fn put(&self, _: &SealContext, _: &Secret<Vec<u8>>) -> Result<SecretRef> {
        Err(SecretError::ManagedNotSupported)
    }
    async fn get(&self, _: &SealContext, _: &SecretRef) -> Result<Secret<Vec<u8>>> {
        Err(SecretError::NotFound)
    }
    async fn delete(&self, _: &SealContext, _: &SecretRef) -> Result<()> {
        Ok(())
    }
    async fn resolve_reference(
        &self,
        _: &SealContext,
        _: &str,
        _: Option<&str>,
    ) -> Result<SecretRef> {
        Err(SecretError::NotFound)
    }
    async fn health(&self) -> StoreHealth {
        StoreHealth::Healthy
    }
}

#[test]
fn unmanaged_default_rejected() {
    let mut reg = StoreRegistry::from_config(&cfg("inline", &[]), keyring("k1")).unwrap();
    reg.register(Arc::new(Unmanaged));
    assert!(matches!(
        reg.set_default("ro"),
        Err(SecretError::InvalidConfig(_))
    ));
}

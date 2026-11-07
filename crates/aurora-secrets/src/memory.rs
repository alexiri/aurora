//! Test-only in-memory store (feature `memory-store`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use aurora_core::Secret;
use uuid::Uuid;

use crate::{
    Purpose, Result, SealContext, SecretError, SecretRef, SecretStore, StoreCapabilities,
    StoreHealth,
};

type Binding = (Purpose, Uuid, Uuid);
type ManagedMap = HashMap<(String, String), (Binding, Secret<Vec<u8>>)>;
type RefMap = HashMap<(String, Option<String>), (String, Secret<Vec<u8>>)>;

#[derive(Default)]
struct Inner {
    /// (path, version) -> (binding, value)
    managed: ManagedMap,
    /// (path, field) -> (version, value)
    references: RefMap,
    /// org id -> path globs (`prefix*` or exact)
    allow: HashMap<Uuid, Vec<String>>,
}

pub struct MemoryStore {
    id: String,
    unavailable: AtomicBool,
    counter: AtomicU64,
    inner: Mutex<Inner>,
}

fn glob_match(glob: &str, path: &str) -> bool {
    match glob.strip_suffix('*') {
        Some(prefix) => path.starts_with(prefix),
        None => glob == path,
    }
}

impl MemoryStore {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            unavailable: AtomicBool::new(false),
            counter: AtomicU64::new(0),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    /// Preload an externally-written value (version "1").
    pub fn preload(&self, path: &str, field: Option<&str>, value: &[u8]) {
        self.lock().references.insert(
            (path.to_owned(), field.map(str::to_owned)),
            ("1".to_owned(), Secret::new(value.to_vec())),
        );
    }

    /// Permit an org to reference paths matching `glob` (`prefix*` or exact).
    pub fn allow_reference(&self, org_id: Uuid, glob: &str) {
        self.lock()
            .allow
            .entry(org_id)
            .or_default()
            .push(glob.to_owned());
    }

    /// Number of managed versions currently held.
    pub fn managed_count(&self) -> usize {
        self.lock().managed.len()
    }

    fn check_up(&self) -> Result<()> {
        if self.unavailable.load(Ordering::SeqCst) {
            Err(SecretError::StoreUnavailable)
        } else {
            Ok(())
        }
    }

    fn permitted(inner: &Inner, org: Uuid, path: &str) -> bool {
        inner
            .allow
            .get(&org)
            .is_some_and(|gs| gs.iter().any(|g| glob_match(g, path)))
    }

    fn parts<'a>(&self, r: &'a SecretRef) -> Result<(&'a str, Option<&'a str>, &'a str)> {
        match r {
            SecretRef::Ext {
                store_id,
                path,
                field,
                version,
            } if store_id == &self.id => Ok((path, field.as_deref(), version)),
            _ => Err(SecretError::Malformed("reference not for this store")),
        }
    }
}

#[async_trait]
impl SecretStore for MemoryStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> &'static str {
        "memory"
    }

    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            managed: true,
            references: true,
        }
    }

    async fn put(&self, ctx: &SealContext, plaintext: &Secret<Vec<u8>>) -> Result<SecretRef> {
        self.check_up()?;
        let path = format!(
            "managed/{}/{}/{}",
            ctx.org_id,
            ctx.purpose.as_str(),
            ctx.owner_id
        );
        let version = (self.counter.fetch_add(1, Ordering::SeqCst) + 1).to_string();
        self.lock().managed.insert(
            (path.clone(), version.clone()),
            (
                (ctx.purpose, ctx.owner_id, ctx.org_id),
                Secret::new(plaintext.expose().clone()),
            ),
        );
        SecretRef::ext(&self.id, &path, None, &version)
    }

    async fn get(&self, ctx: &SealContext, r: &SecretRef) -> Result<Secret<Vec<u8>>> {
        self.check_up()?;
        let (path, field, version) = self.parts(r)?;
        let inner = self.lock();
        if let Some((binding, value)) = inner.managed.get(&(path.to_owned(), version.to_owned())) {
            if *binding != (ctx.purpose, ctx.owner_id, ctx.org_id) {
                return Err(SecretError::SecretRefMismatch);
            }
            return Ok(value.clone());
        }
        if let Some((v, value)) = inner
            .references
            .get(&(path.to_owned(), field.map(str::to_owned)))
        {
            if !Self::permitted(&inner, ctx.org_id, path) {
                return Err(SecretError::ReferenceNotPermitted);
            }
            if v != version {
                return Err(SecretError::NotFound);
            }
            return Ok(value.clone());
        }
        Err(SecretError::NotFound)
    }

    async fn delete(&self, ctx: &SealContext, r: &SecretRef) -> Result<()> {
        self.check_up()?;
        let (path, _, version) = self.parts(r)?;
        let mut inner = self.lock();
        let key = (path.to_owned(), version.to_owned());
        if let Some((binding, _)) = inner.managed.get(&key) {
            if *binding != (ctx.purpose, ctx.owner_id, ctx.org_id) {
                return Err(SecretError::SecretRefMismatch);
            }
            inner.managed.remove(&key);
        }
        Ok(())
    }

    async fn resolve_reference(
        &self,
        ctx: &SealContext,
        path: &str,
        field: Option<&str>,
    ) -> Result<SecretRef> {
        self.check_up()?;
        let inner = self.lock();
        if !Self::permitted(&inner, ctx.org_id, path) {
            return Err(SecretError::ReferenceNotPermitted);
        }
        let (version, _) = inner
            .references
            .get(&(path.to_owned(), field.map(str::to_owned)))
            .ok_or(SecretError::NotFound)?;
        SecretRef::ext(&self.id, path, field, version)
    }

    async fn health(&self) -> StoreHealth {
        if self.unavailable.load(Ordering::SeqCst) {
            StoreHealth::Unavailable
        } else {
            StoreHealth::Healthy
        }
    }
}

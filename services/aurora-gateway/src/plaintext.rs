//! Zeroizing 60 s plaintext cache keyed by `(SecretRef, version)`
//! (docs/06 §Caches and invalidation). Plaintext is fetched lazily, after policy.

use crate::apps::ConnCand;
use crate::health::Health;
use crate::invalidation::{CacheStats, Invalidatable, Invalidate, Scope};
use crate::resolve::SecretCand;
use aurora_core::Secret;
use aurora_secrets::{Purpose, SealContext, SecretError, SecretVault};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const PLAINTEXT_TTL: Duration = Duration::from_secs(60);
const MAX_ENTRIES: usize = 5_000;

#[derive(Debug)]
pub enum PlaintextError {
    /// External store unreachable.
    StoreUnavailable(String),
    /// Anything else (missing value, malformed ref, ...).
    Unavailable,
    /// (phase 3, RFE-014) Not cached, the value lives in the platform database and that is
    /// unreachable: 503 `gateway_degraded` (`credential_store_unavailable`).
    DatabaseUnavailable,
}

struct Entry {
    value: Arc<Secret<Vec<u8>>>,
    org: Uuid,
    workspace: Option<Uuid>,
    expires: Instant,
    born: Instant,
}

pub struct PlaintextCache {
    vault: Arc<SecretVault>,
    ttl: Duration,
    map: Mutex<HashMap<(String, i64), Entry>>,
    health: Option<Arc<Health>>,
}

impl PlaintextCache {
    pub fn new(vault: Arc<SecretVault>) -> Self {
        Self::with_ttl(vault, PLAINTEXT_TTL)
    }

    pub fn with_ttl(vault: Arc<SecretVault>, ttl: Duration) -> Self {
        Self {
            vault,
            ttl,
            map: Mutex::new(HashMap::new()),
            health: None,
        }
    }

    /// (RFE-014) Refuses an uncached inline value while the database is unreachable.
    #[must_use]
    pub fn with_health(mut self, h: Arc<Health>) -> Self {
        self.health = Some(h);
        self
    }

    pub async fn get(&self, c: &SecretCand) -> Result<Arc<Secret<Vec<u8>>>, PlaintextError> {
        let ctx = SealContext {
            purpose: Purpose::SecretValue,
            owner_id: c.id,
            org_id: c.org_id,
            workspace_id: c.workspace_id,
        };
        self.fetch(&ctx, &c.vref, i64::from(c.version)).await
    }

    /// Credentials JSON of an app connection.
    pub async fn get_connection(
        &self,
        c: &ConnCand,
    ) -> Result<Arc<Secret<Vec<u8>>>, PlaintextError> {
        let ctx = SealContext {
            purpose: Purpose::ConnectionCredentials,
            owner_id: c.id,
            org_id: c.org_id,
            workspace_id: c.workspace_id,
        };
        self.fetch(&ctx, &c.vref, c.version).await
    }

    async fn fetch(
        &self,
        ctx: &SealContext,
        vref: &aurora_secrets::SecretRef,
        version: i64,
    ) -> Result<Arc<Secret<Vec<u8>>>, PlaintextError> {
        let key = (vref.to_column(), version);
        if let Ok(mut m) = self.map.lock()
            && let Some(e) = m.get(&key)
        {
            if e.expires > Instant::now() {
                if self.health.as_ref().is_some_and(|h| h.is_degraded_fast()) {
                    crate::health::count_served("plaintext_cache");
                }
                return Ok(e.value.clone());
            }
            m.remove(&key);
        }
        // (RFE-014) An inline value (and every refresh) needs the platform database; an
        // external store does not and is tried as usual.
        if vref.store_id().is_none() && self.health.as_ref().is_some_and(|h| h.db_unreachable()) {
            return Err(PlaintextError::DatabaseUnavailable);
        }
        let value = match self.vault.open(ctx, vref).await {
            Ok(v) => Arc::new(v),
            Err(SecretError::StoreUnavailable) => {
                return Err(PlaintextError::StoreUnavailable(
                    vref.store_id().unwrap_or("inline").to_owned(),
                ));
            }
            Err(e) => {
                tracing::warn!(owner_id = %ctx.owner_id, error = %e, "secret value unavailable");
                return Err(PlaintextError::Unavailable);
            }
        };
        if let Ok(mut m) = self.map.lock() {
            if m.len() >= MAX_ENTRIES {
                m.clear();
            }
            m.insert(
                key,
                Entry {
                    value: value.clone(),
                    org: ctx.org_id,
                    workspace: ctx.workspace_id,
                    expires: Instant::now() + self.ttl,
                    born: Instant::now(),
                },
            );
        }
        Ok(value)
    }
}

impl Invalidatable for PlaintextCache {
    fn invalidate(&self, ev: &Invalidate) {
        let Ok(mut m) = self.map.lock() else { return };
        match ev.scope {
            Scope::Org(id) => m.retain(|_, e| e.org != id),
            Scope::Workspace(id) => m.retain(|_, e| e.workspace != Some(id)),
            Scope::Connection(_) | Scope::All => m.clear(),
            Scope::Agent(_) | Scope::Credential(_) | Scope::User(_) => {}
        }
    }

    fn flush_all(&self) {
        if let Ok(mut m) = self.map.lock() {
            m.clear();
        }
    }

    fn stats(&self) -> CacheStats {
        let Ok(m) = self.map.lock() else {
            return CacheStats::default();
        };
        let now = Instant::now();
        CacheStats {
            entries: m.len(),
            oldest_age: m.values().map(|e| now - e.born).max(),
        }
    }
}

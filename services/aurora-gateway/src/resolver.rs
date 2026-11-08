//! DB-backed credential resolver with a 60 s identity cache
//! (docs/06 §Credential authentication, §Caches and invalidation).

use crate::auth::{
    ByoAdmission, CredentialKind, CredentialResolver, RequestIdentity, ResolveError,
};
use crate::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_db::PgPool;
use aurora_db::repo::gateway as repo;
use aurora_llm::governance::{ByoInputs, ByoInstance, OrgByo, effective_byo};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const IDENTITY_TTL: Duration = Duration::from_secs(60);
pub const NEGATIVE_TTL: Duration = Duration::from_secs(5);
const MAX_ENTRIES: usize = 50_000;
const MAX_NEGATIVE: usize = 10_000;

type Hash = [u8; 32];

struct Entry {
    identity: Arc<RequestIdentity>,
    expires: Instant,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Hash, Entry>,
    negative: HashMap<Hash, Instant>,
    /// (RFE-006) Credentials of frozen agents, answered 403 `frozen` without a database read for
    /// `NEGATIVE_TTL`; cleared by every invalidation (an unfreeze notifies).
    frozen: HashMap<Hash, (Instant, &'static str)>,
    by_org: HashMap<Uuid, HashSet<Hash>>,
    by_workspace: HashMap<Uuid, HashSet<Hash>>,
    by_agent: HashMap<Uuid, HashSet<Hash>>,
    by_credential: HashMap<Uuid, HashSet<Hash>>,
    by_user: HashMap<Uuid, HashSet<Hash>>,
    /// Bumped on every invalidation; guards against caching a read that raced one.
    generation: u64,
}

fn unindex(map: &mut HashMap<Uuid, HashSet<Hash>>, key: Uuid, h: &Hash) {
    if let Some(s) = map.get_mut(&key) {
        s.remove(h);
        if s.is_empty() {
            map.remove(&key);
        }
    }
}

impl Inner {
    fn remove(&mut self, h: &Hash) {
        let Some(e) = self.entries.remove(h) else {
            return;
        };
        let i = &e.identity;
        unindex(&mut self.by_org, i.org_id, h);
        unindex(&mut self.by_workspace, i.workspace_id, h);
        unindex(&mut self.by_agent, i.agent_id, h);
        unindex(&mut self.by_credential, i.credential_id, h);
        if let Some(u) = i.speaker_user_id {
            unindex(&mut self.by_user, u, h);
        }
    }

    fn insert(&mut self, h: Hash, identity: Arc<RequestIdentity>, ttl: Duration) {
        if self.entries.len() >= MAX_ENTRIES {
            self.clear();
        }
        self.remove(&h);
        self.by_org.entry(identity.org_id).or_default().insert(h);
        self.by_workspace
            .entry(identity.workspace_id)
            .or_default()
            .insert(h);
        self.by_agent
            .entry(identity.agent_id)
            .or_default()
            .insert(h);
        self.by_credential
            .entry(identity.credential_id)
            .or_default()
            .insert(h);
        if let Some(u) = identity.speaker_user_id {
            self.by_user.entry(u).or_default().insert(h);
        }
        self.entries.insert(
            h,
            Entry {
                identity,
                expires: Instant::now() + ttl,
            },
        );
    }

    fn drop_hashes(&mut self, hashes: Option<HashSet<Hash>>) {
        for h in hashes.unwrap_or_default() {
            self.remove(&h);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.by_org.clear();
        self.by_workspace.clear();
        self.by_agent.clear();
        self.by_credential.clear();
        self.by_user.clear();
        self.negative.clear();
        self.frozen.clear();
    }
}

/// Identity cache keyed by token hash, with reverse indexes for precise drops.
pub struct IdentityCache {
    inner: Mutex<Inner>,
    ttl: Duration,
    negative_ttl: Duration,
}

enum Lookup {
    Hit(Arc<RequestIdentity>),
    Denied,
    Frozen(&'static str),
    Miss(u64),
}

impl IdentityCache {
    fn new(ttl: Duration, negative_ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            ttl,
            negative_ttl,
        }
    }

    fn lookup(&self, h: &Hash) -> Lookup {
        let Ok(mut g) = self.inner.lock() else {
            return Lookup::Miss(0);
        };
        let now = Instant::now();
        if let Some(e) = g.entries.get(h) {
            if e.expires > now {
                return Lookup::Hit(e.identity.clone());
            }
            g.remove(h);
        }
        if let Some(until) = g.negative.get(h) {
            if *until > now {
                return Lookup::Denied;
            }
            g.negative.remove(h);
        }
        if let Some((until, level)) = g.frozen.get(h).copied() {
            if until > now {
                return Lookup::Frozen(level);
            }
            g.frozen.remove(h);
        }
        Lookup::Miss(g.generation)
    }

    fn store_frozen(&self, h: Hash, level: &'static str, gen_seen: u64) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.generation != gen_seen {
            return;
        }
        if g.frozen.len() >= MAX_NEGATIVE {
            g.frozen.clear();
        }
        g.frozen
            .insert(h, (Instant::now() + self.negative_ttl, level));
    }

    fn store(&self, h: Hash, found: Option<(Arc<RequestIdentity>, Option<f64>)>, gen_seen: u64) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.generation != gen_seen {
            return; // an invalidation raced this read; do not cache it
        }
        match found {
            Some((i, expires_in)) => {
                // Context credentials must not outlive `expires_at` (SEC-25).
                let ttl = match expires_in {
                    Some(s) => self.ttl.min(Duration::from_secs_f64(s.max(0.0))),
                    None => self.ttl,
                };
                g.insert(h, i, ttl);
            }
            None => {
                if g.negative.len() >= MAX_NEGATIVE {
                    g.negative.clear();
                }
                g.negative.insert(h, Instant::now() + self.negative_ttl);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.entries.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Invalidatable for IdentityCache {
    fn invalidate(&self, ev: &Invalidate) {
        let Ok(mut g) = self.inner.lock() else { return };
        g.generation += 1;
        g.negative.clear();
        g.frozen.clear();
        match ev.scope {
            Scope::All => g.clear(),
            Scope::Org(id) => {
                let s = g.by_org.remove(&id);
                g.drop_hashes(s);
            }
            Scope::Workspace(id) => {
                let s = g.by_workspace.remove(&id);
                g.drop_hashes(s);
            }
            Scope::Agent(id) => {
                let s = g.by_agent.remove(&id);
                g.drop_hashes(s);
            }
            Scope::Credential(id) => {
                let s = g.by_credential.remove(&id);
                g.drop_hashes(s);
            }
            Scope::User(id) => {
                let s = g.by_user.remove(&id);
                g.drop_hashes(s);
            }
            Scope::Connection(_) => {}
        }
    }

    fn flush_all(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.generation += 1;
            g.clear();
        }
    }
}

pub struct DbCredentialResolver {
    pool: PgPool,
    cache: Arc<IdentityCache>,
    /// (RFE-025) the instance level of BYO governance.
    byo: ByoInstance,
}

impl DbCredentialResolver {
    pub fn new(pool: PgPool) -> Self {
        Self::with_ttls(pool, IDENTITY_TTL, NEGATIVE_TTL)
    }

    pub fn with_ttls(pool: PgPool, ttl: Duration, negative_ttl: Duration) -> Self {
        Self {
            pool,
            cache: Arc::new(IdentityCache::new(ttl, negative_ttl)),
            byo: ByoInstance::default(),
        }
    }

    /// Sets the instance BYO configuration (`[byo]`, RFE-025).
    #[must_use]
    pub fn with_byo(mut self, byo: ByoInstance) -> Self {
        self.byo = byo;
        self
    }

    /// The cache, to register with the invalidation hub.
    pub fn cache(&self) -> Arc<IdentityCache> {
        self.cache.clone()
    }

    async fn load(
        &self,
        hash: &Hash,
    ) -> Result<Option<(RequestIdentity, Option<f64>)>, ResolveError> {
        let unavailable = |e: aurora_db::DbError| ResolveError::Unavailable(e.to_string());
        let Some(row) = repo::find_credential_identity(&self.pool, hash)
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        self.identity(row).await
    }

    async fn identity(
        &self,
        row: repo::CredentialIdentityRow,
    ) -> Result<Option<(RequestIdentity, Option<f64>)>, ResolveError> {
        let unavailable = |e: aurora_db::DbError| ResolveError::Unavailable(e.to_string());
        // (RFE-006) A frozen agent's credentials are refused whatever their kind; never cached
        // as an identity.
        if let Some(l) = row.frozen_level.as_deref() {
            return Err(ResolveError::Frozen {
                level: match l {
                    "org" => "org",
                    "workspace" => "workspace",
                    _ => "agent",
                },
            });
        }
        let kind = match row.kind.as_str() {
            "agent" => CredentialKind::Agent,
            "boot" => CredentialKind::Boot,
            "context" => CredentialKind::Context,
            other => {
                tracing::warn!(kind = other, "unknown credential kind");
                return Ok(None);
            }
        };
        // RFE-005 (D-49): only a hosted context credential carries a speaker, and only while the
        // person still holds `can_use` (checked here, re-checked after a `user`/`workspace`/`org`
        // invalidation drops the cached identity). A BYO agent mints its own context credentials,
        // so the column is never trusted there.
        let speaker_user_id = match row.speaker_user_id {
            Some(u) if kind == CredentialKind::Context && row.agent_kind != "byo" => {
                repo::can_use(&self.pool, row.workspace_id, u)
                    .await
                    .map_err(unavailable)?
                    .then_some(u)
            }
            _ => None,
        };
        let expires_in = row.expires_in_secs;
        let byo = (row.agent_kind == "byo" && kind != CredentialKind::Boot).then(|| {
            // Unparsable stored values fail closed inside the governance library.
            let eff = effective_byo(
                &self.byo,
                &ByoInputs {
                    org: OrgByo::from_stored(row.org_byo.as_ref()),
                    workspace_posture: row.ws_byo_posture.as_deref(),
                    agent_cidrs: row.agent_byo_source_cidrs.as_deref(),
                    agent_require_context: row.agent_byo_require_context,
                },
            );
            ByoAdmission {
                effective: eff,
                org_slug: row.org_slug.clone(),
                ws_slug: row.ws_slug.clone(),
            }
        });
        Ok(Some((
            RequestIdentity {
                credential_id: row.credential_id,
                kind,
                agent_id: row.agent_id,
                workspace_id: row.workspace_id,
                org_id: row.org_id,
                conversation_id: row.conversation_id,
                turn_id: row.turn_id,
                context_label: row.context_label,
                speaker_user_id,
                capabilities: row.capabilities,
                byo,
            },
            expires_in,
        )))
    }
}

fn has_aurora_prefix(token: &str) -> bool {
    ["aug_", "aub_", "auc_"]
        .iter()
        .any(|p| token.starts_with(p))
}

#[async_trait::async_trait]
impl CredentialResolver for DbCredentialResolver {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        if !has_aurora_prefix(token) {
            return Ok(None);
        }
        let hash = aurora_core::token::hash(token);
        let gen_seen = match self.cache.lookup(&hash) {
            Lookup::Hit(i) => return Ok(Some((*i).clone())),
            Lookup::Denied => return Ok(None),
            Lookup::Frozen(level) => return Err(ResolveError::Frozen { level }),
            Lookup::Miss(g) => g,
        };
        let found = match self.load(&hash).await {
            Err(ResolveError::Frozen { level }) => {
                self.cache.store_frozen(hash, level, gen_seen);
                return Err(ResolveError::Frozen { level });
            }
            other => other?,
        };
        let Some((identity, expires_in)) = found else {
            self.cache.store(hash, None, gen_seen);
            return Ok(None);
        };
        self.cache.store(
            hash,
            Some((Arc::new(identity.clone()), expires_in)),
            gen_seen,
        );
        Ok(Some(identity))
    }

    async fn resolve_credential(&self, id: Uuid) -> Result<Option<RequestIdentity>, ResolveError> {
        let Some(row) = repo::find_credential_identity_by_id(&self.pool, id)
            .await
            .map_err(|e| ResolveError::Unavailable(e.to_string()))?
        else {
            return Ok(None);
        };
        Ok(self.identity(row).await?.map(|(i, _)| i))
    }
}

//! DB-backed credential resolver with a 60 s identity cache
//! (docs/06 §Credential authentication, §Caches and invalidation).

use crate::auth::{
    ByoAdmission, CredentialKind, CredentialResolver, RequestIdentity, ResolveError,
};
use crate::health::{self, Health};
use crate::invalidation::{CacheStats, Invalidatable, Invalidate, Scope};
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

/// (RFE-013, C-316) A credential's use is recorded at most once a minute.
pub const LAST_USED_INTERVAL: Duration = Duration::from_secs(60);
/// Bound on credentials waiting for their `last_used_at` write; beyond it, uses are dropped.
const MAX_PENDING_TOUCH: usize = 10_000;

struct Entry {
    identity: Arc<RequestIdentity>,
    expires: Instant,
    born: Instant,
    /// When this credential's use was last queued for `last_used_at` (None = never).
    touched: Option<Instant>,
    /// (RFE-013) When an agent token expires (None = never, or not an agent token).
    deadline: Option<Instant>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Hash, Entry>,
    /// (FIX-050, C-630) Refusals, answered without a database read for `NEGATIVE_TTL`: the
    /// `None` kind of entry is a plain unknown token; `Some` carries the classified refusal
    /// (`revoked`, `expired`, `no_active_owner`) replayed as a `Refused` error. Cleared by
    /// every invalidation like the other negative answers.
    negative: HashMap<Hash, (Instant, Option<Refusal>)>,
    /// (RFE-006) Credentials of frozen agents, answered 403 `frozen` without a database read for
    /// `NEGATIVE_TTL`; cleared by every invalidation (an unfreeze notifies).
    frozen: HashMap<Hash, (Instant, &'static str)>,
    /// (RFE-013) Expired agent tokens, answered `agent_token_expired` without a database read
    /// for `NEGATIVE_TTL`. An expiry never lifts by itself, but the entry is short-lived and
    /// cleared by every invalidation like the other negative answers.
    expired: HashMap<Hash, Instant>,
    /// (RFE-013) Credentials whose use awaits a batched `last_used_at` write.
    touch: HashSet<Uuid>,
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

    fn insert(
        &mut self,
        h: Hash,
        identity: Arc<RequestIdentity>,
        ttl: Duration,
        deadline: Option<Instant>,
    ) {
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
                born: Instant::now(),
                touched: None,
                deadline,
            },
        );
    }

    /// Queues the credential's use unless it was queued less than a minute ago.
    fn note_use(&mut self, h: &Hash, now: Instant) {
        let Some(e) = self.entries.get_mut(h) else {
            return;
        };
        if e.touched
            .is_some_and(|t| now.duration_since(t) < LAST_USED_INTERVAL)
        {
            return;
        }
        e.touched = Some(now);
        if self.touch.len() < MAX_PENDING_TOUCH {
            self.touch.insert(e.identity.credential_id);
        }
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
        self.expired.clear();
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
    /// A cached negative answer: `Some` = a classified refusal (FIX-050), `None` = a plain
    /// unknown token.
    Denied(Option<Refusal>),
    Frozen(&'static str),
    Expired,
    Miss(u64),
}

/// (FIX-050, C-630) A refusal the repository classified, cached with the negative TTL and
/// replayed as [`ResolveError::Refused`]. Never any token material: the hash is the cache key.
#[derive(Clone, Copy, Debug)]
struct Refusal {
    reason: &'static str,
    kind: Option<&'static str>,
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
            // `expires` is already capped at the credential's own `expires_at` (see `store`), so
            // a cached agent token stops being honoured at its expiry, not at the TTL boundary.
            if e.expires > now {
                let identity = e.identity.clone();
                g.note_use(h, now);
                return Lookup::Hit(identity);
            }
            g.remove(h);
        }
        if let Some(until) = g.expired.get(h) {
            if *until > now {
                return Lookup::Expired;
            }
            g.expired.remove(h);
        }
        if let Some((until, refused)) = g.negative.get(h).copied() {
            if until > now {
                return Lookup::Denied(refused);
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

    fn store_expired(&self, h: Hash, gen_seen: u64) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.generation != gen_seen {
            return;
        }
        if g.expired.len() >= MAX_NEGATIVE {
            g.expired.clear();
        }
        g.expired.insert(h, Instant::now() + self.negative_ttl);
    }

    /// The expiry of the cached agent token with this credential id, if it has one.
    fn deadline_of(&self, credential: Uuid) -> Option<Instant> {
        let g = self.inner.lock().ok()?;
        g.by_credential
            .get(&credential)?
            .iter()
            .find_map(|h| g.entries.get(h).and_then(|e| e.deadline))
    }

    /// Takes the credentials whose use awaits a `last_used_at` write.
    fn take_touched(&self) -> Vec<Uuid> {
        self.inner
            .lock()
            .map(|mut g| std::mem::take(&mut g.touch).into_iter().collect())
            .unwrap_or_default()
    }

    /// Queues a use recorded on a freshly loaded entry.
    fn note_loaded(&self, h: &Hash) {
        if let Ok(mut g) = self.inner.lock() {
            g.note_use(h, Instant::now());
        }
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
                let deadline = (i.kind == CredentialKind::Agent)
                    .then_some(expires_in)
                    .flatten()
                    .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
                g.insert(h, i, ttl, deadline);
            }
            None => {
                if g.negative.len() >= MAX_NEGATIVE {
                    g.negative.clear();
                }
                g.negative
                    .insert(h, (Instant::now() + self.negative_ttl, None));
            }
        }
    }

    /// (FIX-050, C-630) Caches a classified refusal with the negative TTL; the reason is
    /// replayed at the next lookups within that window (and so logged, not re-read).
    fn store_refused(&self, h: Hash, r: Refusal, gen_seen: u64) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.generation != gen_seen {
            return; // an invalidation raced this read; do not cache it
        }
        if g.negative.len() >= MAX_NEGATIVE {
            g.negative.clear();
        }
        g.negative
            .insert(h, (Instant::now() + self.negative_ttl, Some(r)));
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
        g.expired.clear();
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

    fn stats(&self) -> CacheStats {
        let Ok(g) = self.inner.lock() else {
            return CacheStats::default();
        };
        let now = Instant::now();
        CacheStats {
            entries: g.entries.len(),
            oldest_age: g.entries.values().map(|e| now - e.born).max(),
        }
    }
}

pub struct DbCredentialResolver {
    pool: PgPool,
    cache: Arc<IdentityCache>,
    /// (RFE-025) the instance level of BYO governance.
    byo: ByoInstance,
    /// (RFE-014) Outage state: lookups are refused at once while the database is unreachable and
    /// connection-class failures are reported.
    health: Option<Arc<Health>>,
    flights: crate::coalesce::Coalescer<Hash>,
}

/// A repository failure as a resolve error; a connection-class one is an outage signal.
fn resolve_err(h: Option<&Arc<Health>>, e: &aurora_db::DbError) -> ResolveError {
    if health::is_db_connection_error(e) {
        if let Some(h) = h {
            h.report_db_error();
        }
        ResolveError::DatabaseUnavailable
    } else {
        ResolveError::Unavailable(e.to_string())
    }
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
            health: None,
            flights: crate::coalesce::Coalescer::default(),
        }
    }

    /// Reports outages to `h` and refuses cache misses while the database is unreachable.
    #[must_use]
    pub fn with_health(mut self, h: Arc<Health>) -> Self {
        self.health = Some(h);
        self
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
        let unavailable = |e: aurora_db::DbError| resolve_err(self.health.as_ref(), &e);
        let Some(row) = repo::find_credential_identity_incl_expired(&self.pool, hash)
            .await
            .map_err(unavailable)?
        else {
            // (FIX-050, C-630) Nothing was found under the full identity conditions: classify
            // why (a revoked row, or a live row of an organization without an active owner —
            // the owner-integrity incident), so the refusal is observable. One extra read on
            // the miss path only, cached with the negative entry's TTL.
            return match repo::find_refused_credential(&self.pool, hash)
                .await
                .map_err(|e| {
                    // A classification failure must not turn a plain 407 into a 503: the
                    // credential stays refused, as `unknown`.
                    tracing::debug!(error = %e, "refusal classification failed");
                }) {
                Ok(Some(row)) => {
                    let kind = static_kind(&row.kind);
                    // Token-level facts outrank the org-level owner state: a revoked row says
                    // `revoked`, an expired row `expired`, whatever the owner state.
                    let reason = if row.revoked {
                        "revoked"
                    } else if row.expired {
                        "expired"
                    } else if row.has_owner {
                        // Live, not revoked, not expired, an owner exists: only an unusable
                        // row remains; answered like an unknown credential.
                        "unknown"
                    } else {
                        "no_active_owner"
                    };
                    Err(ResolveError::Refused { reason, kind })
                }
                _ => Ok(None),
            };
        };
        // (RFE-013) Past `expires_at`: an agent token says so (407 `agent_token_expired`); the
        // other kinds are refused like an unknown credential, now classified (FIX-050).
        if row.expires_in_secs.is_some_and(|s| s <= 0.0) {
            return if row.kind == "agent" {
                Err(ResolveError::Expired)
            } else {
                Err(ResolveError::Refused {
                    reason: "expired",
                    kind: static_kind(&row.kind),
                })
            };
        }
        self.identity(row).await
    }

    /// (RFE-013, C-316) Writes the queued `last_used_at` updates in one statement. Best effort:
    /// skipped while the database is unreachable (the uses are dropped, never retried), a failure
    /// is logged. Returns the number of rows written.
    pub async fn flush_last_used(&self) -> u64 {
        let ids = self.cache.take_touched();
        if ids.is_empty() {
            return 0;
        }
        if self
            .health
            .as_ref()
            .is_some_and(|h| h.db_unreachable() || h.is_degraded_fast())
        {
            return 0;
        }
        match repo::touch_last_used(&self.pool, &ids).await {
            Ok(n) => n,
            Err(e) => {
                resolve_err(self.health.as_ref(), &e);
                tracing::debug!(error = %e, "last_used_at write failed");
                0
            }
        }
    }

    /// Runs [`Self::flush_last_used`] every `interval` until `shutdown`, with a final flush.
    pub async fn run_last_used(
        self: Arc<Self>,
        interval: Duration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => { self.flush_last_used().await; }
                r = shutdown.changed() => {
                    if r.is_err() || *shutdown.borrow() {
                        self.flush_last_used().await;
                        return;
                    }
                }
            }
        }
    }

    async fn identity(
        &self,
        row: repo::CredentialIdentityRow,
    ) -> Result<Option<(RequestIdentity, Option<f64>)>, ResolveError> {
        let unavailable = |e: aurora_db::DbError| resolve_err(self.health.as_ref(), &e);
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
        // (P4-GRP-1, D-131) The speaker's group ids come with the speaker: none without one.
        let speaker_group_ids = match speaker_user_id {
            Some(u) => repo::speaker_group_ids(&self.pool, row.org_id, u)
                .await
                .map_err(unavailable)?,
            None => Vec::new(),
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
                speaker_group_ids,
                capabilities: row.capabilities,
                byo,
            },
            expires_in,
        )))
    }
}

/// The credential kind as a `'static` label, when it is one of the known ones (never token
/// material: the string is one of `agent` | `boot` | `context` or nothing).
fn static_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "agent" => Some("agent"),
        "boot" => Some("boot"),
        "context" => Some("context"),
        _ => None,
    }
}

/// Rebuilds the cached form of a refusal (the reason must be one of the classified ones).
fn refusal_of(reason: &'static str, kind: Option<&'static str>) -> Option<Refusal> {
    matches!(
        reason,
        "revoked" | "expired" | "no_active_owner" | "unknown"
    )
    .then_some(Refusal { reason, kind })
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
        match self.cache.lookup(&hash) {
            Lookup::Hit(i) => {
                if self.health.as_ref().is_some_and(|h| h.is_degraded_fast()) {
                    health::count_served("identity_cache");
                }
                return Ok(Some((*i).clone()));
            }
            Lookup::Denied(Some(r)) => {
                return Err(ResolveError::Refused {
                    reason: r.reason,
                    kind: r.kind,
                });
            }
            Lookup::Denied(None) => return Ok(None),
            Lookup::Expired => return Err(ResolveError::Expired),
            Lookup::Frozen(level) => return Err(ResolveError::Frozen { level }),
            Lookup::Miss(_) => {}
        }
        // (RFE-014) Not cached and the database is unreachable: the gateway cannot tell an
        // invalid token from an unreadable store, so it refuses (never 407) without asking.
        if self.health.as_ref().is_some_and(|h| h.db_unreachable()) {
            return Err(ResolveError::DatabaseUnavailable);
        }
        // Misses are coalesced per credential: one database read, the others wait for its
        // result (a flush right after a reconnect turns every entry into a miss).
        let _flight = self.flights.enter(&hash).await;
        let gen_seen = match self.cache.lookup(&hash) {
            Lookup::Hit(i) => return Ok(Some((*i).clone())),
            Lookup::Denied(Some(r)) => {
                return Err(ResolveError::Refused {
                    reason: r.reason,
                    kind: r.kind,
                });
            }
            Lookup::Denied(None) => return Ok(None),
            Lookup::Expired => return Err(ResolveError::Expired),
            Lookup::Frozen(level) => return Err(ResolveError::Frozen { level }),
            Lookup::Miss(g) => g,
        };
        if self.health.as_ref().is_some_and(|h| h.db_unreachable()) {
            return Err(ResolveError::DatabaseUnavailable);
        }
        let found = match self.load(&hash).await {
            Err(ResolveError::Frozen { level }) => {
                self.cache.store_frozen(hash, level, gen_seen);
                return Err(ResolveError::Frozen { level });
            }
            Err(ResolveError::Expired) => {
                self.cache.store_expired(hash, gen_seen);
                return Err(ResolveError::Expired);
            }
            Err(e @ ResolveError::Refused { .. }) => {
                if let ResolveError::Refused { reason, kind } = &e
                    && let Some(r) = refusal_of(reason, *kind)
                {
                    self.cache.store_refused(hash, r, gen_seen);
                }
                return Err(e);
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
        self.cache.note_loaded(&hash);
        Ok(Some(identity))
    }

    fn token_deadline(&self, credential: Uuid) -> Option<Instant> {
        self.cache.deadline_of(credential)
    }

    async fn resolve_credential(&self, id: Uuid) -> Result<Option<RequestIdentity>, ResolveError> {
        let Some(row) = repo::find_credential_identity_by_id(&self.pool, id)
            .await
            .map_err(|e| resolve_err(self.health.as_ref(), &e))?
        else {
            return Ok(None);
        };
        Ok(self.identity(row).await?.map(|(i, _)| i))
    }
}

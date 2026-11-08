//! Connect-time resolution: the cached policy view and secret/LLM candidates
//! for (org, workspace, credential, host) (docs/06 §MITM lane, §Caches).

use crate::apps::ConnCand;
use crate::auth::RequestIdentity;
use crate::invalidation::{Invalidatable, Invalidate, Scope};
use crate::snapshot::{Loaded, SnapshotStore};
use async_trait::async_trait;
use aurora_db::repo::gateway::{self as repo, AppConnectionRow, ScopePolicyRows, SecretMetaRow};
use aurora_llm::{AuthMode, LlmInjection, ProviderRegistry};
use aurora_policy::facts::{Level, SecretKind};
use aurora_policy::{Credential, HostPattern, PathPattern, Rule, ScopePolicy};
use aurora_secrets::{InjectionRecipe, SecretRef};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const RESOLVE_TTL: Duration = Duration::from_secs(60);
const MAX_ENTRIES: usize = 20_000;

/// How a secret is applied to a request.
#[derive(Debug, Clone)]
pub enum Injection {
    Recipe(InjectionRecipe),
    Llm(LlmInjection),
}

#[derive(Debug, Clone)]
pub struct SecretCand {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub scope: Level,
    pub kind: SecretKind,
    pub path: Option<PathPattern>,
    pub injection: Injection,
    pub vref: SecretRef,
    pub version: i32,
    pub llm_endpoint: Option<String>,
    pub provider: Option<String>,
    /// Stored key origin (`secrets.origin`, LLM secrets).
    pub origin: Option<serde_json::Value>,
}

impl SecretCand {
    pub fn credential(&self) -> Credential {
        Credential::Secret {
            id: self.id,
            kind: self.kind,
            scope: self.scope,
        }
    }
}

#[derive(Debug)]
pub struct ResolvedHost {
    pub org_policy: ScopePolicy,
    pub ws_policy: ScopePolicy,
    /// Candidates whose host pattern matches (path patterns are per request).
    pub secrets: Vec<Arc<SecretCand>>,
    pub org_slug: String,
    pub ws_slug: String,
    pub is_llm_host: bool,
    /// Effective LLM/egress governance of the org (`None` = governance not wired).
    pub governance: Option<Arc<aurora_llm::governance::EffectiveGovernance>>,
    /// The registry snapshot this view was built against.
    pub registry: Option<Arc<Loaded>>,
    /// Active app connections of the apps serving this host (any path).
    pub conns: Vec<Arc<ConnCand>>,
    /// Apps with a `disabled` connection in scope (guidance).
    pub disabled_apps: HashSet<String>,
    /// Apps removed by a `restricted` org posture (`app_unavailable`).
    pub unavailable_apps: HashSet<String>,
    /// Some active host entry of the snapshot covers this host.
    pub known_app_host: bool,
    /// App ids whose active host entries cover this host (open-lane `app` targets).
    pub host_apps: Vec<String>,
    /// The workspace's published open-lane posture (RFE-003).
    pub ws_open_lane: aurora_llm::governance::OpenLanePosture,
}

impl ResolvedHost {
    /// All rules of both scopes (for injection selection).
    pub fn all_rules(&self) -> impl Iterator<Item = &Rule> {
        self.org_policy
            .ordered_rules
            .iter()
            .chain(self.ws_policy.ordered_rules.iter())
    }

    pub fn needs_body(&self) -> bool {
        self.all_rules().any(|r| !r.conditions.is_empty())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("resolution failed: {0}")]
pub struct ResolveFailure(pub String);

/// Where the policy view comes from (the database in production).
#[async_trait]
pub trait PolicySource: Send + Sync + 'static {
    async fn scope_policy(&self, org: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String>;
    async fn secrets(&self, org: Uuid, ws: Uuid) -> Result<Vec<SecretMetaRow>, String>;
    async fn slugs(&self, ws: Uuid) -> Result<Option<(String, String)>, String>;
    /// `active`/`disabled` connections of the given apps (default: none).
    async fn connections(
        &self,
        _org: Uuid,
        _ws: Uuid,
        _app_ids: &[String],
    ) -> Result<Vec<AppConnectionRow>, String> {
        Ok(Vec::new())
    }
    /// `(restricted, available app ids)` (default: open posture).
    async fn app_availability(&self, _org: Uuid, _ws: Uuid) -> Result<(bool, Vec<String>), String> {
        Ok((false, Vec::new()))
    }
}

pub struct DbSource(pub aurora_db::PgPool);

#[async_trait]
impl PolicySource for DbSource {
    async fn scope_policy(&self, org: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        repo::load_scope_policy(&self.0, org, ws)
            .await
            .map_err(|e| e.to_string())
    }
    async fn secrets(&self, org: Uuid, ws: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        repo::active_secrets(&self.0, org, ws)
            .await
            .map_err(|e| e.to_string())
    }
    async fn slugs(&self, ws: Uuid) -> Result<Option<(String, String)>, String> {
        repo::scope_slugs(&self.0, ws)
            .await
            .map_err(|e| e.to_string())
    }
    async fn connections(
        &self,
        org: Uuid,
        ws: Uuid,
        app_ids: &[String],
    ) -> Result<Vec<AppConnectionRow>, String> {
        repo::app_connections(&self.0, org, ws, app_ids)
            .await
            .map_err(|e| e.to_string())
    }
    async fn app_availability(&self, org: Uuid, ws: Uuid) -> Result<(bool, Vec<String>), String> {
        repo::app_availability(&self.0, org, ws)
            .await
            .map_err(|e| e.to_string())
    }
}

fn to_scope_policy(rows: ScopePolicyRows) -> Result<ScopePolicy, String> {
    let mut rules = Vec::with_capacity(rows.rules.len() + rows.derived.len());
    for r in rows.rules {
        let v = serde_json::json!({
            "logical_id": r.logical_id, "name": r.name, "enabled": r.enabled,
            "identities": r.identities, "targets": r.targets, "action": r.action,
            "require_approval": r.require_approval,
            "approval_timeout_secs": r.approval_timeout_secs, "rate_limit": r.rate_limit,
            "conditions": r.conditions,
        });
        rules.push(serde_json::from_value::<Rule>(v).map_err(|e| format!("policy rule: {e}"))?);
    }
    for (logical_id, mut v) in rows.derived {
        if let Some(o) = v.as_object_mut() {
            o.insert("logical_id".into(), serde_json::json!(logical_id));
        }
        rules.push(serde_json::from_value::<Rule>(v).map_err(|e| format!("derived rule: {e}"))?);
    }
    let v = serde_json::json!({
        "default_action": rows.default_action,
        "uncredentialed_default": rows.uncredentialed_default,
        "uncredentialed_allowlist": rows.uncredentialed_allowlist,
    });
    let mut p: ScopePolicy = serde_json::from_value(v).map_err(|e| format!("policy: {e}"))?;
    p.ordered_rules = rules;
    Ok(p)
}

fn to_candidate(r: SecretMetaRow, org: Uuid, llm: &ProviderRegistry) -> Result<SecretCand, String> {
    let (kind, injection) = if r.kind == "llm" {
        let provider = r
            .llm_provider
            .as_deref()
            .and_then(|p| llm.provider(p))
            .ok_or("unknown llm provider")?;
        (
            SecretKind::Llm,
            Injection::Llm(provider.injection(&AuthMode::ApiKey)),
        )
    } else {
        let recipe: InjectionRecipe =
            serde_json::from_value(r.injection).map_err(|e| format!("recipe: {e}"))?;
        recipe.validate()?;
        (SecretKind::Generic, Injection::Recipe(recipe))
    };
    Ok(SecretCand {
        id: r.id,
        org_id: org,
        workspace_id: r.workspace_id,
        name: r.name,
        scope: if r.workspace_id.is_some() {
            Level::Workspace
        } else {
            Level::Organization
        },
        kind,
        path: r
            .path_pattern
            .as_deref()
            .map(PathPattern::parse)
            .transpose()
            .map_err(|e| format!("path pattern: {e:?}"))?,
        injection,
        vref: r
            .value_ref
            .parse()
            .map_err(|_| "bad value_ref".to_string())?,
        version: r.value_version,
        llm_endpoint: r.llm_endpoint_id,
        provider: r.llm_provider,
        origin: r.origin,
    })
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    credential: Uuid,
    host: String,
    port: u16,
    snapshot: Option<Arc<str>>,
}

struct Entry {
    value: Arc<ResolvedHost>,
    expires: Instant,
}

#[derive(Default)]
struct Cache {
    map: HashMap<Key, Entry>,
    generation: u64,
}

pub struct Resolver {
    source: Arc<dyn PolicySource>,
    llm: Arc<ProviderRegistry>,
    ttl: Duration,
    cache: Arc<ResolveCache>,
    governance: Option<Arc<crate::governance::Governance>>,
    snapshots: Option<Arc<SnapshotStore>>,
}

/// The invalidatable part (registered with the hub).
#[derive(Default)]
pub struct ResolveCache {
    inner: Mutex<Cache>,
}

impl Invalidatable for ResolveCache {
    fn invalidate(&self, ev: &Invalidate) {
        let Ok(mut g) = self.inner.lock() else { return };
        g.generation += 1;
        match ev.scope {
            Scope::Org(id) => g.map.retain(|k, _| k.org != id),
            Scope::Workspace(id) => g.map.retain(|k, _| k.ws != id),
            Scope::Agent(id) => g.map.retain(|k, _| k.agent != id),
            Scope::Credential(id) => g.map.retain(|k, _| k.credential != id),
            // Connections and `all` drop everything; users do not affect this cache.
            Scope::Connection(_) | Scope::All => g.map.clear(),
            Scope::User(_) => {}
        }
    }

    fn flush_all(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.generation += 1;
            g.map.clear();
        }
    }
}

impl Resolver {
    pub fn new(source: Arc<dyn PolicySource>, llm: Arc<ProviderRegistry>) -> Self {
        Self::with_ttl(source, llm, RESOLVE_TTL)
    }

    pub fn with_ttl(
        source: Arc<dyn PolicySource>,
        llm: Arc<ProviderRegistry>,
        ttl: Duration,
    ) -> Self {
        Self {
            source,
            llm,
            ttl,
            cache: Arc::new(ResolveCache::default()),
            governance: None,
            snapshots: None,
        }
    }

    /// Apply org governance (08 §11): egress posture as the org uncredentialed default and
    /// the governed-host view used by step 6a.
    pub fn with_governance(mut self, g: Arc<crate::governance::Governance>) -> Self {
        self.governance = Some(g);
        self
    }

    /// Serve app connections declared by the registry snapshot.
    pub fn with_snapshots(mut self, s: Arc<SnapshotStore>) -> Self {
        self.snapshots = Some(s);
        self
    }

    pub fn cache(&self) -> Arc<ResolveCache> {
        self.cache.clone()
    }

    pub fn llm(&self) -> &ProviderRegistry {
        &self.llm
    }

    pub async fn resolve(
        &self,
        id: &RequestIdentity,
        host: &str,
        port: u16,
    ) -> Result<Arc<ResolvedHost>, ResolveFailure> {
        let key = Key {
            org: id.org_id,
            ws: id.workspace_id,
            agent: id.agent_id,
            credential: id.credential_id,
            host: host.to_ascii_lowercase(),
            port,
            snapshot: self
                .snapshots
                .as_ref()
                .and_then(|s| s.current())
                .map(|l| l.hash.clone()),
        };
        let gen_seen = {
            let Ok(mut g) = self.cache.inner.lock() else {
                return self.load(&key).await.map(Arc::new);
            };
            if let Some(e) = g.map.get(&key) {
                if e.expires > Instant::now() {
                    return Ok(e.value.clone());
                }
                g.map.remove(&key);
            }
            g.generation
        };
        let value = Arc::new(self.load(&key).await?);
        if let Ok(mut g) = self.cache.inner.lock()
            && g.generation == gen_seen
        {
            if g.map.len() >= MAX_ENTRIES {
                g.map.clear();
            }
            g.map.insert(
                key,
                Entry {
                    value: value.clone(),
                    expires: Instant::now() + self.ttl,
                },
            );
        }
        Ok(value)
    }

    async fn load(&self, k: &Key) -> Result<ResolvedHost, ResolveFailure> {
        let fail = |e: String| ResolveFailure(e);
        let org_rows = self.source.scope_policy(k.org, None).await.map_err(fail)?;
        let ws_rows = self
            .source
            .scope_policy(k.org, Some(k.ws))
            .await
            .map_err(fail)?;
        let secrets = self.source.secrets(k.org, k.ws).await.map_err(fail)?;
        let (org_slug, ws_slug) = self
            .source
            .slugs(k.ws)
            .await
            .map_err(fail)?
            .ok_or_else(|| ResolveFailure("workspace not found".into()))?;
        let host_port = if k.port == 443 || k.port == 80 {
            k.host.clone()
        } else {
            format!("{}:{}", k.host, k.port)
        };
        let gov = match &self.governance {
            Some(g) => Some(g.effective(k.org).await.map_err(fail)?),
            None => None,
        };
        let is_llm_host = self.llm.is_endpoint_host(&host_port)
            || gov
                .as_ref()
                .is_some_and(|g| g.is_known_llm_host(&host_port));
        let mut cands = Vec::new();
        for r in secrets {
            // Generic secrets are never candidates on a governed LLM host (08 §6).
            if is_llm_host && r.kind != "llm" {
                continue;
            }
            let pat = HostPattern::lenient(&r.host_pattern);
            if !pat.matches(&k.host, k.port) {
                continue;
            }
            match to_candidate(r, k.org, &self.llm) {
                Ok(c) => cands.push(Arc::new(c)),
                Err(e) => tracing::warn!(error = %e, "skipping unusable secret"),
            }
        }
        let registry = self.snapshots.as_ref().and_then(|s| s.current());
        let (mut conns, mut disabled_apps, mut unavailable_apps, mut known_app_host) =
            (Vec::new(), HashSet::new(), HashSet::new(), false);
        let mut host_apps: Vec<String> = Vec::new();
        // Governed LLM hosts never serve app connections (SEC-37).
        if let Some(l) = registry
            .as_ref()
            .filter(|l| !is_llm_host && !l.is_reserved(&k.host, k.port))
        {
            let entries = l.entries_for_host(&k.host, k.port);
            known_app_host = !entries.is_empty();
            let mut app_ids: Vec<String> = entries.iter().map(|e| e.app_id.clone()).collect();
            app_ids.sort();
            app_ids.dedup();
            host_apps = app_ids.clone();
            if !app_ids.is_empty() {
                let rows = self
                    .source
                    .connections(k.org, k.ws, &app_ids)
                    .await
                    .map_err(fail)?;
                let (restricted, available) = self
                    .source
                    .app_availability(k.org, k.ws)
                    .await
                    .map_err(fail)?;
                if restricted {
                    unavailable_apps = app_ids
                        .iter()
                        .filter(|a| !available.contains(a))
                        .cloned()
                        .collect();
                }
                for r in rows {
                    if unavailable_apps.contains(&r.app_id) {
                        continue;
                    }
                    if r.status == "disabled" {
                        disabled_apps.insert(r.app_id);
                        continue;
                    }
                    match ConnCand::from_row(r, k.org) {
                        Ok(c) => conns.push(Arc::new(c)),
                        Err(e) => tracing::warn!(error = %e, "skipping unusable connection"),
                    }
                }
            }
        }
        // An unparsable stored posture fails closed.
        let ws_open_lane =
            aurora_llm::governance::OpenLanePosture::parse(&ws_rows.open_lane_posture)
                .unwrap_or(aurora_llm::governance::OpenLanePosture::Disabled);
        let mut org_policy = to_scope_policy(org_rows).map_err(fail)?;
        if let Some(g) = &gov {
            crate::governance::apply_egress_posture(&mut org_policy, g);
        }
        Ok(ResolvedHost {
            org_policy,
            ws_policy: to_scope_policy(ws_rows).map_err(fail)?,
            secrets: cands,
            org_slug,
            ws_slug,
            is_llm_host,
            governance: gov,
            registry,
            conns,
            disabled_apps,
            unavailable_apps,
            known_app_host,
            host_apps,
            ws_open_lane,
        })
    }
}

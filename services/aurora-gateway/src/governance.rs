//! LLM and egress governance enforcement (docs/06 step 6a, step 11 origin preflight,
//! step 14 origin backstop; docs/08 §11).
//!
//! * [`Governance::effective`] caches the org's effective governance (instance ∩ org) for
//!   60 s, invalidated by `org` / `all` events.
//! * [`apply_egress_posture`] turns the egress posture into the org scope's
//!   uncredentialed default (07 §Rule sources).
//! * [`Governance::check_6a`] is the pure, cache-only governance step.
//! * [`Governance::preflight`] / [`Governance::backstop`] verify key origins.

use crate::invalidation::{Invalidatable, Invalidate, Scope};
use crate::resolve::{Injection, SecretCand};
use async_trait::async_trait;
use aurora_core::limits::{LLM_ORIGIN_CHECK_TIMEOUT, LLM_ORIGIN_RECHECK};
use aurora_core::{ApiError, Secret};
use aurora_db::repo::gateway as repo;
use aurora_llm::governance::{
    EffectiveGovernance, EgressPosture, Instance, LlmPosture, OrgGovernance, OriginStatus,
    StoredOrigin, effective,
};
use aurora_llm::{KeyOrigin, KeyOriginRule, LlmError, ModelRef, ProviderRegistry, origin};
use aurora_policy::eval::{AllowEntry, ScopePolicy, UncredentialedDefault};
use aurora_policy::facts::Level;
use http::{HeaderMap, Method, StatusCode};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const EFFECTIVE_TTL: Duration = Duration::from_secs(60);
const MAX_ENTRIES: usize = 10_000;

/// Where governance state is read from / origin results are written to.
#[async_trait]
pub trait GovSource: Send + Sync + 'static {
    /// Raw `organizations.governance`; `None` = unknown org.
    async fn org_governance(&self, org: Uuid) -> Result<Option<serde_json::Value>, String>;
    /// Persist the origin and announce it (`NOTIFY aurora_secret_origin`).
    async fn write_origin(&self, secret_id: Uuid, origin: serde_json::Value, status: &'static str);
}

pub struct DbGovSource(pub aurora_db::PgPool);

#[async_trait]
impl GovSource for DbGovSource {
    async fn org_governance(&self, org: Uuid) -> Result<Option<serde_json::Value>, String> {
        repo::org_governance(&self.0, org)
            .await
            .map_err(|e| crate::health::db_err(&e))
    }

    async fn write_origin(&self, secret_id: Uuid, origin: serde_json::Value, status: &'static str) {
        if let Err(e) = repo::update_secret_origin(&self.0, secret_id, &origin).await {
            tracing::warn!(%secret_id, error = %e, "origin write-back failed");
            return;
        }
        let payload = json!({ "secret_id": secret_id, "status": status });
        if let Err(e) =
            aurora_db::bus::notify(&self.0, aurora_db::bus::Channel::SecretOrigin, &payload).await
        {
            tracing::warn!(%secret_id, error = %e, "origin notify failed");
        }
    }
}

struct Cached {
    value: Arc<EffectiveGovernance>,
    expires: Instant,
}

#[derive(Default)]
struct Inner {
    map: HashMap<Uuid, Cached>,
    generation: u64,
}

/// `(secret_id, sha256(plaintext))`
type VerKey = (Uuid, [u8; 32]);

pub struct Governance {
    instance: Instance,
    registry: Arc<ProviderRegistry>,
    source: Arc<dyn GovSource>,
    http: reqwest::Client,
    ttl: Duration,
    recheck: Duration,
    check_timeout: Duration,
    cache: Mutex<Inner>,
    verified: Mutex<HashMap<VerKey, Instant>>,
    header_missing: AtomicU64,
}

/// What the response backstop needs, captured at injection time.
#[derive(Debug, Clone)]
pub struct BackstopInfo {
    pub secret_id: Uuid,
    pub endpoint_id: String,
    pub rule: KeyOriginRule,
    pub version: i32,
    pub manage_url: String,
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

impl Governance {
    pub fn new(
        instance: Instance,
        registry: Arc<ProviderRegistry>,
        source: Arc<dyn GovSource>,
    ) -> Self {
        Self {
            instance,
            registry,
            source,
            http: reqwest::Client::new(),
            ttl: EFFECTIVE_TTL,
            recheck: LLM_ORIGIN_RECHECK,
            check_timeout: LLM_ORIGIN_CHECK_TIMEOUT,
            cache: Mutex::new(Inner::default()),
            verified: Mutex::new(HashMap::new()),
            header_missing: AtomicU64::new(0),
        }
    }

    /// Override the origin re-check TTL (tests).
    pub fn with_recheck(mut self, d: Duration) -> Self {
        self.recheck = d;
        self
    }

    /// Responses without origin headers (`gateway_llm_origin_header_missing_total`).
    pub fn header_missing_total(&self) -> u64 {
        self.header_missing.load(Ordering::Relaxed)
    }

    /// Effective governance of an org; any failure (including a malformed document) is an
    /// error so the caller fails closed.
    pub async fn effective(&self, org: Uuid) -> Result<Arc<EffectiveGovernance>, String> {
        let gen_seen = {
            let mut g = self.cache.lock().map_err(|_| "governance cache poisoned")?;
            if let Some(c) = g.map.get(&org) {
                if c.expires > Instant::now() {
                    return Ok(c.value.clone());
                }
                g.map.remove(&org);
            }
            g.generation
        };
        let raw = self
            .source
            .org_governance(org)
            .await?
            .ok_or("organization not found")?;
        let doc: OrgGovernance = aurora_llm::governance::from_stored(&raw, &self.instance)
            .map_err(|e| format!("governance document: {e}"))?;
        let value = Arc::new(effective(&self.instance, &doc));
        if let Ok(mut g) = self.cache.lock()
            && g.generation == gen_seen
        {
            if g.map.len() >= MAX_ENTRIES {
                g.map.clear();
            }
            g.map.insert(
                org,
                Cached {
                    value: value.clone(),
                    expires: Instant::now() + self.ttl,
                },
            );
        }
        Ok(value)
    }

    /// Step 6a. `host` is `host:port`. `body` is only consulted when a model allow-list applies.
    #[allow(clippy::too_many_arguments)]
    pub fn check_6a(
        &self,
        gov: &EffectiveGovernance,
        host: &str,
        method: &Method,
        path: &str,
        body: &[u8],
        truncated: bool,
        winner: Option<&SecretCand>,
        manage_url: &str,
    ) -> Result<(), ApiError> {
        let not_permitted = |h: &str| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "llm_provider_not_permitted",
                format!(
                    "Requests to the LLM provider {h} are not permitted for this organization."
                ),
            )
            .with_details(json!({ "host": h }))
        };
        let Some(ep_id) = gov.endpoint_for_host(host) else {
            // Known LLM host Aurora has no configured endpoint for.
            if gov.is_known_llm_host(host) && gov.llm_posture == LlmPosture::SanctionedOnly {
                return Err(not_permitted(host));
            }
            return Ok(());
        };
        let permitted = gov.endpoints.get(ep_id).is_some_and(|e| e.permitted);
        if !permitted {
            return Err(not_permitted(host));
        }
        let llm_winner = winner.filter(|c| matches!(c.injection, Injection::Llm(_)));
        let Some(c) = llm_winner else {
            if gov.llm_posture == LlmPosture::SanctionedOnly {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "llm_key_required",
                    format!(
                        "This organization only allows LLM requests that use a key stored in Aurora. Store or request a key for {ep_id} at {manage_url}"
                    ),
                )
                .with_details(json!({ "endpoint_id": ep_id, "manage_url": manage_url })));
            }
            return Ok(());
        };
        let scope = if c.scope == Level::Workspace {
            "workspace"
        } else {
            "organization"
        };
        if c.llm_endpoint.as_deref().is_some_and(|e| e != ep_id) || !gov.key_permitted(ep_id, scope)
        {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "llm_key_not_permitted",
                format!(
                    "This key's scope ({scope}) is not permitted for the LLM endpoint {ep_id}."
                ),
            )
            .with_details(json!({ "secret_id": c.id })));
        }
        let stored = c.origin.clone().unwrap_or(serde_json::Value::Null);
        if !gov.origin_complies(ep_id, &stored, i64::from(c.version)) {
            return Err(origin_refusal(
                c.id,
                classify(&stored, c.version),
                manage_url,
            ));
        }
        let allowed = gov
            .endpoints
            .get(ep_id)
            .is_some_and(|e| !e.allowed_models.is_empty());
        if allowed {
            let model = self
                .registry
                .endpoint(ep_id)
                .and_then(|ep| {
                    self.registry
                        .provider_for(ep)
                        .map(|p| p.model_from_request(ep, method, path, body, truncated))
                })
                .unwrap_or(ModelRef::Unknown);
            if let Err(list) = gov.model_allowed(ep_id, &model) {
                let name = match &model {
                    ModelRef::Model(m) => Some(m.as_str()),
                    _ => None,
                };
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "llm_model_not_allowed",
                    match name {
                        Some(m) => format!("The model `{m}` is not allowed. Allowed models: {}", list.join(", ")),
                        None => format!(
                            "The requested model could not be determined, so the request is refused. Allowed models: {}",
                            list.join(", ")
                        ),
                    },
                )
                .with_details(json!({ "model": name, "allowed_models": list })));
            }
        }
        Ok(())
    }

    /// Whether step 6a needs the request body prefix for this winner.
    pub fn needs_body(gov: &EffectiveGovernance, host: &str, winner: Option<&SecretCand>) -> bool {
        winner.is_some_and(|c| matches!(c.injection, Injection::Llm(_)))
            && gov
                .endpoint_for_host(host)
                .and_then(|id| gov.endpoints.get(id))
                .is_some_and(|e| e.permitted && !e.allowed_models.is_empty())
    }

    /// Open lane: nothing can be injected, so governed LLM hosts are refused when the
    /// endpoint is not permitted or the posture is `sanctioned_only`.
    pub fn check_open_lane(gov: &EffectiveGovernance, host: &str) -> Result<(), ApiError> {
        let refuse = || {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "llm_provider_not_permitted",
                format!("Direct tunnels to the LLM provider {host} are not permitted for this organization. Use the default proxy lane."),
            )
            .with_details(json!({ "host": host }))
        };
        match gov.endpoint_for_host(host) {
            Some(id) => {
                let permitted = gov.endpoints.get(id).is_some_and(|e| e.permitted);
                if !permitted || gov.llm_posture == LlmPosture::SanctionedOnly {
                    return Err(refuse());
                }
            }
            None => {
                if gov.is_known_llm_host(host) && gov.llm_posture == LlmPosture::SanctionedOnly {
                    return Err(refuse());
                }
            }
        }
        Ok(())
    }

    /// The backstop description for an LLM key about to be injected, if its endpoint has a
    /// key-origin rule.
    pub fn backstop_info(
        gov: &EffectiveGovernance,
        host: &str,
        c: &SecretCand,
        manage_url: &str,
    ) -> Option<BackstopInfo> {
        if !matches!(c.injection, Injection::Llm(_)) {
            return None;
        }
        let ep = gov.endpoint_for_host(host)?;
        let rule = gov.endpoints.get(ep)?.origin_rule.clone()?;
        Some(BackstopInfo {
            secret_id: c.id,
            endpoint_id: ep.to_owned(),
            rule,
            version: c.version,
            manage_url: manage_url.to_owned(),
        })
    }

    /// Step 11 preflight: verify an unseen key value with the provider before injecting it.
    pub async fn preflight(&self, b: &BackstopInfo, plaintext: &[u8]) -> Result<(), ApiError> {
        let key = (b.secret_id, sha(plaintext));
        if let Ok(mut v) = self.verified.lock() {
            match v.get(&key) {
                Some(t) if t.elapsed() < self.recheck => return Ok(()),
                Some(_) => {
                    v.remove(&key);
                }
                None => {}
            }
        }
        let unavailable = || {
            ApiError::unavailable(
                "llm_origin_check_unavailable",
                "The LLM provider could not be reached to verify the key's account. Retry shortly.",
            )
            .with_details(json!({ "endpoint_id": b.endpoint_id }))
        };
        let (Some(ep), Ok(text)) = (
            self.registry.endpoint(&b.endpoint_id),
            std::str::from_utf8(plaintext),
        ) else {
            return Err(self.mark(b, None, "unverifiable"));
        };
        let Some(provider) = self.registry.provider_for(ep) else {
            return Err(unavailable());
        };
        let secret = Secret::new(text.to_owned());
        let fut = provider.verify_key_origin(ep, b.rule.method, &secret, &self.http);
        let res = match tokio::time::timeout(self.check_timeout, fut).await {
            Ok(r) => r,
            Err(_) => return Err(unavailable()),
        };
        match res {
            Ok(o) if origin::complies(&o, &b.rule) => {
                if let Ok(mut v) = self.verified.lock() {
                    if v.len() >= MAX_ENTRIES {
                        v.clear();
                    }
                    v.insert(key, Instant::now());
                }
                Ok(())
            }
            Ok(o) => Err(self.mark(b, Some(&o), "mismatch")),
            Err(LlmError::Unreachable(_) | LlmError::Status(_)) => Err(unavailable()),
            Err(_) => Err(self.mark(b, None, "unverifiable")),
        }
    }

    /// Step 14 backstop: `Some(refusal)` when the provider-reported account does not comply.
    pub fn backstop(&self, b: &BackstopInfo, headers: &HeaderMap) -> Option<ApiError> {
        let ep = self.registry.endpoint(&b.endpoint_id)?;
        let provider = self.registry.provider_for(ep)?;
        match provider.origin_from_response(ep, headers) {
            None => {
                self.header_missing.fetch_add(1, Ordering::Relaxed);
                None
            }
            Some(o) if origin::complies(&o, &b.rule) => None,
            Some(o) => Some(self.mark(b, Some(&o), "mismatch")),
        }
    }

    /// Record a non-compliant result (write-back + notify) and build the refusal.
    fn mark(&self, b: &BackstopInfo, o: Option<&KeyOrigin>, status: &'static str) -> ApiError {
        let stored = StoredOrigin {
            status: if status == "mismatch" {
                OriginStatus::Mismatch
            } else {
                OriginStatus::Unverifiable
            },
            method: Some(b.rule.method.as_str().to_owned()),
            organization_id: o.and_then(|o| o.organization_id.clone()),
            workspace_id: o.and_then(|o| o.workspace_id.clone()),
            team_id: o.and_then(|o| o.team_id.clone()),
            user_id: o.and_then(|o| o.user_id.clone()),
            value_version: i64::from(b.version),
            checked_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        if let Ok(v) = serde_json::to_value(&stored) {
            let src = self.source.clone();
            let id = b.secret_id;
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    h.spawn(async move { src.write_origin(id, v, status).await });
                }
                Err(_) => tracing::warn!("no runtime for origin write-back"),
            }
        }
        origin_refusal(
            b.secret_id,
            if status == "mismatch" {
                OriginFail::Wrong
            } else {
                OriginFail::Unverifiable
            },
            &b.manage_url,
        )
    }
}

#[derive(Clone, Copy)]
enum OriginFail {
    Wrong,
    Unverifiable,
}

fn classify(stored: &serde_json::Value, version: i32) -> OriginFail {
    match serde_json::from_value::<StoredOrigin>(stored.clone()) {
        Ok(o)
            if o.status == OriginStatus::Mismatch
                || (o.status == OriginStatus::Verified
                    && o.value_version == i64::from(version)) =>
        {
            OriginFail::Wrong
        }
        _ => OriginFail::Unverifiable,
    }
}

fn origin_refusal(secret_id: Uuid, f: OriginFail, manage_url: &str) -> ApiError {
    match f {
        OriginFail::Wrong => ApiError::new(
            StatusCode::FORBIDDEN,
            "llm_key_wrong_organization",
            format!("This key belongs to an account the organization's rules do not allow. Use a different key: {manage_url}"),
        )
        .with_details(json!({ "secret_id": secret_id, "manage_url": manage_url })),
        OriginFail::Unverifiable => ApiError::new(
            StatusCode::FORBIDDEN,
            "llm_key_unverifiable",
            "The account this key belongs to could not be verified, so it was not used.",
        )
        .with_details(json!({ "secret_id": secret_id })),
    }
}

impl Invalidatable for Governance {
    fn invalidate(&self, ev: &Invalidate) {
        match ev.scope {
            Scope::Org(id) => {
                if let Ok(mut g) = self.cache.lock() {
                    g.generation += 1;
                    g.map.remove(&id);
                }
            }
            Scope::All => self.flush_all(),
            Scope::Workspace(_) => {
                // Origin verifications are keyed by secret only; drop them all.
                if let Ok(mut v) = self.verified.lock() {
                    v.clear();
                }
            }
            _ => {}
        }
        if matches!(ev.scope, Scope::Org(_))
            && let Ok(mut v) = self.verified.lock()
        {
            v.clear();
        }
    }

    fn flush_all(&self) {
        if let Ok(mut g) = self.cache.lock() {
            g.generation += 1;
            g.map.clear();
        }
        if let Ok(mut v) = self.verified.lock() {
            v.clear();
        }
    }
}

/// Org scope uncredentialed default from the effective egress posture (07 §Rule sources).
pub fn apply_egress_posture(p: &mut ScopePolicy, gov: &EffectiveGovernance) {
    if gov.egress_posture == EgressPosture::CredentialedOnly {
        p.uncredentialed_default = UncredentialedDefault::Block;
        p.uncredentialed_allowlist = gov
            .egress_allowlist
            .iter()
            .map(|a| AllowEntry {
                host: a.host.clone(),
                path: a.path.clone(),
            })
            .collect();
    }
}

/// Instance ceiling from the validated gateway configuration.
pub fn instance_from_config(
    g: &aurora_config::Governance,
    registry: &ProviderRegistry,
) -> aurora_llm::governance::Instance {
    use aurora_llm::governance::{AllowEntry as Entry, InstanceEndpoint};
    Instance {
        allowed_key_scopes: g.allowed_key_scopes.clone(),
        llm_minimum_posture: match g.llm_minimum_posture {
            aurora_config::LlmPosture::Open => LlmPosture::Open,
            aurora_config::LlmPosture::SanctionedOnly => LlmPosture::SanctionedOnly,
        },
        egress_minimum_posture: match g.egress_minimum_posture {
            aurora_config::EgressPosture::Open => EgressPosture::Open,
            aurora_config::EgressPosture::CredentialedOnly => EgressPosture::CredentialedOnly,
        },
        open_lane_minimum: g.open_lane_minimum.into(),
        byo: aurora_llm::governance::ByoInstance::from_config(&g.byo),
        require_key_origin: g.require_key_origin,
        endpoints: registry
            .endpoints()
            .iter()
            .map(|ep| {
                let p = registry.provider_for(ep);
                InstanceEndpoint {
                    id: ep.id.clone(),
                    allowed_models: ep.allowed_models.clone(),
                    key_origin: ep.key_origin.clone(),
                    hosts: p.map(|p| p.hosts(ep)).unwrap_or_default(),
                    origin_methods: p.map(|p| p.origin_methods().to_vec()).unwrap_or_default(),
                }
            })
            .collect(),
        known_llm_hosts: g.known_llm_hosts.clone(),
        baseline_allowlist: g
            .baseline_allowlist
            .iter()
            .map(|e| Entry {
                host: e.host.clone(),
                path: e.path.clone(),
            })
            .collect(),
    }
}

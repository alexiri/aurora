//! Model selection for hosted agents (docs/08 §Model selection for hosted agents).

use crate::app::AppState;
use crate::domains::secrets::gov;
use aurora_core::ApiError;
use aurora_db::repo::hosted::{self, LlmCandidate};
use aurora_llm::governance::{EffectiveGovernance, OriginStatus, StoredOrigin};
use aurora_llm::{AuthMode, Dialect, Endpoint, ModelRef};
use aurora_protocol::runner::ModelChoice;
use sqlx::PgConnection;
use uuid::Uuid;

/// Dialects each harness speaks, **in preference order** (`12 §Harness selection and adapter
/// contract`, declared by the adapters): the dialect used is the first one the endpoint serves.
pub fn harness_dialects(harness: &str) -> &'static [Dialect] {
    match harness {
        // The pinned Codex CLI (0.167.0) dropped `wire_api = "chat"`: a version that drops a
        // dialect removes it here (`12 §Harness selection`, D-153).
        "codex" => &[Dialect::OpenaiResponses],
        _ => &[Dialect::AnthropicMessages],
    }
}

/// The dialect used: the **first of the harness's own list** that the endpoint also serves
/// (`12 §Harness selection`, `08 §Model selection`); `None` = the key is `no_shared_dialect`.
pub fn first_shared(harness: &str, endpoint: &[Dialect]) -> Option<Dialect> {
    harness_dialects(harness)
        .iter()
        .copied()
        .find(|d| endpoint.contains(d))
}

#[cfg(test)]
mod dialect_tests {
    use super::*;

    #[test]
    fn dialect_is_the_first_of_the_harness_list_the_endpoint_serves() {
        use Dialect::{AnthropicMessages as A, OpenaiChat as C, OpenaiResponses as R};
        // LiteLLM serves all three (chat listed first): codex still gets `responses`.
        assert_eq!(first_shared("codex", &[C, R, A]), Some(R));
        assert_eq!(first_shared("claude-code", &[C, R, A]), Some(A));
        // Chat-only and anthropic-only endpoints do not serve the pinned Codex.
        assert_eq!(first_shared("codex", &[C]), None);
        assert_eq!(first_shared("codex", &[A]), None);
        assert_eq!(first_shared("claude-code", &[C, R]), None);
        assert_eq!(first_shared("codex", &[]), None);
    }

    #[test]
    fn registry_names_every_known_harness_and_dialects_are_declared() {
        for (id, name) in HARNESSES {
            assert!(!name.is_empty() && !harness_dialects(id).is_empty(), "{id}");
        }
        assert_eq!(harness_dialects("codex"), &[Dialect::OpenaiResponses]);
    }
}

/// Registry of the harnesses this build knows, `(id, display name)`; `hosted.enabled_harnesses`
/// selects among them (`fake` only with `hosted.allow_fake_harness`).
pub const HARNESSES: [(&str, &str); 3] = [
    ("claude-code", "Claude Code"),
    ("codex", "OpenAI Codex"),
    ("fake", "Scripted test harness"),
];

/// Harness ids agents may use on this instance: `hosted.enabled_harnesses`, plus `fake` when it
/// is allowed (tests and dev).
pub fn allowed_harnesses(state: &AppState) -> Vec<String> {
    let h = &state.config.hosted;
    let mut v = h.enabled_harnesses.clone();
    if h.allow_fake_harness && !v.iter().any(|x| x == "fake") {
        v.push("fake".into());
    }
    v
}

/// A usable granted key together with its endpoint.
pub struct Usable<'a> {
    pub secret: LlmCandidate,
    pub ep: &'a Endpoint,
    /// First dialect both the harness and the endpoint speak.
    pub dialect: Dialect,
}

pub enum Selection {
    Chosen {
        choice: ModelChoice,
        /// Platform notices for the next turn (e.g. a dropped model override).
        notices: Vec<String>,
    },
    /// No usable key. The message lists reason counts, never key names (C-501).
    NoKey { message: String },
    /// A usable key but no model resolves (D-136).
    NoDefaultModel,
}

impl Selection {
    /// `(error_code, message)` of the turn failure this selection implies, if any.
    pub fn failure(&self) -> Option<(&'static str, String)> {
        match self {
            Self::Chosen { .. } => None,
            Self::NoKey { message } => Some(("no_model_key", message.clone())),
            Self::NoDefaultModel => Some(("no_default_model", NO_DEFAULT_MODEL_MSG.to_owned())),
        }
    }
}

pub const NO_DEFAULT_MODEL_MSG: &str = "An LLM key is usable but no model is chosen for this agent \
                                        and its endpoint has no default model.";

/// Why a granted key is not usable, in the `08 §Model readiness` check order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NotActive,
    EndpointUnknown,
    ProviderDisabled,
    ScopeNotPermitted,
    OriginUnverified,
    OriginNotCompliant,
    NoSharedDialect,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotActive => "not_active",
            Self::EndpointUnknown => "endpoint_unknown",
            Self::ProviderDisabled => "provider_disabled",
            Self::ScopeNotPermitted => "scope_not_permitted",
            Self::OriginUnverified => "origin_unverified",
            Self::OriginNotCompliant => "origin_not_compliant",
            Self::NoSharedDialect => "no_shared_dialect",
        }
    }
}

/// One granted key with its readiness. `reason = None` means usable.
pub struct KeyState<'a> {
    pub secret: LlmCandidate,
    pub ep: Option<&'a Endpoint>,
    pub reason: Option<Reason>,
    /// First dialect both the harness and the endpoint speak (set when usable).
    pub dialect: Option<Dialect>,
}

/// Where the default model came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Override,
    EndpointDefault,
    ProviderDefault,
    AllowList,
    StaticList,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::EndpointDefault => "endpoint_default",
            Self::ProviderDefault => "provider_default",
            Self::AllowList => "allow_list",
            Self::StaticList => "static_list",
        }
    }
}

/// What the next turn would use without further input.
pub struct Resolved {
    /// Index into [`Readiness::keys`] of the key serving the model.
    pub key: usize,
    pub model: String,
    pub source: Source,
}

/// Every granted key's readiness plus the default selection (D-136). The dispatcher and
/// `GET /v1/agents/{agent}/model-readiness` both read this.
pub struct Readiness<'a> {
    /// In `provider_preference` order, oldest first within a provider.
    pub keys: Vec<KeyState<'a>>,
    pub default: Option<Resolved>,
    /// The agent has a model override that is not usable (dropped, with a notice, next turn).
    pub override_dropped: Option<String>,
    pub effort: String,
}

impl Readiness<'_> {
    pub fn any_usable(&self) -> bool {
        self.keys.iter().any(|k| k.reason.is_none())
    }

    /// `No LLM key is granted…` or `No usable LLM key: 1 granted key not usable (reason)`.
    /// Reason counts only; never key names (participants who are not owners read it).
    pub fn no_key_message(&self) -> String {
        if self.keys.is_empty() {
            return "No LLM key is granted to this agent.".to_owned();
        }
        let n = self.keys.len();
        let order = [
            Reason::NotActive,
            Reason::EndpointUnknown,
            Reason::ProviderDisabled,
            Reason::ScopeNotPermitted,
            Reason::OriginUnverified,
            Reason::OriginNotCompliant,
            Reason::NoSharedDialect,
        ];
        let parts: Vec<String> = order
            .iter()
            .filter_map(|r| {
                let c = self.keys.iter().filter(|k| k.reason == Some(*r)).count();
                match c {
                    0 => None,
                    1 => Some(r.as_str().to_owned()),
                    c => Some(format!("{} ×{c}", r.as_str())),
                }
            })
            .collect();
        format!(
            "No usable LLM key: {n} granted key{} not usable ({}).",
            if n == 1 { "" } else { "s" },
            parts.join(", ")
        )
    }
}

/// Readiness of one granted key, first failing check in the `08` order.
fn key_reason<'a>(
    state: &'a AppState,
    eff: &EffectiveGovernance,
    harness: &str,
    c: &LlmCandidate,
) -> (Option<&'a Endpoint>, Option<Reason>, Option<Dialect>) {
    let ep = state.llm.endpoint(&c.llm_endpoint_id);
    if c.status != "active" {
        return (ep, Some(Reason::NotActive), None);
    }
    let Some(ep) = ep else {
        return (None, Some(Reason::EndpointUnknown), None);
    };
    // The key's provider is no longer the one this endpoint serves, or is not enabled.
    let provider = state
        .llm
        .provider_for(ep)
        .filter(|_| ep.provider == c.llm_provider)
        .filter(|_| state.config.llm.enabled_providers.contains(&c.llm_provider));
    let Some(provider) = provider else {
        return (Some(ep), Some(Reason::ProviderDisabled), None);
    };
    let scope = if c.workspace_id.is_some() {
        "workspace"
    } else {
        "organization"
    };
    if !eff.key_permitted(&ep.id, scope) {
        return (Some(ep), Some(Reason::ScopeNotPermitted), None);
    }
    if eff
        .endpoints
        .get(&ep.id)
        .is_some_and(|e| e.origin_rule.is_some())
    {
        let origin = c.origin.clone().unwrap_or(serde_json::Value::Null);
        let stored = serde_json::from_value::<StoredOrigin>(origin.clone()).ok();
        // A stored `mismatch` is a verified-but-wrong account (governance re-evaluation or a
        // re-verify), which the owner fixes with another key, not with Re-verify.
        let judged = stored.as_ref().is_some_and(|o| {
            o.value_version == i64::from(c.value_version)
                && matches!(o.status, OriginStatus::Verified | OriginStatus::Mismatch)
        });
        if !judged {
            return (Some(ep), Some(Reason::OriginUnverified), None);
        }
        if !eff.origin_complies(&ep.id, &origin, i64::from(c.value_version)) {
            return (Some(ep), Some(Reason::OriginNotCompliant), None);
        }
    }
    let eps = provider.dialects(ep);
    let Some(dialect) = first_shared(harness, &eps) else {
        return (Some(ep), Some(Reason::NoSharedDialect), None);
    };
    (Some(ep), None, Some(dialect))
}

/// Usable keys in `provider_preference` order, for the model pickers.
pub fn usable<'a>(
    state: &'a AppState,
    eff: &EffectiveGovernance,
    harness: &str,
    cands: Vec<LlmCandidate>,
) -> Vec<Usable<'a>> {
    keys_of(state, eff, harness, cands)
        .into_iter()
        .filter_map(|k| match (k.reason, k.ep, k.dialect) {
            (None, Some(ep), Some(dialect)) => Some(Usable {
                secret: k.secret,
                ep,
                dialect,
            }),
            _ => None,
        })
        .collect()
}

fn keys_of<'a>(
    state: &'a AppState,
    eff: &EffectiveGovernance,
    harness: &str,
    cands: Vec<LlmCandidate>,
) -> Vec<KeyState<'a>> {
    let pref = &state.config.llm.provider_preference;
    let mut out: Vec<(usize, KeyState<'a>)> = cands
        .into_iter()
        .map(|c| {
            let (ep, reason, dialect) = key_reason(state, eff, harness, &c);
            let rank = ep
                .and_then(|e| pref.iter().position(|p| *p == e.provider))
                .unwrap_or(usize::MAX);
            (
                rank,
                KeyState {
                    secret: c,
                    ep,
                    reason,
                    dialect,
                },
            )
        })
        .collect();
    out.sort_by_key(|(r, _)| *r); // stable: oldest key first within a provider
    out.into_iter().map(|(_, k)| k).collect()
}

fn model_ok(eff: &EffectiveGovernance, ep: &str, model: &str) -> bool {
    eff.model_allowed(ep, &ModelRef::Model(model.to_owned()))
        .is_ok()
}

/// The endpoint's default model (or the first allowed one when the default is not allowed),
/// with where it came from.
fn default_model(
    state: &AppState,
    eff: &EffectiveGovernance,
    ep: &Endpoint,
) -> Option<(String, Source)> {
    let provider = state.llm.provider_for(ep)?;
    let allowed = eff
        .endpoints
        .get(&ep.id)
        .map(|e| e.allowed_models.clone())
        .unwrap_or_default();
    let ok = |m: &String| allowed.is_empty() || allowed.contains(m);
    if let Some(m) = ep.default_model.clone().filter(ok) {
        return Some((m, Source::EndpointDefault));
    }
    if let Some(m) = provider.default_model(ep).filter(ok) {
        return Some((m, Source::ProviderDefault));
    }
    if let Some(m) = allowed.first() {
        return Some((m.clone(), Source::AllowList));
    }
    ep.models.first().map(|m| (m.clone(), Source::StaticList))
}

/// The one readiness function (D-136, `08 §Model readiness`): every granted key's `usable`
/// or first failing reason, plus the default selection with its source. Keeps the selection
/// semantics: the override first, never re-pointed, dropped when no longer usable.
pub async fn readiness<'a>(
    state: &'a AppState,
    conn: &mut PgConnection,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
    harness: &str,
) -> Result<Readiness<'a>, ApiError> {
    let eff = gov::effective_for(state, org).await?;
    let cands = hosted::granted_llm_secrets(conn, agent, org, workspace).await?;
    let (prov, epid, mid, effort) = hosted::model_prefs(conn, agent).await?;
    let effort = effort.unwrap_or_else(|| "medium".to_owned());
    let keys = keys_of(state, &eff, harness, cands);
    let mut override_dropped = None;
    let mut default = None;
    if let (Some(p), Some(e), Some(m)) = (prov, epid, mid) {
        // An explicit model is stamped with provider + endpoint; never re-pointed.
        let hit = keys.iter().position(|k| {
            k.reason.is_none() && k.ep.is_some_and(|ep| ep.provider == p && ep.id == e)
        });
        match hit {
            Some(i) if model_ok(&eff, &e, &m) => {
                default = Some(Resolved {
                    key: i,
                    model: m,
                    source: Source::Override,
                });
            }
            _ => override_dropped = Some(m),
        }
    }
    if default.is_none() {
        for (i, k) in keys.iter().enumerate() {
            let (None, Some(ep)) = (k.reason, k.ep) else {
                continue;
            };
            if let Some((model, source)) = default_model(state, &eff, ep) {
                default = Some(Resolved {
                    key: i,
                    model,
                    source,
                });
                break;
            }
        }
    }
    Ok(Readiness {
        keys,
        default,
        override_dropped,
        effort,
    })
}

fn choice(
    state: &AppState,
    ep: &Endpoint,
    dialect: Dialect,
    model: String,
    effort: &str,
) -> ModelChoice {
    let client_env = state
        .llm
        .provider_for(ep)
        .map(|p| p.client_env(ep, dialect, &AuthMode::ApiKey))
        .unwrap_or_default();
    ModelChoice {
        provider: ep.provider.clone(),
        endpoint_id: ep.id.clone(),
        base_url: ep.base_url.clone(),
        dialect: dialect.as_str().to_owned(),
        model,
        effort: effort.to_owned(),
        client_env,
    }
}

/// Pick the model for an agent's next turn.
pub async fn select(
    state: &AppState,
    conn: &mut PgConnection,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
    harness: &str,
) -> Result<Selection, ApiError> {
    // The scripted `fake` harness talks to no LLM: no key, no model (12 §Adapter fake).
    if harness == "fake" && state.config.hosted.allow_fake_harness {
        return Ok(Selection::Chosen {
            choice: ModelChoice {
                provider: "fake".into(),
                endpoint_id: "fake".into(),
                base_url: String::new(),
                dialect: Dialect::AnthropicMessages.as_str().into(),
                model: "fake".into(),
                effort: "medium".into(),
                client_env: vec![],
            },
            notices: vec![],
        });
    }
    let r = readiness(state, conn, agent, org, workspace, harness).await?;
    if !r.any_usable() {
        return Ok(Selection::NoKey {
            message: r.no_key_message(),
        });
    }
    let Some(d) = &r.default else {
        return Ok(Selection::NoDefaultModel);
    };
    let k = &r.keys[d.key];
    let (Some(ep), Some(dialect)) = (k.ep, k.dialect) else {
        return Ok(Selection::NoDefaultModel);
    };
    let mut notices = Vec::new();
    if let Some(m) = &r.override_dropped {
        notices.push(format!(
            "The model override `{m}` is no longer usable (key, endpoint or model not permitted); \
             the default model is used instead."
        ));
    }
    Ok(Selection::Chosen {
        choice: choice(state, ep, dialect, d.model.clone(), &r.effort),
        notices,
    })
}

//! Model selection for hosted agents (docs/08 §Model selection for hosted agents).

use crate::app::AppState;
use crate::domains::secrets::gov;
use aurora_core::ApiError;
use aurora_db::repo::hosted::{self, LlmCandidate};
use aurora_llm::governance::EffectiveGovernance;
use aurora_llm::{AuthMode, Dialect, Endpoint, ModelRef};
use aurora_protocol::runner::ModelChoice;
use sqlx::PgConnection;
use uuid::Uuid;

/// Dialects each harness speaks (`12 §Harness interface`, declared by the adapters).
pub fn harness_dialects(_harness: &str) -> &'static [Dialect] {
    &[Dialect::AnthropicMessages]
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
    NoKey,
}

/// Usable keys in `provider_preference` order: granted, active, governance-admitted, origin
/// compliant, with a dialect the harness speaks.
pub fn usable<'a>(
    state: &'a AppState,
    eff: &EffectiveGovernance,
    harness: &str,
    cands: Vec<LlmCandidate>,
) -> Vec<Usable<'a>> {
    let pref = &state.config.llm.provider_preference;
    let mut out: Vec<(usize, Usable<'a>)> = Vec::new();
    for c in cands {
        let Some(ep) = state.llm.endpoint(&c.llm_endpoint_id) else {
            continue;
        };
        let Some(provider) = state.llm.provider_for(ep) else {
            continue;
        };
        if ep.provider != c.llm_provider {
            continue;
        }
        let scope = if c.workspace_id.is_some() {
            "workspace"
        } else {
            "organization"
        };
        if !eff.key_permitted(&ep.id, scope) {
            continue;
        }
        let origin = c.origin.clone().unwrap_or(serde_json::Value::Null);
        if !eff.origin_complies(&ep.id, &origin, i64::from(c.value_version)) {
            continue;
        }
        let eps = provider.dialects(ep);
        let Some(dialect) = harness_dialects(harness)
            .iter()
            .copied()
            .find(|d| eps.contains(d))
        else {
            continue;
        };
        let rank = pref
            .iter()
            .position(|p| *p == ep.provider)
            .unwrap_or(usize::MAX);
        out.push((
            rank,
            Usable {
                secret: c,
                ep,
                dialect,
            },
        ));
    }
    out.sort_by_key(|(r, _)| *r); // stable: oldest key first within a provider
    out.into_iter().map(|(_, u)| u).collect()
}

fn model_ok(eff: &EffectiveGovernance, ep: &str, model: &str) -> bool {
    eff.model_allowed(ep, &ModelRef::Model(model.to_owned()))
        .is_ok()
}

/// The endpoint's default model, or the first allowed one when the default is not allowed.
fn default_model(state: &AppState, eff: &EffectiveGovernance, u: &Usable<'_>) -> Option<String> {
    let provider = state.llm.provider_for(u.ep)?;
    let allowed = eff
        .endpoints
        .get(&u.ep.id)
        .map(|e| e.allowed_models.clone())
        .unwrap_or_default();
    let d =
        u.ep.default_model
            .clone()
            .or_else(|| provider.default_model(u.ep))
            .filter(|m| allowed.is_empty() || allowed.contains(m));
    d.or_else(|| allowed.first().cloned())
        .or_else(|| u.ep.models.first().cloned())
}

fn choice(state: &AppState, u: &Usable<'_>, model: String, effort: &str) -> ModelChoice {
    let client_env = state
        .llm
        .provider_for(u.ep)
        .map(|p| p.client_env(u.ep, u.dialect, &AuthMode::ApiKey))
        .unwrap_or_default();
    ModelChoice {
        provider: u.ep.provider.clone(),
        endpoint_id: u.ep.id.clone(),
        base_url: u.ep.base_url.clone(),
        dialect: u.dialect.as_str().to_owned(),
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
    let eff = gov::effective_for(state, org).await?;
    let cands = hosted::granted_llm_secrets(conn, agent, org, workspace).await?;
    let (prov, epid, mid, effort) = hosted::model_prefs(conn, agent).await?;
    let effort = effort.unwrap_or_else(|| "medium".to_owned());
    let list = usable(state, &eff, harness, cands);
    let mut notices = Vec::new();
    if let (Some(p), Some(e), Some(m)) = (prov, epid, mid) {
        // An explicit model is stamped with provider + endpoint; never re-pointed.
        if let Some(u) = list
            .iter()
            .find(|u| u.ep.provider == p && u.ep.id == e)
            .filter(|u| model_ok(&eff, &u.ep.id, &m))
        {
            return Ok(Selection::Chosen {
                choice: choice(state, u, m, &effort),
                notices,
            });
        }
        notices.push(format!(
            "The model override `{m}` is no longer usable (key, endpoint or model not permitted); \
             the default model is used instead."
        ));
    }
    for u in &list {
        if let Some(m) = default_model(state, &eff, u) {
            return Ok(Selection::Chosen {
                choice: choice(state, u, m, &effort),
                notices,
            });
        }
    }
    Ok(Selection::NoKey)
}

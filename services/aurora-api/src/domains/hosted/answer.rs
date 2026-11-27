//! `GET /v1/agents/{agent}/answer-readiness` (C-1062, D-414, D-408): can this agent answer, and
//! what fixes it **for the caller**. Visible to everyone who can use the agent (members see the
//! same reason owners do); key names only reach people who can own the workspace. The key logic
//! is [`super::model::readiness`] (the dispatcher's own function), never a copy.

use super::model;
use crate::app::AppState;
use crate::domains::conversations::service::load_agent;
use crate::domains::placement::service as placement;
use crate::domains::secrets::gov;
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use crate::http::who_can::{WhoCan, WhoCanRole};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::placement as placement_repo;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// Why an agent cannot answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnswerReason {
    /// No AI key is granted to the agent.
    NoAiKey,
    /// Keys are granted but none is usable now (origin, provider, dialect).
    KeyNotUsable,
    /// A usable key serves no allowed model (D-372).
    NoModel,
}

/// The key `give_key` offers. The name is shown only to people who can own the workspace.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct FixKey {
    pub secret_id: Uuid,
    pub name: String,
    /// `organization` | `workspace`.
    pub scope: String,
}

/// What fixes it, chosen for the caller.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AnswerFix {
    /// Give this agent a key that already exists (the caller can own the workspace).
    GiveKey { key: FixKey },
    /// Add the organization's first AI key (the caller administers the organization).
    AddKey,
    /// Someone else must: the roles that can (never names).
    AskAdmin { who_can: WhoCan },
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AnswerReadiness {
    pub can_answer: bool,
    pub reason: Option<AnswerReason>,
    /// Granted keys the dispatcher can use now.
    pub usable_keys: u32,
    pub fix: Option<AnswerFix>,
}

/// The reason from the dispatcher's facts: keys granted, how many usable, whether a default
/// model resolves. `None` = it can answer.
pub fn decide_reason(granted: usize, usable: usize, has_default: bool) -> Option<AnswerReason> {
    if granted == 0 {
        Some(AnswerReason::NoAiKey)
    } else if usable == 0 {
        Some(AnswerReason::KeyNotUsable)
    } else if !has_default {
        Some(AnswerReason::NoModel)
    } else {
        None
    }
}

/// (P5-REFINE-6, C-1090, D-425) Why a hosted agent cannot answer, without the caller's fix: the
/// same facts as [`compute`] (the dispatcher's [`model::readiness`]; key listings are cached per
/// endpoint, so a list fetches each endpoint's at most once). `None` = it can answer.
pub async fn reason_of(
    s: &AppState,
    conn: &mut sqlx::PgConnection,
    agent: uuid::Uuid,
    org: uuid::Uuid,
    workspace: uuid::Uuid,
    harness: Option<&str>,
) -> Result<Option<AnswerReason>, ApiError> {
    let harness = harness.map_or_else(|| s.ops().hosted.default_harness.clone(), str::to_owned);
    let llm = s.llm();
    let r = model::readiness(s, &llm, conn, agent, org, workspace, &harness).await?;
    let usable = r.keys.iter().filter(|k| k.reason.is_none()).count();
    Ok(decide_reason(r.keys.len(), usable, r.default.is_some()))
}

/// [`reason_of`] for a poll (the attention badge): reads key listings from the cache only and never calls
/// a provider. A `litellm` key whose listing is not cached is "listing unknown": a missing default model
/// is then not `NoModel` (the listing might supply one), so a cache miss never yields a reason by itself.
pub async fn reason_of_cached(
    s: &AppState,
    conn: &mut sqlx::PgConnection,
    agent: uuid::Uuid,
    org: uuid::Uuid,
    workspace: uuid::Uuid,
    harness: Option<&str>,
) -> Result<Option<AnswerReason>, ApiError> {
    let harness = harness.map_or_else(|| s.ops().hosted.default_harness.clone(), str::to_owned);
    let llm = s.llm();
    let r = model::readiness_cached(s, &llm, conn, agent, org, workspace, &harness).await?;
    let usable = r.keys.iter().filter(|k| k.reason.is_none()).count();
    let listing_unknown = r.keys.iter().any(|k| {
        k.reason.is_none() && k.listing.is_none() && k.ep.is_some_and(|ep| ep.provider == "litellm")
    });
    Ok(
        match decide_reason(r.keys.len(), usable, r.default.is_some()) {
            Some(AnswerReason::NoModel) if listing_unknown => None,
            other => other,
        },
    )
}

/// What the caller can do and what exists (the inputs of [`choose_fix`]).
pub struct FixInput {
    /// The caller owns the agent's workspace (the grant route's own check, `require_own`).
    pub can_own: bool,
    /// The caller administers the organization or is an instance admin.
    pub can_add_key: bool,
    /// A usable key that is not granted to this agent exists (the starter key first).
    pub giveable: Option<FixKey>,
    /// Any active key exists in the agent's organization or workspace, granted or not.
    pub any_key: bool,
}

pub enum FixChoice {
    Give(FixKey),
    Add,
    Ask(WhoCanRole),
}

/// Never returns a key the caller could not grant: `Give` needs `can_own`.
pub fn choose_fix(i: FixInput) -> FixChoice {
    if i.can_own
        && let Some(k) = i.giveable.clone()
    {
        return FixChoice::Give(k);
    }
    if !i.any_key && i.can_add_key {
        return FixChoice::Add;
    }
    // A key exists that the workspace's owners can give; otherwise the organization's
    // administrators have to add one.
    if i.giveable.is_some() {
        FixChoice::Ask(WhoCanRole::WorkspaceOwner)
    } else {
        FixChoice::Ask(WhoCanRole::OrgAdmin)
    }
}

/// Can this agent answer, and what fixes it (C-1062; any caller who can use the agent).
#[utoipa::path(get, path = "/agents/{agent}/answer-readiness", operation_id = "getAgentAnswerReadiness", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AnswerReadiness),
              (status = 404, description = "Not found or not visible", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
pub(super) async fn answer_readiness(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<AnswerReadiness>, ApiError> {
    let a = load_agent(&s, &c, agent).await?;
    Ok(Json(compute(&s, &c, &a).await?))
}

/// The answer for `a` as seen by `c`; also the "Try it is available" input of setup status.
pub async fn compute(s: &AppState, c: &Caller, a: &AgentRow) -> Result<AnswerReadiness, ApiError> {
    if a.kind != "hosted" {
        return Ok(AnswerReadiness {
            can_answer: true,
            reason: None,
            usable_keys: 0,
            fix: None,
        });
    }
    let auth = c.workspace(s, a.workspace_id).await?;
    let harness = a
        .harness
        .clone()
        .unwrap_or_else(|| s.ops().hosted.default_harness.clone());
    let llm = s.llm();
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let r = model::readiness(s, &llm, &mut conn, a.id, a.org_id, a.workspace_id, &harness).await?;
    let granted = r.keys.len();
    let usable = r.keys.iter().filter(|k| k.reason.is_none()).count();
    let Some(reason) = decide_reason(granted, usable, r.default.is_some()) else {
        return Ok(AnswerReadiness {
            can_answer: true,
            reason: None,
            usable_keys: usable as u32,
            fix: None,
        });
    };
    drop(r);
    // Keys the agent could be given: usable by the dispatcher's own rules, the starter key first.
    let cands =
        placement_repo::ungranted_llm_keys(&mut *conn, a.id, a.org_id, a.workspace_id).await?;
    let any_key = granted > 0 || !cands.is_empty();
    let eff = gov::effective_for(s, a.org_id).await?;
    let mut ok: Vec<aurora_db::repo::hosted::LlmCandidate> =
        model::usable(s, &llm, &eff, &harness, cands)
            .into_iter()
            .map(|u| u.secret)
            .collect();
    let starters = placement_repo::get(&mut *conn)
        .await?
        .map(|row| placement::starter_ids(&row).0)
        .unwrap_or_default();
    drop(conn);
    ok.sort_by_key(|k| !starters.contains(&k.id)); // stable: starter first, else the dispatcher's order
    let can_own = auth.can_own();
    let giveable = ok.first().map(|k| FixKey {
        secret_id: k.id,
        // Names only for people who can own the workspace (as `model-readiness`).
        name: if can_own {
            k.name.clone()
        } else {
            String::new()
        },
        scope: if k.workspace_id.is_some() {
            "workspace"
        } else {
            "organization"
        }
        .to_owned(),
    });
    let fix = match choose_fix(FixInput {
        can_own,
        can_add_key: auth.org_role.is_admin() || c.is_instance_admin,
        giveable,
        any_key,
    }) {
        FixChoice::Give(key) => AnswerFix::GiveKey { key },
        FixChoice::Add => AnswerFix::AddKey,
        FixChoice::Ask(role) => {
            let mut who =
                WhoCan::build(s, role, a.org_id, Some(a.workspace_id), "add_ai_key", None).await;
            // Roles, never names (D-407).
            who.people.clear();
            AnswerFix::AskAdmin { who_can: who }
        }
    };
    Ok(AnswerReadiness {
        can_answer: false,
        reason: Some(reason),
        usable_keys: usable as u32,
        fix: Some(fix),
    })
}

/// (FIX-249, C-1168) One AI key that exists in the agent's organization or workspace and is not
/// granted to the agent yet. Metadata only: never the value, never an endpoint address.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct AgentAiKey {
    pub secret_id: Uuid,
    pub name: String,
    /// `organization` | `workspace`.
    pub scope: String,
    /// One of the organization's starter keys.
    pub starter: bool,
    /// Whether the dispatcher could use this key for the agent now.
    pub usable: bool,
    /// Why not, in short plain words (the dispatcher's own reason); `null` when usable.
    pub reason: Option<String>,
}

/// The answer of `GET /v1/agents/{agent}/ai-keys`: the starter key first, then the dispatcher's
/// order, at most 50.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentAiKeys {
    pub items: Vec<AgentAiKey>,
}

/// The dispatcher's per-key reason in short plain words, for the key pickers (`08 §Model
/// readiness` is the check order; here only the words, never the internal code).
fn reason_words(r: model::Reason) -> &'static str {
    match r {
        model::Reason::NotActive => "It is not active.",
        model::Reason::EndpointUnknown => "Its service is not set up here.",
        model::Reason::ProviderDisabled => "Its service is not allowed here.",
        model::Reason::ScopeNotPermitted => "Keys like this one are not allowed here.",
        model::Reason::OriginUnverified => "Its account still needs to be verified.",
        model::Reason::OriginNotCompliant => "Its account is not on the allowed list.",
        model::Reason::NoSharedDialect => "It does not work with this agent's model tool.",
    }
}

/// The AI keys this agent does **not** have yet that exist in its organization or workspace
/// (`can_own` on the agent's workspace, the grant route's refusal for everyone else; FIX-249,
/// C-1168). The starter key first, then the dispatcher's order; a key the dispatcher cannot use
/// is listed with its reason instead of being dropped. Metadata only, at most 50.
#[utoipa::path(get, path = "/agents/{agent}/ai-keys", operation_id = "getAgentAiKeys", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AgentAiKeys),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found, not hosted, or not visible", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
pub(super) async fn agent_ai_keys(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<AgentAiKeys>, ApiError> {
    let a = load_agent(&s, &c, agent).await?;
    let auth = c.workspace(&s, a.workspace_id).await?;
    require_own(&auth)?;
    if a.kind != "hosted" {
        return Err(ApiError::not_found("Agent has no sandbox."));
    }
    let harness = a
        .harness
        .clone()
        .unwrap_or_else(|| s.ops().hosted.default_harness.clone());
    let llm = s.llm();
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let cands =
        placement_repo::ungranted_llm_keys(&mut *conn, a.id, a.org_id, a.workspace_id).await?;
    let starters = placement_repo::get(&mut *conn)
        .await?
        .map(|row| placement::starter_ids(&row).0)
        .unwrap_or_default();
    let eff = gov::effective_for(&s, a.org_id).await?;
    let mut keys = model::key_states(&s, &llm, &eff, &harness, cands);
    drop(conn);
    keys.sort_by_key(|k| !starters.contains(&k.secret.id)); // stable: the starter first
    keys.truncate(50);
    Ok(Json(AgentAiKeys {
        items: keys
            .into_iter()
            .map(|k| AgentAiKey {
                secret_id: k.secret.id,
                name: k.secret.name,
                scope: if k.secret.workspace_id.is_some() {
                    "workspace"
                } else {
                    "organization"
                }
                .to_owned(),
                starter: starters.contains(&k.secret.id),
                usable: k.reason.is_none(),
                reason: k.reason.map(reason_words).map(str::to_owned),
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> FixKey {
        FixKey {
            secret_id: Uuid::nil(),
            name: "Company key".into(),
            scope: "organization".into(),
        }
    }

    #[test]
    fn reason_is_decided_in_order() {
        assert_eq!(decide_reason(0, 0, false), Some(AnswerReason::NoAiKey));
        assert_eq!(decide_reason(2, 0, false), Some(AnswerReason::KeyNotUsable));
        assert_eq!(decide_reason(2, 1, false), Some(AnswerReason::NoModel));
        assert_eq!(decide_reason(2, 1, true), None);
    }

    #[test]
    fn owner_gives_a_key_member_asks_owners_admin_adds() {
        let owner = choose_fix(FixInput {
            can_own: true,
            can_add_key: false,
            giveable: Some(key()),
            any_key: true,
        });
        assert!(matches!(owner, FixChoice::Give(_)));
        let member = choose_fix(FixInput {
            can_own: false,
            can_add_key: false,
            giveable: Some(key()),
            any_key: true,
        });
        assert!(matches!(member, FixChoice::Ask(WhoCanRole::WorkspaceOwner)));
        let admin_no_key = choose_fix(FixInput {
            can_own: true,
            can_add_key: true,
            giveable: None,
            any_key: false,
        });
        assert!(matches!(admin_no_key, FixChoice::Add));
        let member_no_key = choose_fix(FixInput {
            can_own: false,
            can_add_key: false,
            giveable: None,
            any_key: false,
        });
        assert!(matches!(
            member_no_key,
            FixChoice::Ask(WhoCanRole::OrgAdmin)
        ));
    }
}

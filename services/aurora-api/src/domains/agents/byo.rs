//! BYO governance on the API side (phase 2, RFE-025, D-59; 08 §11.9): effective posture,
//! creation/regeneration authorization, source-list validation, and the agent `byo` view.
use crate::app::AppState;
use aurora_core::cidr::Cidr;
use aurora_core::limits::BYO_CIDR_MAX;
use aurora_core::{ApiError, FieldError};
use aurora_db::authz::WorkspaceAuth;
use aurora_db::repo::{agents::AgentRow, gateway, workspaces};
use aurora_llm::governance::{
    ByoInputs, ByoLevel, ByoPosture, EffectiveByo, OrgByo, creation_allowed, effective_byo,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

/// The `byo` object of an [`Agent`](super::dto::Agent) (BYO agents only).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ByoView {
    /// Effective BYO posture of the agent's workspace: `enabled` | `owners_only` | `disabled`.
    pub posture: String,
    /// The layer that decided it: `instance` | `org` | `workspace`.
    pub posture_level: String,
    /// The agent's own source list (`null` = inherit).
    pub source_cidrs: Option<Vec<String>>,
    /// The agent's own require-context flag.
    pub require_context: bool,
    /// The list the gateway enforces (agent, else org, else instance); `null` = unrestricted.
    pub effective_source_cidrs: Option<Vec<String>>,
    /// Which layer the effective list comes from: `agent` | `org` | `instance` (null = none).
    pub effective_source_level: Option<String>,
    /// Instance ∨ org ∨ agent.
    pub effective_require_context: bool,
    /// Which layer switched it on: `instance` | `org` | `agent` (null = off).
    pub effective_require_context_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ByoPostureView {
    /// `enabled` | `owners_only` | `disabled`.
    pub value: String,
    /// `instance` | `org` | `workspace`.
    pub level: String,
}

/// `PUT /v1/agents/{agent}/byo`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutByoRequest {
    /// The agent's own source CIDR list; `null` = inherit the org/instance default.
    pub source_cidrs: Option<Vec<String>>,
    pub require_context: bool,
}

/// The org's stored `governance.byo` (a malformed value fails closed).
pub async fn org_byo(state: &AppState, org: Uuid) -> Result<OrgByo, ApiError> {
    let doc = gateway::org_governance(&state.pool, org)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    Ok(OrgByo::from_stored(doc.get("byo")))
}

pub fn effective(
    state: &AppState,
    org: &OrgByo,
    ws_posture: Option<&str>,
    agent: Option<&AgentRow>,
) -> EffectiveByo {
    effective_byo(
        &state.gov_instance.byo,
        &ByoInputs {
            org: org.clone(),
            workspace_posture: ws_posture,
            agent_cidrs: agent.and_then(|a| a.byo_source_cidrs.as_deref()),
            agent_require_context: agent.is_some_and(|a| a.byo_require_context),
        },
    )
}

/// Effective controls of one agent (loads the org document and the workspace posture).
pub async fn effective_for_agent(
    state: &AppState,
    agent: &AgentRow,
) -> Result<EffectiveByo, ApiError> {
    let org = org_byo(state, agent.org_id).await?;
    let ws = workspaces::get(&state.pool, agent.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    Ok(effective(
        state,
        &org,
        ws.byo_posture.as_deref(),
        Some(agent),
    ))
}

fn lvl(l: Option<ByoLevel>) -> Option<String> {
    l.map(|l| l.as_str().to_owned())
}

pub fn view(eff: &EffectiveByo, agent: &AgentRow) -> ByoView {
    ByoView {
        posture: eff.posture.as_str().to_owned(),
        posture_level: eff.posture_level.as_str().to_owned(),
        source_cidrs: agent.byo_source_cidrs.clone(),
        require_context: agent.byo_require_context,
        effective_source_cidrs: eff.source_strings(),
        effective_source_level: lvl(eff.source_level),
        effective_require_context: eff.require_context,
        effective_require_context_level: lvl(eff.require_context_level),
    }
}

/// Views for the BYO agents of one workspace (`None` for hosted agents); the org document and
/// the workspace posture are read once.
pub async fn views(
    state: &AppState,
    org: Uuid,
    ws: Uuid,
    agents: &[&AgentRow],
) -> Result<Vec<Option<ByoView>>, ApiError> {
    if !agents.iter().any(|a| a.kind == "byo") {
        return Ok(agents.iter().map(|_| None).collect());
    }
    let org = org_byo(state, org).await?;
    let ws = workspaces::get(&state.pool, ws)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    Ok(agents
        .iter()
        .map(|a| {
            (a.kind == "byo").then(|| {
                view(
                    &effective(state, &org, ws.byo_posture.as_deref(), Some(a)),
                    a,
                )
            })
        })
        .collect())
}

fn manage_url(state: &AppState, org_slug: &str, ws_slug: &str, level: ByoLevel) -> String {
    let base = state.config.external_url.trim_end_matches('/');
    match level {
        ByoLevel::Workspace | ByoLevel::Agent => {
            format!("{base}/o/{org_slug}/w/{ws_slug}/settings")
        }
        ByoLevel::Org | ByoLevel::Instance => format!("{base}/o/{org_slug}/governance"),
    }
}

async fn slugs(state: &AppState, ws: Uuid) -> Result<(String, String), ApiError> {
    let (o, w): (String, String) = sqlx::query_as(
        "SELECT o.slug, w.slug FROM workspaces w JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(ws)
    .fetch_one(&state.pool)
    .await
    .map_err(aurora_db::DbError::from)?;
    Ok((o, w))
}

/// 403 `byo_disabled` for an effective posture of `disabled`.
pub async fn disabled_error(
    state: &AppState,
    ws: Uuid,
    level: ByoLevel,
) -> Result<ApiError, ApiError> {
    let (o, w) = slugs(state, ws).await?;
    let url = manage_url(state, &o, &w, level);
    Ok(ApiError::new(
        StatusCode::FORBIDDEN,
        "byo_disabled",
        format!(
            "BYO agents are disabled by the {} settings. Review them at {url}",
            level.as_str()
        ),
    )
    .with_details(json!({ "level": level.as_str(), "manage_url": url })))
}

/// Posture check of BYO agent creation / token regeneration (`08 §11.9`): `enabled` needs
/// `can_use` (implied by the workspace being visible), `owners_only` needs `can_own`, `disabled`
/// refuses everyone.
pub async fn require_creation(
    state: &AppState,
    auth: &WorkspaceAuth,
    ws: Uuid,
    eff: &EffectiveByo,
) -> Result<(), ApiError> {
    if creation_allowed(eff.posture, true, auth.can_own()) {
        return Ok(());
    }
    if eff.posture == ByoPosture::Disabled {
        return Err(disabled_error(state, ws, eff.posture_level).await?);
    }
    let (o, w) = slugs(state, ws).await?;
    let url = manage_url(state, &o, &w, eff.posture_level);
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "byo_not_permitted",
        "Creating BYO agents or regenerating their tokens needs the workspace owner role under this workspace's BYO posture.",
    )
    .with_details(json!({
        "posture": eff.posture.as_str(),
        "level": eff.posture_level.as_str(),
        "manage_url": url,
    })))
}

/// 403 `byo_disabled` when the agent's effective posture is `disabled` (agent-token routes).
pub async fn require_not_disabled(
    state: &AppState,
    agent: &AgentRow,
) -> Result<EffectiveByo, ApiError> {
    let eff = effective_for_agent(state, agent).await?;
    if eff.posture == ByoPosture::Disabled {
        return Err(disabled_error(state, agent.workspace_id, eff.posture_level).await?);
    }
    Ok(eff)
}

fn cidr_invalid(path: &str, msg: String) -> ApiError {
    ApiError::unprocessable("byo_cidr_invalid", "The source list is not valid.")
        .with_details(json!({ "fields": [FieldError::new(path, msg)] }))
}

/// Validates an agent's own source list: 1–32 strict CIDRs, no `/0`, and (tighten-only) every
/// entry inside the inherited list — the org default, else the instance default — when that is
/// non-empty. Returns the normalised, deduplicated list.
pub fn validate_source_list(
    list: &[String],
    inherited: Option<&[Cidr]>,
) -> Result<Vec<String>, ApiError> {
    if list.is_empty() {
        return Err(cidr_invalid(
            "/source_cidrs",
            "an empty list is not allowed; send null to inherit".into(),
        ));
    }
    if list.len() > BYO_CIDR_MAX {
        return Err(cidr_invalid(
            "/source_cidrs",
            format!("at most {BYO_CIDR_MAX} entries"),
        ));
    }
    let mut out: Vec<String> = Vec::new();
    for (i, s) in list.iter().enumerate() {
        let c = Cidr::parse(s)
            .map_err(|e| cidr_invalid(&format!("/source_cidrs/{i}"), format!("`{s}`: {e}")))?;
        if let Some(outer) = inherited
            && !outer.is_empty()
            && !outer.iter().any(|o| o.contains_cidr(&c))
        {
            return Err(ApiError::unprocessable(
                "byo_cidr_outside_default",
                "A source CIDR lies outside the organization or instance default list.",
            )
            .with_details(json!({ "fields": [FieldError::new(
                format!("/source_cidrs/{i}"),
                format!("`{s}` is outside the default list")
            )] })));
        }
        let n = c.to_string();
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

/// The list an agent's own list must stay inside: the org default, else the instance default.
pub fn inherited_list(state: &AppState, org: &OrgByo) -> Option<Vec<Cidr>> {
    if !org.source_cidrs.is_empty() {
        // Stored values were validated at write time; an unparsable one matches nothing.
        return Some(aurora_llm::governance::parse_byo_list(&org.source_cidrs).unwrap_or_default());
    }
    let d = &state.gov_instance.byo.default_source_cidrs;
    (!d.is_empty()).then(|| d.clone())
}

/// Workspace posture view for `GET/PATCH /v1/workspaces/{ws}`.
pub async fn workspace_view(
    state: &AppState,
    org: Uuid,
    ws_posture: Option<&str>,
) -> Result<ByoPostureView, ApiError> {
    let o = org_byo(state, org).await?;
    let e = effective(state, &o, ws_posture, None);
    Ok(ByoPostureView {
        value: e.posture.as_str().to_owned(),
        level: e.posture_level.as_str().to_owned(),
    })
}

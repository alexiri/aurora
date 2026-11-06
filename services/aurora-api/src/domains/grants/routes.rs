use super::dto::*;
use super::{effective, service};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::grants;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_grants))
        .routes(routes!(put_secret_grant, delete_secret_grant))
        .routes(routes!(agent_effective))
}

fn record(state: &AppState, caller: &Caller, action: &str, agent: &AgentRow, secret: Uuid) {
    audit::record(
        state,
        AuditEvent::new(AuditActor::from_caller(caller), action, Outcome::Success)
            .org(agent.org_id)
            .workspace(agent.workspace_id)
            .target("agent", agent.id)
            .metadata(json!({"secret_id": secret, "agent_handle": agent.handle})),
    );
}

/// Grants of an agent (`can_use`).
#[utoipa::path(
    get, path = "/agents/{agent}/grants", operation_id = "listAgentGrants", tag = "grants",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, description = "Grants", body = GrantsResponse),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_grants(
    caller: Caller,
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<GrantsResponse>, ApiError> {
    let (a, _) = service::visible_agent(&caller, &state, agent).await?;
    let secrets = grants::list_secret_grants(&state.pool, a.id).await?;
    Ok(Json(GrantsResponse {
        secrets: secrets.into_iter().map(Into::into).collect(),
        connections: Vec::new(),
    }))
}

/// Grant a secret to an agent (`can_own`); idempotent.
#[utoipa::path(
    put, path = "/agents/{agent}/grants/secrets/{secret}", operation_id = "grantSecret", tag = "grants",
    params(("agent" = Uuid, Path, description = "Agent id"), ("secret" = Uuid, Path, description = "Secret id")),
    responses((status = 200, description = "Grant", body = SecretGrant),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Agent, or secret not in the agent's workspace or org", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn put_secret_grant(
    caller: Caller,
    State(state): State<AppState>,
    Path((agent, secret)): Path<(Uuid, Uuid)>,
) -> Result<Json<SecretGrant>, ApiError> {
    let (a, auth) = service::visible_agent(&caller, &state, agent).await?;
    require_own(&auth)?;
    let g = service::grant_secret(&state, &caller, &a, secret).await?;
    record(&state, &caller, "grant.create", &a, secret);
    Ok(Json(g))
}

/// Revoke a secret grant (`can_own`).
#[utoipa::path(
    delete, path = "/agents/{agent}/grants/secrets/{secret}", operation_id = "revokeSecretGrant", tag = "grants",
    params(("agent" = Uuid, Path, description = "Agent id"), ("secret" = Uuid, Path, description = "Secret id")),
    responses((status = 204, description = "Revoked"),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn delete_secret_grant(
    caller: Caller,
    State(state): State<AppState>,
    Path((agent, secret)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let (a, auth) = service::visible_agent(&caller, &state, agent).await?;
    require_own(&auth)?;
    service::revoke_secret(&state, &a, secret).await?;
    record(&state, &caller, "grant.delete", &a, secret);
    Ok(StatusCode::NO_CONTENT)
}

/// Effective credentials of an agent (`can_use`).
#[utoipa::path(
    get, path = "/agents/{agent}/effective", operation_id = "getAgentEffective", tag = "grants",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, description = "Effective view", body = Effective),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn agent_effective(
    caller: Caller,
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Effective>, ApiError> {
    let (a, _) = service::visible_agent(&caller, &state, agent).await?;
    Ok(Json(effective::compute(&state, &a).await?))
}

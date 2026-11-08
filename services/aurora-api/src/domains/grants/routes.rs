use super::dto::*;
use super::{effective, service};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::connections::service as conn_service;
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::{gateway, grants};
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_grants))
        .routes(routes!(put_secret_grant, delete_secret_grant))
        .routes(routes!(put_connection_grant, delete_connection_grant))
        .routes(routes!(connection_grants))
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

/// Grants of an agent (`can_own`; I-27). Members see what they need via `/agents/{agent}/effective`.
#[utoipa::path(
    get, path = "/agents/{agent}/grants", operation_id = "listAgentGrants", tag = "grants",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, description = "Grants", body = GrantsResponse),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_grants(
    caller: Caller,
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<GrantsResponse>, ApiError> {
    let (a, auth) = service::visible_agent(&caller, &state, agent).await?;
    require_own(&auth)?;
    let secrets = grants::list_secret_grants(&state.pool, a.id).await?;
    let conns = grants::list_connection_grants(&state.pool, a.id).await?;
    Ok(Json(GrantsResponse {
        secrets: secrets.into_iter().map(Into::into).collect(),
        connections: conns.into_iter().map(Into::into).collect(),
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
    params(("agent" = Uuid, Path, description = "Agent id"), EffectiveQuery),
    responses((status = 200, description = "Effective view", body = Effective),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 400, description = "Invalid speaker", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn agent_effective(
    caller: Caller,
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<EffectiveQuery>,
) -> Result<Json<Effective>, ApiError> {
    let (a, _) = service::visible_agent(&caller, &state, agent).await?;
    // RFE-005: `?speaker=<user_id>|none`, default the caller. A speaker who does not hold
    // `can_use` never binds in the gateway, so naming one is a 400 field error; the default caller who
    // holds no `can_use` (e.g. an instance admin) simply sees speakerless traffic.
    let speaker = match q.speaker.as_deref() {
        None => gateway::can_use(&state.pool, a.workspace_id, caller.user_id)
            .await?
            .then_some(caller.user_id),
        Some("none") => None,
        Some(v) => {
            let id = Uuid::parse_str(v).map_err(|_| {
                ApiError::invalid_request(
                    "Invalid request.",
                    vec![FieldError::new("/speaker", "must be a user id or \"none\"")],
                )
            })?;
            if !gateway::can_use(&state.pool, a.workspace_id, id).await? {
                return Err(ApiError::invalid_request(
                    "Invalid request.",
                    vec![FieldError::new(
                        "/speaker",
                        "not a member with access to this workspace",
                    )],
                ));
            }
            Some(id)
        }
    };
    Ok(Json(effective::compute_for(&state, &a, speaker).await?))
}

fn record_conn(
    state: &AppState,
    caller: &Caller,
    action: &str,
    agent: &AgentRow,
    connection: Uuid,
    meta: serde_json::Value,
) {
    audit::record(
        state,
        AuditEvent::new(AuditActor::from_caller(caller), action, Outcome::Success)
            .org(agent.org_id)
            .workspace(agent.workspace_id)
            .target("agent", agent.id)
            .metadata(
                json!({"connection_id": connection, "agent_handle": agent.handle, "grant": meta}),
            ),
    );
}

/// Grant a connection to an agent, replacing any previous grant (`can_own`).
#[utoipa::path(
    put, path = "/agents/{agent}/grants/connections/{conn}", operation_id = "grantConnection", tag = "grants",
    params(("agent" = Uuid, Path, description = "Agent id"), ("conn" = Uuid, Path, description = "Connection id")),
    request_body = PutConnectionGrant,
    responses((status = 200, description = "Grant", body = ConnectionGrant),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Agent, or connection not in the agent's workspace or org", body = ErrorEnvelope),
              (status = 422, description = "grant_invalid (details.reason: empty_custom | tool_in_both | unknown_tool)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn put_connection_grant(
    caller: Caller,
    State(state): State<AppState>,
    Path((agent, conn)): Path<(Uuid, Uuid)>,
    Json(req): Json<PutConnectionGrant>,
) -> Result<Json<ConnectionGrant>, ApiError> {
    let (a, auth) = service::visible_agent(&caller, &state, agent).await?;
    require_own(&auth)?;
    let g = service::grant_connection(&state, &caller, &a, conn, req).await?;
    record_conn(
        &state,
        &caller,
        "grant.create",
        &a,
        conn,
        json!({"mode": g.mode, "allow_tools": g.allow_tools, "ask_tools": g.ask_tools}),
    );
    Ok(Json(g))
}

/// Revoke a connection grant (`can_own`).
#[utoipa::path(
    delete, path = "/agents/{agent}/grants/connections/{conn}", operation_id = "revokeConnectionGrant", tag = "grants",
    params(("agent" = Uuid, Path, description = "Agent id"), ("conn" = Uuid, Path, description = "Connection id")),
    responses((status = 204, description = "Revoked"),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn delete_connection_grant(
    caller: Caller,
    State(state): State<AppState>,
    Path((agent, conn)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let (a, auth) = service::visible_agent(&caller, &state, agent).await?;
    require_own(&auth)?;
    service::revoke_connection(&state, &a, conn).await?;
    record_conn(&state, &caller, "grant.delete", &a, conn, json!(null));
    Ok(StatusCode::NO_CONTENT)
}

/// Grants held on a connection (`can_own` / owner-admin; I-27).
#[utoipa::path(
    get, path = "/connections/{id}/grants", operation_id = "listConnectionGrants", tag = "grants",
    params(("id" = Uuid, Path, description = "Connection id")),
    responses((status = 200, description = "Grants", body = ConnectionGrantList),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn connection_grants(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConnectionGrantList>, ApiError> {
    let (row, access) = conn_service::visible(&caller, &state, id).await?;
    access.require_write()?;
    let items = grants::list_for_connection(&state.pool, row.id).await?;
    Ok(Json(ConnectionGrantList {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

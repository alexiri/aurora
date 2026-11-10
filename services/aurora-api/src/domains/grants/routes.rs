use super::dto::*;
use super::{effective, service};
use crate::app::AppState;
use crate::domains::connections::service as conn_service;
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::{gateway, grants};
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_grants))
        .routes(routes!(put_secret_grant, delete_secret_grant))
        .routes(routes!(put_connection_grant, delete_connection_grant))
        .routes(routes!(connection_grants, bulk_connection_grants))
        .routes(routes!(secret_grants, bulk_secret_grants))
        .routes(routes!(agent_effective))
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
    // (P4-GRANTS, C-410) each connection grant carries its app's restrictions.
    let ids: Vec<Uuid> = conns.iter().map(|c| c.connection_id).collect();
    let restr = service::restrictions_by_connection(&state, &ids).await?;
    let converted_by_app = grants::standing_conversions(&state.pool, a.id)
        .await?
        .into_iter()
        .map(|c| GrantConversion {
            connection_id: c.connection_id,
            app_id: c.app_id,
            converted_at: c.converted_at,
        })
        .collect();
    Ok(Json(GrantsResponse {
        converted_by_app,
        secrets: secrets.into_iter().map(Into::into).collect(),
        connections: conns
            .into_iter()
            .map(|c| {
                let r = restr.get(&c.connection_id).copied().unwrap_or_default();
                ConnectionGrant::from(c).restricted(r)
            })
            .collect(),
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
    // can_own and scope checks live in the shared service function (SEC-77).
    let g = service::grant_secret(&state, &caller, agent, secret).await?;
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
    service::revoke_secret(&state, &caller, &a, secret).await?;
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
    let g = service::grant_connection(&state, &caller, agent, conn, req).await?;
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
    service::revoke_connection(&state, &caller, &a, conn).await?;
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
    // FIX-020: org connections are readable by owners of at least one workspace; the list only
    // shows agents of workspaces the caller can own.
    access.require_read()?;
    let items = service::list_connection_grants(&state, &caller, row.id).await?;
    Ok(Json(ConnectionGrantList { items }))
}

/// Grant a connection to several agents at once (FIX-020, C-504; `can_own` per agent). Each
/// agent passes exactly the checks of `PUT /agents/{agent}/grants/connections/{conn}`;
/// all-or-nothing; an existing grant is returned `existing` and not modified.
#[utoipa::path(
    post, path = "/connections/{id}/grants", operation_id = "grantConnectionToAgents", tag = "grants",
    params(("id" = Uuid, Path, description = "Connection id")),
    request_body = BulkConnectionGrantRequest,
    responses((status = 200, description = "Grants", body = BulkConnectionGrantResponse),
              (status = 400, description = "Invalid agent_ids (empty, over GRANT_BULK_MAX, duplicates)", body = ErrorEnvelope),
              (status = 403, description = "Forbidden (details.agent_id)", body = ErrorEnvelope),
              (status = 404, description = "Agent or connection not found (details.agent_id)", body = ErrorEnvelope),
              (status = 422, description = "grant_invalid, as the agent-side route (details.agent_id)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn bulk_connection_grants(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<BulkConnectionGrantRequest>,
) -> Result<Json<BulkConnectionGrantResponse>, ApiError> {
    let items = service::bulk_grant_connection(&state, &caller, id, req).await?;
    let app_restrictions = service::restrictions_by_connection(&state, &[id])
        .await?
        .remove(&id)
        .unwrap_or_default();
    Ok(Json(BulkConnectionGrantResponse {
        items,
        app_restrictions,
    }))
}

/// Agents granted a secret (FIX-020, C-503): `can_own` on the secret's scope; only agents of
/// workspaces the caller can own.
#[utoipa::path(
    get, path = "/secrets/{id}/grants", operation_id = "listSecretGrants", tag = "grants",
    params(("id" = Uuid, Path, description = "Secret id")),
    responses((status = 200, description = "Grants", body = SecretGrantViewList),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn secret_grants(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SecretGrantViewList>, ApiError> {
    let row = crate::domains::secrets::service::load_readable(&caller, &state, id).await?;
    let items = service::list_secret_grants(&state, &caller, row.id).await?;
    Ok(Json(SecretGrantViewList { items }))
}

/// Grant a secret to several agents at once (FIX-020, C-503; `can_own` per agent). Same checks
/// as `PUT /agents/{agent}/grants/secrets/{secret}`; all-or-nothing; idempotent.
#[utoipa::path(
    post, path = "/secrets/{id}/grants", operation_id = "grantSecretToAgents", tag = "grants",
    params(("id" = Uuid, Path, description = "Secret id")),
    request_body = BulkSecretGrantRequest,
    responses((status = 200, description = "Grants", body = BulkSecretGrantResponse),
              (status = 400, description = "Invalid agent_ids (empty, over GRANT_BULK_MAX, duplicates)", body = ErrorEnvelope),
              (status = 403, description = "Forbidden (details.agent_id)", body = ErrorEnvelope),
              (status = 404, description = "Agent or secret not found (details.agent_id)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn bulk_secret_grants(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<BulkSecretGrantRequest>,
) -> Result<Json<BulkSecretGrantResponse>, ApiError> {
    let items = service::bulk_grant_secret(&state, &caller, id, &req.agent_ids).await?;
    Ok(Json(BulkSecretGrantResponse { items }))
}

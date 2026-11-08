use super::byo::PutByoRequest;
use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{AgentCaller, Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams};
use aurora_core::ApiError;
use aurora_db::repo::agents;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(super::avatar::router())
        .routes(routes!(list_agents, create_agent))
        .routes(routes!(get_agent, patch_agent, delete_agent))
        .routes(routes!(regenerate_token))
        .routes(routes!(put_agent_byo))
        .routes(routes!(agent_container_config))
        .routes(routes!(self_container_config))
        .routes(routes!(create_context_credential))
        .routes(routes!(gateway_ca))
}

/// List the agents of a workspace (`can_use`).
#[utoipa::path(
    get,
    path = "/workspaces/{ws}/agents",
    operation_id = "listAgents",
    tag = "agents",
    params(("ws" = Uuid, Path, description = "Workspace id"), PageParams),
    responses(
        (status = 200, description = "Agents", body = Page<Agent>),
        (status = 404, description = "Not found", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn list_agents(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(params): Query<PageParams>,
) -> Result<Json<Page<Agent>>, ApiError> {
    let p = params.resolve()?;
    let a = caller.workspace(&state, ws).await?;
    let rows = agents::list_for_workspace(&state.pool, ws, p.after.map(|c| (c.t, c.id)), p.fetch())
        .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    let refs: Vec<&agents::AgentRow> = page.items.iter().collect();
    let views = super::byo::views(&state, a.org_id, ws, &refs).await?;
    let mut items: Vec<Agent> = page
        .items
        .into_iter()
        .zip(views)
        .map(|(r, v)| {
            let mut a = Agent::from(r);
            a.byo = v;
            a
        })
        .collect();
    crate::domains::freeze::service::decorate_agents(&state, &mut items).await?;
    Ok(Json(Page {
        items,
        next_cursor: page.next_cursor,
    }))
}

/// Get one agent (`can_use` on its workspace).
#[utoipa::path(
    get,
    path = "/agents/{agent}",
    operation_id = "getAgent",
    tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses(
        (status = 200, description = "Agent", body = Agent),
        (status = 404, description = "Not found", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn get_agent(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Agent>, ApiError> {
    let row = agents::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    caller.workspace(&state, row.workspace_id).await?;
    Ok(Json(with_byo(&state, row).await?))
}

/// The agent DTO with its `byo` view loaded (BYO agents).
async fn with_byo(state: &AppState, row: agents::AgentRow) -> Result<Agent, ApiError> {
    let byo = if row.kind == "byo" {
        Some(super::byo::view(
            &super::byo::effective_for_agent(state, &row).await?,
            &row,
        ))
    } else {
        None
    };
    let mut a = Agent::from(row);
    a.byo = byo;
    crate::domains::freeze::service::decorate_agents(state, std::slice::from_mut(&mut a)).await?;
    Ok(a)
}

/// Create an agent (`can_use`). BYO agents return their `aug_` token once.
#[utoipa::path(
    post,
    path = "/workspaces/{ws}/agents",
    operation_id = "createAgent",
    tag = "agents",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = CreateAgentRequest,
    responses(
        (status = 201, description = "Created", body = CreatedAgent),
        (status = 400, description = "Invalid", body = ErrorEnvelope),
        (status = 404, description = "Not found", body = ErrorEnvelope),
        (status = 403, description = "byo_not_permitted | byo_disabled", body = ErrorEnvelope),
        (status = 409, description = "handle_taken", body = ErrorEnvelope),
        (status = 422, description = "hosted_disabled | agent_limit_reached", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn create_agent(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<CreateAgentRequest>,
) -> Result<(StatusCode, Json<CreatedAgent>), ApiError> {
    let a = caller.workspace(&state, ws).await?;
    let created = service::create(&state, &caller, &a, a.org_id, ws, req).await?;
    audit::record(
        &state,
        AuditEvent::new(
            AuditActor::from_caller(&caller),
            "agent.create",
            Outcome::Success,
        )
        .org(a.org_id)
        .workspace(ws)
        .target("agent", created.agent.id)
        .metadata(json!({"handle": created.agent.handle, "kind": created.agent.kind})),
    );
    let (container_config, agent) = match &created.token {
        Some(t) => {
            let eff = super::byo::effective_for_agent(&state, &created.agent).await?;
            (
                Some(service::container_config(&state, Some(t), Some(&eff)).await?),
                with_byo(&state, created.agent).await?,
            )
        }
        None => (None, created.agent.into()),
    };
    Ok((
        StatusCode::CREATED,
        Json(CreatedAgent {
            agent,
            token: created.token,
            container_config,
        }),
    ))
}

async fn visible_agent(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<agents::AgentRow, ApiError> {
    Ok(visible_agent_auth(caller, state, id).await?.0)
}

async fn visible_agent_auth(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<(agents::AgentRow, aurora_db::authz::WorkspaceAuth), ApiError> {
    let row = agents::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let auth = caller.workspace(state, row.workspace_id).await?;
    Ok((row, auth))
}

/// Update an agent (`can_use`); `handle` and `kind` are immutable.
#[utoipa::path(
    patch,
    path = "/agents/{agent}",
    operation_id = "updateAgent",
    tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = PatchAgentRequest,
    responses(
        (status = 200, description = "Agent", body = Agent),
        (status = 400, description = "Invalid", body = ErrorEnvelope),
        (status = 403, description = "Forbidden", body = ErrorEnvelope),
        (status = 404, description = "Not found", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn patch_agent(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchAgentRequest>,
) -> Result<Json<Agent>, ApiError> {
    let before = visible_agent(&caller, &state, id).await?;
    let row = service::patch(&state, &caller, id, &req).await?;
    audit::record(
        &state,
        AuditEvent::new(
            AuditActor::from_caller(&caller),
            "agent.update",
            Outcome::Success,
        )
        .org(row.org_id)
        .workspace(before.workspace_id)
        .target("agent", id),
    );
    Ok(Json(with_byo(&state, row).await?))
}

/// Delete an agent and revoke all its gateway credentials (`can_use`).
#[utoipa::path(
    delete,
    path = "/agents/{agent}",
    operation_id = "deleteAgent",
    tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn delete_agent(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let row = visible_agent(&caller, &state, id).await?;
    service::delete(&state, id).await?;
    audit::record(
        &state,
        AuditEvent::new(
            AuditActor::from_caller(&caller),
            "agent.delete",
            Outcome::Success,
        )
        .org(row.org_id)
        .workspace(row.workspace_id)
        .target("agent", id)
        .metadata(json!({"handle": row.handle})),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Regenerate the BYO agent token (revokes the previous token and context credentials).
#[utoipa::path(
    post,
    path = "/agents/{agent}/token",
    operation_id = "regenerateAgentToken",
    tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses(
        (status = 200, description = "New token", body = AgentToken),
        (status = 403, description = "hosted_agent_token_hidden | byo_not_permitted | byo_disabled", body = ErrorEnvelope),
        (status = 404, description = "Not found", body = ErrorEnvelope),
    ),
    security(("session" = []))
)]
async fn regenerate_token(
    caller: SessionCaller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentToken>, ApiError> {
    let (row, auth) = visible_agent_auth(&caller, &state, id).await?;
    refuse_hosted(&row)?;
    // (phase 2, RFE-025) same posture check as creation.
    let eff = super::byo::effective_for_agent(&state, &row).await?;
    super::byo::require_creation(&state, &auth, row.workspace_id, &eff).await?;
    let token = service::regenerate(&state, &caller, &row).await?;
    let container_config = service::container_config(&state, Some(&token), Some(&eff)).await?;
    Ok(Json(AgentToken {
        token,
        container_config,
    }))
}

fn refuse_hosted(row: &agents::AgentRow) -> Result<(), ApiError> {
    if row.kind == "hosted" {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "hosted_agent_token_hidden",
            "Hosted agents do not expose a gateway token.",
        ));
    }
    Ok(())
}

/// BYO container config with an `<AGENT_TOKEN>` placeholder.
#[utoipa::path(
    get,
    path = "/agents/{agent}/container-config",
    operation_id = "getAgentContainerConfig",
    tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses(
        (status = 200, description = "Container config", body = ContainerConfig),
        (status = 403, description = "hosted_agent_token_hidden", body = ErrorEnvelope),
        (status = 404, description = "Not found", body = ErrorEnvelope),
        (status = 503, description = "gateway_ca_unavailable", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn agent_container_config(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ContainerConfig>, ApiError> {
    let row = visible_agent(&caller, &state, id).await?;
    refuse_hosted(&row)?;
    // Viewing the placeholder document stays `can_use` even when BYO is disabled (08 §11.9).
    let eff = super::byo::effective_for_agent(&state, &row).await?;
    Ok(Json(
        service::container_config(&state, None, Some(&eff)).await?,
    ))
}

/// Container config for the calling agent, token embedded (agent token).
#[utoipa::path(
    get,
    path = "/agent/container-config",
    operation_id = "getSelfContainerConfig",
    tag = "agents",
    responses(
        (status = 200, description = "Container config", body = ContainerConfig),
        (status = 401, description = "Unauthenticated", body = ErrorEnvelope),
        (status = 403, description = "byo_disabled", body = ErrorEnvelope),
        (status = 503, description = "gateway_ca_unavailable", body = ErrorEnvelope),
    ),
    security(("agent_token" = []))
)]
async fn self_container_config(
    agent: AgentCaller,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<ContainerConfig>, ApiError> {
    // The extractor already validated the bearer token; embed it verbatim.
    let tok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .map(|(_, t)| t.trim())
        .unwrap_or_default();
    // (phase 2, RFE-025) refused while the effective BYO posture is `disabled`.
    let row = agents::get(&state.pool, agent.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    crate::domains::freeze::service::ensure_agent_active_pool(&state, row.id).await?;
    let eff = super::byo::require_not_disabled(&state, &row).await?;
    Ok(Json(
        service::container_config(&state, Some(tok), Some(&eff)).await?,
    ))
}

/// Mint a context credential (`auc_`) for attribution (agent token).
#[utoipa::path(
    post,
    path = "/agent/context-credentials",
    operation_id = "createContextCredential",
    tag = "agents",
    request_body = ContextCredentialRequest,
    responses(
        (status = 201, description = "Created", body = ContextCredential),
        (status = 400, description = "Invalid", body = ErrorEnvelope),
        (status = 401, description = "Unauthenticated", body = ErrorEnvelope),
        (status = 403, description = "byo_disabled", body = ErrorEnvelope),
        (status = 404, description = "Conversation not found", body = ErrorEnvelope),
    ),
    security(("agent_token" = []))
)]
async fn create_context_credential(
    agent: AgentCaller,
    State(state): State<AppState>,
    Json(req): Json<ContextCredentialRequest>,
) -> Result<(StatusCode, Json<ContextCredential>), ApiError> {
    // (phase 2, RFE-025) refused while the effective BYO posture is `disabled`.
    let row = agents::get(&state.pool, agent.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    crate::domains::freeze::service::ensure_agent_active_pool(&state, row.id).await?;
    super::byo::require_not_disabled(&state, &row).await?;
    let out = service::mint_context(&state, agent.agent_id, req).await?;
    audit::record(
        &state,
        AuditEvent::new(
            AuditActor {
                kind: "agent",
                id: Some(agent.agent_id),
                label: "agent".into(),
                source: "api",
                ip: None,
            },
            "agent.context_credential_create",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("agent", agent.agent_id),
    );
    Ok((StatusCode::CREATED, Json(out)))
}

/// Set a BYO agent's own source CIDR list and require-context flag (`can_own`; RFE-025).
#[utoipa::path(
    put,
    path = "/agents/{agent}/byo",
    operation_id = "putAgentByo",
    tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = PutByoRequest,
    responses(
        (status = 200, description = "Agent", body = Agent),
        (status = 403, description = "Forbidden", body = ErrorEnvelope),
        (status = 404, description = "Not found", body = ErrorEnvelope),
        (status = 422, description = "not_byo | byo_cidr_invalid | byo_cidr_outside_default", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn put_agent_byo(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PutByoRequest>,
) -> Result<Json<Agent>, ApiError> {
    let (row, auth) = visible_agent_auth(&caller, &state, id).await?;
    crate::http::auth::require_own(&auth)?;
    let updated = service::put_byo(&state, &caller, &row, &req).await?;
    Ok(Json(with_byo(&state, updated).await?))
}

/// The gateway CA certificate (PEM).
#[utoipa::path(
    get,
    path = "/gateway/ca.pem",
    operation_id = "getGatewayCa",
    tag = "agents",
    responses(
        (status = 200, description = "PEM", body = String, content_type = "application/x-pem-file"),
        (status = 503, description = "gateway_ca_unavailable", body = ErrorEnvelope),
    ),
    security(("session" = []), ("api_key" = []))
)]
async fn gateway_ca(
    _caller: Caller,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let pem = service::read_ca(&state).await?;
    Ok(([(header::CONTENT_TYPE, "application/x-pem-file")], pem))
}

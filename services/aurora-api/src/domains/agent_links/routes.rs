use super::dto::{
    AgentLink, AgentLinkList, CreateLinkBody, LinkableAgentList, SetDirectionBody, SetPolicyBody,
};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_links, create_link))
        .routes(routes!(set_policy))
        // (P5-REFINE-10, C-1085) people link agents
        .routes(routes!(linkable))
        .routes(routes!(set_direction))
        .routes(routes!(delete_link))
}

/// Links of an agent, each seen from this agent's side.
#[utoipa::path(get, path = "/agents/{agent}/links", operation_id = "listAgentLinks", tag = "agent-links",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AgentLinkList), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_links(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<AgentLinkList>, ApiError> {
    Ok(Json(AgentLinkList {
        items: service::list(&s, &c, agent).await?,
    }))
}

/// Set one agent's side of a link (`ask` | `allow` | `blocked`).
#[utoipa::path(put, path = "/agent-links/{id}/policy", operation_id = "setAgentLinkPolicy", tag = "agent-links",
    params(("id" = Uuid, Path, description = "Link id")),
    request_body = SetPolicyBody,
    responses((status = 200, body = AgentLink), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "stale_revision", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn set_policy(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<SetPolicyBody>,
) -> Result<Json<AgentLink>, ApiError> {
    Ok(Json(service::set_policy(&s, &c, id, b).await?))
}

/// Agents a person may link this agent to (C-1085): same organization, workspaces the caller
/// can use; name, handle and workspace only.
#[utoipa::path(get, path = "/agents/{agent}/linkable", operation_id = "listLinkableAgents", tag = "agent-links",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = LinkableAgentList), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn linkable(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<LinkableAgentList>, ApiError> {
    Ok(Json(LinkableAgentList {
        items: service::linkable(&s, &c, agent).await?,
    }))
}

/// Link this agent to another (C-1085, SEC-142). 201 when created; 200 with the existing link
/// when the pair is linked already with the same direction.
#[utoipa::path(post, path = "/agents/{agent}/links", operation_id = "createAgentLink", tag = "agent-links",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreateLinkBody,
    responses((status = 201, body = AgentLink), (status = 200, description = "Already linked", body = AgentLink),
        (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "link_self | not_hosted | link_exists", body = ErrorEnvelope),
        (status = 423, description = "frozen", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_link(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(b): Json<CreateLinkBody>,
) -> Result<(StatusCode, Json<AgentLink>), ApiError> {
    let (created, link) = service::create(&s, &c, agent, b).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(link)))
}

/// Change who may hand tasks to whom (C-1085): widening needs an approver of both sides.
#[utoipa::path(put, path = "/agent-links/{id}/direction", operation_id = "setAgentLinkDirection", tag = "agent-links",
    params(("id" = Uuid, Path, description = "Link id")),
    request_body = SetDirectionBody,
    responses((status = 200, body = AgentLink), (status = 400, body = ErrorEnvelope),
        (status = 403, description = "approver_required", body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "stale_revision", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn set_direction(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<SetDirectionBody>,
) -> Result<Json<AgentLink>, ApiError> {
    Ok(Json(service::set_direction(&s, &c, id, b).await?))
}

/// Remove a link (C-1085): cancels the pair's open and queued tasks and expires its pending
/// approvals.
#[utoipa::path(delete, path = "/agent-links/{id}", operation_id = "deleteAgentLink", tag = "agent-links",
    params(("id" = Uuid, Path, description = "Link id")),
    responses((status = 204, description = "Removed"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_link(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

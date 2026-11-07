use super::dto::{AgentLink, AgentLinkList, SetPolicyBody};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_links))
        .routes(routes!(set_policy))
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

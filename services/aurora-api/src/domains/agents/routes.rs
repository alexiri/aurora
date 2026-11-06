use super::dto::Agent;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams};
use aurora_core::ApiError;
use aurora_db::repo::agents;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_agents))
        .routes(routes!(get_agent))
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
    caller.workspace(&state, ws).await?;
    let rows = agents::list_for_workspace(&state.pool, ws, p.after.map(|c| (c.t, c.id)), p.fetch())
        .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(Agent::from).collect(),
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
    Ok(Json(row.into()))
}

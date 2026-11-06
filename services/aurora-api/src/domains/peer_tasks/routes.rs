use super::dto::{ListQuery, PeerTask, STATUSES};
use crate::app::AppState;
use crate::domains::conversations::service::load_agent;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::peer_tasks as repo;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list_peer_tasks))
}

/// Peer tasks the agent requested or received, newest first.
#[utoipa::path(get, path = "/agents/{agent}/peer-tasks", operation_id = "listPeerTasks", tag = "peer-tasks",
    params(("agent" = Uuid, Path, description = "Agent id"), ListQuery),
    responses((status = 200, body = Page<PeerTask>), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_peer_tasks(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<PeerTask>>, ApiError> {
    if q.status.as_deref().is_some_and(|v| !STATUSES.contains(&v)) {
        return Err(ApiError::invalid_request(
            "Invalid status.",
            vec![FieldError::new("/status", "unknown status")],
        ));
    }
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    load_agent(&s, &c, agent).await?;
    let rows = repo::list_for_agent(
        &s.pool,
        agent,
        q.status.as_deref(),
        p.after.map(|a| (a.t, a.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page
            .items
            .into_iter()
            .map(|r| PeerTask::from_row(r, agent))
            .collect(),
        next_cursor: page.next_cursor,
    }))
}

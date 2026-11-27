use super::dto::{ListQuery, PeerTask, STATUSES};
use crate::app::AppState;
use crate::domains::conversations::service::load_agent;
use crate::domains::peer_tasks::service as peers;
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::peer_tasks as repo;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_peer_tasks))
        .routes(routes!(cancel_peer_task))
}

/// (FIX-252) The person who asked for a peer task cancels it (the origin card's button and the
/// web card): sessions only, the stored requester only — nobody else's task is revealed (404).
#[utoipa::path(post, path = "/peer-tasks/{id}/cancel", operation_id = "cancelPeerTask", tag = "peer-tasks",
    params(("id" = Uuid, Path, description = "Peer task id")),
    responses((status = 200, body = PeerTask), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn cancel_peer_task(
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<PeerTask>, ApiError> {
    // (FIX-264) One transaction: the close, its report, the abandoned approval and the
    // promotion land together or not at all (an error below drops it, which rolls back).
    let mut tx = s.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let row = match peers::cancel_by_requester(&mut tx, c.user_id, id).await? {
        peers::RequesterCancel::Done(t) => t,
        peers::RequesterCancel::AlreadyClosed => {
            return Err(ApiError::conflict(
                "already_closed",
                "This task is already closed.",
            ));
        }
        peers::RequesterCancel::NotYours => {
            return Err(ApiError::not_found("Peer task not found."));
        }
    };
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    // The task's workspace is the sending agent's; the audit row names the task. The requester
    // identity was already checked by `cancel_by_requester`, so the agent row is only read for
    // the audit labels — the requester can always cancel, whatever their access is now.
    let from_agent = row.from_agent_id;
    if let Some(from) = aurora_db::repo::agents::get(&s.pool, from_agent).await? {
        crate::audit::record(
            &s,
            crate::audit::AuditEvent::by(&c.0, "peer_task.cancel")
                .org(from.org_id)
                .workspace(from.workspace_id)
                .target("peer_task", row.id)
                .metadata(serde_json::json!({"via": "dashboard"})),
        );
    }
    Ok(Json(PeerTask::from_row(row, from_agent)))
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
    // Authorise before parsing the query (FIX-181 F7, C-1041).
    load_agent(&s, &c, agent).await?;
    if q.status.as_deref().is_some_and(|v| !STATUSES.contains(&v)) {
        return Err(ApiError::invalid_field(
            "Invalid status.",
            "/status",
            "unknown status",
        ));
    }
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = repo::list_for_agent(
        &s.pool,
        agent,
        q.status.as_deref(),
        p.after_key(),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    let mut items: Vec<PeerTask> = page
        .items
        .into_iter()
        .map(|r| PeerTask::from_row(r, agent))
        .collect();
    // RFE-009: redact references to direct conversations the caller cannot see.
    let ids: Vec<Uuid> = items
        .iter()
        .filter_map(|i| i.origin_conversation_id)
        .collect();
    let hidden = aurora_db::authz::hidden_conversations(&s.pool, c.user_id, &ids).await?;
    for i in &mut items {
        if i.origin_conversation_id
            .is_some_and(|id| hidden.contains(&id))
        {
            i.origin_conversation_id = None;
            i.conversation_hidden = true;
        }
    }
    Ok(Json(Page {
        items,
        next_cursor: page.next_cursor,
    }))
}

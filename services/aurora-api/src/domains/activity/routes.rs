use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::request_log::{self, ActivityFilter, LogRow};
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(activity))
}

#[derive(Serialize, ToSchema)]
pub struct RequestLogEntry {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    /// `agent` | `boot` | `context`
    pub credential_kind: String,
    /// `mitm` | `open` | `db`
    pub lane: String,
    pub method: String,
    pub host: String,
    pub path: String,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub connection_id: Option<Uuid>,
    pub secret_id: Option<Uuid>,
    pub decision: String,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    pub upstream_status: Option<i32>,
    pub status: i32,
    pub error_code: Option<String>,
    pub injected: i16,
    pub latency_ms: i32,
    pub bytes_up: i64,
    pub bytes_down: i64,
    /// (phase 2, RFE-025) Label of the BYO context credential (a labelled `aurora run`), if any.
    pub context_label: Option<String>,
}

impl From<LogRow> for RequestLogEntry {
    fn from(r: LogRow) -> Self {
        Self {
            id: r.id,
            ts: r.ts,
            agent_id: r.agent_id,
            conversation_id: r.conversation_id,
            turn_id: r.turn_id,
            credential_kind: r.credential_kind,
            lane: r.lane,
            method: r.method,
            host: r.host,
            path: r.path,
            app_id: r.app_id,
            tool_id: r.tool_id,
            connection_id: r.connection_id,
            secret_id: r.secret_id,
            decision: r.decision,
            rule_logical_id: r.rule_logical_id,
            rule_name: r.rule_name,
            upstream_status: r.upstream_status,
            status: r.status,
            error_code: r.error_code,
            injected: r.injected,
            latency_ms: r.latency_ms,
            bytes_up: r.bytes_up,
            bytes_down: r.bytes_down,
            context_label: r.context_label,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ActivityQuery {
    pub agent_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub decision: Option<String>,
    pub host: Option<String>,
    pub app_id: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Gateway request log of a workspace, newest first.
#[utoipa::path(get, path = "/workspaces/{ws}/activity", operation_id = "listWorkspaceActivity", tag = "activity",
    params(("ws" = Uuid, Path, description = "Workspace id"), ActivityQuery),
    responses((status = 200, body = Page<RequestLogEntry>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn activity(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<Page<RequestLogEntry>>, ApiError> {
    c.workspace(&s, ws).await?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let f = ActivityFilter {
        agent_id: q.agent_id,
        conversation_id: q.conversation_id,
        decision: q.decision,
        host: q.host,
        app_id: q.app_id,
    };
    let rows = request_log::list_activity(
        &s.pool,
        c.user_id,
        ws,
        &f,
        p.after.map(|a| (a.t, a.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.ts, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(RequestLogEntry::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

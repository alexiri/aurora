use crate::app::AppState;
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::audit::{self, AuditRow, Scope};
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(org_audit))
        .routes(routes!(workspace_audit))
}

#[derive(Serialize, ToSchema)]
pub struct AuditEntry {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    /// `user` | `service` | `agent` | `system` | `instance_admin`
    pub actor_kind: String,
    pub actor_id: Option<Uuid>,
    pub actor_label: String,
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    /// `success` | `failure` | `denied`
    pub outcome: String,
    /// `dashboard` | `api` | `cli` | `channel` | `system`
    pub source: String,
    pub ip: Option<String>,
    pub metadata: serde_json::Value,
    /// Row of the security-critical set (`04 §Audit`), written atomically with its change.
    pub critical: bool,
    pub created_at: DateTime<Utc>,
}

impl From<AuditRow> for AuditEntry {
    fn from(r: AuditRow) -> Self {
        Self {
            id: r.id,
            org_id: r.org_id,
            workspace_id: r.workspace_id,
            actor_kind: r.actor_kind,
            actor_id: r.actor_id,
            actor_label: r.actor_label,
            action: r.action,
            target_kind: r.target_kind,
            target_id: r.target_id,
            outcome: r.outcome,
            source: r.source,
            ip: r.ip,
            metadata: r.metadata,
            critical: r.critical,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Exact action, e.g. `org.create`.
    pub action: Option<String>,
    pub actor_id: Option<Uuid>,
    /// `true` lists only the security-critical set; `false` or absent lists everything.
    pub critical: Option<bool>,
}

async fn list(
    s: &AppState,
    scope: Scope,
    q: AuditQuery,
) -> Result<Json<Page<AuditEntry>>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = audit::list(
        &s.pool,
        scope,
        q.action.as_deref(),
        q.actor_id,
        q.critical.unwrap_or(false),
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(AuditEntry::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Organization audit log (owner/admin).
#[utoipa::path(get, path = "/orgs/{org}/audit", operation_id = "listOrgAudit", tag = "audit",
    params(("org" = Uuid, Path, description = "Organization id"), AuditQuery),
    responses((status = 200, body = Page<AuditEntry>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn org_audit(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Page<AuditEntry>>, ApiError> {
    c.org(&s, org).await?.require_admin()?;
    list(&s, Scope::Org(org), q).await
}

/// Workspace audit log (workspace owners).
#[utoipa::path(get, path = "/workspaces/{ws}/audit", operation_id = "listWorkspaceAudit", tag = "audit",
    params(("ws" = Uuid, Path, description = "Workspace id"), AuditQuery),
    responses((status = 200, body = Page<AuditEntry>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn workspace_audit(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Page<AuditEntry>>, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    list(&s, Scope::Workspace(ws), q).await
}

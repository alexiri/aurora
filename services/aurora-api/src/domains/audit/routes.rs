use crate::app::AppState;
use crate::http::auth::{Caller, InstanceAdmin, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::{ApiError, FieldError};
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
        .routes(routes!(instance_audit))
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

/// An audit row with the organization it concerns (instance view).
#[derive(Serialize, ToSchema)]
pub struct InstanceAuditEntry {
    #[serde(flatten)]
    pub entry: AuditEntry,
    /// Slug of `org_id` (`null` for instance-level rows).
    pub org_slug: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct InstanceAuditQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// `instance` (default): rows without an organization; `all`: every row, orgs included.
    pub scope: Option<String>,
    /// Only this organization's rows (implies `scope=all`).
    pub org: Option<Uuid>,
    /// Action prefix, e.g. `plugin.` or `trusted_key.create`.
    pub action: Option<String>,
    pub actor_id: Option<Uuid>,
    pub critical: Option<bool>,
    /// Inclusive lower bound on `created_at` (RFC 3339).
    pub since: Option<DateTime<Utc>>,
    /// Exclusive upper bound on `created_at` (RFC 3339).
    pub until: Option<DateTime<Utc>>,
}

/// Instance audit log (instance administrators): instance-level rows, or every row.
#[utoipa::path(get, path = "/admin/audit", operation_id = "listInstanceAudit", tag = "audit",
    params(InstanceAuditQuery),
    responses((status = 200, body = Page<InstanceAuditEntry>), (status = 403, body = ErrorEnvelope),
        (status = 400, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn instance_audit(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<InstanceAuditQuery>,
) -> Result<Json<Page<InstanceAuditEntry>>, ApiError> {
    let all_orgs = match q.scope.as_deref() {
        None | Some("instance") => false,
        Some("all") => true,
        Some(_) => {
            return Err(ApiError::invalid_request(
                "Invalid audit query.",
                vec![FieldError::new("/scope", "scope must be instance or all")],
            ));
        }
    };
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let action = q.action.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let rows = audit::list_instance(
        &s.pool,
        &audit::InstanceFilter {
            all_orgs,
            org: q.org,
            action_prefix: action,
            actor_id: q.actor_id,
            critical_only: q.critical.unwrap_or(false),
            since: q.since,
            until: q.until,
        },
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page
            .items
            .into_iter()
            .map(|r| InstanceAuditEntry {
                org_slug: r.org_slug.clone(),
                entry: AuditEntry::from(AuditRow {
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
                }),
            })
            .collect(),
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

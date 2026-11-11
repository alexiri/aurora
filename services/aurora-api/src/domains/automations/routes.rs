use super::dto::*;
use super::service::{self, Acting, To};
use crate::app::AppState;
use crate::audit::AuditActor;
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::authz;
use aurora_db::repo::members;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(org_offboarding))
        .routes(routes!(workspace_offboarding))
        .routes(routes!(bulk_reassign))
}

/// What suspending or removing an org member would disable, across every workspace of the org
/// (org owner/admin).
#[utoipa::path(get, path = "/orgs/{org}/members/{user}/offboarding", operation_id = "orgMemberOffboarding",
    tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id"), ("user" = Uuid, Path, description = "User id")),
    responses((status = 200, body = Offboarding), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn org_offboarding(
    c: Caller,
    State(s): State<AppState>,
    Path((org, user)): Path<(Uuid, Uuid)>,
) -> Result<Json<Offboarding>, ApiError> {
    c.org(&s, org).await?.require_admin()?;
    if members::get(&s.pool, org, user).await?.is_none() {
        return Err(ApiError::not_found("Member not found."));
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(Json(
        service::offboarding(&mut conn, org, None, user).await?,
    ))
}

/// What removing a person's binding on this workspace would disable (`can_own`). Org
/// owners/admins have implicit access that a binding removal cannot take away, so their list
/// is empty.
#[utoipa::path(get, path = "/workspaces/{ws}/members/{user}/offboarding", operation_id = "workspaceMemberOffboarding",
    tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("user" = Uuid, Path, description = "User id")),
    responses((status = 200, body = Offboarding), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn workspace_offboarding(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, user)): Path<(Uuid, Uuid)>,
) -> Result<Json<Offboarding>, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let target = authz::workspace_access(&s.pool, user, ws)
        .await?
        .ok_or_else(|| ApiError::not_found("Member not found."))?;
    if target.org_role.is_admin() {
        return Ok(Json(Offboarding {
            schedules: vec![],
            webhooks: vec![],
            peer_tasks: vec![],
            api_keys: 0,
            byo_agents_created: 0,
        }));
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(Json(
        service::offboarding(&mut conn, a.org_id, Some(ws), user).await?,
    ))
}

/// Reassign every schedule/webhook of one creator in the workspace (or just `ids`) to another
/// member or to the workspace, in one transaction with one audit row per item (`can_own`).
#[utoipa::path(post, path = "/workspaces/{ws}/automations/reassign", operation_id = "bulkReassignAutomations",
    tag = "workspaces", params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = BulkReassignRequest,
    responses((status = 200, body = BulkReassignResponse), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn bulk_reassign(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<BulkReassignRequest>,
) -> Result<Json<BulkReassignResponse>, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let to = To::parse("/to", &req.to)?;
    let (mut sch, mut hook) = (req.kinds.is_none(), req.kinds.is_none());
    for k in req.kinds.iter().flatten() {
        match k.as_str() {
            "schedule" => sch = true,
            "webhook" => hook = true,
            _ => {
                return Err(ApiError::invalid_request(
                    "Invalid request.",
                    vec![FieldError::new("/kinds", "schedule or webhook")],
                ));
            }
        }
    }
    let actor = AuditActor::from_caller(&c);
    let acting = Acting {
        actor: &actor,
        org: a.org_id,
        workspace: ws,
    };
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let n = service::reassign_all(
        &mut tx,
        &acting,
        req.from_user_id,
        to,
        sch,
        hook,
        req.ids.as_deref(),
    )
    .await?;
    // A user target is validated even when nothing matched, so a typo is not silently accepted.
    service::ensure_target(&mut tx, to, ws).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Json(BulkReassignResponse { reassigned: n }))
}

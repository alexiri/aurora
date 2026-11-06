use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::account;
use axum::extract::State;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_users))
        .routes(routes!(patch_user))
}

/// List all users (instance administrators).
#[utoipa::path(get, path = "/admin/users", operation_id = "adminListUsers", tag = "admin",
    params(ListUsersQuery),
    responses((status = 200, body = Page<AdminUser>), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_users(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<ListUsersQuery>,
) -> Result<Json<Page<AdminUser>>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let needle = q.q.as_deref().map(str::trim).filter(|v| !v.is_empty());
    let rows =
        account::list_users(&s.pool, needle, p.after.map(|c| (c.t, c.id)), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(AdminUser::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Grant/revoke instance-admin, disable/enable a user.
#[utoipa::path(patch, path = "/admin/users/{id}", operation_id = "adminPatchUser", tag = "admin",
    params(("id" = Uuid, Path, description = "User id")),
    request_body = PatchAdminUserRequest,
    responses((status = 200, body = AdminUser), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn patch_user(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchAdminUserRequest>,
) -> Result<Json<AdminUser>, ApiError> {
    let row = service::patch_user(&s, c.user_id, id, req.is_instance_admin, req.disabled).await?;
    audit::record(
        &s,
        AuditEvent::new(
            AuditActor::instance_admin(&c),
            "admin.user_update",
            Outcome::Success,
        )
        .target("user", id)
        .metadata(json!({"is_instance_admin": req.is_instance_admin, "disabled": req.disabled})),
    );
    Ok(Json(row.into()))
}

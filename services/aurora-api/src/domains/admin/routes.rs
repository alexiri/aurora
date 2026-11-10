use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{AuditActor, AuditEvent, Outcome};
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
        .routes(routes!(identity))
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
    let ids: Vec<Uuid> = page.items.iter().map(|r| r.id).collect();
    let mut states: std::collections::HashMap<Uuid, IdpRoles> =
        aurora_db::repo::idp_roles::states_of(&s.pool, &ids)
            .await?
            .into_iter()
            .map(|st| (st.user_id, IdpRoles::from(st)))
            .collect();
    Ok(Json(Page {
        items: page
            .items
            .into_iter()
            .map(|r| {
                let id = r.id;
                let mut u = AdminUser::from(r);
                u.idp_roles = states.remove(&id);
                u
            })
            .collect(),
        next_cursor: page.next_cursor,
    }))
}

/// The Helm-configured role mapping per OIDC provider, read-only (D-139, C-511).
#[utoipa::path(get, path = "/admin/identity", operation_id = "adminIdentity", tag = "admin",
    responses((status = 200, body = AdminIdentity), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn identity(_c: InstanceAdmin, State(s): State<AppState>) -> Json<AdminIdentity> {
    Json(AdminIdentity {
        providers: s
            .config
            .auth
            .oidc_providers
            .iter()
            .map(|p| IdentityProvider {
                id: p.id.clone(),
                display_name: p.display_name.clone(),
                issuer: p.issuer.clone(),
                role_claim: p.role_claim.clone(),
                role_claim_source: if p.role_claim_from_userinfo() {
                    "userinfo".into()
                } else {
                    "id_token".into()
                },
                require_role: p.require_role,
                role_mappings: p
                    .role_mappings
                    .iter()
                    .map(|m| IdentityMapping {
                        value: m.value.clone(),
                        instance_admin: m.instance_admin,
                        org: m.org.as_ref().map(|o| IdpOrgGrant {
                            slug: o.slug.clone(),
                            role: o.role.clone(),
                        }),
                        workspaces: m
                            .workspaces
                            .iter()
                            .map(|w| IdpWorkspaceGrant {
                                slug: w.slug.clone(),
                                role: w.role.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    })
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
    let ev = AuditEvent::new(
        AuditActor::instance_admin(&c),
        "admin.user_update",
        Outcome::Success,
    )
    .target("user", id)
    .metadata(json!({"is_instance_admin": req.is_instance_admin, "disabled": req.disabled}));
    let row =
        service::patch_user(&s, c.user_id, id, req.is_instance_admin, req.disabled, &ev).await?;
    Ok(Json(row.into()))
}

//! Admin routes for addon sources (C-590..C-602, P5-ASRC, D-250). All IA, all audited.

use super::dto::{
    AddonSource, AddonSourceCheckAll, AddonSourceCheckResult, AddonSourceList, AddonSourcePreview,
};
use super::{
    check_all, check_now, confirm_key, preview, register, remove, source_dto, update_interval_for,
};
use crate::app::AppState;
use crate::audit::AuditActor;
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_sources, create_source))
        .routes(routes!(check_all_route))
        .routes(routes!(preview_source))
        .routes(routes!(get_source, delete_source, update_source))
        .routes(routes!(check_source_route))
        .routes(routes!(confirm_key_route))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAddonSourceRequest {
    /// The repo URL or the direct manifest URL (https); the normalized manifest URL is stored.
    pub url: String,
    /// `6h|12h|1d|1w|2w|1mo|never`; default: `plugins.addon_sources.update_interval`.
    pub update_interval: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewAddonSourceRequest {
    pub url: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAddonSourceRequest {
    /// `6h|12h|1d|1w|2w|1mo|never`.
    pub update_interval: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmKeyRequest {
    /// The new fingerprint the admin saw (echoed back).
    pub new_key_fingerprint: String,
}

/// List addon sources (instance administrators).
#[utoipa::path(get, path = "/admin/addon-sources", operation_id = "adminListAddonSources", tag = "plugins",
    responses((status = 200, body = AddonSourceList), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_sources(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<AddonSourceList>, ApiError> {
    let rows = aurora_db::repo::addon_sources::list(&s.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        items.push(source_dto(&s, r).await?);
    }
    Ok(Json(AddonSourceList { items }))
}

/// Preview a manifest before registering (C-592): fetch and parse only, nothing is stored.
#[utoipa::path(post, path = "/admin/addon-sources/preview", operation_id = "adminPreviewAddonSource", tag = "plugins",
    request_body = PreviewAddonSourceRequest,
    responses((status = 200, body = AddonSourcePreview), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn preview_source(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<PreviewAddonSourceRequest>,
) -> Result<Json<AddonSourcePreview>, ApiError> {
    Ok(Json(preview(&s, &req.url).await?))
}

/// Register a source (TOFU: the manifest's signing key becomes a trusted key, C-593) and ingest
/// what it lists; every package waits for the ordinary approval.
#[utoipa::path(post, path = "/admin/addon-sources", operation_id = "adminCreateAddonSource", tag = "plugins",
    request_body = CreateAddonSourceRequest,
    responses((status = 201, body = AddonSource), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<CreateAddonSourceRequest>,
) -> Result<(StatusCode, Json<AddonSource>), ApiError> {
    let actor = AuditActor::instance_admin(&c);
    let interval = req
        .update_interval
        .clone()
        .unwrap_or_else(|| s.config.plugins.addon_sources.update_interval.clone());
    let reg = register(&s, &actor, c.user_id, &req.url, &interval).await?;
    Ok((
        StatusCode::CREATED,
        Json(source_dto(&s, &reg.source).await?),
    ))
}

/// One addon source (from its last accepted manifest, not a live fetch).
#[utoipa::path(get, path = "/admin/addon-sources/{id}", operation_id = "adminGetAddonSource", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    responses((status = 200, body = AddonSource), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_source(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AddonSource>, ApiError> {
    let r = aurora_db::repo::addon_sources::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Addon source not found."))?;
    Ok(Json(source_dto(&s, &r).await?))
}

/// Edit the check schedule (C-594).
#[utoipa::path(patch, path = "/admin/addon-sources/{id}", operation_id = "adminUpdateAddonSource", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    request_body = UpdateAddonSourceRequest,
    responses((status = 200, body = AddonSource), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn update_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAddonSourceRequest>,
) -> Result<Json<AddonSource>, ApiError> {
    let r = update_interval_for(
        &s,
        &AuditActor::instance_admin(&c),
        id,
        &req.update_interval,
    )
    .await?;
    Ok(Json(source_dto(&s, &r).await?))
}

/// Remove a source (refused while its packages are active; packages stay, C-597).
#[utoipa::path(delete, path = "/admin/addon-sources/{id}", operation_id = "adminDeleteAddonSource", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    remove(&s, &AuditActor::instance_admin(&c), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Check one source now (the shared poll path with `trigger = manual`, C-596).
#[utoipa::path(post, path = "/admin/addon-sources/{id}/check", operation_id = "adminCheckAddonSource", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    responses((status = 200, body = AddonSourceCheckResult), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn check_source_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AddonSourceCheckResult>, ApiError> {
    Ok(Json(
        check_now(&s, &AuditActor::instance_admin(&c), id).await?,
    ))
}

/// Check every source (C-596: per-source outcomes, a failure never blocks the others).
#[utoipa::path(post, path = "/admin/addon-sources/check-all", operation_id = "adminCheckAllAddonSources", tag = "plugins",
    responses((status = 200, body = AddonSourceCheckAll), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn check_all_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<AddonSourceCheckAll>, ApiError> {
    Ok(Json(check_all(&s, &AuditActor::instance_admin(&c)).await?))
}

/// Re-confirm a changed signing key (C-595): updates the source and its trusted key.
#[utoipa::path(post, path = "/admin/addon-sources/{id}/confirm-key", operation_id = "adminConfirmAddonSourceKey", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    request_body = ConfirmKeyRequest,
    responses((status = 200, body = AddonSource), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn confirm_key_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ConfirmKeyRequest>,
) -> Result<Json<AddonSource>, ApiError> {
    let r = confirm_key(
        &s,
        &AuditActor::instance_admin(&c),
        id,
        &req.new_key_fingerprint,
    )
    .await?;
    Ok(Json(source_dto(&s, &r).await?))
}

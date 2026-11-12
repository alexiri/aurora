//! Admin routes for addon sources (C-590..C-602, P5-ASRC, D-164). All IA, all audited.

use super::manifest;
use super::{check_all, check_now, confirm_key, remove, to_dto, update_interval_for};
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
    /// The repo URL or the direct manifest URL (https).
    pub url: String,
    /// `6h|12h|1d|1w|2w|1mo|never` (default `1d`).
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
    responses((status = 200, body = serde_json::Value), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_sources(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows = aurora_db::repo::addon_sources::list(&s.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        items.push(to_dto(&s, r).await?);
    }
    Ok(Json(serde_json::json!({ "items": items })))
}

/// Preview a manifest before registering (C-592): fetch and parse only.
#[utoipa::path(post, path = "/admin/addon-sources/preview", operation_id = "adminPreviewAddonSource", tag = "plugins",
    request_body = PreviewAddonSourceRequest,
    responses((status = 200, body = serde_json::Value), (status = 403, body = ErrorEnvelope),
        (status = 413, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn preview_source(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<PreviewAddonSourceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = manifest::normalize_url(&req.url).map_err(super::refused_public)?;
    let (m, _bytes) = super::fetch_manifest_public(&s, &url).await?;
    Ok(Json(serde_json::json!({
        "url": url,
        "repo": {
            "name": m.name, "description": m.description,
            "homepage": m.homepage, "contact": m.contact,
        },
        "publisher": m.publisher,
        "key_fingerprint": manifest::key_fingerprint(&m.signing_key),
        "key_description": m.signing_key_description,
        "addons": super::addon_views(&m, &std::collections::BTreeMap::new())
            .iter().map(|a| serde_json::json!({
                "id": a.id, "name": a.name, "description": a.description,
                "versions": a.versions.iter().map(|v| serde_json::json!({
                    "version": v.version, "min_core": v.min_core,
                    "ingestable": v.ingestable,
                    "not_ingestable_reason": v.not_ingestable_reason,
                    "installed_version": v.installed_version,
                    "update_available": v.update_available,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        "packages_new": m.addons.iter()
            .map(|a| a.versions.len()).sum::<usize>(),
    })))
}

/// Register a source (TOFU: the manifest's signing key becomes a trusted key, C-593).
#[utoipa::path(post, path = "/admin/addon-sources", operation_id = "adminCreateAddonSource", tag = "plugins",
    request_body = CreateAddonSourceRequest,
    responses((status = 201, body = serde_json::Value), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<CreateAddonSourceRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let actor = AuditActor::instance_admin(&c);
    let reg = super::register(
        &s,
        &actor,
        c.user_id,
        &req.url,
        req.update_interval.as_deref().unwrap_or("1d"),
    )
    .await?;
    // Ingest the listed packages (outside the registration transaction: a failing version
    // must not roll the source back; C-593/C-607).
    let (m, _bytes) = super::fetch_manifest_public(&s, &reg.source.url).await?;
    for a in &m.addons {
        for (v, ver) in &a.versions {
            if super::core_supports_public(ver.min_core.as_deref()).is_err() {
                continue;
            }
            let Some(sem) = aurora_registry::manifest::parse_semver(v) else {
                continue;
            };
            if !super::is_newer(&s, &a.id, sem).await? {
                continue;
            }
            match super::fetch_package(&s, &ver.package).await {
                Ok(bytes) => {
                    if super::digest_matches(&bytes, &ver.sha256) {
                        let _ = crate::domains::plugins::ingest::ingest_for_source(
                            &s,
                            &bytes,
                            &reg.source,
                            &actor,
                        )
                        .await;
                    }
                }
                Err(_) => continue,
            }
        }
    }
    let dto = to_dto(&s, &reg.source).await?;
    Ok((StatusCode::CREATED, Json(dto)))
}

/// One addon source.
#[utoipa::path(get, path = "/admin/addon-sources/{id}", operation_id = "adminGetAddonSource", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    responses((status = 200, body = serde_json::Value), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_source(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let r = aurora_db::repo::addon_sources::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Addon source not found."))?;
    Ok(Json(to_dto(&s, &r).await?))
}

/// Edit the check schedule (C-594).
#[utoipa::path(patch, path = "/admin/addon-sources/{id}", operation_id = "adminUpdateAddonSource", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    request_body = UpdateAddonSourceRequest,
    responses((status = 200, body = serde_json::Value), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn update_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAddonSourceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let r = update_interval_for(
        &s,
        &AuditActor::instance_admin(&c),
        id,
        &req.update_interval,
    )
    .await?;
    Ok(Json(to_dto(&s, &r).await?))
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
    responses((status = 200, body = serde_json::Value), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn check_source_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (status, ingested, failed) = check_now(&s, &AuditActor::instance_admin(&c), id).await?;
    Ok(Json(serde_json::json!({
        "source_id": id, "status": status,
        "ingested": ingested.iter().map(|p| serde_json::json!({"id": p.id,
            "package_id": p.package_id, "version": p.version, "status": p.status}))
            .collect::<Vec<_>>(),
        "failed_versions": failed.iter().map(|(p, v, r)| serde_json::json!({
            "package_id": p, "version": v, "reason": r})).collect::<Vec<_>>(),
        "check_failed_reason": serde_json::Value::Null,
    })))
}

/// Check every source (C-596: per-source outcomes, a failure never blocks the others).
#[utoipa::path(post, path = "/admin/addon-sources/check-all", operation_id = "adminCheckAllAddonSources", tag = "plugins",
    responses((status = 200, body = serde_json::Value), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn check_all_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let items = check_all(&s, &AuditActor::instance_admin(&c)).await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// Re-confirm a changed signing key (C-595): updates the source and its trusted key.
#[utoipa::path(post, path = "/admin/addon-sources/{id}/confirm-key", operation_id = "adminConfirmAddonSourceKey", tag = "plugins",
    params(("id" = Uuid, Path, description = "Addon source id")),
    request_body = ConfirmKeyRequest,
    responses((status = 200, body = serde_json::Value), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn confirm_key_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ConfirmKeyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let r = confirm_key(
        &s,
        &AuditActor::instance_admin(&c),
        id,
        &req.new_key_fingerprint,
    )
    .await?;
    Ok(Json(to_dto(&s, &r).await?))
}

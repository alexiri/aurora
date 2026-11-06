use super::dto::*;
use super::ingest::{self, Source};
use super::{keys, service};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams, Pagination};
use aurora_core::limits::PACKAGE_MAX;
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::plugins;
use axum::extract::{DefaultBodyLimit, Multipart, State, multipart::MultipartRejection};
use http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_plugins, upload_plugin))
        .routes(routes!(get_plugin))
        .routes(routes!(list_trusted_keys, create_trusted_key))
        .routes(routes!(revoke_trusted_key))
        // Multipart framing overhead on top of the package cap.
        .layer(DefaultBodyLimit::max(PACKAGE_MAX + 64 * 1024))
}

/// List plugin packages (instance administrators).
#[utoipa::path(get, path = "/admin/plugins", operation_id = "adminListPlugins", tag = "plugins",
    params(ListPluginsQuery),
    responses((status = 200, body = Page<PluginPackage>), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_plugins(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<ListPluginsQuery>,
) -> Result<Json<Page<PluginPackage>>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    if let Some(st) = q.status.as_deref() {
        service::check_status(st)?;
    }
    let rows = plugins::list(
        &s.pool,
        q.status.as_deref(),
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    let mut items = Vec::with_capacity(page.items.len());
    for r in page.items {
        items.push(service::to_dto(&s, r).await?);
    }
    Ok(Json(Page {
        items,
        next_cursor: page.next_cursor,
    }))
}

/// Upload a plugin package (multipart part `package`).
#[utoipa::path(post, path = "/admin/plugins", operation_id = "adminUploadPlugin", tag = "plugins",
    request_body(content = UploadPluginRequest, content_type = "multipart/form-data"),
    responses((status = 201, body = PluginPackage), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn upload_plugin(
    c: InstanceAdmin,
    State(s): State<AppState>,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<PluginPackage>), ApiError> {
    let mut mp = mp.map_err(|_| {
        ApiError::invalid_request(
            "Expected multipart/form-data.",
            vec![FieldError::new(
                "/",
                "content type must be multipart/form-data",
            )],
        )
    })?;
    let mut bytes = None;
    while let Some(field) = mp.next_field().await.map_err(multipart_error)? {
        if field.name() != Some("package") || bytes.is_some() {
            return Err(ApiError::invalid_request(
                "Unexpected multipart part.",
                vec![FieldError::new(
                    "/package",
                    "exactly one part `package` is allowed",
                )],
            ));
        }
        bytes = Some(field.bytes().await.map_err(multipart_error)?);
    }
    let bytes = bytes.ok_or_else(|| {
        ApiError::invalid_request(
            "Missing package.",
            vec![FieldError::new("/package", "part `package` is required")],
        )
    })?;
    let out = ingest::ingest(&s, &bytes, Source::Upload, &AuditActor::instance_admin(&c)).await?;
    Ok((
        StatusCode::CREATED,
        Json(service::to_dto(&s, out.row).await?),
    ))
}

fn multipart_error(e: axum::extract::multipart::MultipartError) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::too_large("Package is too large.")
    } else {
        ApiError::invalid_request(
            "Malformed multipart body.",
            vec![FieldError::new("/", "malformed multipart body")],
        )
    }
}

/// One plugin package.
#[utoipa::path(get, path = "/admin/plugins/{id}", operation_id = "adminGetPlugin", tag = "plugins",
    params(("id" = Uuid, Path, description = "Package row id")),
    responses((status = 200, body = PluginPackage), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_plugin(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<PluginPackage>, ApiError> {
    let row = service::get_row(&s, id).await?;
    Ok(Json(service::to_dto(&s, row).await?))
}

/// List trusted signing keys, including revoked ones.
#[utoipa::path(get, path = "/admin/trusted-keys", operation_id = "adminListTrustedKeys", tag = "plugins",
    params(PageParams),
    responses((status = 200, body = Page<TrustedKey>), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_trusted_keys(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<TrustedKey>>, ApiError> {
    let p = q.resolve()?;
    let rows = plugins::list_trusted_keys(&s.pool, p.after.map(|c| (c.t, c.id)), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(TrustedKey::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Trust a publisher key.
#[utoipa::path(post, path = "/admin/trusted-keys", operation_id = "adminCreateTrustedKey", tag = "plugins",
    request_body = CreateTrustedKeyRequest,
    responses((status = 201, body = TrustedKey), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_trusted_key(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<CreateTrustedKeyRequest>,
) -> Result<(StatusCode, Json<TrustedKey>), ApiError> {
    let row = keys::create(&s, c.user_id, &req.publisher, &req.name, &req.public_key).await?;
    audit::record(
        &s,
        AuditEvent::new(
            AuditActor::instance_admin(&c),
            "trusted_key.create",
            Outcome::Success,
        )
        .target("trusted_key", row.id)
        .metadata(json!({"publisher": row.publisher, "name": row.name})),
    );
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Revoke a trusted key; packages it signed become `disabled`.
#[utoipa::path(delete, path = "/admin/trusted-keys/{id}", operation_id = "adminRevokeTrustedKey", tag = "plugins",
    params(("id" = Uuid, Path, description = "Trusted key id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn revoke_trusted_key(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (key, disabled) = keys::revoke(&s, id).await?;
    audit::record(
        &s,
        AuditEvent::new(
            AuditActor::instance_admin(&c),
            "trusted_key.revoke",
            Outcome::Success,
        )
        .target("trusted_key", key.id)
        .metadata(json!({"publisher": key.publisher, "disabled_packages": disabled})),
    );
    Ok(StatusCode::NO_CONTENT)
}

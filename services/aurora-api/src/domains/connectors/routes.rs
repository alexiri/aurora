//! Management routes: instance administration of connectors and the public catalogue.

use super::dto::*;
use super::{admin, managed};
use crate::app::AppState;
use crate::audit::AuditActor;
use crate::http::auth::{Caller, InstanceAdmin};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_connectors))
        .routes(routes!(connector_icon))
        .routes(routes!(admin_list, admin_create))
        .routes(routes!(admin_delete, admin_patch))
        .routes(routes!(admin_connector_types))
        .routes(routes!(admin_overview))
        .routes(routes!(admin_approve))
        .routes(routes!(admin_rotate))
}

/// Active connectors available for integrations.
#[utoipa::path(get, path = "/connectors", operation_id = "listConnectors", tag = "channels",
    responses((status = 200, body = ConnectorInfoList), (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_connectors(
    _c: Caller,
    State(s): State<AppState>,
) -> Result<Json<ConnectorInfoList>, ApiError> {
    Ok(Json(admin::catalogue(&s).await?))
}

/// Connector icon (`image/png`).
#[utoipa::path(get, path = "/connectors/{id}/icon", operation_id = "getConnectorIcon", tag = "channels",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 200, description = "PNG", content_type = "image/png", body = Vec<u8>),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn connector_icon(
    _c: Caller,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let bytes = admin::icon(&s, &id).await?;
    let mut r = (StatusCode::OK, bytes).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(r)
}

/// List connectors with replicas, integration counts and widening reasons (instance administrators).
#[utoipa::path(get, path = "/admin/connectors", operation_id = "adminListConnectors", tag = "admin",
    responses((status = 200, body = ConnectorAdminList), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_list(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<ConnectorAdminList>, ApiError> {
    Ok(Json(admin::list(&s).await?))
}

/// Provision a self-hosted connector (the `aun_` token is returned once) or, with `managed: true`,
/// add a managed connector run by a connector host (no token; C-451).
#[utoipa::path(post, path = "/admin/connectors", operation_id = "adminCreateConnector", tag = "admin",
    request_body = CreateConnectorRequest,
    responses((status = 201, body = ConnectorWithToken), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 503, description = "secret_store_unavailable", body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_create(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<CreateConnectorRequest>,
) -> Result<(StatusCode, Json<ConnectorWithToken>), ApiError> {
    let out = admin::create(&s, req, &AuditActor::instance_admin(&c)).await?;
    Ok((StatusCode::CREATED, Json(out)))
}

/// Delete a connector (409 `connector_in_use` while integrations exist).
#[utoipa::path(delete, path = "/admin/connectors/{id}", operation_id = "adminDeleteConnector", tag = "admin",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_delete(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    admin::delete(&s, &id, &AuditActor::instance_admin(&c)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Edit a connector (C-453): name, `enabled`; for a managed connector also settings, secrets and
/// regeneration of generated secrets (write-only; no response carries a secret value).
#[utoipa::path(patch, path = "/admin/connectors/{id}", operation_id = "adminUpdateConnector", tag = "admin",
    params(("id" = String, Path, description = "Connector id")),
    request_body = PatchConnectorRequest,
    responses((status = 200, body = ConnectorAdmin), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 503, description = "secret_store_unavailable", body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_patch(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchConnectorRequest>,
) -> Result<Json<ConnectorAdmin>, ApiError> {
    Ok(Json(
        managed::patch(&s, &id, req, &AuditActor::instance_admin(&c)).await?,
    ))
}

/// Connector types announced by the live connector hosts (C-450).
#[utoipa::path(get, path = "/admin/connector-types", operation_id = "adminListConnectorTypes", tag = "admin",
    responses((status = 200, body = ConnectorTypeList), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_connector_types(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<ConnectorTypeList>, ApiError> {
    Ok(Json(managed::catalogue(&s).await?))
}

/// Instance overview (C-459): the connector hosts seen within 24 hours.
#[utoipa::path(get, path = "/admin/overview", operation_id = "adminOverview", tag = "admin",
    responses((status = 200, body = AdminOverview), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_overview(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<AdminOverview>, ApiError> {
    Ok(Json(managed::overview(&s).await?))
}

/// Approve exactly the announced manifest (409 `stale_manifest` if it changed).
#[utoipa::path(post, path = "/admin/connectors/{id}/approve", operation_id = "adminApproveConnector", tag = "admin",
    params(("id" = String, Path, description = "Connector id")),
    request_body = ApproveConnectorRequest,
    responses((status = 200, body = ConnectorAdmin), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_approve(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ApproveConnectorRequest>,
) -> Result<Json<ConnectorAdmin>, ApiError> {
    let out = admin::approve(
        &s,
        &id,
        &req.manifest_digest,
        c.user_id,
        &AuditActor::instance_admin(&c),
    )
    .await?;
    Ok(Json(out))
}

/// Rotate the connector token; the new `aun_` token is returned once.
#[utoipa::path(post, path = "/admin/connectors/{id}/token", operation_id = "adminRotateConnectorToken", tag = "admin",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 200, body = ConnectorToken), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_rotate(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConnectorToken>, ApiError> {
    let out = admin::rotate(&s, &id, &AuditActor::instance_admin(&c)).await?;
    Ok(Json(out))
}

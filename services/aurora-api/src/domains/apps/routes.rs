use super::dto::*;
use super::{assets, service};
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::{ApiError, FieldError};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_apps))
        .routes(routes!(get_app))
        .routes(routes!(app_icon))
        .routes(routes!(app_guide))
}

/// App catalogue from the active registry snapshot, with availability and connection counts.
#[utoipa::path(
    get, path = "/apps", operation_id = "listApps", tag = "apps",
    params(AppsQuery),
    responses((status = 200, description = "Apps", body = AppList),
              (status = 404, description = "Workspace not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_apps(
    caller: Caller,
    State(state): State<AppState>,
    Query(q): Query<AppsQuery>,
) -> Result<Json<AppList>, ApiError> {
    let av = service::avail(&state, &caller, q.workspace_id).await?;
    let mut items = Vec::new();
    if let Some(snap) = service::snapshot(&state).await? {
        for a in &snap.apps {
            if let Ok((a, p)) = service::find_app(&snap, &a.app_id) {
                items.push(service::summary(&state, a, p, &av).await);
            }
        }
    }
    Ok(Json(AppList { items }))
}

/// App detail: connect methods, scopes, tools, hosts and agent documentation.
#[utoipa::path(
    get, path = "/apps/{app}", operation_id = "getApp", tag = "apps",
    params(("app" = String, Path, description = "App id"), AppsQuery),
    responses((status = 200, description = "App", body = AppDetail),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_app(
    caller: Caller,
    State(state): State<AppState>,
    Path(app): Path<String>,
    Query(q): Query<AppsQuery>,
) -> Result<Json<AppDetail>, ApiError> {
    let av = service::avail(&state, &caller, q.workspace_id).await?;
    let snap = service::snapshot(&state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let (a, p) = service::find_app(&snap, &app)?;
    Ok(Json(service::detail(&state, &snap, a, p, &av).await))
}

fn with_type(bytes: Vec<u8>, ct: &'static str) -> Response {
    let mut r = (StatusCode::OK, bytes).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    r
}

/// App icon (`image/png`).
#[utoipa::path(
    get, path = "/apps/{app}/icon", operation_id = "getAppIcon", tag = "apps",
    params(("app" = String, Path, description = "App id"), IconQuery),
    responses((status = 200, description = "PNG", content_type = "image/png", body = Vec<u8>),
              (status = 400, description = "Invalid variant", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn app_icon(
    _caller: Caller,
    State(state): State<AppState>,
    Path(app): Path<String>,
    Query(q): Query<IconQuery>,
) -> Result<Response, ApiError> {
    let dark = match q.variant.as_deref() {
        None | Some("light") => false,
        Some("dark") => true,
        Some(_) => {
            return Err(ApiError::invalid_request(
                "Invalid variant.",
                vec![FieldError::new("/variant", "must be light or dark")],
            ));
        }
    };
    let snap = service::snapshot(&state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let (_, p) = service::find_app(&snap, &app)?;
    Ok(with_type(
        assets::icon(&state, &p.digest, dark).await?,
        "image/png",
    ))
}

/// Setup guide (`text/markdown`).
#[utoipa::path(
    get, path = "/apps/{app}/guide", operation_id = "getAppGuide", tag = "apps",
    params(("app" = String, Path, description = "App id")),
    responses((status = 200, description = "Markdown", content_type = "text/markdown", body = String),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn app_guide(
    _caller: Caller,
    State(state): State<AppState>,
    Path(app): Path<String>,
) -> Result<Response, ApiError> {
    let snap = service::snapshot(&state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let (_, p) = service::find_app(&snap, &app)?;
    Ok(with_type(
        assets::guide(&state, &p.digest).await?,
        "text/markdown; charset=utf-8",
    ))
}

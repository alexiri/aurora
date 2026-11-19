use super::dto::*;
use super::{assets, guide_render, service, setup};
use crate::app::AppState;
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::contacts;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_apps))
        .routes(routes!(get_app))
        .routes(routes!(app_icon))
        .routes(routes!(app_guide))
        .routes(routes!(app_setup))
        .routes(routes!(app_connect_plan))
}

/// Resolve and authorize the level of a setup status: a workspace needs `can_own`, an
/// organization alone needs owner or admin (the refusals carry `who_can`, C-981).
async fn setup_view(
    caller: &Caller,
    state: &AppState,
    q: &SetupQuery,
) -> Result<setup::SetupView, ApiError> {
    match (q.workspace_id, q.org_id) {
        (Some(ws), org) => {
            let a = caller.workspace(state, ws).await?;
            require_own(&a)?;
            if org.is_some_and(|o| o != a.org_id) {
                return Err(ApiError::invalid_request(
                    "org_id does not match the workspace.",
                    vec![FieldError::new(
                        "/org_id",
                        "not the workspace's organization",
                    )],
                ));
            }
            Ok(setup::SetupView {
                org: a.org_id,
                workspace: Some(ws),
                org_admin: a.org_role.is_admin(),
            })
        }
        (None, Some(org)) => {
            caller.org(state, org).await?.require_admin()?;
            Ok(setup::SetupView {
                org,
                workspace: None,
                org_admin: true,
            })
        }
        (None, None) => Err(ApiError::invalid_request(
            "Name a level.",
            vec![FieldError::new(
                "/workspace_id",
                "give workspace_id, or org_id for an organization",
            )],
        )),
    }
}

/// Setup status of an app for a workspace or an organization: which ways to connect work here,
/// what is missing and who can fix it (C-931).
#[utoipa::path(
    get, path = "/apps/{app}/setup", operation_id = "getAppSetup", tag = "apps",
    params(("app" = String, Path, description = "App id"), SetupQuery),
    responses((status = 200, description = "Setup status", body = setup::AppSetupStatus),
              (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn app_setup(
    caller: Caller,
    State(state): State<AppState>,
    Path(app): Path<String>,
    Query(q): Query<SetupQuery>,
) -> Result<Json<setup::AppSetupStatus>, ApiError> {
    let view = setup_view(&caller, &state, &q).await?;
    Ok(Json(setup::setup_status(&state, &app, view).await?))
}

/// The connect journey's plan: a projection of the setup status (C-955).
#[utoipa::path(
    get, path = "/apps/{app}/connect-plan", operation_id = "getAppConnectPlan", tag = "apps",
    params(("app" = String, Path, description = "App id"), SetupQuery),
    responses((status = 200, description = "Connect plan", body = setup::ConnectPlan),
              (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn app_connect_plan(
    caller: Caller,
    State(state): State<AppState>,
    Path(app): Path<String>,
    Query(q): Query<SetupQuery>,
) -> Result<Json<setup::ConnectPlan>, ApiError> {
    let view = setup_view(&caller, &state, &q).await?;
    Ok(Json(setup::setup_status(&state, &app, view).await?.into()))
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
        // (P5-POLISH-3, C-1029) Readiness for every app in one pass, and the workspace's accounts of apps
        // in one list, when the catalogue is read for a workspace.
        let mut facts = match (av.view(), q.workspace_id) {
            (Some(view), Some(_)) => {
                let (slug, _) = contacts::slugs(&state.pool, view.org, view.workspace)
                    .await?
                    .ok_or_else(|| ApiError::not_found("Organization not found."))?;
                Some(setup::Facts::load(&state, view, slug, None).await?)
            }
            _ => None,
        };
        let mut asked = match av.view() {
            Some(view) if q.workspace_id.is_some() => {
                crate::domains::app_setup_requests::service::requested(
                    &state,
                    view.org,
                    caller.user_id,
                )
                .await?
            }
            _ => HashMap::new(),
        };
        let mut accounts = match q.workspace_id {
            Some(ws) => service::accounts_by_app(&state, &caller, ws).await?,
            None => HashMap::new(),
        };
        for a in &snap.apps {
            let Ok((a, p)) = service::find_app(&snap, &a.app_id) else {
                continue;
            };
            let available = av.is_available(&a.app_id);
            // Apps switched off are not returned to callers who cannot change availability.
            if !available && av.view().is_some() && !av.can_change_availability() {
                continue;
            }
            let mut item = service::summary(&state, a, p, &av).await;
            if let Some(f) = facts.as_mut() {
                let mut r = setup::readiness(f, a, p, available).await;
                r.requested = asked.remove(&a.app_id);
                item.readiness = Some(r);
                item.accounts = accounts.remove(&a.app_id).unwrap_or_default();
            }
            items.push(item);
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

/// Setup guide (`text/markdown`), filled in for the caller (C-940): the callback URL, the
/// organization's external id (members only) and links to the app's setup page and to the tabs
/// the guide names, at the highest level the caller can open, else who can. Name the level with
/// `workspace_id` or `org_id`; without either only the callback URL is a real value.
#[utoipa::path(
    get, path = "/apps/{app}/guide", operation_id = "getAppGuide", tag = "apps",
    params(("app" = String, Path, description = "App id"), SetupQuery),
    responses((status = 200, description = "Markdown", content_type = "text/markdown", body = String),
              (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn app_guide(
    caller: Caller,
    State(state): State<AppState>,
    Path(app): Path<String>,
    Query(q): Query<SetupQuery>,
) -> Result<Response, ApiError> {
    let snap = service::snapshot(&state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let (a, p) = service::find_app(&snap, &app)?;
    let raw = assets::guide(&state, &p.digest).await?;
    let values = guide_render::resolve(
        &state,
        &caller,
        &a.app_id,
        &a.about.name,
        q.workspace_id,
        q.org_id,
    )
    .await?;
    let text = guide_render::render(&String::from_utf8_lossy(&raw), &values);
    Ok(with_type(text.into_bytes(), "text/markdown; charset=utf-8"))
}

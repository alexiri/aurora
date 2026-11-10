use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_libraries, create_library))
        .routes(routes!(get_library, patch_library, delete_library))
        .routes(routes!(refresh_library))
        .routes(routes!(browse_library))
        .routes(routes!(get_library_skill))
}

/// Org owner/admin of the library's org (404 for anyone who cannot see the org).
async fn admin_of(
    c: &Caller,
    s: &AppState,
    library: Uuid,
) -> Result<aurora_db::repo::skill_libraries::LibraryRow, ApiError> {
    service::ensure_enabled(s)?;
    let row = service::get(s, library).await?;
    c.org(s, row.org_id).await?.require_admin()?;
    Ok(row)
}

/// Browse audience: any person who can use a workspace of the org (org owners/admins included).
async fn browse_of(
    c: &Caller,
    s: &AppState,
    library: Uuid,
) -> Result<aurora_db::repo::skill_libraries::LibraryRow, ApiError> {
    service::ensure_enabled(s)?;
    let row = service::get(s, library).await?;
    browse_audience(c, s, row.org_id).await?;
    Ok(row)
}

pub async fn browse_audience(c: &Caller, s: &AppState, org: Uuid) -> Result<bool, ApiError> {
    let a = c.org(s, org).await?;
    if a.role.is_admin() {
        return Ok(true);
    }
    if aurora_db::authz::uses_any_workspace(&s.pool, c.user_id, org).await? {
        Ok(false)
    } else {
        Err(ApiError::forbidden("Workspace access required."))
    }
}

/// List the organization's libraries: owners/admins get everything; other people who can use a
/// workspace get `id`, `label` and `status` of the active ones (the browse picker).
#[utoipa::path(get, path = "/orgs/{org}/skill-libraries", operation_id = "listSkillLibraries",
    tag = "skill-libraries", params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = Page<SkillLibrary>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_libraries(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<Page<SkillLibrary>>, ApiError> {
    service::ensure_enabled(&s)?;
    let admin = browse_audience(&c, &s, org).await?;
    let rows = service::list(&s, org).await?;
    let mut items = Vec::new();
    for r in rows {
        if !admin && r.status != "active" {
            continue;
        }
        items.push(service::view(&s, &r, admin).await?);
    }
    Ok(Json(Page {
        items,
        next_cursor: None,
    }))
}

/// Connect a repository as a skill library (org owner/admin).
#[utoipa::path(post, path = "/orgs/{org}/skill-libraries", operation_id = "createSkillLibrary",
    tag = "skill-libraries", params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateSkillLibraryRequest,
    responses((status = 201, body = SkillLibrary), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_library(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateSkillLibraryRequest>,
) -> Result<(StatusCode, Json<SkillLibrary>), ApiError> {
    service::ensure_enabled(&s)?;
    c.org(&s, org).await?.require_admin()?;
    let row = service::create(&s, &c, org, &req).await?;
    Ok((
        StatusCode::CREATED,
        Json(service::view(&s, &row, true).await?),
    ))
}

/// Read one library (org owner/admin).
#[utoipa::path(get, path = "/skill-libraries/{id}", operation_id = "getSkillLibrary",
    tag = "skill-libraries", params(("id" = Uuid, Path, description = "Library id")),
    responses((status = 200, body = SkillLibrary), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SkillLibrary>, ApiError> {
    let row = admin_of(&c, &s, id).await?;
    Ok(Json(service::view(&s, &row, true).await?))
}

/// Rename, pause or resume, or change connection, ref or path (re-resolves; stored skills are
/// never touched).
#[utoipa::path(patch, path = "/skill-libraries/{id}", operation_id = "updateSkillLibrary",
    tag = "skill-libraries", params(("id" = Uuid, Path, description = "Library id")),
    request_body = PatchSkillLibraryRequest,
    responses((status = 200, body = SkillLibrary), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchSkillLibraryRequest>,
) -> Result<Json<SkillLibrary>, ApiError> {
    admin_of(&c, &s, id).await?;
    let row = service::patch(&s, &c, id, &req).await?;
    Ok(Json(service::view(&s, &row, true).await?))
}

/// Delete the library; every skill added from it is detached and keeps its content.
#[utoipa::path(delete, path = "/skill-libraries/{id}", operation_id = "deleteSkillLibrary",
    tag = "skill-libraries", params(("id" = Uuid, Path, description = "Library id")),
    responses((status = 204, description = "Deleted"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    admin_of(&c, &s, id).await?;
    service::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// "Check now": refresh soon (at most once per minute per library).
#[utoipa::path(post, path = "/skill-libraries/{id}/refresh", operation_id = "refreshSkillLibrary",
    tag = "skill-libraries", params(("id" = Uuid, Path, description = "Library id")),
    responses((status = 202, description = "Queued"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn refresh_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    admin_of(&c, &s, id).await?;
    service::check_now(&s, id).await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BrowseParams {
    /// Case-insensitive filter on name and description.
    pub q: Option<String>,
    pub cursor: Option<String>,
}

const BROWSE_PAGE: usize = 50;

/// Browse the library's current head live (nothing is stored; cached for 5 minutes).
#[utoipa::path(get, path = "/skill-libraries/{id}/skills", operation_id = "browseSkillLibrary",
    tag = "skill-libraries",
    params(("id" = Uuid, Path, description = "Library id"), BrowseParams),
    responses((status = 200, body = LibraryCatalogue), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 502, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn browse_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<BrowseParams>,
) -> Result<Response, ApiError> {
    let row = browse_of(&c, &s, id).await?;
    let l = service::listing(&s, &row).await?;
    let added = crate::domains::skills::library::added_by_path(&s, row.id).await?;
    let needle =
        q.q.as_deref()
            .map(str::to_lowercase)
            .filter(|n| !n.is_empty());
    let matching: Vec<_> = l
        .items
        .iter()
        .filter(|i| {
            needle.as_ref().is_none_or(|n| {
                i.name.to_lowercase().contains(n)
                    || i.description
                        .as_deref()
                        .is_some_and(|d| d.to_lowercase().contains(n))
            })
        })
        .collect();
    let start: usize = q
        .cursor
        .as_deref()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let page: Vec<LibrarySkillItem> = matching
        .iter()
        .skip(start)
        .take(BROWSE_PAGE)
        .map(|i| LibrarySkillItem {
            path: i.path.clone(),
            name: i.name.clone(),
            description: i.description.clone(),
            valid: i.valid,
            reason: i.reason.clone(),
            size: i.size,
            files: i.files,
            added: added.get(&i.path).cloned().unwrap_or_default(),
        })
        .collect();
    let next = (start + BROWSE_PAGE < matching.len()).then(|| (start + BROWSE_PAGE).to_string());
    let body = LibraryCatalogue {
        commit_sha: l.commit_sha.clone(),
        items: page,
        truncated: l.truncated,
        next_cursor: next,
    };
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SkillParams {
    /// The commit to read; default the current head.
    pub commit_sha: Option<String>,
}

/// One skill's full content at a commit: exactly what an add would store (and the diff base).
#[utoipa::path(get, path = "/skill-libraries/{id}/skills/{path}",
    operation_id = "getSkillLibrarySkill", tag = "skill-libraries",
    params(("id" = Uuid, Path, description = "Library id"),
        ("path" = String, Path, description = "The skill's directory name"), SkillParams),
    responses((status = 200, body = LibrarySkillContent), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 502, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_library_skill(
    c: Caller,
    State(s): State<AppState>,
    Path((id, path)): Path<(Uuid, String)>,
    Query(q): Query<SkillParams>,
) -> Result<Response, ApiError> {
    let row = browse_of(&c, &s, id).await?;
    let content = service::skill_content(&s, &row, &path, q.commit_sha.as_deref()).await?;
    let mut resp = Json(content).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

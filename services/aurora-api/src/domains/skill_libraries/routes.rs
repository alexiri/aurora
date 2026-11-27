use super::access::{self, ReadAs};
use super::dto::*;
use super::service::{self, View};
use crate::app::AppState;
use crate::domains::skills::service::Owner;
use crate::http::auth::{Caller, InstanceAdmin};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use aurora_db::repo::skill_libraries::LibraryRow;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_libraries, create_library))
        .routes(routes!(check_library))
        .routes(routes!(list_instance_libraries, create_instance_library))
        .routes(routes!(check_instance_library))
        .routes(routes!(get_library, patch_library, delete_library))
        .routes(routes!(refresh_library))
        .routes(routes!(browse_library))
        .routes(routes!(get_library_skill))
}

/// Who may manage this library (`access::manage`), kill-switch first.
async fn admin_of(c: &Caller, s: &AppState, library: Uuid) -> Result<LibraryRow, ApiError> {
    service::ensure_enabled(s)?;
    let row = service::get(s, library).await?;
    access::manage(c, s, &row).await?;
    Ok(row)
}

/// Browse audience (`access::read`): any person who can use a workspace of the organization the
/// library is read as (org owners/admins included); members only see `active` libraries.
async fn browse_of(
    c: &Caller,
    s: &AppState,
    library: Uuid,
    org: Option<Uuid>,
) -> Result<(LibraryRow, ReadAs), ApiError> {
    service::ensure_enabled(s)?;
    let row = service::get(s, library).await?;
    let who = access::read(c, s, &row, org).await?;
    Ok((row, who))
}

pub use access::audience as browse_audience;

/// List the organization's libraries, then the instance libraries (D-449): owners/admins get
/// everything (an instance library with this organization's numbers); other people who can use a
/// workspace get `id`, `label`, `status` and `scope` of the active ones (the browse picker).
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
    let rows = service::list_for_org(&s, org).await?;
    let mut items = Vec::new();
    for r in rows {
        if !admin && r.status != "active" {
            continue;
        }
        let how = if admin { View::Org(org) } else { View::Picker };
        items.push(service::view(&s, &r, how).await?);
    }
    Ok(Json(Page {
        items,
        next_cursor: None,
    }))
}

/// (P5-SKLIB-2, C-1128) The instance's libraries with their totals over every organization and
/// `orgs_using` (instance admins, session only).
#[utoipa::path(get, path = "/admin/skill-libraries", operation_id = "listInstanceSkillLibraries",
    tag = "skill-libraries",
    responses((status = 200, body = Page<SkillLibrary>), (status = 401, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_instance_libraries(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<Page<SkillLibrary>>, ApiError> {
    service::ensure_enabled(&s)?;
    let mut items = Vec::new();
    for r in service::list_instance(&s).await? {
        items.push(service::view(&s, &r, View::Manage).await?);
    }
    Ok(Json(Page {
        items,
        next_cursor: None,
    }))
}

/// (P5-SKLIB-2, C-1128) Add an instance library: every organization sees it (instance admins,
/// session only).
#[utoipa::path(post, path = "/admin/skill-libraries", operation_id = "createInstanceSkillLibrary",
    tag = "skill-libraries", request_body = CreateSkillLibraryRequest,
    responses((status = 201, body = SkillLibrary), (status = 400, body = ErrorEnvelope),
        (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_instance_library(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<CreateSkillLibraryRequest>,
) -> Result<(StatusCode, Json<SkillLibrary>), ApiError> {
    service::ensure_enabled(&s)?;
    let row = service::create(&s, &c, None, &req).await?;
    Ok((
        StatusCode::CREATED,
        Json(service::view(&s, &row, View::Manage).await?),
    ))
}

/// (P5-SKLIB-2, C-1124, C-1128) The check before saving an instance library; stores nothing.
#[utoipa::path(post, path = "/admin/skill-libraries/check", operation_id = "checkInstanceSkillLibrary",
    tag = "skill-libraries", request_body = CheckSkillLibraryRequest,
    responses((status = 200, body = LibraryCheck), (status = 401, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn check_instance_library(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<CheckSkillLibraryRequest>,
) -> Result<Response, ApiError> {
    service::ensure_enabled(&s)?;
    check_rate(&c, &s)?;
    check_response(service::check(&s, &req).await?)
}

/// `?org=` of the routes that read a library as an organization (C-1128).
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OrgParam {
    /// The organization to read the library as: required for an instance library, optional (and
    /// equal to the library's organization) for an organization library.
    pub org: Option<Uuid>,
}

/// Read one library. An organization library: its owner/admin. An instance library: an instance
/// admin (totals), or an owner/admin of an organization with `?org=` (that organization's view);
/// anyone else 404.
#[utoipa::path(get, path = "/skill-libraries/{id}", operation_id = "getSkillLibrary",
    tag = "skill-libraries", params(("id" = Uuid, Path, description = "Library id"), OrgParam),
    responses((status = 200, body = SkillLibrary), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<OrgParam>,
) -> Result<Json<SkillLibrary>, ApiError> {
    service::ensure_enabled(&s)?;
    let row = service::get(&s, id).await?;
    let how = access::view_as(&c, &s, &row, q.org).await?;
    Ok(Json(service::view(&s, &row, how).await?))
}

/// Rename, pause or resume, change ref, path or token (re-reads; stored skills are never
/// touched), or set the address of a migrated library that has none.
#[utoipa::path(patch, path = "/skill-libraries/{id}", operation_id = "updateSkillLibrary",
    tag = "skill-libraries", params(("id" = Uuid, Path, description = "Library id")),
    request_body = PatchSkillLibraryRequest,
    responses((status = 200, body = SkillLibrary), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchSkillLibraryRequest>,
) -> Result<Json<SkillLibrary>, ApiError> {
    admin_of(&c, &s, id).await?;
    let row = service::patch(&s, &c, id, &req).await?;
    Ok(Json(service::view(&s, &row, View::Manage).await?))
}

/// Delete the library; every skill added from it is detached and keeps its content (an instance
/// library: in every organization, with an audit row in each).
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

/// (P5-SKLIB-1, C-1124) 10 checks per minute per person.
pub(crate) const CHECKS_PER_MINUTE: u32 = 10;

/// The check rate (per person, per replica) and its 429.
pub(crate) fn check_rate(c: &Caller, s: &AppState) -> Result<(), ApiError> {
    s.rate_limiter.check_quota(
        "skill_library_check",
        &c.user_id.to_string(),
        crate::http::ratelimit::Quota {
            capacity: CHECKS_PER_MINUTE,
            per: std::time::Duration::from_secs(60),
        },
    )
}

fn check_response(body: LibraryCheck) -> Result<Response, ApiError> {
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// Read a repository like a create would and say what was found; nothing is stored
/// (org owner/admin).
#[utoipa::path(post, path = "/orgs/{org}/skill-libraries/check", operation_id = "checkSkillLibrary",
    tag = "skill-libraries", params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CheckSkillLibraryRequest,
    responses((status = 200, body = LibraryCheck), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn check_library(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CheckSkillLibraryRequest>,
) -> Result<Response, ApiError> {
    service::ensure_enabled(&s)?;
    c.org(&s, org).await?.require_admin()?;
    check_rate(&c, &s)?;
    check_response(service::check(&s, &req).await?)
}

/// Add a git repository as a skill library (org owner/admin).
#[utoipa::path(post, path = "/orgs/{org}/skill-libraries", operation_id = "createSkillLibrary",
    tag = "skill-libraries", params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateSkillLibraryRequest,
    responses((status = 201, body = SkillLibrary), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_library(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateSkillLibraryRequest>,
) -> Result<(StatusCode, Json<SkillLibrary>), ApiError> {
    service::ensure_enabled(&s)?;
    c.org(&s, org).await?.require_admin()?;
    let row = service::create(&s, &c, Some(org), &req).await?;
    Ok((
        StatusCode::CREATED,
        Json(service::view(&s, &row, View::Manage).await?),
    ))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BrowseParams {
    /// Case-insensitive filter on name and description.
    pub q: Option<String>,
    pub cursor: Option<String>,
    /// The organization to browse as: required for an instance library (`added` then lists only
    /// that organization's skills the caller can see), optional for an organization library.
    pub org: Option<Uuid>,
    /// (C-1176) Say what cannot be added to this tier: `agent` (with `agent`), `workspace`
    /// (with `workspace`) or `organization`. The tier must be of the organization browsed as.
    #[param(inline)]
    pub tier: Option<BrowseTier>,
    pub agent: Option<Uuid>,
    pub workspace: Option<Uuid>,
}

#[derive(Deserialize, utoipa::ToSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum BrowseTier {
    Agent,
    Workspace,
    Organization,
}

/// The tier a browse asks about, once the caller may see its skills; the same 404 for a tier of
/// another organization as for an unknown one.
async fn browse_tier(
    c: &Caller,
    s: &AppState,
    org: Uuid,
    q: &BrowseParams,
) -> Result<Option<Owner>, ApiError> {
    let bad = |m: &str| ApiError::invalid_field("Invalid request.", "/tier", m);
    let agent_given = q.agent.is_some();
    let ws_given = q.workspace.is_some();
    match q.tier {
        None if !agent_given && !ws_given => Ok(None),
        None => Err(bad("required with agent or workspace")),
        Some(BrowseTier::Agent) => {
            let (Some(id), false) = (q.agent, ws_given) else {
                return Err(bad("agent needs the agent parameter only"));
            };
            let missing = || ApiError::not_found("Agent not found.");
            let a = aurora_db::repo::agents::get(&s.pool, id)
                .await?
                .filter(|a| a.org_id == org)
                .ok_or_else(missing)?;
            c.workspace(s, a.workspace_id).await?;
            Ok(Some(Owner {
                agent: Some(a.id),
                workspace: None,
            }))
        }
        Some(BrowseTier::Workspace) => {
            let (Some(id), false) = (q.workspace, agent_given) else {
                return Err(bad("workspace needs the workspace parameter only"));
            };
            let a = c.workspace(s, id).await?;
            if a.org_id != org {
                return Err(ApiError::not_found("Workspace not found."));
            }
            Ok(Some(Owner {
                agent: None,
                workspace: Some(id),
            }))
        }
        Some(BrowseTier::Organization) => {
            if agent_given || ws_given {
                return Err(bad("organization takes neither agent nor workspace"));
            }
            c.org(s, org).await?;
            Ok(Some(Owner::default()))
        }
    }
}

const BROWSE_PAGE: usize = 50;

/// Browse the library's current head live (nothing is stored; cached for 5 minutes).
#[utoipa::path(get, path = "/skill-libraries/{id}/skills", operation_id = "browseSkillLibrary",
    tag = "skill-libraries",
    params(("id" = Uuid, Path, description = "Library id"), BrowseParams),
    responses((status = 200, body = LibraryCatalogue), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 502, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn browse_library(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<BrowseParams>,
) -> Result<Response, ApiError> {
    let (row, who) = browse_of(&c, &s, id, q.org).await?;
    let tier = browse_tier(&c, &s, who.org, &q).await?;
    let l = service::listing(&s, &row).await?;
    // Never cached with the listing: what the tier holds changes with every add.
    let room = match tier {
        Some(owner) => Some(crate::domains::skills::library::tier_room(&s, who.org, owner).await?),
        None => None,
    };
    // Never cached with the listing: the chips are this organization's skills the caller can see.
    let added =
        crate::domains::skills::library::added_by_path(&s, &c, row.id, who.org, who.admin).await?;
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
            blocked: room
                .as_ref()
                .and_then(|r| r.blocked(&i.name, i.valid, i.reason.as_deref(), i.size)),
        })
        .collect();
    let next = (start + BROWSE_PAGE < matching.len()).then(|| (start + BROWSE_PAGE).to_string());
    let body = LibraryCatalogue {
        commit_sha: l.commit_sha.clone(),
        items: page,
        truncated: l.truncated,
        next_cursor: next,
        budget: room.as_ref().map(|r| r.budget()),
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
    /// The organization to read the skill as (required for an instance library).
    pub org: Option<Uuid>,
}

/// One skill's full content at a commit: exactly what an add would store (and the diff base).
#[utoipa::path(get, path = "/skill-libraries/{id}/skills/{path}",
    operation_id = "getSkillLibrarySkill", tag = "skill-libraries",
    params(("id" = Uuid, Path, description = "Library id"),
        ("path" = String, Path, description = "The skill's directory name"), SkillParams),
    responses((status = 200, body = LibrarySkillContent), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 502, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_library_skill(
    c: Caller,
    State(s): State<AppState>,
    Path((id, path)): Path<(Uuid, String)>,
    Query(q): Query<SkillParams>,
) -> Result<Response, ApiError> {
    let (row, _) = browse_of(&c, &s, id, q.org).await?;
    let content = service::skill_content(&s, &row, &path, q.commit_sha.as_deref()).await?;
    let mut resp = Json(content).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

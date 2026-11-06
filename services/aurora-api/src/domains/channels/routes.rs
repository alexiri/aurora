use super::dto::*;
use super::{bindings, integrations, links, presences, reach};
use crate::app::AppState;
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams};
use aurora_core::{ApiError, FieldError};
use axum::body::Bytes;
use axum::extract::State;
use http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_integrations, create_integration))
        .routes(routes!(
            get_integration,
            patch_integration,
            delete_integration
        ))
        .routes(routes!(list_user_links, create_user_link))
        .routes(routes!(delete_user_link))
        .routes(routes!(list_presences, attach_presence))
        .routes(routes!(detach_presence))
        .routes(routes!(reattach_presence))
        .routes(routes!(list_bindings))
        .routes(routes!(patch_binding, delete_binding))
        .routes(routes!(my_links))
        .routes(routes!(delete_my_link))
        .routes(routes!(confirm_link))
        .routes(routes!(list_dm_defaults, set_dm_default))
        .routes(routes!(list_reach))
        .routes(routes!(decide_reach))
}

// ----------------------------------------------------------------- integrations

/// List the organization's channel integrations (org owner/admin).
#[utoipa::path(get, path = "/orgs/{org}/channel-integrations", operation_id = "listChannelIntegrations",
    tag = "channels",
    params(("org" = Uuid, Path, description = "Organization id"), PageParams),
    responses((status = 200, body = Page<ChannelIntegration>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_integrations(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(p): Query<PageParams>,
) -> Result<Json<Page<ChannelIntegration>>, ApiError> {
    let p = p.resolve()?;
    Ok(Json(integrations::list(&s, &c, org, p).await?))
}

/// Create an integration: credentials are sealed and validated synchronously by the connector
/// (C-33). 409 `tenant_already_bound`, 422 `credentials_invalid`, 503 `connector_unavailable`.
#[utoipa::path(post, path = "/orgs/{org}/channel-integrations", operation_id = "createChannelIntegration",
    tag = "channels",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateIntegrationRequest,
    responses((status = 201, body = ChannelIntegration), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 502, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_integration(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateIntegrationRequest>,
) -> Result<(StatusCode, Json<ChannelIntegration>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(integrations::create(&s, &c, org, req).await?),
    ))
}

/// Get one integration (org owner/admin). Credentials are never returned.
#[utoipa::path(get, path = "/channel-integrations/{id}", operation_id = "getChannelIntegration",
    tag = "channels",
    params(("id" = Uuid, Path, description = "Integration id")),
    responses((status = 200, body = ChannelIntegration), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_integration(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ChannelIntegration>, ApiError> {
    Ok(Json(integrations::get(&s, &c, id).await?))
}

/// Replace config/credentials (re-validated) or set `status` to `active` | `disabled`.
#[utoipa::path(patch, path = "/channel-integrations/{id}", operation_id = "patchChannelIntegration",
    tag = "channels",
    params(("id" = Uuid, Path, description = "Integration id")),
    request_body = PatchIntegrationRequest,
    responses((status = 200, body = ChannelIntegration), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 502, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_integration(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchIntegrationRequest>,
) -> Result<Json<ChannelIntegration>, ApiError> {
    Ok(Json(integrations::patch(&s, &c, id, req).await?))
}

/// Delete an integration; dedicated presences are deprovisioned through connector operations.
#[utoipa::path(delete, path = "/channel-integrations/{id}", operation_id = "deleteChannelIntegration",
    tag = "channels",
    params(("id" = Uuid, Path, description = "Integration id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_integration(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    integrations::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------- user links

/// List the user links of an integration (org owner/admin).
#[utoipa::path(get, path = "/channel-integrations/{id}/user-links", operation_id = "listIntegrationUserLinks",
    tag = "channels",
    params(("id" = Uuid, Path, description = "Integration id"), PageParams),
    responses((status = 200, body = Page<UserLink>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_user_links(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(p): Query<PageParams>,
) -> Result<Json<Page<UserLink>>, ApiError> {
    let p = p.resolve()?;
    Ok(Json(links::list_for_integration(&s, &c, id, p).await?))
}

/// Link a platform user to an Aurora user (org owner/admin). 409 `already_linked`.
#[utoipa::path(post, path = "/channel-integrations/{id}/user-links", operation_id = "createIntegrationUserLink",
    tag = "channels",
    params(("id" = Uuid, Path, description = "Integration id")),
    request_body = CreateUserLinkRequest,
    responses((status = 201, body = UserLink), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_user_link(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateUserLinkRequest>,
) -> Result<(StatusCode, Json<UserLink>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(links::admin_create(&s, &c, id, req).await?),
    ))
}

/// Remove a user link (org owner/admin).
#[utoipa::path(delete, path = "/user-links/{id}", operation_id = "deleteUserLink", tag = "channels",
    params(("id" = Uuid, Path, description = "Link id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_user_link(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    links::admin_delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------- presences

/// Presences of an agent (`can_use`).
#[utoipa::path(get, path = "/agents/{agent}/presences", operation_id = "listAgentPresences", tag = "channels",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = PresenceList), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_presences(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<PresenceList>, ApiError> {
    Ok(Json(PresenceList {
        items: presences::list(&s, &c, agent).await?,
    }))
}

/// Attach an agent to an integration as `shared` (immediately active) or `dedicated`
/// (`pending_setup` + provisioning, or validated manual fields) (C-34).
#[utoipa::path(post, path = "/agents/{agent}/presences", operation_id = "attachAgentPresence", tag = "channels",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = AttachPresenceRequest,
    responses((status = 201, body = Presence), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 502, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn attach_presence(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<AttachPresenceRequest>,
) -> Result<(StatusCode, Json<Presence>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(presences::attach(&s, &c, agent, req).await?),
    ))
}

/// Detach a presence (dedicated bots are deprovisioned best-effort).
#[utoipa::path(delete, path = "/presences/{id}", operation_id = "detachPresence", tag = "channels",
    params(("id" = Uuid, Path, description = "Presence id")),
    responses((status = 204), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn detach_presence(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    presences::detach(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Re-run provisioning/validation. The body is optional (`fields` for manual setup).
#[utoipa::path(post, path = "/presences/{id}/reattach", operation_id = "reattachPresence", tag = "channels",
    params(("id" = Uuid, Path, description = "Presence id")),
    request_body(content = ReattachRequest, description = "Optional"),
    responses((status = 200, body = Presence), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn reattach_presence(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Result<Json<Presence>, ApiError> {
    let req: ReattachRequest = if body.iter().all(u8::is_ascii_whitespace) {
        ReattachRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|_| {
            ApiError::invalid_request(
                "Invalid request body.",
                vec![FieldError::new("", "expected {\"fields\"?: {…}}")],
            )
        })?
    };
    Ok(Json(presences::reattach(&s, &c, id, req.fields).await?))
}

// --------------------------------------------------------------------- bindings

/// Space bindings of an agent (`can_own`).
#[utoipa::path(get, path = "/agents/{agent}/space-bindings", operation_id = "listAgentSpaceBindings", tag = "channels",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = SpaceBindingList), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_bindings(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<SpaceBindingList>, ApiError> {
    Ok(Json(SpaceBindingList {
        items: bindings::list(&s, &c, agent).await?,
    }))
}

/// Edit a binding (`can_own`).
#[utoipa::path(patch, path = "/space-bindings/{id}", operation_id = "patchSpaceBinding", tag = "channels",
    params(("id" = Uuid, Path, description = "Binding id")),
    request_body = PatchBindingRequest,
    responses((status = 200, body = SpaceBinding), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_binding(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchBindingRequest>,
) -> Result<Json<SpaceBinding>, ApiError> {
    Ok(Json(bindings::patch(&s, &c, id, req).await?))
}

/// Remove a binding (`can_own`).
#[utoipa::path(delete, path = "/space-bindings/{id}", operation_id = "deleteSpaceBinding", tag = "channels",
    params(("id" = Uuid, Path, description = "Binding id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_binding(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    bindings::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ----------------------------------------------------------------------- me

/// The caller's own channel links.
#[utoipa::path(get, path = "/me/channel-links", operation_id = "listMyChannelLinks", tag = "channels",
    responses((status = 200, body = UserLinkList)),
    security(("session" = [])))]
async fn my_links(
    c: SessionCaller,
    State(s): State<AppState>,
) -> Result<Json<UserLinkList>, ApiError> {
    Ok(Json(UserLinkList {
        items: links::mine(&s, &c).await?,
    }))
}

/// Remove one of the caller's own channel links.
#[utoipa::path(delete, path = "/me/channel-links/{id}", operation_id = "deleteMyChannelLink", tag = "channels",
    params(("id" = Uuid, Path, description = "Link id")),
    responses((status = 204), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_my_link(
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    links::delete_mine(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Confirm a sign-in link token for the signed-in user (C-36). Session only.
#[utoipa::path(post, path = "/channel-links/confirm", operation_id = "confirmChannelLink", tag = "channels",
    request_body = ConfirmLinkRequest,
    responses((status = 200, body = UserLink), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 410, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn confirm_link(
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<ConfirmLinkRequest>,
) -> Result<Json<UserLink>, ApiError> {
    Ok(Json(links::confirm(&s, &c, req).await?))
}

/// DM defaults for every platform user linked to the caller.
#[utoipa::path(get, path = "/me/dm-defaults", operation_id = "listDmDefaults", tag = "channels",
    responses((status = 200, body = DmDefaultList)),
    security(("session" = [])))]
async fn list_dm_defaults(
    c: SessionCaller,
    State(s): State<AppState>,
) -> Result<Json<DmDefaultList>, ApiError> {
    Ok(Json(DmDefaultList {
        items: links::dm_defaults_list(&s, &c).await?,
    }))
}

/// Set the agent that answers the caller's DMs with an integration's shared bot (C-37).
#[utoipa::path(put, path = "/me/dm-defaults", operation_id = "setDmDefault", tag = "channels",
    request_body = SetDmDefaultRequest,
    responses((status = 200, body = DmDefault), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn set_dm_default(
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<SetDmDefaultRequest>,
) -> Result<Json<DmDefault>, ApiError> {
    Ok(Json(links::dm_default_set(&s, &c, req).await?))
}

// ------------------------------------------------------------------------ reach

/// Reach grants of an agent (`can_use`).
#[utoipa::path(get, path = "/agents/{agent}/reach", operation_id = "listAgentReach", tag = "channels",
    params(("agent" = Uuid, Path, description = "Agent id"), PageParams),
    responses((status = 200, body = Page<ReachGrant>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_reach(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(p): Query<PageParams>,
) -> Result<Json<Page<ReachGrant>>, ApiError> {
    let p = p.resolve()?;
    Ok(Json(reach::list(&s, &c, agent, p).await?))
}

/// Approve or block a reach grant (`can_own`); 409 `stale_revision`.
#[utoipa::path(post, path = "/reach-grants/{id}/decision", operation_id = "decideReachGrant", tag = "channels",
    params(("id" = Uuid, Path, description = "Grant id")),
    request_body = ReachDecisionRequest,
    responses((status = 200, body = ReachGrant), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn decide_reach(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ReachDecisionRequest>,
) -> Result<Json<ReachGrant>, ApiError> {
    Ok(Json(reach::decide(&s, &c, id, req).await?))
}

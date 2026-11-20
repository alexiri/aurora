use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent};
use crate::http::apilimit::{Charge, Spec};
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::invitations as repo;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_invitations, create_invitation))
        .routes(routes!(preview_invitation))
        .routes(routes!(accept_invitation))
        .routes(routes!(cancel_invitation))
        .routes(routes!(create_invitation_link))
        .routes(routes!(resend_invitation))
}

/// (C-1069, SEC-140) Links re-issued per organization per hour, `link` and `resend` together.
const LINKS_PER_ORG_HOUR: u32 = 20;

/// Load the invitation and authorize the caller as an owner or admin of its organization (404 for
/// an unknown invitation, and for another organization's, as `cancel_invitation`).
async fn authorized(c: &Caller, s: &AppState, id: Uuid) -> Result<repo::InvitationRow, ApiError> {
    let inv = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Invitation not found."))?;
    c.org(s, inv.org_id).await?.require_admin()?;
    Ok(inv)
}

/// Take one of the organization's hourly links (429 past [`LINKS_PER_ORG_HOUR`]).
fn take_link(s: &AppState, org: Uuid) -> Result<(), ApiError> {
    s.api_limiter
        .take(&[Charge {
            key: format!("invitation-link:org:{org}"),
            spec: Spec::per_hour(LINKS_PER_ORG_HOUR, LINKS_PER_ORG_HOUR),
            scope: "organization",
            class: "invitation_link",
        }])
        .map_err(|r| r.into_error())
}

/// Re-issue an invitation's link to copy (organization owner/admin; C-1069, SEC-140). The new link
/// replaces the old one, which stops working; the expiry is reset to 7 days. Shown once.
#[utoipa::path(post, path = "/invitations/{id}/link", operation_id = "createInvitationLink", tag = "invitations",
    params(("id" = Uuid, Path, description = "Invitation id")),
    responses((status = 200, body = InvitationLink), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_invitation_link(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let inv = authorized(&c, &s, id).await?;
    take_link(&s, inv.org_id)?;
    let (link, expires_at) = service::reissue(
        &s,
        &inv,
        service::Reissue::Link,
        AuditActor::from_caller(&c),
    )
    .await?;
    let mut resp = Json(InvitationLink { link, expires_at }).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// E-mail an invitation again with a new link (organization owner/admin; C-1069). 409
/// `mail_disabled` when the instance's mail is log-only.
#[utoipa::path(post, path = "/invitations/{id}/resend", operation_id = "resendInvitation", tag = "invitations",
    params(("id" = Uuid, Path, description = "Invitation id")),
    responses((status = 202, body = Invitation), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn resend_invitation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Invitation>), ApiError> {
    let inv = authorized(&c, &s, id).await?;
    // The same answer as `GET /v1/instance` `email_delivery`: Resend is offered only when mail works.
    if !s.auth().smtp {
        return Err(ApiError::conflict(
            "mail_disabled",
            "This server can't send e-mail. Copy the invitation link instead.",
        ));
    }
    take_link(&s, inv.org_id)?;
    service::reissue(
        &s,
        &inv,
        service::Reissue::Resend,
        AuditActor::from_caller(&c),
    )
    .await?;
    let fresh = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Invitation not found."))?;
    Ok((StatusCode::ACCEPTED, Json(Invitation::from(fresh))))
}

fn event(c: &Caller, action: &str) -> AuditEvent {
    AuditEvent::by(c, action)
}

/// List an organization's invitations (owner/admin).
#[utoipa::path(get, path = "/orgs/{org}/invitations", operation_id = "listInvitations", tag = "invitations",
    params(("org" = Uuid, Path, description = "Organization id"), ListInvitationsQuery),
    responses((status = 200, body = Page<Invitation>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_invitations(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<ListInvitationsQuery>,
) -> Result<Json<Page<Invitation>>, ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = repo::list(
        &s.pool,
        org,
        q.status.map(InvitationStatus::as_str),
        p.after_key(),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(page.map(Invitation::from)))
}

/// Invite someone by email (owner/admin). Re-inviting a pending email rotates the token.
#[utoipa::path(post, path = "/orgs/{org}/invitations", operation_id = "createInvitation", tag = "invitations",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateInvitationRequest,
    responses((status = 201, body = Invitation), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_invitation(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<Invitation>), ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let inv = service::create(&s, org, c.user_id, &req, AuditActor::from_caller(&c)).await?;
    Ok((StatusCode::CREATED, Json(inv)))
}

/// Cancel a pending invitation (owner/admin).
#[utoipa::path(delete, path = "/invitations/{id}", operation_id = "cancelInvitation", tag = "invitations",
    params(("id" = Uuid, Path, description = "Invitation id")),
    responses((status = 204, description = "Cancelled"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn cancel_invitation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let nf = || ApiError::not_found("Invitation not found.");
    let inv = repo::get(&s.pool, id).await?.ok_or_else(nf)?;
    c.org(&s, inv.org_id).await?.require_admin()?;
    let mut tx = audit::begin(&s).await?;
    if !repo::cancel(&mut *tx, id).await? {
        return Err(nf());
    }
    let ev = event(&c, "invitation.cancel")
        .org(inv.org_id)
        .target("invitation", id)
        .metadata(json!({"email": inv.email}));
    audit::commit_critical(tx, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Preview an invitation by token (public). Only pending, unexpired invitations are found.
#[utoipa::path(get, path = "/invitations/preview", operation_id = "previewInvitation", tag = "invitations",
    params(PreviewQuery),
    responses((status = 200, body = InvitationPreview), (status = 404, body = ErrorEnvelope)))]
async fn preview_invitation(
    State(s): State<AppState>,
    Query(q): Query<PreviewQuery>,
) -> Result<Json<InvitationPreview>, ApiError> {
    Ok(Json(service::preview(&s, &q.token).await?))
}

/// Accept an invitation as the signed-in user (verified matching email).
#[utoipa::path(post, path = "/invitations/accept", operation_id = "acceptInvitation", tag = "invitations",
    request_body = AcceptInvitationRequest,
    responses((status = 200, body = AcceptInvitationResponse), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 410, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn accept_invitation(
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<AcceptInvitationRequest>,
) -> Result<Json<AcceptInvitationResponse>, ApiError> {
    let (org, workspace_ids, _) =
        service::accept(&s, c.user_id, &req.token, AuditActor::from_caller(&c)).await?;
    Ok(Json(AcceptInvitationResponse { org, workspace_ids }))
}

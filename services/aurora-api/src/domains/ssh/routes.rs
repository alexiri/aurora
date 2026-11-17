//! The SSH routes (P4-SSH-1, C-790..C-792). Every one answers 404 `ssh_disabled` first (before
//! authentication) while `ssh.enabled` is off. Key writes are session-only (`SO`); everything
//! else takes a session or the person's own API key.
use super::dto::*;
use super::service;
use super::sessions;
use crate::app::AppState;
use crate::http::auth::{Caller, InstanceAdmin, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_keys, create_key))
        .routes(routes!(rename_key, delete_key))
        .routes(routes!(mint_certificate))
        .routes(routes!(get_facts))
        .routes(routes!(list_my_sessions))
        .routes(routes!(list_agent_sessions))
        .routes(routes!(disconnect_session))
        .routes(routes!(list_terminators))
        // The terminator plane (P4-SSH-2): plain routes, not in the dashboard OpenAPI document.
        .merge(super::plane::router())
}

/// The caller's open SSH sessions.
#[utoipa::path(get, path = "/me/ssh-sessions", operation_id = "listMySshSessions", tag = "ssh",
    responses((status = 200, body = SshSessionList), (status = 401, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_my_sessions(
    _on: SshOn,
    c: Caller,
    State(s): State<AppState>,
) -> Result<Json<SshSessionList>, ApiError> {
    Ok(Json(sessions::list_mine(&s, &c).await?))
}

/// Open SSH sessions on an agent: everyone with `can_own` sees all (with the person), a member
/// with `can_use` only their own.
#[utoipa::path(get, path = "/agents/{agent}/ssh-sessions", operation_id = "listAgentSshSessions",
    tag = "ssh", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = SshSessionList), (status = 401, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_agent_sessions(
    _on: SshOn,
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<SshSessionList>, ApiError> {
    Ok(Json(sessions::list_for_agent(&s, &c, agent).await?))
}

/// Disconnect a session: its person or a workspace owner. The terminator closes it at its next
/// heartbeat (within 30 s).
#[utoipa::path(delete, path = "/ssh-sessions/{id}", operation_id = "disconnectSshSession",
    tag = "ssh", params(("id" = Uuid, Path, description = "SSH session id")),
    responses((status = 202, body = Disconnecting), (status = 401, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn disconnect_session(
    _on: SshOn,
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Disconnecting>), ApiError> {
    sessions::disconnect(&s, &c, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(Disconnecting {
            disconnecting: true,
        }),
    ))
}

/// The registered terminator replicas (instance admin).
#[utoipa::path(get, path = "/admin/terminators", operation_id = "listTerminators", tag = "ssh",
    responses((status = 200, body = TerminatorList), (status = 401, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_terminators(
    _on: SshOn,
    _a: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<TerminatorList>, ApiError> {
    Ok(Json(sessions::admin_terminators(&s).await?))
}

/// Extractor placed first in every handler: 404 `ssh_disabled` while SSH is off, before any
/// authentication work (so an anonymous probe learns nothing but "off").
struct SshOn;

impl FromRequestParts<AppState> for SshOn {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        if s.config.ssh.enabled {
            Ok(Self)
        } else {
            Err(service::ssh_disabled())
        }
    }
}

/// The caller's registered SSH public keys, newest first.
#[utoipa::path(get, path = "/me/ssh-keys", operation_id = "listSshKeys", tag = "ssh",
    responses((status = 200, body = SshKeyList), (status = 401, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_keys(
    _on: SshOn,
    c: Caller,
    State(s): State<AppState>,
) -> Result<Json<SshKeyList>, ApiError> {
    Ok(Json(service::list_keys(&s, &c).await?))
}

/// Register an `ssh-ed25519` public key (session only).
#[utoipa::path(post, path = "/me/ssh-keys", operation_id = "createSshKey", tag = "ssh",
    request_body = CreateSshKeyRequest,
    responses((status = 201, body = SshKey), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_key(
    _on: SshOn,
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<CreateSshKeyRequest>,
) -> Result<(StatusCode, Json<SshKey>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(service::create_key(&s, &c, req).await?),
    ))
}

/// Rename one of the caller's keys (session only).
#[utoipa::path(patch, path = "/me/ssh-keys/{id}", operation_id = "renameSshKey", tag = "ssh",
    params(("id" = Uuid, Path, description = "SSH key id")),
    request_body = RenameSshKeyRequest,
    responses((status = 200, body = SshKey), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn rename_key(
    _on: SshOn,
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameSshKeyRequest>,
) -> Result<Json<SshKey>, ApiError> {
    Ok(Json(service::rename_key(&s, &c, id, &req.name).await?))
}

/// Delete one of the caller's keys (session only); sessions certified by it end at the next
/// heartbeat.
#[utoipa::path(delete, path = "/me/ssh-keys/{id}", operation_id = "deleteSshKey", tag = "ssh",
    params(("id" = Uuid, Path, description = "SSH key id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_key(
    _on: SshOn,
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete_key(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Mint a short-lived SSH user certificate for an agent (session or the caller's own API key).
#[utoipa::path(post, path = "/agents/{agent}/ssh-certificate", operation_id = "mintSshCertificate",
    tag = "ssh",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = MintCertificateRequest,
    responses((status = 201, body = SshCertificate), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 423, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn mint_certificate(
    _on: SshOn,
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<MintCertificateRequest>,
) -> Result<(StatusCode, Json<SshCertificate>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(service::mint_certificate(&s, &c, agent, req).await?),
    ))
}

/// Connection facts: the address, the certificate lifetime and the terminators' host keys.
#[utoipa::path(get, path = "/ssh", operation_id = "getSshFacts", tag = "ssh",
    responses((status = 200, body = SshFacts), (status = 401, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_facts(
    _on: SshOn,
    _c: Caller,
    State(s): State<AppState>,
) -> Result<Json<SshFacts>, ApiError> {
    Ok(Json(service::facts(&s).await?))
}

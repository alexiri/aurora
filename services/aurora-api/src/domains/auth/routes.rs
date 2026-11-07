use super::dto::*;
use super::service::{self, Client};
use crate::app::AppState;
use crate::http::auth::{Principal, SESSION_COOKIE, SessionCaller, clear_session_cookie};
use crate::http::error::ErrorEnvelope;
use crate::http::json::Json;
use aurora_core::ApiError;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(logout))
        .routes(routes!(revoke_others))
}

/// Revoke the current session and clear the cookie.
#[utoipa::path(post, path = "/auth/logout", operation_id = "logout", tag = "auth",
    responses((status = 204, description = "Signed out"), (status = 401, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn logout(State(s): State<AppState>, c: SessionCaller) -> Result<Response, ApiError> {
    if let Principal::Session { id } = c.principal {
        service::logout(&s, id).await?;
        crate::audit::record(
            &s,
            crate::audit::AuditEvent::new(
                crate::audit::AuditActor::from_caller(&c),
                "user.logout",
                crate::audit::Outcome::Success,
            )
            .target("user", c.user_id),
        );
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    if let Ok(v) = header::HeaderValue::from_str(&clear_session_cookie(&s)) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    let _ = SESSION_COOKIE;
    Ok(r)
}

/// Revoke all other sessions of the caller (session only).
#[utoipa::path(post, path = "/auth/sessions/revoke-others", operation_id = "revokeOtherSessions", tag = "auth",
    responses((status = 200, body = RevokedResponse), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn revoke_others(
    State(s): State<AppState>,
    c: SessionCaller,
) -> Result<Json<RevokedResponse>, ApiError> {
    let Principal::Session { id } = c.principal else {
        return Err(ApiError::forbidden("Session required."));
    };
    let cl = Client {
        ip: c.ip,
        user_agent: None,
    };
    let revoked = service::revoke_others(&s, &cl, c.user_id, id, &c.email).await?;
    Ok(Json(RevokedResponse { revoked }))
}

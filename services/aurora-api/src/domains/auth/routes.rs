use super::dto::*;
use super::service::{self, Client};
use crate::app::AppState;
use crate::http::auth::{
    Principal, SESSION_COOKIE, SessionCaller, clear_session_cookie, session_cookie,
};
use crate::http::client_ip::ClientIp;
use crate::http::error::ErrorEnvelope;
use crate::http::json::Json;
use aurora_core::ApiError;
use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, StatusCode, header};
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(signup))
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(verify_email))
        .routes(routes!(resend_verification))
        .routes(routes!(forgot_password))
        .routes(routes!(reset_password))
        .routes(routes!(change_password))
        .routes(routes!(revoke_others))
}

fn client(ip: &ClientIp, headers: &HeaderMap) -> Client {
    Client {
        ip: ip.0,
        user_agent: headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    }
}

fn with_cookie(status: StatusCode, cookie: String, body: impl serde::Serialize) -> Response {
    let mut r = (status, axum::Json(body)).into_response();
    if let Ok(v) = header::HeaderValue::from_str(&cookie) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r
}

/// Create an account (and, in `open` mode, its organization); sets the session cookie.
#[utoipa::path(post, path = "/auth/signup", operation_id = "signup", tag = "auth",
    request_body = SignupRequest,
    responses((status = 201, body = UserResponse),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)))]
async fn signup(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    Json(req): Json<SignupRequest>,
) -> Result<Response, ApiError> {
    let a = service::signup(
        &s,
        &client(&ip, &headers),
        service::SignupInput {
            email: req.email,
            password: req.password,
            display_name: req.display_name,
            invitation_token: req.invitation_token,
        },
    )
    .await?;
    Ok(with_cookie(
        StatusCode::CREATED,
        session_cookie(&s, &a.session_token),
        UserResponse {
            user: a.user.into(),
        },
    ))
}

/// Sign in with email and password; sets the session cookie.
#[utoipa::path(post, path = "/auth/login", operation_id = "login", tag = "auth",
    request_body = LoginRequest,
    responses((status = 200, body = UserResponse),
        (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)))]
async fn login(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let a = service::login(&s, &client(&ip, &headers), &req.email, req.password).await?;
    Ok(with_cookie(
        StatusCode::OK,
        session_cookie(&s, &a.session_token),
        UserResponse {
            user: a.user.into(),
        },
    ))
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

/// Consume an email-verification token.
#[utoipa::path(post, path = "/auth/verify-email", operation_id = "verifyEmail", tag = "auth",
    request_body = VerifyEmailRequest,
    responses((status = 200, body = UserResponse), (status = 400, body = ErrorEnvelope)))]
async fn verify_email(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    Json(req): Json<VerifyEmailRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let u = service::verify_email(&s, &client(&ip, &headers), &req.token).await?;
    Ok(Json(UserResponse { user: u.into() }))
}

/// Send a fresh verification email to the signed-in user.
#[utoipa::path(post, path = "/auth/verify-email/resend", operation_id = "resendVerification", tag = "auth",
    responses((status = 202, description = "Accepted"), (status = 409, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn resend_verification(
    State(s): State<AppState>,
    c: SessionCaller,
) -> Result<StatusCode, ApiError> {
    service::resend_verification(&s, c.user_id).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Request a password-reset email (always 202).
#[utoipa::path(post, path = "/auth/password/forgot", operation_id = "forgotPassword", tag = "auth",
    request_body = ForgotRequest,
    responses((status = 202, description = "Accepted"), (status = 429, body = ErrorEnvelope)))]
async fn forgot_password(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    Json(req): Json<ForgotRequest>,
) -> Result<StatusCode, ApiError> {
    service::forgot_password(&s, &client(&ip, &headers), &req.email).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Set a new password with a reset token; revokes all sessions.
#[utoipa::path(post, path = "/auth/password/reset", operation_id = "resetPassword", tag = "auth",
    request_body = ResetRequest,
    responses((status = 204, description = "Password changed"), (status = 400, body = ErrorEnvelope)))]
async fn reset_password(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    Json(req): Json<ResetRequest>,
) -> Result<StatusCode, ApiError> {
    service::reset_password(&s, &client(&ip, &headers), &req.token, req.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Change the password (session only).
#[utoipa::path(post, path = "/auth/password/change", operation_id = "changePassword", tag = "auth",
    request_body = ChangePasswordRequest,
    responses((status = 204, description = "Password changed"), (status = 400, body = ErrorEnvelope),
        (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn change_password(
    State(s): State<AppState>,
    c: SessionCaller,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let cl = Client {
        ip: c.ip,
        user_agent: None,
    };
    service::change_password(&s, &cl, c.user_id, req.current_password, req.new_password).await?;
    Ok(StatusCode::NO_CONTENT)
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

use super::service::{self, OIDC_COOKIE, Params, StartRequest};
use crate::app::AppState;
use crate::domains::auth::service::Client;
use crate::http::auth::{SESSION_COOKIE, cookie_value, session_cookie};
use crate::http::client_ip::ClientIp;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Path, Query};
use crate::http::ratelimit::Bucket;
use aurora_core::ApiError;
use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(start))
        .routes(routes!(callback))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StartQuery {
    /// Same-origin path to return to after sign-in (default `/`).
    pub return_to: Option<String>,
    /// `1` = re-authenticate the signed-in user (`prompt=login&max_age=0`); needs a session.
    pub reauth: Option<String>,
    /// Passed to the provider as `login_hint` (the development IdP signs that user in).
    pub login_hint: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
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

fn redirect(status: StatusCode, location: &str) -> Response {
    let mut r = status.into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

fn append_cookie(r: &mut Response, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
}

/// `Secure` is always set (required by the `__Host-` prefix; browsers treat localhost as secure).
fn flow_cookie(value: &str, max_age: u64) -> String {
    format!("{OIDC_COOKIE}={value}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={max_age}")
}

/// Redirect to the OIDC provider (PKCE S256 + nonce + encrypted state cookie).
#[utoipa::path(get, path = "/auth/oidc/{provider}/start", operation_id = "oidcStart", tag = "auth",
    params(("provider" = String, Path, description = "Provider id"), StartQuery),
    responses((status = 302, description = "Redirect to the provider"),
        (status = 400, body = ErrorEnvelope), (status = 401, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope),
        (status = 502, body = ErrorEnvelope)))]
async fn start(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    Path(provider): Path<String>,
    Query(q): Query<StartQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let client = client(&ip, &headers);
    let req = StartRequest {
        return_to: q.return_to.as_deref(),
        reauth: matches!(q.reauth.as_deref(), Some("1" | "true")),
        login_hint: q.login_hint.as_deref(),
    };
    let st = service::start(
        &s,
        &client,
        &provider,
        req,
        cookie_value(&headers, SESSION_COOKIE),
    )
    .await?;
    let mut r = redirect(StatusCode::FOUND, &st.location);
    let ttl = aurora_core::limits::OAUTH_STATE_TTL.as_secs();
    append_cookie(&mut r, &flow_cookie(&st.cookie, ttl));
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(r)
}

/// Append `reauth_error=<code>` to a same-origin path, before any `#fragment`; `&` when the path
/// already has a query, else `?`. Codes are fixed snake_case ASCII.
fn append_reauth_error(return_to: &str, code: &str) -> String {
    let (path, frag) = match return_to.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (return_to, None),
    };
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut out = format!("{path}{sep}reauth_error={code}");
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// Complete OIDC sign-in: sets the session cookie and redirects to `return_to`, or to
/// `/auth/login?error=<code>`. A failed re-authentication (the flow state opened and the flow was
/// a re-authentication) redirects back to the flow's `return_to` with `reauth_error=<code>`.
#[utoipa::path(get, path = "/auth/oidc/{provider}/callback", operation_id = "oidcCallback", tag = "auth",
    params(("provider" = String, Path, description = "Provider id"), CallbackQuery),
    responses((status = 302, description = "Redirect to `return_to` (on a failed re-authentication, with `reauth_error=<code>` appended) or `/auth/login?error=<code>`"),
        (status = 404, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)))]
async fn callback(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    Path(provider): Path<String>,
    Query(q): Query<CallbackQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    service::provider(&s, &provider)?;
    let client = client(&ip, &headers);
    crate::http::counters::check_security_bucket(&s, Bucket::OidcCallbackIp, &client.ip_key())
        .await?;
    let cookie = cookie_value(&headers, OIDC_COOKIE);
    let params = Params {
        code: q.code,
        state: q.state,
        error: q.error,
    };
    let mut r = match service::callback(&s, &client, &provider, params, cookie).await {
        Ok(ok) => {
            let mut r = redirect(StatusCode::FOUND, &ok.return_to);
            if let Some(tok) = &ok.session_token {
                append_cookie(&mut r, &session_cookie(&s, tok));
            }
            r
        }
        Err(f) => match f.return_to {
            Some(back) => redirect(StatusCode::FOUND, &append_reauth_error(&back, f.code)),
            None => redirect(StatusCode::FOUND, &format!("/auth/login?error={}", f.code)),
        },
    };
    append_cookie(&mut r, &flow_cookie("", 0));
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::append_reauth_error;

    #[test]
    fn appends_reauth_error() {
        assert_eq!(
            append_reauth_error("/admin/orgs", "reauth_stale"),
            "/admin/orgs?reauth_error=reauth_stale"
        );
        assert_eq!(
            append_reauth_error("/x?a=1", "oidc_failed"),
            "/x?a=1&reauth_error=oidc_failed"
        );
        assert_eq!(
            append_reauth_error("/x?a=1#frag", "oidc_failed"),
            "/x?a=1&reauth_error=oidc_failed#frag"
        );
        assert_eq!(
            append_reauth_error("/x#frag", "oidc_failed"),
            "/x?reauth_error=oidc_failed#frag"
        );
    }
}

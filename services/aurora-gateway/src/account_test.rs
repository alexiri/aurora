//! `POST /internal/v1/account-test` (phase 5, P5-UX-14, D-335, C-943, SEC-129): the checks of Test that only the
//! gateway can make, because an account's credential is used only here.
//!
//! * `database`: a login to the connection's database through the gateway's own guarded dialler (private-address
//!   refusal, `allowed_private`, the egress proxy, TLS settings of the connection) up to `ReadyForQuery`, then the
//!   socket is closed. **No statement is ever sent.**
//! * `mint`: one mint of a hook-minted token through the token manager (the same call a request would make).
//! * `fresh`: refresh a token that is expiring, so the API's probe does not judge a stale one.
//!
//! The answer is a result code with at most a host or a protocol code — never an upstream body, header, error text
//! or credential. The route needs the internal secret and is on the internal listener (unreachable from sandboxes).
//! It writes nothing itself: the API stores the result.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use aurora_crypto::SecretRing;
use aurora_db::PgPool;
use aurora_db::repo::gateway as repo;

use crate::apps::ConnCand;
use crate::db::DbEndpoint;
use crate::db::pg::server::{self, ConnectError};
use crate::guard::Guard;
use crate::plaintext::PlaintextCache;
use crate::refresh::{RefreshError, TokenManager};
use crate::snapshot::SnapshotStore;
use crate::upstream::EgressProxy;

pub struct AccountTest {
    pub ring: Arc<SecretRing>,
    pub snapshots: Arc<SnapshotStore>,
    pub pool: PgPool,
    pub tokens: Arc<TokenManager>,
    pub plaintext: Arc<PlaintextCache>,
    pub guard: Arc<Guard>,
    pub egress: Option<EgressProxy>,
    pub external_url: String,
    pub limiter: crate::library_fetch::Limiter,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    org_id: Uuid,
    connection_id: Uuid,
    check: CheckName,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CheckName {
    Database,
    Mint,
    Fresh,
}

pub fn router(t: Arc<AccountTest>) -> Router {
    Router::new()
        .route("/internal/v1/account-test", post(handle))
        .with_state(t)
}

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}

fn answer(result: &str, host: Option<&str>, code: Option<&str>) -> Response {
    let mut detail = serde_json::Map::new();
    if let Some(h) = host {
        detail.insert("host".into(), json!(h));
    }
    if let Some(c) = code {
        detail.insert("code".into(), json!(c));
    }
    let mut body = json!({ "result": result });
    if !detail.is_empty() {
        body["detail"] = Value::Object(detail);
    }
    axum::Json(body).into_response()
}

async fn handle(
    State(t): State<Arc<AccountTest>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let key_id = headers
        .get(aurora_core::limits::KEY_ID_HEADER)
        .map(|v| v.to_str().unwrap_or("\u{0}"));
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if !t.ring.verify_bearer_header(key_id, auth) {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "internal secret required",
        );
    }
    let Ok(req) = serde_json::from_slice::<Request>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "malformed account test request",
        );
    };
    if !t.limiter.allow(req.org_id) {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "account test rate limit",
        );
    }
    check(&t, &req).await
}

async fn check(t: &AccountTest, req: &Request) -> Response {
    let Some(loaded) = t.snapshots.current() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway_degraded",
            "no registry snapshot is loaded",
        );
    };
    // The connection must exist in the named organization and be active.
    let row = match repo::app_connection_in_org(&t.pool, req.org_id, req.connection_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err(
                StatusCode::NOT_FOUND,
                "connection_not_found",
                "no such connection in this organization",
            );
        }
        Err(_) => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "gateway_degraded",
                "the platform database is unreachable",
            );
        }
    };
    if row.status != "active" {
        return answer("not_testable", None, None);
    }
    let Some(app) = loaded.snapshot.app(&row.app_id) else {
        return answer("not_testable", None, None);
    };
    let Ok(mut conn) = ConnCand::from_row(row, req.org_id) else {
        return answer("unexpected", None, None);
    };
    if let Some(id) = conn.instance_id {
        // The registered instance's address, for `{instance}` in a hook's egress.
        if let Ok(v) = repo::visible_instances(&t.pool, req.org_id).await
            && let Some(i) = v
                .into_iter()
                .find(|i| i.id == id && i.app_id == conn.app_id)
        {
            conn.instance_host = Some(i.host);
            conn.instance_base_path = i.base_path;
        }
    } else {
        conn.instance_host = app.instances.as_ref().and_then(|i| i.default_host.clone());
    }
    let pkg_ctx: &[String] = loaded
        .snapshot
        .packages
        .iter()
        .find(|p| p.package_id == app.package_id)
        .map_or(&[], |p| p.context.as_slice());
    let mut context = HashMap::new();
    let mut put = |k: &str, v: String| {
        if pkg_ctx.iter().any(|a| a == k) {
            context.insert(k.to_owned(), v);
        }
    };
    put("org_id", req.org_id.to_string());
    put("instance_url", t.external_url.clone());

    let refreshable = TokenManager::applies(app, conn.connect_method.as_deref());
    match req.check {
        CheckName::Mint | CheckName::Fresh => {
            if !refreshable {
                return answer("not_testable", None, None);
            }
            // `Mint` forces one mint even when the stored token looks fine; `Fresh` only when it is expiring.
            match t
                .tokens
                .ensure_fresh(&loaded, &conn, &context, req.check == CheckName::Mint)
                .await
            {
                Ok(_) => answer("ok", None, None),
                // The refresh or mint itself says the sign-in cannot be renewed (it also marks the connection).
                Err(RefreshError::Reauth) => answer("rejected", None, None),
                Err(_) => answer("unexpected", None, None),
            }
        }
        CheckName::Database => {
            if app.database.is_none() {
                return answer("not_testable", None, None);
            }
            let fresh = if refreshable {
                match t.tokens.ensure_fresh(&loaded, &conn, &context, false).await {
                    Ok(c) => c,
                    Err(RefreshError::Reauth) => return answer("rejected", None, None),
                    Err(_) => return answer("unexpected", None, None),
                }
            } else {
                conn
            };
            let raw = match t.plaintext.get_connection(&fresh).await {
                Ok(r) => r,
                Err(_) => return answer("unexpected", None, None),
            };
            let Ok(endpoint) = DbEndpoint::from_credentials(raw.expose(), &fresh.metadata) else {
                return answer("unexpected", None, None);
            };
            database_login(t, &endpoint).await
        }
    }
}

/// Dial, negotiate TLS, authenticate up to `ReadyForQuery`; then drop the socket. Fixed texts only.
async fn database_login(t: &AccountTest, ep: &DbEndpoint) -> Response {
    match server::connect(&t.guard, t.egress.as_ref(), ep, Some("aurora-account-test")).await {
        // The session is dropped here, closing the socket: no statement has been or will be sent.
        Ok(_) => answer("ok", None, None),
        // The server answered the login with an error: the stored credentials are refused.
        Err(ConnectError::Server(_)) => answer("rejected", Some(&ep.host), None),
        Err(ConnectError::Dial(_) | ConnectError::Timeout) => {
            answer("unreachable", Some(&ep.host), None)
        }
        Err(ConnectError::Tls(_)) => answer("unexpected", Some(&ep.host), Some("tls")),
        Err(ConnectError::Protocol(_) | ConnectError::Unsupported(_)) => {
            answer("unexpected", Some(&ep.host), Some("protocol"))
        }
    }
}

//! The `/scim/v2` router (`16 §W15`): discovery documents, Users, Groups, the media-type
//! gates and the error fallback. This surface is a plain `axum::Router` nested at
//! `/scim/v2` in `app.rs` — deliberately **not** part of the OpenAPI document: it is not
//! a `/v1` API and never appears in `openapi.json`.
//!
//! Orchestration lives here, not in the resource handlers: the request body is read through
//! a bounded reader (413 above `SCIM_BODY_MAX`), authentication runs after the body is read
//! (one identical 401 with `WWW-Authenticate: Bearer`), every handler error is rendered in
//! the SCIM error format, and every response — success or error — carries
//! `Content-Type: application/scim+json`.

use super::auth::ScimToken;
use super::error::{ScimError, ScimResult};
use crate::app::AppState;
use aurora_core::limits;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http_body_util::LengthLimitError;
use serde_json::json;
use uuid::Uuid;

const BASE: &str = "/scim/v2";

/// `GET /ServiceProviderConfig` (`16 §W15` Discovery).
async fn service_provider_config(State(state): State<AppState>, req: Request) -> Response {
    let caller = match ScimToken::authenticate(&state, req.headers()).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let _ = &caller;
    let body = json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
        "documentationUri": format!("{}/docs", state.config.external_url.trim_end_matches('/')),
        "patch": { "supported": true },
        "bulk": { "supported": false, "maxOperations": 0, "maxPayloadSize": 0 },
        "filter": { "supported": true, "maxResults": limits::SCIM_PAGE_MAX },
        "changePassword": { "supported": false },
        "sort": { "supported": false },
        "etag": { "supported": false },
        "authenticationSchemes": [{
            "type": "oauthbearertoken",
            "name": "OAuth Bearer Token",
            "description": "The organization's SCIM provisioning token (`aud_…`).",
            "primary": true,
        }],
        "meta": {
            "resourceType": "ServiceProviderConfig",
            "location": format!("{state_base}/scim/v2/ServiceProviderConfig",
                state_base = state.config.external_url.trim_end_matches('/')),
        },
    });
    scim_response(StatusCode::OK, body)
}

/// `GET /ResourceTypes`.
async fn resource_types(State(state): State<AppState>, req: Request) -> Response {
    if let Err(e) = ScimToken::authenticate(&state, req.headers()).await {
        return e.into_response();
    }
    let base = state.config.external_url.trim_end_matches('/');
    let body = json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
        "totalResults": 2,
        "startIndex": 1,
        "itemsPerPage": 2,
        "Resources": [
            {
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],
                "id": "User",
                "name": "User",
                "endpoint": "/Users",
                "description": "The organization's directory users.",
                "schema": "urn:ietf:params:scim:schemas:core:2.0:User",
                "meta": {
                    "resourceType": "ResourceType",
                    "location": format!("{base}{BASE}/ResourceTypes/User"),
                },
            },
            {
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],
                "id": "Group",
                "name": "Group",
                "endpoint": "/Groups",
                "description": "The organization's SCIM groups.",
                "schema": "urn:ietf:params:scim:schemas:core:2.0:Group",
                "meta": {
                    "resourceType": "ResourceType",
                    "location": format!("{base}{BASE}/ResourceTypes/Group"),
                },
            }
        ],
    });
    scim_response(StatusCode::OK, body)
}

/// `GET /Schemas` — the User and Group core schemas with exactly the attributes of the
/// subset (the enterprise extension is absent, `16 §W15`).
async fn schemas(State(state): State<AppState>, req: Request) -> Response {
    if let Err(e) = ScimToken::authenticate(&state, req.headers()).await {
        return e.into_response();
    }
    scim_response(
        StatusCode::OK,
        json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
            "totalResults": 2,
            "startIndex": 1,
            "itemsPerPage": 2,
            "Resources": [user_schema_doc(), group_schema_doc()],
        }),
    )
}

/// The User schema document (`serde_json::json!` is not `const`-legal, so this is a function).
fn user_schema_doc() -> serde_json::Value {
    json!({
    "id": "urn:ietf:params:scim:schemas:core:2.0:User",
    "name": "User",
    "description": "User Account",
    "endpoint": "/Users",
    "schema": "urn:ietf:params:scim:schemas:core:2.0:User",
    "attributes": [
        {"name": "userName", "type": "string", "multiValued": false, "required": true,
         "caseExact": false, "mutability": "readWrite", "returned": "default",
         "uniqueness": "server"},
        {"name": "externalId", "type": "string", "multiValued": false, "required": false,
         "caseExact": true, "mutability": "readWrite", "returned": "default",
         "uniqueness": "none"},
        {"name": "displayName", "type": "string", "multiValued": false, "required": false,
         "caseExact": false, "mutability": "readWrite", "returned": "default",
         "uniqueness": "none"},
        {"name": "name", "type": "complex", "multiValued": false, "required": false,
         "mutability": "readWrite", "returned": "default", "uniqueness": "none",
         "subAttributes": [
            {"name": "givenName", "type": "string", "multiValued": false, "required": false,
             "caseExact": false, "mutability": "readWrite", "returned": "default",
             "uniqueness": "none"},
            {"name": "familyName", "type": "string", "multiValued": false, "required": false,
             "caseExact": false, "mutability": "readWrite", "returned": "default",
             "uniqueness": "none"},
            {"name": "formatted", "type": "string", "multiValued": false, "required": false,
             "caseExact": false, "mutability": "readWrite", "returned": "default",
             "uniqueness": "none"}
         ]},
        {"name": "emails", "type": "complex", "multiValued": true, "required": false,
         "mutability": "readWrite", "returned": "default", "uniqueness": "none",
         "subAttributes": [
            {"name": "value", "type": "string", "multiValued": false, "required": false,
             "caseExact": false, "mutability": "readWrite", "returned": "default",
             "uniqueness": "none"},
            {"name": "type", "type": "string", "multiValued": false, "required": false,
             "caseExact": false, "mutability": "readWrite", "returned": "default",
             "uniqueness": "none", "canonicalValues": ["work", "home", "other"]},
            {"name": "primary", "type": "boolean", "multiValued": false, "required": false,
             "mutability": "readWrite", "returned": "default", "uniqueness": "none"}
         ]},
        {"name": "active", "type": "boolean", "multiValued": false, "required": false,
         "mutability": "readWrite", "returned": "default", "uniqueness": "none"},
        {"name": "groups", "type": "complex", "multiValued": true, "required": false,
         "mutability": "readOnly", "returned": "default", "uniqueness": "none"},
        {"name": "roles", "type": "complex", "multiValued": true, "required": false,
         "mutability": "readWrite", "returned": "default", "uniqueness": "none",
         "description": "Refused on input: Aurora roles are managed in Aurora."},
        {"name": "password", "type": "string", "multiValued": false, "required": false,
         "mutability": "writeOnly", "returned": "never", "uniqueness": "none",
         "description": "Refused on input: there is no local password."},
        {"name": "entitlements", "type": "complex", "multiValued": true, "required": false,
         "mutability": "readWrite", "returned": "default", "uniqueness": "none",
         "description": "Refused on input."},
        {"name": "x509Certificates", "type": "complex", "multiValued": true, "required": false,
         "mutability": "readWrite", "returned": "default", "uniqueness": "none",
         "description": "Refused on input."}
    ],
    "meta": {
        "resourceType": "Schema",
        "location": "/scim/v2/Schemas/urn:ietf:params:scim:schemas:core:2.0:User"
    }
    })
}

/// The Group schema document.
fn group_schema_doc() -> serde_json::Value {
    json!({
    "id": "urn:ietf:params:scim:schemas:core:2.0:Group",
    "name": "Group",
    "description": "Group",
    "endpoint": "/Groups",
    "schema": "urn:ietf:params:scim:schemas:core:2.0:Group",
    "attributes": [
        {"name": "displayName", "type": "string", "multiValued": false, "required": true,
         "caseExact": false, "mutability": "readWrite", "returned": "default",
         "uniqueness": "none"},
        {"name": "externalId", "type": "string", "multiValued": false, "required": false,
         "caseExact": true, "mutability": "readWrite", "returned": "default",
         "uniqueness": "none"},
        {"name": "members", "type": "complex", "multiValued": true, "required": false,
         "mutability": "readWrite", "returned": "default", "uniqueness": "none",
         "subAttributes": [
            {"name": "value", "type": "string", "multiValued": false, "required": false,
             "caseExact": true, "mutability": "readWrite", "returned": "default",
             "uniqueness": "none"},
            {"name": "display", "type": "string", "multiValued": false, "required": false,
             "caseExact": false, "mutability": "readOnly", "returned": "default",
             "uniqueness": "none"},
            {"name": "type", "type": "string", "multiValued": false, "required": false,
             "caseExact": false, "mutability": "readWrite", "returned": "default",
             "uniqueness": "none", "canonicalValues": ["User"]}
         ]}
    ],
    "meta": {
        "resourceType": "Schema",
        "location": "/scim/v2/Schemas/urn:ietf:params:scim:schemas:core:2.0:Group"
    }
    })
}

/// The media-type gate of every SCIM request (`16 §W15` Transport): `Accept` naming no JSON
/// type → 406; a body content type that is neither `application/scim+json` nor
/// `application/json` → 415.
pub fn media_type_checks(headers: &HeaderMap, has_body: bool) -> ScimResult<()> {
    if let Some(accept) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        let ok = accept.split(',').any(|part| {
            let p = part.split(';').next().unwrap_or("").trim();
            p.eq_ignore_ascii_case("application/scim+json")
                || p.eq_ignore_ascii_case("application/json")
                || p.eq_ignore_ascii_case("*/*")
                || p.eq_ignore_ascii_case("application/*")
        });
        if !ok {
            return Err(ScimError::new(
                StatusCode::NOT_ACCEPTABLE,
                super::error::ScimType::None,
                "Responses are `application/scim+json`.",
            ));
        }
    }
    if has_body
        && let Some(ct) = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
    {
        {
            let p = ct.split(';').next().unwrap_or("").trim();
            if !p.eq_ignore_ascii_case("application/scim+json")
                && !p.eq_ignore_ascii_case("application/json")
            {
                return Err(ScimError::new(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    super::error::ScimType::None,
                    "Requests use `application/scim+json` (or `application/json`).",
                ));
            }
        }
    }
    Ok(())
}

async fn media_type_middleware(req: Request, next: Next) -> Response {
    let has_body = matches!(
        *req.method(),
        axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::PATCH
    );
    if let Err(e) = media_type_checks(req.headers(), has_body) {
        return e.into_response();
    }
    let mut res = next.run(req).await;
    if !res.headers().contains_key(header::CONTENT_TYPE) {
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/scim+json"),
        );
    }
    res
}

/// The router of `/scim/v2` (rate-limit layer innermost so the bearer is resolved before
/// the 401 is answered; the client-IP and the instance-404 layers are applied in `app.rs`).
/// The stateful middlewares take the state here (`with_state` on the parent router cannot
/// feed a `from_fn` inside a nested router).
pub fn router(state: &crate::app::AppState) -> axum::Router<AppState> {
    use axum::middleware;
    axum::Router::new()
        .route("/ServiceProviderConfig", get(service_provider_config))
        .route("/ResourceTypes", get(resource_types))
        .route("/Schemas", get(schemas))
        .route("/Users", get(users_list).post(users_create))
        .route(
            "/Users/{id}",
            get(users_get)
                .put(users_put)
                .patch(users_patch)
                .delete(users_delete),
        )
        .route("/Groups", get(groups_list).post(groups_create))
        .route(
            "/Groups/{id}",
            get(groups_get)
                .put(groups_put)
                .patch(groups_patch)
                .delete(groups_delete),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            super::rate_limit::middleware,
        ))
        .layer(middleware::from_fn(media_type_middleware))
        .layer(axum::middleware::from_fn(metrics_middleware))
}

/// `aurora_scim_requests_total{outcome}`: one increment per `/scim/v2` response. The label is
/// the status class only (`ok`, `unauthorized`, `rate_limited`, `client_error`,
/// `server_error`): never an org, token, e-mail or path.
async fn metrics_middleware(req: Request, next: Next) -> Response {
    let res = next.run(req).await;
    let outcome = match res.status().as_u16() {
        401 => "unauthorized",
        429 => "rate_limited",
        s if s >= 500 => "server_error",
        s if s >= 400 => "client_error",
        _ => "ok",
    };
    metrics::counter!("aurora_scim_requests_total", "outcome" => outcome).increment(1);
    res
}
/// Marker: the router wrappers insert the parsed `{id}` here.
#[derive(Debug, Clone, Copy)]
pub struct PathId(pub Uuid);

/// The `{id}` of an item route, parsed **after** authentication: a malformed one is a SCIM
/// `404` error body (never axum's plain-text 400 from a `Path<Uuid>` extractor, and never
/// something an unauthenticated caller can provoke instead of the one 401).
fn parse_id(raw: &str) -> ScimResult<Uuid> {
    Uuid::parse_str(raw).map_err(|_| ScimError::not_found())
}

fn content_length(req: &Request) -> Option<usize> {
    req.headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

/// Build a SCIM response with the right content type.
fn scim_response(status: StatusCode, body: serde_json::Value) -> Response {
    let mut res = (status, axum::Json(body)).into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/scim+json"),
    );
    res
}

/// Map a `Result<T, ScimError>` to a response with `Content-Type: application/scim+json`.
fn finish<T: IntoResponse>(r: ScimResult<T>) -> Response {
    match r {
        Ok(ok) => {
            let mut res = ok.into_response();
            res.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/scim+json"),
            );
            res
        }
        Err(e) => e.into_response(),
    }
}

// ---- the shared orchestration -------------------------------------------------------
//
// The wrappers take ownership of the `Request` (axum handlers cannot borrow from the request
// beyond the call) and run one of three shapes: a list (no id, maybe query), an item
// (path id, no body), a body request (collection POST or item PUT/PATCH). The `{id}` of the
// item shapes is extracted by axum's `Path` extractor in the handler signatures above.

async fn users_list(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    finish(super::resource_users::list_users(&state, &parts.headers, parts.uri.query()).await)
}

async fn groups_list(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    finish(super::resource_groups::list_groups(&state, &parts.headers, parts.uri.query()).await)
}

/// A body request: **authenticate first** (FIX-081: the one 401 for a bearer-less or dead-token
/// request, before anything of the body is read — a 2 MiB body or malformed JSON is never
/// parsed for a caller who is not allowed to send it), then read the body through the bounded
/// reader, run the handler, render.
macro_rules! body_request {
    ($state:expr, $req:ident, |$s:ident, $c:ident, $body:ident| $call:expr) => {{
        let mut req = $req;
        match ScimToken::authenticate($state, req.headers()).await {
            Ok($c) => {
                let $body = match bounded_body(&mut req).await {
                    Ok(b) => b,
                    Err(e) => return e.into_response(),
                };
                let $s = $state;
                finish($call.await)
            }
            Err(e) => e.into_response(),
        }
    }};
}

async fn users_create(State(state): State<AppState>, req: Request) -> Response {
    body_request!(&state, req, |s, c, body| {
        super::resource_users::create_user(s, c, body)
    })
}

async fn groups_create(State(state): State<AppState>, req: Request) -> Response {
    body_request!(&state, req, |s, c, body| {
        super::resource_groups::create_group(s, c, body)
    })
}

async fn users_put(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    body_request!(&state, req, |s, c, body| {
        async {
            let id = parse_id(&raw)?;
            super::resource_users::put_user(s, c, id, body).await
        }
    })
}

async fn users_patch(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    body_request!(&state, req, |s, c, body| {
        async {
            let id = parse_id(&raw)?;
            super::resource_users::patch_user(s, c, id, body).await
        }
    })
}

async fn groups_put(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    body_request!(&state, req, |s, c, body| {
        async {
            let id = parse_id(&raw)?;
            super::resource_groups::put_group(s, c, id, body).await
        }
    })
}

async fn groups_patch(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    body_request!(&state, req, |s, c, body| {
        async {
            let id = parse_id(&raw)?;
            super::resource_groups::patch_group(s, c, id, body).await
        }
    })
}

/// A body-less item request (GET/DELETE `{id}`): authenticate, parse the id, run, render.
macro_rules! item_request {
    ($state:expr, $req:ident, $raw:expr, $handler:path) => {{
        let req = $req;
        match ScimToken::authenticate($state, req.headers()).await {
            Ok(caller) => match parse_id(&$raw) {
                Ok(id) => finish($handler($state, caller, id).await),
                Err(e) => e.into_response(),
            },
            Err(e) => e.into_response(),
        }
    }};
}

async fn users_get(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    item_request!(&state, req, raw, super::resource_users::get_user)
}

async fn users_delete(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    item_request!(&state, req, raw, super::resource_users::delete_user)
}

async fn groups_get(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    item_request!(&state, req, raw, super::resource_groups::get_group)
}

async fn groups_delete(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    req: Request,
) -> Response {
    item_request!(&state, req, raw, super::resource_groups::delete_group)
}

/// A body request (collection POST or item PUT/PATCH): read the body through the bounded
/// reader, authenticate, run the handler (`$call` may name `id` — the parsed path id, which
/// is `Uuid::nil` on collection routes), render.
/// The request body, capped at `SCIM_BODY_MAX` (413 above it).
pub async fn bounded_body(req: &mut Request) -> ScimResult<serde_json::Value> {
    // A declared length over the cap is refused without reading a byte (the caller has
    // authenticated by now, FIX-081).
    if content_length(req).is_some_and(|n| n > limits::SCIM_BODY_MAX) {
        return Err(ScimError::payload_too_large());
    }
    let bytes = axum::body::to_bytes(std::mem::take(req.body_mut()), limits::SCIM_BODY_MAX + 1)
        .await
        .map_err(|e| {
            // The limit is `SCIM_BODY_MAX + 1`: tripping it means the body is over the
            // cap (413, with or without a trustworthy Content-Length). Anything else is
            // a read failure (400).
            if e.into_inner().downcast_ref::<LengthLimitError>().is_some() {
                ScimError::payload_too_large()
            } else {
                ScimError::invalid_syntax("The body could not be read.")
            }
        })?;
    if bytes.len() > limits::SCIM_BODY_MAX {
        return Err(ScimError::payload_too_large());
    }
    if bytes.is_empty() {
        return Err(ScimError::invalid_syntax("The body is empty."));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| ScimError::invalid_syntax(format!("The body is not valid JSON: {e}")))
}

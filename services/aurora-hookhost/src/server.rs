//! `POST /internal/v1/hooks/{hook}` (docs/16 §W9).

use std::sync::Arc;

use aurora_core::limits::HOOK_OUTPUT;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::json;

use crate::runtime::{CallError, CallSpec, Invocation};
use crate::snapshot::Catalog;
use crate::validate::{self, to_json};
use crate::wire::{HookKind, Request, parse_invocation, secrets};

const BODY_MAX: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub catalog: Arc<Catalog>,
    /// Internal-secret ring: any key is accepted, `X-Aurora-Key-Id` names it (RFE-007).
    pub ring: Arc<aurora_crypto::SecretRing>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/internal/v1/hooks/{hook}", post(handle))
        .route("/internal/v1/status", axum::routing::get(ring_status))
        .layer(DefaultBodyLimit::max(BODY_MAX))
        .with_state(state)
}

fn status(code: &str) -> StatusCode {
    match code {
        "invalid_input" => StatusCode::BAD_REQUEST,
        "timeout" => StatusCode::GATEWAY_TIMEOUT,
        "internal" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

fn err(e: &CallError) -> Response {
    (
        status(e.code),
        axum::Json(json!({ "error": { "code": e.code, "message": e.message } })),
    )
        .into_response()
}

/// `Some(401)` unless the caller presents a key of the ring (constant-time).
fn unauthorized(st: &AppState, headers: &HeaderMap) -> Option<Response> {
    let key_id = headers
        .get(aurora_core::limits::KEY_ID_HEADER)
        .map(|v| v.to_str().unwrap_or("\u{0}"));
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if st.ring.verify_bearer_header(key_id, auth) {
        return None;
    }
    Some(
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "error": { "code": "unauthorized", "message": "internal secret required" } })),
        )
            .into_response(),
    )
}

/// Ring ids + fingerprints (never values), to confirm rotation convergence (`08 §4`).
async fn ring_status(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = unauthorized(&st, &headers) {
        return r;
    }
    let (active, keys) = st.ring.status();
    let keys: Vec<_> = keys
        .into_iter()
        .map(|(id, fp)| json!({ "id": id, "fingerprint": fp }))
        .collect();
    axum::Json(
        json!({ "service": "hookhost", "internal_secret": { "active": active, "keys": keys } }),
    )
    .into_response()
}

async fn handle(
    State(st): State<AppState>,
    Path(hook): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(r) = unauthorized(&st, &headers) {
        return r;
    }
    let Some(kind) = HookKind::parse(&hook) else {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": { "code": "not_found", "message": "unknown hook" } })),
        )
            .into_response();
    };
    let started = std::time::Instant::now();
    let mut package = String::new();
    let res = run(&st, kind, &body, &mut package).await;
    let outcome = match &res {
        Ok(_) => "ok",
        Err(e) => e.code,
    };
    metrics::counter!("aurora_hook_calls_total", "package" => package.clone(), "hook" => kind.name(), "outcome" => outcome).increment(1);
    if res.is_err() {
        metrics::counter!("aurora_hook_errors_total", "package" => package.clone(), "hook" => kind.name(), "code" => outcome).increment(1);
    }
    metrics::histogram!("aurora_hook_duration_seconds", "package" => package, "hook" => kind.name())
        .record(started.elapsed().as_secs_f64());
    match res {
        Ok(v) => axum::Json(json!({ "ok": v })).into_response(),
        Err(e) => err(&e),
    }
}

async fn run(
    st: &AppState,
    kind: HookKind,
    body: &[u8],
    package_label: &mut String,
) -> Result<serde_json::Value, CallError> {
    let req: Request = serde_json::from_slice(body)
        .map_err(|e| CallError::new("invalid_input", format!("invalid request: {e}")))?;
    let find = |l: &crate::snapshot::Loaded| {
        let app = l.snapshot.app(&req.app_id)?;
        let pkg = l
            .snapshot
            .packages
            .iter()
            .find(|p| p.package_id == app.package_id)?;
        (app.package_id == req.package_id && pkg.digest == req.package_digest).then_some(())
    };
    let mut loaded = st.catalog.current().await;
    if !loaded.as_ref().is_some_and(|l| find(l).is_some()) {
        st.catalog.refresh_on_miss().await;
        loaded = st.catalog.current().await;
    }
    let loaded =
        loaded.ok_or_else(|| CallError::new("internal", "registry snapshot not loaded"))?;
    if find(&loaded).is_none() {
        return Err(CallError::new(
            "invalid_input",
            "unknown package or app for this digest",
        ));
    }
    let snap = &loaded.snapshot;
    let app = snap
        .app(&req.app_id)
        .ok_or_else(|| CallError::new("invalid_input", "unknown app"))?;
    let pkg = snap
        .packages
        .iter()
        .find(|p| p.package_id == app.package_id)
        .ok_or_else(|| CallError::new("invalid_input", "unknown package"))?;
    *package_label = pkg.package_id.clone();
    let digest = pkg
        .hooks_digest
        .as_deref()
        .ok_or_else(|| CallError::new("unsupported", "package has no hooks"))?;

    let secret_values = secrets(&req.input);
    let inv: Invocation = parse_invocation(kind, &app.app_id, req.input, &pkg.context)?;
    let module = st.catalog.module(digest).await?;
    let egress = if kind.egress_allowed() {
        pkg.egress
            .get(&kind.registry())
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let spec = CallSpec {
        package_id: pkg.package_id.clone(),
        app_id: app.app_id.clone(),
        hook: kind.name(),
        egress,
        secrets: secret_values,
        wall: kind.wall(),
    };
    let (res, stats) = st.catalog.runtime().call(&module, spec, inv).await;
    if stats.epoch_kill {
        metrics::counter!("aurora_hook_epoch_kills_total", "package" => pkg.package_id.clone(), "hook" => kind.name()).increment(1);
    }
    let outcome = res?;
    let too_big = || CallError::new("output_too_large", "hook output exceeds the limit");
    if crate::wire::output_json(&outcome).to_string().len() > HOOK_OUTPUT {
        return Err(too_big());
    }
    let outcome = validate::validate(
        &validate::Ctx {
            snapshot: snap,
            package: pkg,
            app,
        },
        outcome,
    )?;
    let json = to_json(&outcome);
    if json.to_string().len() > HOOK_OUTPUT {
        return Err(too_big());
    }
    Ok(json)
}

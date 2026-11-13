//! `POST /internal/v1/library-fetch` (phase 4, P4-GSL, D-149, SEC-84; docs/06 §Library fetch).
//!
//! The API's skill-library jobs read repository files through this route so the connection's
//! credential is used **only** by the gateway. The route is not a proxy: the request is *built*
//! here from the active snapshot's `apps[].library` templates with parameters that matched their
//! grammars (`aurora_registry::library::build`), sent to the block's exact `request_host`
//! through the guarded resolver, never following a redirect, with a response cap and a total
//! timeout. Policy, grants and approvals do not apply (no agent is involved); the authority is
//! the org owner/admin who connected the library and the instance admin who approved the manifest.
//! The internal port is unreachable from sandboxes and the route needs the internal secret.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine;
use http::Method;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use aurora_crypto::SecretRing;
use aurora_db::PgPool;
use aurora_db::repo::gateway as repo;
use aurora_registry::library::{self, Params, Step};
use aurora_registry::manifest::INSTANCE_HOST;

use crate::apps::{ConnCand, Exec, identify_instance_app};
use crate::decide::inject_and_route;
use crate::pipeline::{PreparedRequest, Scheme};
use crate::plaintext::{PlaintextCache, PlaintextError};
use crate::refresh::{RefreshError, TokenManager};
use crate::snapshot::SnapshotStore;
use crate::upstream::Upstream;

/// Total time of one fetch (`LIBRARY_FETCH_TIMEOUT`, docs/02 §Limits).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Rate limits (`LIBRARY_FETCH_RATE`): per gateway and per organization, per second.
pub const RATE_GLOBAL: f64 = 20.0;
pub const RATE_ORG: f64 = 5.0;
/// Hard cap of a response, whatever `max_bytes` asks.
pub const RESPONSE_CAP: usize = 4 * 1024 * 1024;

/// A token bucket refilled at `rate` per second with a burst of `rate`.
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// In-memory limiter of the route (the volume is bounded per replica, `LIBRARY_FETCH_RATE`).
pub struct Limiter {
    inner: Mutex<(Option<Bucket>, HashMap<Uuid, Bucket>)>,
    global_rate: f64,
    org_rate: f64,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::with_rates(RATE_GLOBAL, RATE_ORG)
    }
}

impl Limiter {
    /// Per-second rates (burst = rate) per gateway and per organization.
    #[must_use]
    pub fn with_rates(global: f64, org: f64) -> Self {
        Self {
            inner: Mutex::new((None, HashMap::new())),
            global_rate: global,
            org_rate: org,
        }
    }

    fn take(b: &mut Bucket, rate: f64, now: Instant) -> bool {
        let dt = now.saturating_duration_since(b.at).as_secs_f64();
        b.tokens = (b.tokens + dt * rate).min(rate);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Both buckets must have a token; a refused request consumes none.
    pub fn allow(&self, org: Uuid) -> bool {
        self.allow_at(org, Instant::now())
    }

    /// [`Limiter::allow`] at an explicit instant (the clock is injected by tests).
    pub fn allow_at(&self, org: Uuid, now: Instant) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        let (global, orgs) = &mut *g;
        if orgs.len() > 10_000 {
            orgs.retain(|_, b| now.saturating_duration_since(b.at) < Duration::from_secs(5));
        }
        let (gr, or) = (self.global_rate, self.org_rate);
        let gb = global.get_or_insert(Bucket {
            tokens: gr,
            at: now,
        });
        let ob = orgs.entry(org).or_insert(Bucket {
            tokens: or,
            at: now,
        });
        // Peek both before consuming either.
        let g_ok =
            (gb.tokens + now.saturating_duration_since(gb.at).as_secs_f64() * gr).min(gr) >= 1.0;
        let o_ok =
            (ob.tokens + now.saturating_duration_since(ob.at).as_secs_f64() * or).min(or) >= 1.0;
        if !(g_ok && o_ok) {
            return false;
        }
        Self::take(gb, gr, now) && Self::take(ob, or, now)
    }
}

pub struct LibraryFetch {
    pub ring: Arc<SecretRing>,
    pub snapshots: Arc<SnapshotStore>,
    pub pool: PgPool,
    pub tokens: Arc<TokenManager>,
    pub plaintext: Arc<PlaintextCache>,
    pub upstream: Arc<Upstream>,
    pub external_url: String,
    /// Production is `https` on 443; tests point the route at a loopback `http` listener.
    pub scheme: Scheme,
    pub port: u16,
    pub limiter: Limiter,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchRequest {
    org_id: Uuid,
    connection_id: Uuid,
    step: StepName,
    params: FetchParams,
    max_bytes: usize,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum StepName {
    ResolveRef,
    ListTree,
    GetBlob,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchParams {
    repo: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
    sha: Option<String>,
    blob: Option<String>,
    page: Option<u32>,
}

impl StepName {
    fn step(self) -> Step {
        match self {
            Self::ResolveRef => Step::ResolveRef,
            Self::ListTree => Step::ListTree,
            Self::GetBlob => Step::GetBlob,
        }
    }
}

pub fn router(f: Arc<LibraryFetch>) -> Router {
    Router::new()
        .route("/internal/v1/library-fetch", post(handle))
        .with_state(f)
}

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}

fn count(step: &str, status: &str) {
    metrics::counter!("aurora_gateway_library_fetch_total", "step" => step.to_owned(), "status" => status.to_owned())
        .increment(1);
}

async fn handle(
    State(f): State<Arc<LibraryFetch>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let key_id = headers
        .get(aurora_core::limits::KEY_ID_HEADER)
        .map(|v| v.to_str().unwrap_or("\u{0}"));
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if !f.ring.verify_bearer_header(key_id, auth) {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "internal secret required",
        );
    }
    let Ok(req) = serde_json::from_slice::<FetchRequest>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "malformed library fetch request",
        );
    };
    let step = req.step.step();
    let resp = fetch(&f, &req).await;
    count(step.as_str(), resp.status().as_str());
    resp
}

async fn fetch(f: &LibraryFetch, req: &FetchRequest) -> Response {
    if !f.limiter.allow(req.org_id) {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "library fetch rate limit",
        );
    }
    let step = req.step.step();
    let Some(loaded) = f.snapshots.current() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway_degraded",
            "no registry snapshot is loaded",
        );
    };
    // The connection must exist in the named organization, be org-scoped and active.
    let row = match repo::app_connection_in_org(&f.pool, req.org_id, req.connection_id).await {
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
    if row.workspace_id.is_some() {
        return err(
            StatusCode::CONFLICT,
            "library_unavailable",
            "the connection is not organization-scoped",
        );
    }
    match row.status.as_str() {
        "active" => {}
        "reauth_required" => {
            return err(
                StatusCode::CONFLICT,
                "reauth_required",
                "the connection needs to be reconnected",
            );
        }
        _ => {
            return err(
                StatusCode::CONFLICT,
                "library_unavailable",
                "the connection is not active",
            );
        }
    }
    let Some(app) = loaded.snapshot.app(&row.app_id) else {
        return err(
            StatusCode::CONFLICT,
            "library_unavailable",
            "the connection's app is not installed",
        );
    };
    let Some(lib) = &app.library else {
        return err(
            StatusCode::CONFLICT,
            "library_unavailable",
            "the app cannot serve a skill library",
        );
    };
    // 1. The request is built from the template; every parameter is validated first.
    let params = Params {
        repo: req.params.repo.clone(),
        reference: req.params.reference.clone(),
        sha: req.params.sha.clone(),
        blob: req.params.blob.clone(),
        page: req.params.page,
    };
    let target = match library::build(lib, step, &params) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": { "code": "library_param", "message": "a library parameter was refused", "details": { "param": e.param, "reason": e.reason } } })),
            )
                .into_response();
        }
    };
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
        None => (target, None),
    };
    // 2. The host: the block's exact `request_host`; `{instance}` is the connection's instance.
    let mut conn = match ConnCand::from_row(row, req.org_id) {
        Ok(c) => c,
        Err(_) => {
            return err(
                StatusCode::CONFLICT,
                "library_unavailable",
                "the connection is unusable",
            );
        }
    };
    let on_instance = lib.request_host == INSTANCE_HOST;
    let (host, port) = if on_instance {
        let instance_host = match conn.instance_id {
            Some(id) => match repo::visible_instances(&f.pool, req.org_id).await {
                Ok(v) => v
                    .into_iter()
                    .find(|i| i.id == id && i.app_id == conn.app_id)
                    .map(|i| i.host),
                Err(_) => {
                    return err(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "gateway_degraded",
                        "the platform database is unreachable",
                    );
                }
            },
            None => app.instances.as_ref().and_then(|i| i.default_host.clone()),
        };
        let Some(h) = instance_host else {
            return err(
                StatusCode::CONFLICT,
                "library_unavailable",
                "the connection's instance is not available",
            );
        };
        conn.instance_host = Some(h.clone());
        split_port(&h, f.port)
    } else {
        (lib.request_host.to_ascii_lowercase(), f.port)
    };
    // 3. The host entry whose inject stage authenticates the request.
    let entry = if on_instance {
        identify_instance_app(&loaded, &conn.app_id, &path).map(|m| m.entry)
    } else {
        loaded
            .lookup(&host, port, &path)
            .filter(|e| e.app_id == conn.app_id)
    };
    let Some(entry) = entry else {
        return err(
            StatusCode::CONFLICT,
            "library_unavailable",
            "no host rule of the app covers the library request",
        );
    };
    if entry.rewrite_host.is_some() || entry.finalize.is_some() || entry.intercept.is_some() {
        return err(
            StatusCode::CONFLICT,
            "library_unavailable",
            "the host rule cannot serve a library",
        );
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
    put("instance_url", f.external_url.clone());
    let cap = req.max_bytes.min(RESPONSE_CAP);
    let refresh = TokenManager::applies(app);
    let mut force = false;
    for attempt in 0..2 {
        // 4. token freshness, credentials, inject.
        let fresh = if refresh {
            match f.tokens.ensure_fresh(&loaded, &conn, &context, force).await {
                Ok(c) => c,
                Err(e) => return refresh_error(e),
            }
        } else {
            conn.clone()
        };
        let raw = match f.plaintext.get_connection(&fresh).await {
            Ok(v) => v,
            Err(PlaintextError::StoreUnavailable(_)) => {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "secret_store_unavailable",
                    "the secret store is unreachable",
                );
            }
            Err(PlaintextError::DatabaseUnavailable) => {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "gateway_degraded",
                    "the platform database is unreachable",
                );
            }
            Err(PlaintextError::Unavailable) => {
                return err(
                    StatusCode::CONFLICT,
                    "library_unavailable",
                    "the connection's credential is unavailable",
                );
            }
        };
        let Some(exec) = Exec::new(app, pkg_ctx, &fresh, raw.expose(), context.clone()) else {
            return err(
                StatusCode::CONFLICT,
                "library_unavailable",
                "the connection's credential is unusable",
            );
        };
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::ACCEPT, http::HeaderValue::from_static("*/*"));
        headers.insert(
            http::header::USER_AGENT,
            http::HeaderValue::from_static("aurora-skill-library"),
        );
        let mut p = path.clone();
        let mut q = query.clone();
        if inject_and_route(entry, &exec.vars(), &mut headers, &mut p, &mut q).is_err() {
            return err(
                StatusCode::CONFLICT,
                "library_unavailable",
                "the credential could not be applied",
            );
        }
        let prepared = PreparedRequest {
            scheme: f.scheme,
            host: host.clone(),
            port,
            method: Method::GET,
            path: p,
            query: q,
            headers,
            upgrade: None,
            body: crate::empty_body(),
            hook: None,
            response_headers: http::HeaderMap::new(),
            retry: None,
        };
        let sent = tokio::time::timeout(FETCH_TIMEOUT, async {
            let resp = f.upstream.send(prepared).await?;
            let (parts, mut body) = resp.into_parts();
            let mut buf: Vec<u8> = Vec::new();
            let mut too_big = false;
            let mut broken = false;
            while let Some(frame) = body.frame().await {
                // A reset mid-body is a failure, never a short "complete" file.
                let Ok(frame) = frame else {
                    broken = true;
                    break;
                };
                if let Some(d) = frame.data_ref() {
                    if buf.len() + d.len() > cap {
                        too_big = true;
                        break;
                    }
                    buf.extend_from_slice(d);
                }
            }
            Ok::<_, aurora_core::ApiError>((parts, buf, too_big, broken))
        })
        .await;
        let (parts, buf, too_big, broken) = match sent {
            Err(_) => {
                return err(
                    StatusCode::GATEWAY_TIMEOUT,
                    "library_timeout",
                    "the git host did not answer in time",
                );
            }
            Ok(Err(e)) => {
                tracing::debug!(code = %e.code(),"library fetch upstream failure");
                return err(
                    StatusCode::BAD_GATEWAY,
                    "library_upstream",
                    "the git host could not be reached",
                );
            }
            Ok(Ok(v)) => v,
        };
        if broken {
            tracing::debug!("library fetch: the upstream body ended in an error");
            return err(
                StatusCode::BAD_GATEWAY,
                "library_upstream",
                "the git host's response was interrupted",
            );
        }
        if too_big {
            return err(
                StatusCode::PAYLOAD_TOO_LARGE,
                "library_response_too_large",
                "the git host's response exceeds the limit",
            );
        }
        // A stale token (401) is refreshed once and the same request sent again.
        if parts.status == StatusCode::UNAUTHORIZED && refresh && attempt == 0 {
            force = true;
            continue;
        }
        let mut out_headers = serde_json::Map::new();
        for name in ["link", "retry-after"] {
            if let Some(v) = parts.headers.get(name).and_then(|v| v.to_str().ok()) {
                out_headers.insert(
                    name.to_owned(),
                    Value::String(v.chars().take(2000).collect()),
                );
            }
        }
        return axum::Json(json!({
            "status": parts.status.as_u16(),
            "headers": out_headers,
            "encoding": "base64",
            "body": base64::engine::general_purpose::STANDARD.encode(&buf),
        }))
        .into_response();
    }
    let _ = step;
    err(
        StatusCode::BAD_GATEWAY,
        "library_upstream",
        "the git host rejected the connection's credential",
    )
}

fn refresh_error(e: RefreshError) -> Response {
    match e {
        RefreshError::Reauth => err(
            StatusCode::CONFLICT,
            "reauth_required",
            "the connection needs to be reconnected",
        ),
        RefreshError::StoreUnavailable(_) => err(
            StatusCode::SERVICE_UNAVAILABLE,
            "secret_store_unavailable",
            "the secret store is unreachable",
        ),
        RefreshError::Unavailable => err(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway_degraded",
            "the credential could not be refreshed",
        ),
    }
}

/// `host` or `host:port` of an instance host.
fn split_port(h: &str, default: u16) -> (String, u16) {
    match h.rsplit_once(':') {
        Some((a, p)) if !a.contains(':') => match p.parse() {
            Ok(p) => (a.to_ascii_lowercase(), p),
            Err(_) => (h.to_ascii_lowercase(), default),
        },
        _ => (h.to_ascii_lowercase(), default),
    }
}

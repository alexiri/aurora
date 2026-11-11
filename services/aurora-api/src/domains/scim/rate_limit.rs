//! SCIM rate limiting (04 §SCIM Rate limits and size, docs/05 §Conventions): a per-token
//! bucket (`SCIM_RATE`, burst = a fifth, class `management`, `key = api:scim:token:<id>`)
//! through the same `CounterStore` machinery as the `/v1` limiter, and the existing
//! 30/min/IP rejected-bearer limit (class `security`, never fails open) for bad tokens.
//!
//! `/scim/v2` is outside the `/v1` limiter's scope, so this middleware is the SCIM plane's
//! own limiter. A refusal is 429 in SCIM error format with `retry-after`.

use super::error::ScimError;
use crate::app::AppState;
use crate::http::apilimit::{Charge, Spec};
use aurora_core::limits;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::time::Duration;

/// The rejected-bearer limit of `/v1` (30/min/IP); the SCIM plane applies the same.
const REJECTED_BEARER_PER_MIN: u32 = 30;

fn scim_spec(per_minute: u32) -> Spec {
    Spec::per_min(per_minute, (per_minute / 5).max(1))
}

fn rejected_spec() -> Spec {
    Spec::per_min(REJECTED_BEARER_PER_MIN, REJECTED_BEARER_PER_MIN)
}

/// The local bucket key of one SCIM token.
fn token_key(id: uuid::Uuid) -> String {
    format!("scim:token:{id}")
}

/// The rejected-bearer bucket key of one client IP.
fn rejected_key(ip: &str) -> String {
    format!("scim-rejected:{ip}")
}

fn bearer(headers: &http::HeaderMap) -> Option<&str> {
    let v = headers.get(http::header::AUTHORIZATION)?;
    let s = v.to_str().ok()?;
    let (scheme, rest) = s.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim())
        .filter(|t| !t.is_empty())
}

/// Local + shared take for one charge; `Err` is the 429 to answer.
async fn take(
    state: &AppState,
    charges: &[Charge],
    management: bool,
) -> Result<(), (ScimError, Option<Charge>)> {
    if let Err(r) = state.api_limiter.take(charges) {
        metrics::counter!("aurora_api_rate_limited_total", "class" => r.class, "scope" => r.scope)
            .increment(1);
        return Err((ScimError::rate_limited(r.retry_after_secs), None));
    }
    let class = if management {
        state.counters.management()
    } else {
        state.counters.security()
    };
    let Some(class) = class else {
        return Ok(());
    };
    if let Err(e) = crate::http::counters::take_shared(state, class, charges).await {
        return Err(match e {
            // The shared store refused: answer 429 from the blocking bucket.
            Ok(r) => {
                metrics::counter!(
                    "aurora_api_rate_limited_total",
                    "class" => r.class,
                    "scope" => r.scope
                )
                .increment(1);
                (ScimError::rate_limited(r.retry_after_secs), None)
            }
            // A `closed` class refuses with 503 (never a 429: an outage is not a rate).
            Err(api) => (
                ScimError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    super::error::ScimType::None,
                    api.message().to_owned(),
                ),
                None,
            ),
        });
    }
    Ok(())
}

fn refund(state: &AppState, charges: &[Charge]) {
    state.api_limiter.refund(charges);
}

/// Axum middleware for the `/scim/v2` router (inside the client-IP layer). Resolves the
/// bearer far enough to find its bucket: a *rejected* bearer is limited per IP instead.
pub async fn middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let headers = req.headers().clone();
    let scim_on = state.config.scim.enabled;
    let Some(tok) = bearer(&headers) else {
        // No bearer at all: the 401 is answered by the extractor (identical body). Without a
        // credential there is no bucket; the per-IP limits of the unauthenticated surface are
        // the rejected-bearer bucket only when a bearer was presented, so nothing is charged.
        return next.run(req).await;
    };
    let _ = tok;
    if !scim_on {
        // The whole surface is 404 while the instance switch is off; no buckets apply.
        return next.run(req).await;
    }
    // Resolve the token row (the same read the extractor does; two indexed reads per request,
    // acceptable because SCIM_RATE is the budget).
    let resolved = resolve_for_rate(&state, &headers).await;
    match resolved {
        RateResolve::Token(row, per_minute) => {
            let charge = |k: String| Charge {
                key: k,
                spec: scim_spec(per_minute),
                scope: "general",
                class: "management",
            };
            match take(&state, &[charge(token_key(row.id))], true).await {
                Ok(()) => {
                    let mut res = next.run(req).await;
                    // Remember the resolved token for the extractor (it re-reads; the row may
                    // have died in between, which the extractor then reports).
                    res.extensions_mut().insert(row);
                    res
                }
                Err((e, _)) => {
                    // The local pre-filter already refused; the shared take may have consumed.
                    refund(&state, &[charge(token_key(row.id))]);
                    e.into_response()
                }
            }
        }
        RateResolve::Rejected => {
            let ip = super::auth::client_ip(req.extensions())
                .map(|i| i.to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            let charge = |k: String| Charge {
                key: k,
                spec: rejected_spec(),
                scope: "general",
                class: "unauth",
            };
            match take(&state, &[charge(rejected_key(&ip))], false).await {
                Ok(()) => next.run(req).await,
                Err((e, _)) => {
                    refund(&state, &[charge(rejected_key(&ip))]);
                    e.into_response()
                }
            }
        }
        // A database failure while resolving: let the handler/extractor answer (500/401), not
        // the limiter.
        RateResolve::Error => next.run(req).await,
    }
}

enum RateResolve {
    /// A syntactically valid `aud_` token row (dead or alive: a dead one is still charged —
    /// otherwise a flood of dead tokens would bypass the token bucket and only hit the IP
    /// bucket; the IP bucket alone is the wrong budget for a real token).
    Token(aurora_db::repo::scim::TokenRow, u32),
    /// No `aud_` bearer or malformed: the rejected-bearer IP bucket.
    Rejected,
    Error,
}

async fn resolve_for_rate(state: &AppState, headers: &http::HeaderMap) -> RateResolve {
    use aurora_core::token::{self, TokenPrefix};
    let Some(tok) = bearer(headers) else {
        return RateResolve::Rejected;
    };
    if token::parse_prefix(tok) != Some(TokenPrefix::Scim) {
        return RateResolve::Rejected;
    }
    match aurora_db::repo::scim::token_by_hash(&state.pool, &token::hash(tok)).await {
        Ok(Some(row)) => RateResolve::Token(row, state.config.scim.rate_per_minute),
        Ok(None) => RateResolve::Rejected,
        Err(e) => {
            tracing::error!(error = %e, "scim: token lookup (rate) failed");
            RateResolve::Error
        }
    }
}

/// Seconds per token refill (used by tests).
pub fn refill_secs(per_minute: u32) -> u64 {
    Duration::from_secs_f64(60.0 / f64::from(per_minute.max(1)))
        .as_secs()
        .max(1)
}

/// Body size gate (`SCIM_BODY_MAX`, 413). Applied before the JSON extractor.
pub async fn body_limit_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if !state.config.scim.enabled {
        return next.run(req).await;
    }
    let len = req
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if let Some(n) = len
        && n > limits::SCIM_BODY_MAX
    {
        return ScimError::payload_too_large().into_response();
    }
    // Chunked or missing length: let the extractor's bounded read enforce the cap.
    next.run(req).await
}

//! Outbound operation engine (docs/13 §4, §4.2; BE-071): `POST /ops/claim` (leased,
//! `SKIP LOCKED`, long-polling on `aurora_ops`), `POST /ops/results` (neutral error-code table),
//! the enqueue API for other domains, correlated calls and the lease-lapse sweeper.
//!
//! Decisions:
//! * `maybe_sent` is derived from the old row's `lease_until` (a lapsed claim leaves it set,
//!   whether or not the sweeper already moved the op back to `pending`); definitive answers
//!   that put an op back to `pending` clear it;
//! * only the claiming replica may report (403 otherwise, 404 for an unknown op); a repeated
//!   result for an op that already finished is an idempotent no-op;
//! * within one `results` batch every item is applied independently; the first failure is
//!   returned after the rest were processed;
//! * claim, events, files and link-start require an `active` connector; results do not (in-flight
//!   work may still finish after a manifest-widening re-registration);
//! * a correlated caller that gives up drops its op if it is still `pending`.

use super::auth::ConnectorCtx;
use super::bus::Msg;
use crate::app::AppState;
use crate::http::json::Json;
use aurora_core::ApiError;
use aurora_core::limits::{
    CONNECTOR_OPS_WAIT, OP_BACKOFF_INITIAL, OP_BACKOFF_MAX, OP_LEASE, OP_MAX_ATTEMPTS,
};
use aurora_db::DbError;
use aurora_db::bus::Channel;
use aurora_db::repo::channel_integrations as integrations;
use aurora_db::repo::channel_ops::{self as ops, InsertOp};
use aurora_protocol::common::Validate;
use aurora_protocol::connector::{
    ClaimRequest, ClaimResponse, OkResponse, OpKind, OpResult, Operation, Outcome, ResultsRequest,
};
use axum::extract::State;
use axum::routing::post;
use http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgConnection;
use std::time::Duration;
use tokio::time::Instant;
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

/// Default `retry_after_secs` of a `rate_limited` answer.
const RATE_LIMIT_DEFAULT_SECS: u32 = 30;
/// Cap on a connector-supplied `retry_after_secs`.
const RATE_LIMIT_MAX_SECS: u32 = 24 * 3600;
/// Wait of a correlated call (`file.fetch`: [`FILE_FETCH_WAIT`]).
pub const CORRELATED_WAIT: Duration = Duration::from_secs(10);
pub const FILE_FETCH_WAIT: Duration = Duration::from_secs(30);
/// Re-read interval while waiting (notifications are only hints).
const POLL_EVERY: Duration = Duration::from_secs(1);

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .route("/connector/ops/claim", post(claim))
        .route("/connector/ops/results", post(results))
}

/// Wait to use for a correlated op of this kind.
pub fn wait_for(kind: &str) -> Duration {
    if kind == "file.fetch" {
        FILE_FETCH_WAIT
    } else {
        CORRELATED_WAIT
    }
}

pub(super) fn require_active(c: &ConnectorCtx) -> Result<(), ApiError> {
    if c.connector.status == "active" {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "connector_not_active",
            "This connector is not active.",
        ))
    }
}

/// `NOTIFY` on the caller's connection (delivered on commit inside a transaction).
pub(super) async fn notify(
    conn: &mut PgConnection,
    channel: Channel,
    payload: &Value,
) -> Result<(), ApiError> {
    aurora_db::bus::notify(conn, channel, payload)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

pub(super) fn invalid(e: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(e.to_string(), vec![])
}

// ------------------------------------------------------------------ enqueue API

/// An operation to queue for a connector.
#[derive(Debug, Clone)]
pub struct NewOp {
    pub kind: String,
    /// Needed when `integration_id` is `None` (e.g. `credentials.validate` for a new
    /// integration); ignored otherwise (the integration's connector is used).
    pub connector_id: Option<String>,
    pub integration_id: Option<Uuid>,
    pub presence_id: Option<Uuid>,
    pub payload: Value,
    /// A second enqueue with the same key returns the first op.
    pub dedupe_key: Option<String>,
}

/// Queue `op` in the caller's transaction and `NOTIFY aurora_ops` (delivered on commit).
/// Returns the op id (the existing one when `dedupe_key` already exists).
pub async fn enqueue(tx: &mut PgConnection, op: NewOp) -> Result<Uuid, ApiError> {
    enqueue_with(tx, op, None).await
}

async fn enqueue_with(
    tx: &mut PgConnection,
    op: NewOp,
    correlation_id: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let id = Uuid::now_v7();
    let ins = ops::insert(
        tx,
        &InsertOp {
            id,
            connector_id: op.connector_id.as_deref(),
            integration_id: op.integration_id,
            presence_id: op.presence_id,
            kind: &op.kind,
            payload: &op.payload,
            dedupe_key: op.dedupe_key.as_deref(),
            correlation_id,
        },
    )
    .await?;
    let (id, connector, inserted) = ins.ok_or_else(|| {
        ApiError::invalid_request("An operation needs a connector or an integration.", vec![])
    })?;
    if inserted {
        notify(&mut *tx, Channel::Ops, &json!({"connector": connector})).await?;
    }
    Ok(id)
}

/// Why a correlated call did not produce a result.
#[derive(Debug, Clone, PartialEq)]
pub enum CorrelatedError {
    /// No answer in time (or the bus is down): `connector_unavailable`.
    Unavailable,
    /// The connector answered with a definitive error (`failed` / `dropped`).
    Failed { code: String, message: String },
}

impl From<CorrelatedError> for ApiError {
    fn from(e: CorrelatedError) -> Self {
        match e {
            CorrelatedError::Unavailable => ApiError::unavailable(
                "connector_unavailable",
                "The connector did not answer in time.",
            ),
            CorrelatedError::Failed { code, message } => {
                ApiError::new(StatusCode::BAD_GATEWAY, "connector_error", message)
                    .with_details(json!({"connector_code": code}))
            }
        }
    }
}

/// Queue `op` with a `correlation_id` and wait up to `timeout` (see [`wait_for`]) for its
/// result on `aurora_op_result`. A still-`pending` op is dropped when the wait times out.
pub async fn call_correlated(
    state: &AppState,
    op: NewOp,
    timeout: Duration,
) -> Result<Value, CorrelatedError> {
    // Subscribe before inserting so the result notification cannot be missed.
    let mut rx = state
        .connector_bus
        .subscribe(&state.pool)
        .await
        .map_err(|_| CorrelatedError::Unavailable)?;
    let correlation = Uuid::now_v7();
    let id = async {
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        let id = enqueue_with(&mut tx, op, Some(correlation)).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok::<_, ApiError>(id)
    }
    .await
    .map_err(|e| {
        tracing::error!(error = %e.message(), "correlated op not queued");
        CorrelatedError::Unavailable
    })?;
    let deadline = Instant::now() + timeout;
    loop {
        match ops::get(&state.pool, id).await {
            Ok(Some(row)) => match row.status.as_str() {
                "done" => return Ok(row.result.unwrap_or(Value::Null)),
                "failed" | "dropped" => {
                    let message = row
                        .result
                        .as_ref()
                        .and_then(|r| r.pointer("/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("The operation failed.")
                        .to_owned();
                    return Err(CorrelatedError::Failed {
                        code: row.error_code.unwrap_or_else(|| row.status.clone()),
                        message,
                    });
                }
                _ => {}
            },
            Ok(None) => return Err(CorrelatedError::Unavailable),
            Err(e) => tracing::warn!(error = %e, "correlated op read failed"),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            if let Err(e) = ops::drop_pending(&state.pool, id).await {
                tracing::warn!(error = %e, "abandoned correlated op not dropped");
            }
            return Err(CorrelatedError::Unavailable);
        }
        // Wake on the matching notification, any resync, or the poll interval.
        let _ = tokio::time::timeout(left.min(POLL_EVERY), async {
            loop {
                match rx.recv().await {
                    Ok(Msg::OpResult { op_id }) if op_id == id => break,
                    Ok(Msg::OpResult { .. } | Msg::Ops { .. } | Msg::Inbox) => {}
                    Ok(Msg::Resync) | Err(_) => break,
                }
            }
        })
        .await;
    }
}

// ------------------------------------------------------------------------ claim

async fn claim(
    c: ConnectorCtx,
    State(s): State<AppState>,
    Json(req): Json<ClaimRequest>,
) -> Result<Json<ClaimResponse>, ApiError> {
    req.validate().map_err(invalid)?;
    require_active(&c)?;
    let wait = Duration::from_secs(u64::from(req.wait_secs)).min(CONNECTOR_OPS_WAIT);
    let mut rx = if wait.is_zero() {
        None
    } else {
        Some(s.connector_bus.subscribe(&s.pool).await?)
    };
    let deadline = Instant::now() + wait;
    loop {
        let rows = ops::claim(
            &s.pool,
            &c.connector.id,
            c.replica,
            i64::from(req.max),
            OP_LEASE.as_secs_f64(),
            OP_MAX_ATTEMPTS as i32,
        )
        .await?;
        if !rows.is_empty() {
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let kind =
                    OpKind::from_wire(&r.kind).map_err(|e| ApiError::internal(e.to_string()))?;
                out.push(Operation {
                    op_id: r.id,
                    kind,
                    integration_id: r.integration_id,
                    presence_id: r.presence_id,
                    attempt: u32::try_from(r.attempts).unwrap_or(1),
                    maybe_sent: r.was_leased,
                    payload: r.payload,
                });
            }
            return Ok(Json(ClaimResponse { ops: out }));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let Some(rx) = rx.as_mut().filter(|_| !left.is_zero()) else {
            return Ok(Json(ClaimResponse { ops: Vec::new() }));
        };
        // Wake for this connector's ops or a resync; poll too, for ops whose `not_before`
        // arrives without a notification.
        let mine = c.connector.id.as_str();
        let _ = tokio::time::timeout(left.min(Duration::from_secs(2)), async {
            loop {
                match rx.recv().await {
                    Ok(Msg::Ops { connector }) if connector == mine => break,
                    Ok(Msg::Ops { .. } | Msg::OpResult { .. } | Msg::Inbox) => {}
                    Ok(Msg::Resync) | Err(_) => break,
                }
            }
        })
        .await;
    }
}

// ---------------------------------------------------------------------- results

async fn results(
    c: ConnectorCtx,
    State(s): State<AppState>,
    Json(req): Json<ResultsRequest>,
) -> Result<Json<OkResponse>, ApiError> {
    req.validate().map_err(invalid)?;
    let mut first_err = None;
    for r in req.results {
        if let Err(e) = apply_result(&s, &c, r).await {
            first_err.get_or_insert(e);
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(Json(OkResponse { ok: true })),
    }
}

/// Backoff before retry number `attempts` (1-based) of an `unavailable` op.
fn backoff_secs(attempts: i32) -> f64 {
    let exp = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(30);
    (OP_BACKOFF_INITIAL.as_secs_f64() * f64::from(2u32.saturating_pow(exp)))
        .min(OP_BACKOFF_MAX.as_secs_f64())
}

/// What the core knows when a connector reports a dead credential.
pub struct CredentialDead<'a> {
    pub connector_id: &'a str,
    pub op_id: Uuid,
    pub kind: &'a str,
    pub integration_id: Option<Uuid>,
    pub presence_id: Option<Uuid>,
}

/// Notice hook for `credential_dead`; the presence/integration is already `disabled` when it
/// runs (same transaction). Operators are told through the channels domain.
pub async fn on_credential_dead(
    s: &AppState,
    tx: &mut PgConnection,
    ev: &CredentialDead<'_>,
) -> Result<(), ApiError> {
    tracing::warn!(
        connector = ev.connector_id, op_id = %ev.op_id, kind = ev.kind,
        integration_id = ?ev.integration_id, presence_id = ?ev.presence_id,
        "connector reported a dead credential"
    );
    crate::domains::channels::presences::on_credential_dead(
        tx,
        s,
        ev.integration_id,
        ev.presence_id,
        ev.connector_id,
    )
    .await
}

async fn apply_result(s: &AppState, c: &ConnectorCtx, r: OpResult) -> Result<(), ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let op = ops::lock(&mut tx, r.op_id, &c.connector.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Operation not found."))?;
    if op.claimed_by_replica != Some(c.replica) {
        return Err(ApiError::forbidden(
            "Only the claiming replica may report this operation.",
        ));
    }
    if !matches!(op.status.as_str(), "claimed" | "pending") {
        return Ok(()); // already finished: idempotent retry
    }
    let fail = |code: &str, message: &str| json!({"error": {"code": code, "message": message}});
    match (r.outcome, r.error) {
        (Outcome::Ok, _) => {
            let data = r.data.unwrap_or_else(|| json!({}));
            ops::finish(&mut *tx, op.id, "done", Some(&data), None).await?;
            if op.kind == "presence.provision"
                && let Some(p) = op.presence_id
            {
                crate::domains::channels::presences::on_provision_done(s, &mut tx, p, &data)
                    .await?;
            }
            notify(&mut tx, Channel::OpResult, &json!({"op_id": op.id})).await?;
        }
        (Outcome::Error, None) => return Err(invalid("error required when outcome is error")),
        (Outcome::Error, Some(e)) => {
            let code = e.code.as_str().to_owned();
            let mut terminal = Some("failed");
            match code.as_str() {
                "rate_limited" => {
                    let secs = e.retry_after_secs.unwrap_or(RATE_LIMIT_DEFAULT_SECS);
                    ops::reschedule(
                        &mut *tx,
                        op.id,
                        f64::from(secs.min(RATE_LIMIT_MAX_SECS)),
                        true,
                    )
                    .await?;
                    notify(
                        &mut tx,
                        Channel::Ops,
                        &json!({"connector": op.connector_id}),
                    )
                    .await?;
                    terminal = None;
                }
                "unavailable" => {
                    if op.attempts < OP_MAX_ATTEMPTS as i32 {
                        ops::reschedule(&mut *tx, op.id, backoff_secs(op.attempts), false).await?;
                        notify(
                            &mut tx,
                            Channel::Ops,
                            &json!({"connector": op.connector_id}),
                        )
                        .await?;
                        terminal = None;
                    }
                }
                "credential_dead" => {
                    if let Some(p) = op.presence_id {
                        ops::disable_presence(&mut *tx, p, "credential_dead").await?;
                    } else if let Some(i) = op.integration_id {
                        integrations::set_status(&mut *tx, i, "disabled", Some("credential_dead"))
                            .await?;
                    }
                    on_credential_dead(
                        s,
                        &mut tx,
                        &CredentialDead {
                            connector_id: &op.connector_id,
                            op_id: op.id,
                            kind: &op.kind,
                            integration_id: op.integration_id,
                            presence_id: op.presence_id,
                        },
                    )
                    .await?;
                }
                "forbidden" => {}
                "not_found" => terminal = Some("dropped"),
                // Core bug or capability mismatch (and any code this build does not know).
                other => tracing::error!(
                    op_id = %op.id, kind = %op.kind, code = other, message = %e.message,
                    "connector rejected an operation as invalid"
                ),
            }
            if let Some(status) = terminal {
                if op.kind == "presence.provision"
                    && let Some(p) = op.presence_id
                {
                    crate::domains::channels::presences::on_provision_failed(&mut tx, p, &code)
                        .await?;
                }
                ops::finish(
                    &mut *tx,
                    op.id,
                    status,
                    Some(&fail(&code, &e.message)),
                    Some(&code),
                )
                .await?;
                notify(&mut tx, Channel::OpResult, &json!({"op_id": op.id})).await?;
            }
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ----------------------------------------------------------------------- sweeper

/// Lease-lapse sweep (idempotent, every replica): lapsed `claimed` ops go back to `pending`
/// (their next claim reports `maybe_sent`) or `failed` when all attempts are used. Returns the
/// number of ops moved.
pub async fn sweep_lapsed(pool: &aurora_db::PgPool) -> Result<usize, ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let moved = ops::sweep_lapsed(&mut tx, OP_MAX_ATTEMPTS as i32).await?;
    let mut seen = std::collections::HashSet::new();
    for (id, connector, status) in &moved {
        if status == "failed" {
            notify(&mut tx, Channel::OpResult, &json!({"op_id": id})).await?;
        } else if seen.insert(connector.clone()) {
            notify(&mut tx, Channel::Ops, &json!({"connector": connector})).await?;
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(moved.len())
}

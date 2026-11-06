//! Inbox processor skeleton (docs/13 §4, docs/05 §Background jobs): claims `channel_inbox` rows
//! with an `INBOX_LEASE` on `aurora_inbox` and every 5 s, and dispatches them by `kind` to an
//! [`InboxHandler`]. BE-072/073 implement handlers and add them to [`handlers`]. Handlers must be
//! idempotent: a row is delivered to one worker at a time, but a lapsed lease redelivers it.
//!
//! No handler for a kind: the row is marked `done` with outcome `{"result":"unhandled"}`.
//! `Retry` backs off exponentially (like ops) and fails the row at `OP_MAX_ATTEMPTS`.

use super::bus::Msg;
use crate::app::AppState;
use aurora_core::ApiError;
use aurora_core::limits::{INBOX_LEASE, OP_BACKOFF_INITIAL, OP_BACKOFF_MAX, OP_MAX_ATTEMPTS};
use aurora_db::repo::channel_inbox::{self as inbox, InboxRow};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Periodic fallback poll.
const POLL_EVERY: Duration = Duration::from_secs(5);
const BATCH: i64 = 20;

#[derive(Debug)]
pub enum InboxError {
    /// Transient: retried with backoff.
    Retry(String),
    /// Permanent: the row becomes `failed`.
    Fatal(String),
}

#[async_trait::async_trait]
pub trait InboxHandler: Send + Sync {
    fn handles(&self, kind: &str) -> bool;
    /// Returns the row's `outcome`.
    async fn handle(&self, state: &AppState, row: &InboxRow) -> Result<Value, InboxError>;
}

/// The registered handlers (BE-072/073 add theirs here).
pub fn handlers() -> Vec<Arc<dyn InboxHandler>> {
    Vec::new()
}

fn backoff_secs(attempts: i32) -> f64 {
    let exp = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(30);
    (OP_BACKOFF_INITIAL.as_secs_f64() * f64::from(2u32.saturating_pow(exp)))
        .min(OP_BACKOFF_MAX.as_secs_f64())
}

/// Claim and process up to one batch; returns the number of rows claimed.
pub async fn process_batch(
    state: &AppState,
    handlers: &[Arc<dyn InboxHandler>],
) -> Result<usize, ApiError> {
    let rows = inbox::claim(&state.pool, BATCH, INBOX_LEASE.as_secs_f64()).await?;
    let n = rows.len();
    for row in rows {
        let pool = &state.pool;
        let max = OP_MAX_ATTEMPTS as i32;
        if row.attempts > max {
            inbox::fail(pool, row.id, row.attempts, "attempts exhausted").await?;
            continue;
        }
        let Some(h) = handlers.iter().find(|h| h.handles(&row.kind)) else {
            inbox::complete(pool, row.id, row.attempts, &json!({"result": "unhandled"})).await?;
            continue;
        };
        match h.handle(state, &row).await {
            Ok(outcome) => {
                inbox::complete(pool, row.id, row.attempts, &outcome).await?;
            }
            Err(InboxError::Retry(e)) if row.attempts < max => {
                tracing::warn!(event = %row.event_id, kind = %row.kind, error = %e, "inbox handler failed; retrying");
                inbox::retry(pool, row.id, row.attempts, &e, backoff_secs(row.attempts)).await?;
            }
            Err(InboxError::Retry(e) | InboxError::Fatal(e)) => {
                tracing::error!(event = %row.event_id, kind = %row.kind, error = %e, "inbox event failed");
                inbox::fail(pool, row.id, row.attempts, &e).await?;
            }
        }
    }
    Ok(n)
}

/// Start the processor on the current runtime (every replica; rows are claimed `SKIP LOCKED`).
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let handlers = handlers();
        let mut rx = loop {
            match state.connector_bus.subscribe(&state.pool).await {
                Ok(rx) => break rx,
                Err(e) => {
                    tracing::warn!(error = %e.message(), "inbox processor waiting for the bus");
                    tokio::time::sleep(POLL_EVERY).await;
                }
            }
        };
        loop {
            loop {
                match process_batch(&state, &handlers).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e.message(), "inbox processing failed");
                        break;
                    }
                }
            }
            let _ = tokio::time::timeout(POLL_EVERY, async {
                loop {
                    match rx.recv().await {
                        Ok(Msg::Inbox | Msg::Resync) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            })
            .await;
        }
    });
}

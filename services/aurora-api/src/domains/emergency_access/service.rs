//! Emergency read access service logic (FIX-236, C-1164): the expiry job's body.

use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::ApiError;
use aurora_db::repo::emergency_access as repo;
use serde_json::json;
use uuid::Uuid;

/// (C-1164) One expiry pass: every grant whose `expires_at` passed while still open gets
/// `ended_at = expires_at`, `end_reason = 'expired'`, and one security-critical
/// `workspace.emergency_access.expire` audit row each, in the same transaction. Use never waits
/// for this job: `authz::emergency_reader` evaluates `expires_at > now()` itself on every read.
pub async fn expire_due(s: &crate::app::AppState) -> Result<usize, ApiError> {
    let mut tx = audit::begin(s).await?;
    let rows = repo::expire_due(&mut tx).await?;
    for r in &rows {
        let ev = AuditEvent::new(
            AuditActor::system("emergency-access-expiry"),
            "workspace.emergency_access.expire",
            Outcome::Success,
        )
        .org(r.org_id)
        .workspace(r.workspace_id)
        .target("workspace_emergency_access", r.id)
        .metadata(json!({
            "opener": r.user_id,
            "expires_at": r.expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }));
        audit::insert_tx(&mut tx, &ev).await?;
    }
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(rows.len())
}

/// (C-1164) Called inside the membership transaction when an active org owner loses owner
/// status (role lowered, suspended, or removed): every grant they opened ends with
/// `end_reason = 'access_lost'` and one security-critical `workspace.emergency_access.end`
/// audit row per ended grant, in that same transaction — so the emergency window can never
/// outlive the ownership it was opened under.
pub async fn end_lost_in_tx(
    tx: &mut sqlx::PgConnection,
    actor: &AuditActor,
    org: Uuid,
    user: Uuid,
) -> Result<(), ApiError> {
    let rows = repo::end_lost_for_user(tx, org, user).await?;
    for r in &rows {
        let ev = AuditEvent::new(
            actor.clone(),
            "workspace.emergency_access.end",
            Outcome::Success,
        )
        .org(r.org_id)
        .workspace(r.workspace_id)
        .target("workspace_emergency_access", r.id)
        .metadata(json!({
            "end_reason": repo::END_ACCESS_LOST,
            "opener": r.user_id,
        }));
        audit::insert_tx(tx, &ev).await?;
    }
    Ok(())
}

/// The minute sweep (docs/05 §Background jobs): every API replica, idempotent.
pub fn spawn_emergency_expiry(state: crate::app::AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(crate::jobs::EMERGENCY_SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match expire_due(&state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(expired = n, "emergency access grants expired"),
                Err(e) => tracing::warn!(error = %e.message(), "emergency access expiry failed"),
            }
        }
    });
}

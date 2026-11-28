//! Emergency read access service logic (FIX-236, C-1164; FIX-262): the expiry job's body, the
//! end of a grant with its opener's ownership, and the record of what was read.

use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::ApiError;
use aurora_db::authz::ReadAuth;
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
    audit_lost(tx, actor, &rows).await
}

/// [`end_lost_in_tx`] for an account an instance admin disables (FIX-262): every open grant the
/// person holds, in every organization, so enabling the account again brings none back.
pub async fn end_lost_for_account_in_tx(
    tx: &mut sqlx::PgConnection,
    actor: &AuditActor,
    user: Uuid,
) -> Result<(), ApiError> {
    let rows = repo::end_lost_for_account(tx, user).await?;
    audit_lost(tx, actor, &rows).await
}

async fn audit_lost(
    tx: &mut sqlx::PgConnection,
    actor: &AuditActor,
    rows: &[repo::EmergencyRow],
) -> Result<(), ApiError> {
    for r in rows {
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

/// What an emergency reader read beyond conversations (FIX-262, D-502, C-1165): the
/// `workspace.emergency_read` row's `kind`.
#[derive(Debug, Clone, Copy)]
pub enum ReadKind {
    Memory,
    Schedule,
    Webhook,
    Skill,
    Agent,
    Activity,
    Timeline,
}

impl ReadKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Schedule => "schedule",
            Self::Webhook => "webhook",
            Self::Skill => "skill",
            Self::Agent => "agent",
            Self::Activity => "activity",
            Self::Timeline => "timeline",
        }
    }
}

/// What one `workspace.emergency_read` row is about.
#[derive(Debug, Clone, Copy)]
pub enum ReadOf {
    /// One object, by its id: the row's target.
    Object(Uuid),
    /// A list (FIX-285): whose it is — the agent for an agent's list, else the workspace. The
    /// row has no target; it names this in `metadata.listed`.
    List(Uuid),
}

/// The record of a workspace-content read: nothing for an own-access reader; for an emergency
/// reader one `workspace.emergency_read` row, **before the body is returned**. Once per (actor,
/// grant, kind, object or listed id) and hour, under the same advisory-lock dedup as
/// `conversation.emergency_view`. Not best-effort: any failure is the record failing, and the
/// read is refused with 503 `audit_unavailable`.
pub async fn record_read(
    state: &crate::app::AppState,
    actor: &AuditActor,
    read: &ReadAuth,
    kind: ReadKind,
    of: ReadOf,
) -> Result<(), ApiError> {
    let ReadAuth::Emergency(grant) = read else {
        return Ok(());
    };
    let refuse = |_| audit::unavailable();
    let kind = kind.as_str();
    let (target, listed) = match of {
        ReadOf::Object(id) => (Some(id), None),
        ReadOf::List(id) => (None, Some(id)),
    };
    let target_id = target.map(|t| t.to_string());
    let listed_id = listed.map(|t| t.to_string());
    let mut tx = state.pool.begin().await.map_err(refuse)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "emergency_read:{}:{}:{kind}:{}:{}",
            actor.id.unwrap_or(Uuid::nil()),
            grant.id(),
            target_id.as_deref().unwrap_or(""),
            listed_id.as_deref().unwrap_or("")
        ))
        .execute(&mut *tx)
        .await
        .map_err(refuse)?;
    let seen: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM audit_log WHERE workspace_id = $1 AND actor_id = $2
           AND action = 'workspace.emergency_read' AND metadata->>'emergency_access' = $3
           AND metadata->>'kind' = $4 AND target_id IS NOT DISTINCT FROM $5
           AND metadata->>'listed' IS NOT DISTINCT FROM $6
           AND created_at > now() - interval '1 hour')",
    )
    .bind(grant.workspace_id())
    .bind(actor.id)
    .bind(grant.id().to_string())
    .bind(kind)
    .bind(target_id.as_deref())
    .bind(listed_id.as_deref())
    .fetch_one(&mut *tx)
    .await
    .map_err(refuse)?;
    if !seen {
        let mut ev = AuditEvent::new(actor.clone(), "workspace.emergency_read", Outcome::Success)
            .org(grant.org_id())
            .workspace(grant.workspace_id())
            .metadata(json!({
                "emergency_access": grant.id(),
                "kind": kind,
                "target": target,
                "listed": listed,
            }));
        if let Some(t) = target {
            ev = ev.target(kind, t);
        }
        audit::insert_tx(&mut tx, &ev)
            .await
            .map_err(|_| audit::unavailable())?;
    }
    tx.commit().await.map_err(refuse)?;
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

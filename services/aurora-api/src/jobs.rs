//! Background jobs run by every API replica (docs/05 §Background jobs).
//! Idempotent jobs run everywhere; singleton jobs take an advisory lock per run.

use aurora_config::AttachmentsSection;
use aurora_db::PgPool;
use aurora_db::repo::{approvals, request_log};
use chrono::Utc;
use std::time::Duration;

/// `pending` past `deadline + APPROVAL_EXPIRY_GRACE` becomes `expired`.
pub const APPROVAL_EXPIRY_GRACE: Duration = Duration::from_secs(30);
/// Pending rows of gateways unseen this long become `abandoned`.
pub const GATEWAY_UNSEEN: Duration = Duration::from_secs(60);
pub const APPROVAL_SWEEP_EVERY: Duration = Duration::from_secs(10);
/// Lapsed connector-op lease sweep (BE-071).
pub const OP_LEASE_SWEEP_EVERY: Duration = Duration::from_secs(10);
pub const ATTACHMENT_SWEEP_EVERY: Duration = Duration::from_secs(3600);
/// Action-approval and peer-task expiry (docs/05 §Background jobs).
pub const ACTION_SWEEP_EVERY: Duration = Duration::from_secs(60);
/// Conversation retention sweep (phase 2, RFE-010, singleton).
pub const RETENTION_SWEEP_EVERY: Duration = Duration::from_secs(24 * 3600);
const RETENTION_JOB: &str = "conversation_retention";
const ATTACHMENT_SWEEP_JOB: &str = "attachment_sweep";
pub const PARTITION_EVERY: Duration = Duration::from_secs(24 * 3600);
pub const PARTITIONS_AHEAD: u32 = 2;
const PARTITION_JOB: &str = "request_log_partitions";

/// One approval sweep; returns `(expired, abandoned)` counts.
pub async fn sweep_approvals(pool: &PgPool) -> Result<(usize, usize), aurora_db::DbError> {
    let expired = approvals::expire_overdue(pool, APPROVAL_EXPIRY_GRACE.as_secs_f64()).await?;
    let abandoned = approvals::abandon_unseen(pool, GATEWAY_UNSEEN.as_secs_f64()).await?;
    Ok((expired.len(), abandoned.len()))
}

/// Delete `pending` attachments older than `ATTACH_PENDING_TTL` with their blobs, then blobs no
/// attachment row references (older than 1 h). Idempotent, safe on every replica. Returns the
/// number of rows plus orphan blobs removed.
pub async fn sweep_attachments(pool: &PgPool) -> Result<usize, aurora_core::ApiError> {
    let rows = crate::domains::attachments::sweep_pending(pool).await?;
    let orphans = crate::domains::attachments::sweep_orphan_blobs(pool).await?;
    Ok(rows + orphans)
}

/// Delete conversations past their org's retention window (singleton). `Ok(None)` when another
/// replica holds the lock, otherwise the number of conversations deleted.
pub async fn sweep_conversation_retention(
    pool: &PgPool,
    ceiling: Option<u32>,
) -> Result<Option<usize>, aurora_core::ApiError> {
    let ran = aurora_db::jobs::try_singleton(pool, RETENTION_JOB, || {
        crate::domains::conversations::lifecycle::sweep_retention(pool, ceiling)
    })
    .await?;
    ran.transpose()
}

/// Daily attachment sweep (phase 2, RFE-015, singleton): purge bytes past the org windows, remove
/// orphan blobs, reconcile the usage counters, raise quota notices. `Ok(None)` when another
/// replica holds the lock.
pub async fn sweep_attachment_storage(
    pool: &PgPool,
    cfg: &AttachmentsSection,
    mail: Option<&crate::domains::attachments::sweep::QuotaMail>,
) -> Result<Option<crate::domains::attachments::sweep::SweepReport>, aurora_core::ApiError> {
    let ran = aurora_db::jobs::try_singleton(pool, ATTACHMENT_SWEEP_JOB, || {
        crate::domains::attachments::sweep::run_with(pool, cfg, mail)
    })
    .await?;
    ran.transpose()
}

/// Create upcoming partitions and drop expired ones (singleton). `Ok(false)` when
/// another replica holds the lock.
pub async fn maintain_request_log(
    pool: &PgPool,
    retention_days: u32,
) -> Result<bool, aurora_db::DbError> {
    let ran = aurora_db::jobs::try_singleton(pool, PARTITION_JOB, || async {
        let now = Utc::now();
        request_log::ensure_partitions(pool, now, PARTITIONS_AHEAD).await?;
        request_log::drop_expired(pool, now, retention_days).await?;
        Ok::<_, aurora_db::DbError>(())
    })
    .await?;
    match ran {
        None => Ok(false),
        Some(r) => r.map(|()| true),
    }
}

/// Start all jobs on the current runtime. Tasks end with the runtime.
pub fn spawn(
    pool: PgPool,
    retention_days: u32,
    conversation_days_max: Option<u32>,
    attachments: AttachmentsSection,
    quota_mail: Option<crate::domains::attachments::sweep::QuotaMail>,
) {
    let rp = pool.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RETENTION_SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match sweep_conversation_retention(&rp, conversation_days_max).await {
                Ok(Some(n)) if n > 0 => tracing::info!(deleted = n, "conversation retention sweep"),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e.message(), "conversation retention sweep failed")
                }
            }
            // (RFE-011a) Approval e-mail claims are pruned with the daily retention pass.
            if let Err(e) = aurora_db::repo::approval_notify::prune_notifications(
                &rp,
                crate::domains::approvals::notify::CLAIM_RETENTION_DAYS,
            )
            .await
            {
                tracing::warn!(error = %e, "approval notification pruning failed");
            }
            // The attachment sweep shares this daily tick (docs/05 §Background jobs).
            match sweep_attachment_storage(&rp, &attachments, quota_mail.as_ref()).await {
                Ok(Some(r)) if r != Default::default() => {
                    tracing::info!(report = ?r, "attachment storage sweep")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "attachment storage sweep failed"),
            }
        }
    });
    let ap = pool.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(ACTION_SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match crate::domains::action_approvals::service::sweep(&ap).await {
                Ok(n) if n > 0 => tracing::info!(expired = n, "action-approval sweep"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "action-approval sweep failed"),
            }
            match crate::domains::peer_tasks::service::sweep(&ap).await {
                Ok(n) if n > 0 => tracing::info!(closed = n, "peer-task sweep"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "peer-task sweep failed"),
            }
        }
    });
    let cp = pool.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(OP_LEASE_SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match crate::domains::connectors::ops::sweep_lapsed(&cp).await {
                Ok(n) if n > 0 => tracing::info!(moved = n, "connector op lease sweep"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "connector op lease sweep failed"),
            }
        }
    });
    let p = pool.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(APPROVAL_SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match sweep_approvals(&p).await {
                Ok((e, a)) if e + a > 0 => {
                    tracing::info!(expired = e, abandoned = a, "approval sweep")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "approval sweep failed"),
            }
        }
    });
    let p = pool.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(ATTACHMENT_SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match sweep_attachments(&p).await {
                Ok(n) if n > 0 => tracing::info!(removed = n, "attachment pending sweep"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "attachment pending sweep failed"),
            }
        }
    });
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PARTITION_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = maintain_request_log(&pool, retention_days).await {
                tracing::warn!(error = %e, "request-log maintenance failed");
            }
        }
    });
}

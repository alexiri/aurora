//! Background jobs run by every API replica (docs/05 §Background jobs).
//! Idempotent jobs run everywhere; singleton jobs take an advisory lock per run.

use aurora_config::AttachmentsSection;
use aurora_core::limits::{
    CHANNEL_LEDGER_RETENTION, RETENTION_PRUNE_BATCH, SESSION_PRUNE_GRACE,
    WEBHOOK_DELIVERY_MAX_PER_WEBHOOK, WEBHOOK_DELIVERY_RETENTION,
};
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
/// SCIM token expiry notices (P4-GRP-2, singleton; fires once per token per threshold via the
/// audit-log lookup in [`aurora_db::repo::scim::tokens_expiring_within`]).
pub const SCIM_EXPIRY_SWEEP_EVERY: Duration = Duration::from_secs(24 * 3600);
const SCIM_EXPIRY_JOB: &str = "scim_token_expiry";
const RETENTION_PRUNE_JOB: &str = "retention_prune";
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

/// What one daily retention-pruning pass removed (FIX-111).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PruneReport {
    pub channel_inbox: u64,
    pub channel_ops: u64,
    pub channel_cards: u64,
    pub link_requests: u64,
    pub sessions: u64,
    pub cli_logins: u64,
    pub context_credentials: u64,
    pub webhook_deliveries: u64,
}

/// Run one batched delete until a batch comes back short (each statement removes at most
/// `RETENTION_PRUNE_BATCH` rows, so no lock is held for long).
async fn drain<F, Fut>(mut one: F) -> Result<u64, aurora_db::DbError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<u64, aurora_db::DbError>>,
{
    let batch = u64::try_from(RETENTION_PRUNE_BATCH).unwrap_or(u64::MAX);
    let mut total = 0;
    loop {
        let n = one().await?;
        total += n;
        if n < batch {
            return Ok(total);
        }
    }
}

/// The body of the daily retention pass (docs/05 §Background jobs "Retention pruning"):
/// terminal channel ledger rows older than `CHANNEL_LEDGER_RETENTION` (a pending deprovision
/// op survives), expired link requests, sessions and context credentials past
/// `SESSION_PRUNE_GRACE`, webhook deliveries past `WEBHOOK_DELIVERY_RETENTION` or over the
/// per-webhook cap. Used action tokens are pruned where they are consumed.
pub async fn run_retention_prune(pool: &PgPool) -> Result<PruneReport, aurora_db::DbError> {
    use aurora_db::repo::retention as r;
    let ledger = CHANNEL_LEDGER_RETENTION.as_secs_f64();
    let grace = SESSION_PRUNE_GRACE.as_secs_f64();
    let hooks = WEBHOOK_DELIVERY_RETENTION.as_secs_f64();
    let b = RETENTION_PRUNE_BATCH;
    Ok(PruneReport {
        channel_inbox: drain(|| r::prune_channel_inbox(pool, ledger, b)).await?,
        channel_ops: drain(|| r::prune_channel_ops(pool, ledger, b)).await?,
        channel_cards: drain(|| r::prune_channel_cards(pool, ledger, b)).await?,
        link_requests: drain(|| r::prune_link_requests(pool, ledger, b)).await?,
        sessions: drain(|| r::prune_sessions(pool, grace, b)).await?,
        cli_logins: drain(|| r::prune_cli_logins(pool, grace, b)).await?,
        context_credentials: drain(|| r::prune_context_credentials(pool, grace, b)).await?,
        webhook_deliveries: drain(|| r::prune_webhook_deliveries_by_age(pool, hooks, b)).await?
            + drain(|| {
                r::prune_webhook_deliveries_over_cap(pool, WEBHOOK_DELIVERY_MAX_PER_WEBHOOK, b)
            })
            .await?,
    })
}

/// The daily retention pass as a singleton: `Ok(None)` when another replica holds the lock.
pub async fn sweep_retention_prune(
    pool: &PgPool,
) -> Result<Option<PruneReport>, aurora_db::DbError> {
    aurora_db::jobs::try_singleton(pool, RETENTION_PRUNE_JOB, || run_retention_prune(pool))
        .await?
        .transpose()
}

/// (FIX-111) Approved reach grants past `expires_at` become `expired` (every 60 s,
/// idempotent). Returns how many expired.
pub async fn expire_reach_grants(pool: &PgPool) -> Result<u64, aurora_db::DbError> {
    aurora_db::repo::reach_grants::expire_due(pool).await
}

/// (P4-GRP-2, FIX-081) One SCIM token expiry-notice pass: for every live token expiring within
/// 14 days (and again within 1 day) write a non-critical `scim_token.expiry_notice` audit row
/// (the in-app notice; once per token per threshold — the repo query checks the audit log) and
/// e-mail the organization's owners and admins through the channel quota notices use (`mail`:
/// `None` = e-mail off). Singleton: `Ok(None)` when another replica holds the lock. Returns the
/// number of notices written.
pub async fn sweep_scim_token_expiry(
    pool: &PgPool,
    mail: Option<&crate::domains::attachments::sweep::QuotaMail>,
) -> Result<Option<usize>, aurora_core::ApiError> {
    let ran = aurora_db::jobs::try_singleton(pool, SCIM_EXPIRY_JOB, || {
        run_scim_expiry_notices(pool, mail)
    })
    .await?
    .transpose()
    .map_err(aurora_core::ApiError::from)?;
    Ok(ran)
}

/// E-mail the owners/admins of `org` that the SCIM token `name` expires (platform voice: the
/// token name, the date and a dashboard link; never token material).
async fn mail_scim_expiry(
    pool: &PgPool,
    m: &crate::domains::attachments::sweep::QuotaMail,
    org: uuid::Uuid,
    token_name: &str,
    expires_at: chrono::DateTime<Utc>,
    days: i64,
) {
    use crate::domains::auth::mail::Mail;
    let Ok((org_name, slug, owners)) =
        aurora_db::repo::approval_notify::org_owners(pool, org).await
    else {
        return;
    };
    let when = if days <= 1 {
        "within a day"
    } else {
        "within 14 days"
    };
    let subject = format!(
        "{}: a directory-sync token expires {when}",
        aurora_core::sanitize::one_line(&org_name, 80)
    );
    let intro = format!(
        "The provisioning token \"{}\" of {} expires on {}. Create a new token under \
         Organization settings → Directory sync before then, or your identity provider's \
         provisioning stops.",
        aurora_core::sanitize::one_line(token_name, 80),
        aurora_core::sanitize::one_line(&org_name, 80),
        expires_at.format("%Y-%m-%d %H:%M UTC"),
    );
    let link = format!(
        "{}/o/{slug}/settings/directory",
        m.external_url.trim_end_matches('/')
    );
    for (_, email) in owners {
        if let Err(e) = m
            .mailer
            .send(Mail::new(&email, &subject, &intro, &link))
            .await
        {
            tracing::error!(error = %e, "scim token expiry e-mail failed");
        }
    }
}

/// The body of the SCIM token expiry-notice pass (the singleton's closure; returns the
/// number of notices written).
async fn run_scim_expiry_notices(
    pool: &PgPool,
    mail: Option<&crate::domains::attachments::sweep::QuotaMail>,
) -> Result<usize, aurora_db::DbError> {
    let now = Utc::now();
    let mut notices = 0;
    for days in [14i64, 1] {
        let due = aurora_db::repo::scim::tokens_expiring_within(pool, days, now).await?;
        for (id, org, name, expires_at) in due {
            let ev = crate::audit::AuditEvent::new(
                crate::audit::AuditActor {
                    kind: "service",
                    id: None,
                    label: "scim".to_owned(),
                    source: "api",
                    ip: None,
                },
                "scim_token.expiry_notice",
                crate::audit::Outcome::Success,
            )
            .org(org)
            .target("scim_token", id)
            .metadata(serde_json::json!({
                "name": name,
                "expires_at": expires_at,
                "threshold_days": days,
            }));
            // Non-critical: a lost notice row is re-raised on the next daily pass.
            if let Err(e) = crate::audit::insert_tx(&mut *pool.acquire().await?, &ev).await {
                tracing::warn!(error = %e, "scim: expiry notice failed");
            } else {
                notices += 1;
                // The e-mail follows the audit row, so it is sent once per token and
                // threshold (the row is what the next pass looks for).
                if let Some(m) = mail {
                    mail_scim_expiry(pool, m, org, &name, expires_at, days).await;
                }
            }
        }
    }
    Ok(notices)
}

/// `COUNTER_PRUNE_INTERVAL`.
pub const COUNTER_PRUNE_INTERVAL: Duration = Duration::from_secs(60);
/// `COUNTER_PRUNE_BATCH`.
pub const COUNTER_PRUNE_BATCH: i64 = 5_000;

/// Preview housekeeping (docs/05 §Background jobs): clear the stale wake/start stamps of
/// `sandboxes` past their windows, prune expired preview sessions/grants and stale relay
/// replicas, and set the `aurora_preview_shares` gauge (active shares per org). Idempotent,
/// safe on every replica (P4-PRV-2).
pub async fn preview_housekeeping(pool: &PgPool) -> Result<u64, aurora_db::DbError> {
    let cleared = {
        let mut conn = pool.acquire().await.map_err(aurora_db::DbError::from)?;
        let cleared =
            aurora_db::repo::preview_shares::clear_stale_sandbox_stamps(&mut conn).await?;
        drop(conn);
        let pruned = aurora_db::repo::preview_relay::prune_preview_expiries(pool).await?;
        let replicas = aurora_db::repo::preview_relay::prune_stale_relay_replicas(pool).await?;
        tracing::debug!(cleared, pruned, replicas, "preview housekeeping");
        cleared + pruned + replicas
    };
    // Labelled by `state` only (FIX-090): a tenant's slug is not a metric label.
    let counts: Vec<(String, i64)> =
        sqlx::query_as("SELECT state, count(*) FROM preview_shares GROUP BY state")
            .fetch_all(pool)
            .await?;
    for state in ["active", "paused"] {
        let n = counts
            .iter()
            .find(|(s, _)| s == state)
            .map_or(0, |(_, n)| *n);
        metrics::gauge!("aurora_preview_shares", "state" => state).set(n as f64);
        tracing::trace!(state, shares = n, "preview shares gauge");
    }
    Ok(cleared)
}

/// Run the preview-housekeeping arm on this replica every [`PREVIEW_HOUSEKEEPING_EVERY`].
pub const PREVIEW_HOUSEKEEPING_EVERY: Duration = Duration::from_secs(60);

pub fn spawn_preview_housekeeping(pool: PgPool) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PREVIEW_HOUSEKEEPING_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match preview_housekeeping(&pool).await {
                Ok(n) if n > 0 => tracing::debug!(cleared = n, "preview housekeeping"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "preview housekeeping failed"),
            }
        }
    });
}

/// (phase 4, P4-CNT) One `counters_prune` pass: delete dead rows (`expires_at < now()`) of
/// `rate_limit_windows` and `rate_limit_buckets`, `FOR UPDATE SKIP LOCKED`, in batches. Safe on
/// every replica at once. Returns the number of rows deleted.
pub async fn prune_counters(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut total = 0;
    loop {
        let n = aurora_db::counters::prune(pool, COUNTER_PRUNE_BATCH).await?;
        total += n;
        // A full batch in either table means more may be waiting.
        if n < u64::try_from(COUNTER_PRUNE_BATCH).unwrap_or(u64::MAX) {
            break;
        }
    }
    if total > 0 {
        metrics::counter!("aurora_counters_pruned_total").increment(total);
    }
    Ok(total)
}

/// Run the `counters_prune` arm on this replica every [`COUNTER_PRUNE_INTERVAL`].
pub fn spawn_counters_prune(pool: PgPool) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(COUNTER_PRUNE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = prune_counters(&pool).await {
                tracing::warn!(error = %e, "counters prune failed");
            }
        }
    });
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
            // (FIX-111) The documented retention-pruning pass shares the daily tick too.
            match sweep_retention_prune(&rp).await {
                Ok(Some(r)) if r != PruneReport::default() => {
                    tracing::info!(report = ?r, "retention pruning")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "retention pruning failed"),
            }
            // (P4-GRP-2) SCIM token expiry notices share the daily tick as well.
            match sweep_scim_token_expiry(&rp, quota_mail.as_ref()).await {
                Ok(Some(n)) if n > 0 => tracing::info!(notices = n, "scim token expiry notices"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "scim token expiry sweep failed"),
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
            match expire_reach_grants(&ap).await {
                Ok(n) if n > 0 => tracing::info!(expired = n, "reach-grant expiry"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "reach-grant expiry failed"),
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

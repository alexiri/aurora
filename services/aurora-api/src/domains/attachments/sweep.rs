//! Daily attachment sweep (phase 2, RFE-015, docs/05 §Background jobs, singleton): purge bytes
//! past each org's retention window (the row stays, `purged_at`), remove orphan blobs, reconcile
//! the `org_storage_usage` counters, and raise the 80 % / 100 % quota notices.

use super::{blob, quota, service};
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_config::AttachmentsSection;
use aurora_core::ApiError;
use aurora_core::limits::{
    RETENTION_MAX_DAYS, RETENTION_MIN_DAYS, RETENTION_SWEEP_BATCH, RETENTION_SWEEP_PER_ORG,
};
use aurora_db::repo::attachments::{self, Purged};
use aurora_db::{DbError, PgPool};
use serde_json::json;

/// What one sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub purged: usize,
    pub purged_bytes: i64,
    pub orphans: usize,
    /// Orgs whose counter had drifted and was corrected.
    pub reconciled: usize,
    /// Notices raised (`warning`, `full`, `clear` rows written).
    pub notices: usize,
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::from(DbError::from(e))
}

/// Delete the blobs of purged rows after the transaction committed. A failure leaves an
/// unreferenced blob (the key was cleared) for the orphan sweep.
pub(super) async fn remove_blobs(pool: &PgPool, purged: &[Purged]) {
    for p in purged {
        if p.blob_key.is_empty() {
            continue;
        }
        match blob::for_backend(&p.blob_backend, pool) {
            Some(store) => {
                if let Err(e) = store.delete(&p.blob_key).await {
                    tracing::warn!(error = %e.message(), "purged blob not deleted; left to the orphan sweep");
                }
            }
            None => {
                tracing::warn!(backend = %p.blob_backend, "unknown blob backend; bytes left in place")
            }
        }
    }
}

/// Step 1: purge by retention, `RETENTION_SWEEP_BATCH` rows per transaction (one `attachment.purge`
/// audit row per batch, actor `system`, `reason = "retention"`).
async fn purge_by_retention(
    pool: &PgPool,
    cfg: &AttachmentsSection,
    r: &mut SweepReport,
) -> Result<(), ApiError> {
    let orgs = attachments::orgs_with_retention(pool).await?;
    for (org, days) in orgs {
        let Some(days) = quota::effective_days(cfg, Some(days)) else {
            continue;
        };
        if !(RETENTION_MIN_DAYS..=RETENTION_MAX_DAYS).contains(&days) {
            continue;
        }
        let mut left = RETENTION_SWEEP_PER_ORG;
        while left > 0 {
            let mut tx = pool.begin().await.map_err(db)?;
            let batch =
                attachments::purge_expired(&mut tx, org, days, left.min(RETENTION_SWEEP_BATCH))
                    .await?;
            if batch.is_empty() {
                tx.commit().await.map_err(db)?;
                break;
            }
            let bytes: i64 = batch.iter().map(|p| p.size_bytes).sum();
            let ev = AuditEvent::new(
                AuditActor::system("retention"),
                "attachment.purge",
                Outcome::Success,
            )
            .org(org)
            .target("organization", org)
            .metadata(json!({
                "reason": "retention", "retention_days": days,
                "attachments": batch.len(), "bytes": bytes,
            }));
            audit::insert_tx(&mut tx, &ev).await?;
            tx.commit().await.map_err(db)?;
            remove_blobs(pool, &batch).await;
            r.purged += batch.len();
            r.purged_bytes += bytes;
            left -= i64::try_from(batch.len()).unwrap_or(left);
        }
    }
    Ok(())
}

/// Step 3: recompute every org's counter from the rows, under the counter's lock.
async fn reconcile(pool: &PgPool, r: &mut SweepReport) -> Result<(), ApiError> {
    for org in attachments::orgs_with_storage(pool).await? {
        let mut tx = pool.begin().await.map_err(db)?;
        let fixed = attachments::reconcile_org(&mut tx, org).await?;
        tx.commit().await.map_err(db)?;
        if let Some((was, now)) = fixed {
            tracing::warn!(%org, was, now, "attachment usage counter drifted; corrected");
            r.reconciled += 1;
        }
    }
    Ok(())
}

/// Notice level for `used` of `quota`: `full` at 100 %, `warning` from 80 %, else `clear`.
pub fn notice_level(used: i64, quota: Option<i64>) -> &'static str {
    match quota {
        Some(q) if q > 0 && used >= q => "full",
        Some(q) if q > 0 && i128::from(used) * 10 >= i128::from(q) * 8 => "warning",
        _ => "clear",
    }
}

/// Step 4: one durable `attachment.quota_notice` audit row each time an org's level changes
/// (`warning` from 80 %, `full` at 100 %, `clear` below 80 %), so each notice is raised once
/// per crossing. The dashboard shows the live level
/// (`GET /orgs/{org}/storage`), the audit row is the record.
async fn notices(
    pool: &PgPool,
    cfg: &AttachmentsSection,
    r: &mut SweepReport,
) -> Result<(), ApiError> {
    for org in attachments::orgs_with_storage(pool).await? {
        let (mib, _) = attachments::org_limits(pool, org).await?;
        let quota = quota::effective_quota_bytes(cfg, mib);
        let used = attachments::usage(pool, org).await?;
        let level = notice_level(used, quota);
        let last = attachments::last_quota_notice(pool, org)
            .await?
            .unwrap_or_else(|| "clear".to_owned());
        if level == last {
            continue;
        }
        let ev = AuditEvent::new(
            AuditActor::system("attachment-sweep"),
            "attachment.quota_notice",
            Outcome::Success,
        )
        .org(org)
        .target("organization", org)
        .metadata(json!({
            "level": level, "used_bytes": used, "quota_bytes": quota,
        }));
        let mut tx = pool.begin().await.map_err(db)?;
        audit::insert_tx(&mut tx, &ev).await?;
        tx.commit().await.map_err(db)?;
        r.notices += 1;
    }
    Ok(())
}

/// Refresh the storage gauges: `aurora_blob_bytes`, `aurora_org_attachment_bytes{org}` (top 20
/// orgs, the rest summed under `other`).
async fn gauges(pool: &PgPool) -> Result<(), ApiError> {
    let (blob_bytes, _) = attachments::blob_totals(pool).await?;
    metrics::gauge!("aurora_blob_bytes").set(blob_bytes as f64);
    let top = attachments::top_orgs(pool, 20).await?;
    let (total, _) = attachments::usage_totals(pool).await?;
    let mut shown = 0i64;
    for o in &top {
        shown += o.attachment_bytes;
        metrics::gauge!("aurora_org_attachment_bytes", "org" => o.slug.clone())
            .set(o.attachment_bytes as f64);
    }
    metrics::gauge!("aurora_org_attachment_bytes", "org" => "other")
        .set((total - shown).max(0) as f64);
    Ok(())
}

/// One full sweep (no locking; [`crate::jobs`] wraps it in the singleton lock).
pub async fn run(pool: &PgPool, cfg: &AttachmentsSection) -> Result<SweepReport, ApiError> {
    let mut r = SweepReport::default();
    purge_by_retention(pool, cfg, &mut r).await?;
    r.orphans = service::sweep_orphan_blobs(pool).await?;
    reconcile(pool, &mut r).await?;
    notices(pool, cfg, &mut r).await?;
    if let Err(e) = gauges(pool).await {
        tracing::warn!(error = %e.message(), "storage gauges not refreshed");
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(notice_level(0, Some(100)), "clear");
        assert_eq!(notice_level(79, Some(100)), "clear");
        assert_eq!(notice_level(80, Some(100)), "warning");
        assert_eq!(notice_level(99, Some(100)), "warning");
        assert_eq!(notice_level(100, Some(100)), "full");
        assert_eq!(notice_level(1_000, None), "clear");
    }
}

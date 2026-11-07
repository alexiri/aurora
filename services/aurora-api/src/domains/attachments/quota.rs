//! Per-org attachment storage quota (phase 2, RFE-015, D-54, SEC-48). Every path that stores
//! attachment bytes (dashboard/API upload, connector `file.fetch`, runner `send_file`) calls
//! [`reserve`] inside the transaction that inserts the row: it locks the org's counter
//! (`org_storage_usage`), so concurrent writers of one org serialize and the quota cannot be
//! overshot. The authority is always the server; clients' checks are advisory.

use crate::app::AppState;
use aurora_config::AttachmentsSection;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::attachments;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

pub const MIB: i64 = 1024 * 1024;
/// Code of the 422 / tool error / `failure_reason` vocabulary (`failure_reason` is
/// [`FAILURE_REASON`]).
pub const CODE: &str = "attachment_quota_exceeded";
pub const FAILURE_REASON: &str = "quota_exceeded";

/// Quota in bytes of an org that set `org_mib` (`None` = it set none): the org's value or the
/// instance default, capped by the instance ceiling; `None` = unlimited (`0` means unlimited in
/// the instance settings and in an org's own setting).
pub fn effective_quota_bytes(a: &AttachmentsSection, org_mib: Option<i64>) -> Option<i64> {
    let mut mib = org_mib.unwrap_or_else(|| i64::try_from(a.default_org_quota_mib).unwrap_or(0));
    let ceiling = i64::try_from(a.org_quota_mib_max).unwrap_or(0);
    if ceiling > 0 {
        mib = if mib <= 0 { ceiling } else { mib.min(ceiling) };
    }
    (mib > 0).then(|| mib.saturating_mul(MIB))
}

/// Retention window in days of an org that set `org_days`: the smaller of it and the instance
/// maximum; `None` = keep forever.
pub fn effective_days(a: &AttachmentsSection, org_days: Option<i64>) -> Option<i64> {
    let max = a.days_max.map(i64::from);
    match (org_days, max) {
        (Some(d), Some(m)) => Some(d.min(m)),
        (d, None) => d,
        (None, Some(_)) => None,
    }
}

/// The org's effective quota in bytes (`None` = unlimited).
pub async fn quota_of(
    state: &AppState,
    exec: impl sqlx::PgExecutor<'_>,
    org: Uuid,
) -> Result<Option<i64>, ApiError> {
    let (mib, _) = attachments::org_limits(exec, org).await?;
    Ok(effective_quota_bytes(&state.config.attachments, mib))
}

pub fn exceeded(used: i64, quota: i64) -> ApiError {
    metrics::counter!("aurora_attachment_quota_rejections_total").increment(1);
    ApiError::unprocessable(CODE, "Your organization's attachment storage is full.")
        .with_details(json!({ "used_mib": used / MIB, "quota_mib": quota / MIB }))
}

/// Authoritative check, in the inserting transaction: lock the org's counter and refuse when
/// `used + size` would exceed the quota. The reservation is the row insert that follows in the
/// same transaction (the counter is maintained by triggers on `attachments`).
pub async fn reserve(
    state: &AppState,
    conn: &mut PgConnection,
    workspace: Uuid,
    size: i64,
) -> Result<(), ApiError> {
    let org = org_of(&mut *conn, workspace).await?;
    let Some(quota) = quota_of(state, &mut *conn, org).await? else {
        return Ok(());
    };
    let used = attachments::lock_usage(&mut *conn, org).await?;
    if used.saturating_add(size) > quota {
        return Err(exceeded(used, quota));
    }
    Ok(())
}

/// Cheap unlocked check before the bytes are written anywhere. Not authoritative: [`reserve`]
/// decides.
pub async fn precheck(state: &AppState, workspace: Uuid, size: i64) -> Result<(), ApiError> {
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let org = org_of(&mut conn, workspace).await?;
    if let Some(quota) = quota_of(state, &mut *conn, org).await? {
        let used = attachments::usage(&mut *conn, org).await?;
        if used.saturating_add(size) > quota {
            return Err(exceeded(used, quota));
        }
    }
    Ok(())
}

/// Would `size` more bytes exceed the org's quota right now? (connector inbox pre-flight and
/// failure classification; not authoritative.)
pub async fn would_exceed(state: &AppState, workspace: Uuid, size: i64) -> Result<bool, ApiError> {
    Ok(precheck(state, workspace, size)
        .await
        .is_err_and(|e| e.code() == CODE))
}

async fn org_of(conn: &mut PgConnection, workspace: Uuid) -> Result<Uuid, ApiError> {
    attachments::org_of_workspace(&mut *conn, workspace)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))
}

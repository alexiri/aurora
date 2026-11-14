//! Storage visibility (phase 2, RFE-015): `GET /orgs/{org}/storage` and `GET /admin/storage`.
//!
//! FIX-126: the figures come from a **snapshot** (`storage_snapshots`, shared by every replica),
//! recomputed by the daily attachment sweep, by the next read once it is older than
//! `STORAGE_SNAPSHOT_MAX_AGE`, and on demand by the refresh routes — one computation at a time
//! per scope (advisory lock). A page view is therefore cheap and sits in the `general` bucket.
//! An organization's `used_bytes` and `notice` are the one exception: they come from the live
//! `org_storage_usage` counter, so a quota notice is exact even with a cached snapshot.

use super::quota;
use super::sweep::notice_level;
use crate::app::AppState;
use crate::http::auth::Caller;
use aurora_config::AttachmentsSection;
use aurora_core::ApiError;
use aurora_core::limits::STORAGE_SNAPSHOT_MAX_AGE;
use aurora_db::PgPool;
use aurora_db::repo::attachments;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::future::Future;
use utoipa::ToSchema;
use uuid::Uuid;

/// Attachment bytes of one workspace in one direction.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct WorkspaceStorage {
    pub workspace_id: Uuid,
    pub workspace_slug: String,
    pub workspace_name: String,
    /// `in` (people -> agent) | `out` (agent -> people)
    pub direction: String,
    pub files: i64,
    pub bytes: i64,
}

/// `GET /orgs/{org}/storage` (API_CONTRACTS C-171).
#[derive(Serialize, ToSchema)]
pub struct OrgStorage {
    pub org_id: Uuid,
    /// Counter of non-purged attachment bytes (live, never cached).
    pub used_bytes: i64,
    /// Quota in force; null = unlimited.
    pub quota_bytes: Option<i64>,
    /// `clear` (< 80 %) | `warning` (≥ 80 %) | `full` (≥ 100 %); from the live counter.
    pub notice: String,
    /// Files that hold bytes.
    pub files: i64,
    /// Files whose bytes were purged (the rows remain).
    pub purged_files: i64,
    /// The org's attachment retention window; null = keep forever.
    pub attachments_days: Option<i64>,
    /// The window in force (after the instance maximum).
    pub attachments_days_effective: Option<i64>,
    pub by_workspace: Vec<WorkspaceStorage>,
    /// When the file counts and the breakdown were computed (FIX-126).
    pub computed_at: DateTime<Utc>,
}

/// The cached part of an organization's page.
#[derive(Serialize, Deserialize)]
struct OrgFigures {
    files: i64,
    purged_files: i64,
    by_workspace: Vec<WorkspaceStorage>,
}

/// Attachment bytes of one org in the instance breakdown.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AdminOrgStorage {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub attachment_bytes: i64,
    pub files: i64,
    /// Quota in force; null = unlimited.
    pub quota_bytes: Option<i64>,
}

/// On-disk size of one big table (with indexes and TOAST).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct TableSize {
    pub table: String,
    pub bytes: i64,
}

/// `GET /admin/storage` (API_CONTRACTS C-172).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AdminStorage {
    /// Bytes held by the blob backend (`blobs`).
    pub blob_bytes: i64,
    pub blob_count: i64,
    /// Sum of the org counters.
    pub attachment_bytes: i64,
    pub attachment_files: i64,
    pub purged_files: i64,
    /// Top 50 orgs by attachment bytes.
    pub orgs: Vec<AdminOrgStorage>,
    /// `blobs`, `attachments`, `transcript_events`, `request_log`, `audit_log`.
    pub tables: Vec<TableSize>,
    /// When these figures were computed (FIX-126); the page offers a refresh.
    pub computed_at: DateTime<Utc>,
}

fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH
}

const TOP_ORGS: i64 = 50;
const BIG_TABLES: [&str; 5] = [
    "blobs",
    "attachments",
    "transcript_events",
    "request_log",
    "audit_log",
];

const INSTANCE_SCOPE: &str = "instance";

fn db(e: sqlx::Error) -> ApiError {
    aurora_db::DbError::from(e).into()
}

/// The snapshot of `scope`: served as is while younger than `STORAGE_SNAPSHOT_MAX_AGE`
/// (unless `force`), else recomputed once. Concurrent callers (any replica) queue on an advisory
/// lock; one that waited uses what the holder computed after it asked instead of computing again.
async fn snapshot<T, Fut>(
    pool: &PgPool,
    scope: &str,
    force: bool,
    compute: impl FnOnce() -> Fut,
) -> Result<(T, DateTime<Utc>), ApiError>
where
    T: Serialize + DeserializeOwned,
    Fut: Future<Output = Result<T, ApiError>>,
{
    let max_age = chrono::Duration::from_std(STORAGE_SNAPSHOT_MAX_AGE).unwrap_or_default();
    let usable = |at: DateTime<Utc>, asked: Option<DateTime<Utc>>| match asked {
        Some(asked) => at >= asked,
        None => Utc::now() - at < max_age,
    };
    let read = |v: serde_json::Value| serde_json::from_value::<T>(v).ok();
    if !force
        && let Some((v, at)) = attachments::snapshot_get(pool, scope).await?
        && usable(at, None)
        && let Some(t) = read(v)
    {
        return Ok((t, at));
    }
    let asked = attachments::clock(pool).await?;
    let mut tx = pool.begin().await.map_err(db)?;
    attachments::snapshot_lock(&mut tx, scope).await?;
    // Another request may have computed it while this one waited for the lock.
    if let Some((v, at)) = attachments::snapshot_get(&mut *tx, scope).await?
        && usable(at, force.then_some(asked))
        && let Some(t) = read(v)
    {
        return Ok((t, at));
    }
    let fresh = compute().await?;
    let payload = serde_json::to_value(&fresh)
        .map_err(|e| ApiError::internal(format!("storage snapshot: {e}")))?;
    let at = attachments::snapshot_put(&mut *tx, scope, &payload).await?;
    tx.commit().await.map_err(db)?;
    Ok((fresh, at))
}

async fn compute_admin(pool: &PgPool, cfg: &AttachmentsSection) -> Result<AdminStorage, ApiError> {
    let (blob_bytes, blob_count) = attachments::blob_totals(pool).await?;
    let (attachment_bytes, _) = attachments::usage_totals(pool).await?;
    let (attachment_files, purged_files) = attachments::instance_file_counts(pool).await?;
    let mut orgs = Vec::new();
    for o in attachments::top_orgs(pool, TOP_ORGS).await? {
        let (mib, _) = attachments::org_limits(pool, o.org_id).await?;
        orgs.push(AdminOrgStorage {
            org_id: o.org_id,
            slug: o.slug,
            name: o.name,
            attachment_bytes: o.attachment_bytes,
            files: o.files,
            quota_bytes: quota::effective_quota_bytes(cfg, mib),
        });
    }
    let mut tables = Vec::new();
    for t in BIG_TABLES {
        if let Some(bytes) = attachments::relation_size(pool, t).await? {
            tables.push(TableSize {
                table: t.to_owned(),
                bytes,
            });
        }
    }
    Ok(AdminStorage {
        blob_bytes,
        blob_count,
        attachment_bytes,
        attachment_files,
        purged_files,
        orgs,
        tables,
        computed_at: epoch(),
    })
}

async fn compute_org(pool: &PgPool, org: Uuid) -> Result<OrgFigures, ApiError> {
    let (files, purged_files) = attachments::org_file_counts(pool, org).await?;
    let by_workspace = attachments::org_breakdown(pool, org)
        .await?
        .into_iter()
        .map(|r| WorkspaceStorage {
            workspace_id: r.workspace_id,
            workspace_slug: r.slug,
            workspace_name: r.name,
            direction: r.direction,
            files: r.files,
            bytes: r.bytes,
        })
        .collect();
    Ok(OrgFigures {
        files,
        purged_files,
        by_workspace,
    })
}

/// `GET /admin/storage` (`force = false`) and `POST /admin/storage/refresh` (`force = true`).
pub async fn admin_storage(state: &AppState, force: bool) -> Result<AdminStorage, ApiError> {
    let pool = state.pool.clone();
    let cfg = state.config.attachments.clone();
    let (mut s, at) = snapshot(&state.pool, INSTANCE_SCOPE, force, || async {
        compute_admin(&pool, &cfg).await
    })
    .await?;
    s.computed_at = at;
    Ok(s)
}

/// `GET /orgs/{org}/storage` (`force = false`) and `POST /orgs/{org}/storage/refresh`.
pub async fn org_storage(
    state: &AppState,
    c: &Caller,
    org: Uuid,
    force: bool,
) -> Result<OrgStorage, ApiError> {
    c.org(state, org).await?.require_admin()?;
    let pool = state.pool.clone();
    let (figures, at) = snapshot(&state.pool, &org.to_string(), force, || async {
        compute_org(&pool, org).await
    })
    .await?;
    // The quota figures are live: one row, and exact for the notices.
    let (mib, days) = attachments::org_limits(&state.pool, org).await?;
    let quota_bytes = quota::effective_quota_bytes(&state.config.attachments, mib);
    let used = attachments::usage(&state.pool, org).await?;
    Ok(OrgStorage {
        org_id: org,
        used_bytes: used,
        quota_bytes,
        notice: notice_level(used, quota_bytes).to_owned(),
        files: figures.files,
        purged_files: figures.purged_files,
        attachments_days: days,
        attachments_days_effective: quota::effective_days(&state.config.attachments, days),
        by_workspace: figures.by_workspace,
        computed_at: at,
    })
}

/// The daily attachment sweep: recompute the instance snapshot and every org's (`force`).
pub async fn refresh_all(pool: &PgPool, cfg: &AttachmentsSection) -> Result<(), ApiError> {
    snapshot(pool, INSTANCE_SCOPE, true, || compute_admin(pool, cfg)).await?;
    for org in attachments::orgs_with_storage(pool).await? {
        snapshot(pool, &org.to_string(), true, || compute_org(pool, org)).await?;
    }
    Ok(())
}

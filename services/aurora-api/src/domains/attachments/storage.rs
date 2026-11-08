//! Storage visibility (phase 2, RFE-015): `GET /orgs/{org}/storage` and `GET /admin/storage`.

use super::quota;
use super::sweep::notice_level;
use crate::app::AppState;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::repo::attachments;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// Attachment bytes of one workspace in one direction.
#[derive(Serialize, ToSchema)]
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
    /// Counter of non-purged attachment bytes.
    pub used_bytes: i64,
    /// Quota in force; null = unlimited.
    pub quota_bytes: Option<i64>,
    /// `clear` (< 80 %) | `warning` (≥ 80 %) | `full` (≥ 100 %).
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
}

pub async fn org_storage(state: &AppState, c: &Caller, org: Uuid) -> Result<OrgStorage, ApiError> {
    c.org(state, org).await?.require_admin()?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let (mib, days) = attachments::org_limits(&mut *conn, org).await?;
    let quota_bytes = quota::effective_quota_bytes(&state.config.attachments, mib);
    let used = attachments::usage(&mut *conn, org).await?;
    let (files, purged_files) = attachments::org_file_counts(&mut *conn, org).await?;
    let by_workspace = attachments::org_breakdown(&mut *conn, org)
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
    Ok(OrgStorage {
        org_id: org,
        used_bytes: used,
        quota_bytes,
        notice: notice_level(used, quota_bytes).to_owned(),
        files,
        purged_files,
        attachments_days: days,
        attachments_days_effective: quota::effective_days(&state.config.attachments, days),
        by_workspace,
    })
}

/// Attachment bytes of one org in the instance breakdown.
#[derive(Serialize, ToSchema)]
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
#[derive(Serialize, ToSchema)]
pub struct TableSize {
    pub table: String,
    pub bytes: i64,
}

/// `GET /admin/storage` (API_CONTRACTS C-172).
#[derive(Serialize, ToSchema)]
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
}

const TOP_ORGS: i64 = 50;
const BIG_TABLES: [&str; 5] = [
    "blobs",
    "attachments",
    "transcript_events",
    "request_log",
    "audit_log",
];

pub async fn admin_storage(state: &AppState) -> Result<AdminStorage, ApiError> {
    let pool = &state.pool;
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
            quota_bytes: quota::effective_quota_bytes(&state.config.attachments, mib),
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
    })
}

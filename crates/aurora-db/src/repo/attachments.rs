//! Attachments (docs/03 §6, docs/10 §Attachments): inbound uploads (`pending` until bound to a
//! turn) and outbound files (`bound` from birth). Binding lives in `turns::bind_attachments`.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub direction: String,
    pub status: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub blob_backend: String,
    pub blob_key: String,
    pub caption: Option<String>,
    pub created_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// Bytes deleted (retention or explicit delete); the row stays (RFE-015).
    pub purged_at: Option<DateTime<Utc>>,
    pub purge_reason: Option<String>,
}

const COLS: &str = "id, workspace_id, conversation_id, turn_id, direction, status, filename, \
    content_type, size_bytes, sha256, blob_backend, blob_key, caption, created_by_user_id, \
    created_at, purged_at, purge_reason";

pub struct NewAttachment<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    /// `in` | `out`
    pub direction: &'a str,
    /// `pending` | `bound`
    pub status: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub size_bytes: i64,
    pub sha256: &'a str,
    pub blob_backend: &'a str,
    pub blob_key: &'a str,
    pub caption: Option<&'a str>,
    pub created_by_user_id: Option<Uuid>,
}

pub async fn insert(
    exec: impl PgExecutor<'_>,
    n: &NewAttachment<'_>,
) -> Result<AttachmentRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO attachments (id, workspace_id, conversation_id, turn_id, direction, status,
            filename, content_type, size_bytes, sha256, blob_backend, blob_key, caption,
            created_by_user_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
         RETURNING {COLS}"
    ))
    .bind(n.id)
    .bind(n.workspace_id)
    .bind(n.conversation_id)
    .bind(n.turn_id)
    .bind(n.direction)
    .bind(n.status)
    .bind(n.filename)
    .bind(n.content_type)
    .bind(n.size_bytes)
    .bind(n.sha256)
    .bind(n.blob_backend)
    .bind(n.blob_key)
    .bind(n.caption)
    .bind(n.created_by_user_id)
    .fetch_one(exec)
    .await?)
}

pub async fn get(exec: impl PgExecutor<'_>, id: Uuid) -> Result<Option<AttachmentRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM attachments WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Inbound attachment readable by `runner`: `bound` to a turn that is in flight (`dispatched` /
/// `running`) on the current boot of a sandbox this runner owns (docs/16 §W5).
pub async fn get_for_runner(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    runner: Uuid,
) -> Result<Option<AttachmentRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.workspace_id, a.conversation_id, a.turn_id, a.direction, a.status,
                a.filename, a.content_type, a.size_bytes, a.sha256, a.blob_backend, a.blob_key,
                a.caption, a.created_by_user_id, a.created_at, a.purged_at, a.purge_reason
         FROM attachments a
         JOIN turns t ON t.id = a.turn_id AND t.role = 'turn'
         JOIN sandboxes s ON s.agent_id = t.agent_id
         WHERE a.id = $1 AND a.direction = 'in' AND a.status = 'bound' AND a.purged_at IS NULL
           AND t.status IN ('dispatched', 'running')
           AND t.boot_id IS NOT NULL AND s.boot_id = t.boot_id AND s.runner_id = $2",
    )
    .bind(id)
    .bind(runner)
    .fetch_optional(exec)
    .await?)
}

/// `(outbound files already stored for the turn, outbound bytes of the conversation in the last
/// 24 h)`. Call under the conversation lock so concurrent uploads cannot overshoot the caps.
pub async fn out_usage(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    turn_id: Uuid,
) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE turn_id = $2)::bigint,
                COALESCE(sum(size_bytes) FILTER (WHERE created_at > now() - interval '24 hours'), 0)::bigint
         FROM attachments WHERE conversation_id = $1 AND direction = 'out'",
    )
    .bind(conversation_id)
    .bind(turn_id)
    .fetch_one(conn)
    .await?)
}

/// Delete up to `limit` `pending` rows older than `ttl_secs` (row-claiming, so a concurrent bind
/// loses cleanly) and return their `(blob_backend, blob_key)` for blob removal.
pub async fn delete_expired_pending(
    exec: impl PgExecutor<'_>,
    ttl_secs: f64,
    limit: i64,
) -> Result<Vec<(String, String)>, DbError> {
    Ok(sqlx::query_as(
        "DELETE FROM attachments WHERE id IN (
             SELECT id FROM attachments
             WHERE status = 'pending' AND direction = 'in'
               AND created_at < now() - make_interval(secs => $1)
             ORDER BY created_at LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING blob_backend, blob_key",
    )
    .bind(ttl_secs)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// A fetch that failed (file too large, connector error): a `failed` row bound to the turn so
/// the transcript shows what was dropped. It carries no blob.
#[allow(clippy::too_many_arguments)]
pub async fn insert_failed(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    workspace_id: Uuid,
    conversation_id: Uuid,
    turn_id: Uuid,
    filename: &str,
    content_type: &str,
    size_bytes: i64,
    reason: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO attachments (id, workspace_id, conversation_id, turn_id, direction, status,
            filename, content_type, size_bytes, sha256, blob_backend, blob_key, failure_reason)
         VALUES ($1,$2,$3,$4,'in','failed',$5,$6,$7,'','','',$8)",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(conversation_id)
    .bind(turn_id)
    .bind(filename)
    .bind(content_type)
    .bind(size_bytes)
    .bind(reason)
    .execute(exec)
    .await?;
    Ok(())
}

/// Bound outbound files of a turn (`send_file`), oldest first.
pub async fn out_for_turn(
    exec: impl PgExecutor<'_>,
    turn_id: Uuid,
) -> Result<Vec<AttachmentRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM attachments
          WHERE turn_id=$1 AND direction='out' AND status='bound' ORDER BY created_at, id"
    ))
    .bind(turn_id)
    .fetch_all(exec)
    .await?)
}

// ------------------------------------------------------------------ quota (RFE-015)

/// Org that owns a workspace.
pub async fn org_of_workspace(
    exec: impl PgExecutor<'_>,
    workspace: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM workspaces WHERE id = $1")
            .bind(workspace)
            .fetch_optional(exec)
            .await?,
    )
}

/// The org's attachment byte counter, locked `FOR UPDATE` until the transaction ends: concurrent
/// reservations of one org serialize here, so the quota cannot be overshot. Creates the counter
/// row when absent.
pub async fn lock_usage(conn: &mut PgConnection, org: Uuid) -> Result<i64, DbError> {
    sqlx::query(
        "INSERT INTO org_storage_usage (org_id) VALUES ($1) ON CONFLICT (org_id) DO NOTHING",
    )
    .bind(org)
    .execute(&mut *conn)
    .await?;
    Ok(sqlx::query_scalar(
        "SELECT attachment_bytes FROM org_storage_usage WHERE org_id = $1 FOR UPDATE",
    )
    .bind(org)
    .fetch_one(conn)
    .await?)
}

/// The org's attachment byte counter (no lock; `0` when it has none yet).
pub async fn usage(exec: impl PgExecutor<'_>, org: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT attachment_bytes FROM org_storage_usage WHERE org_id = $1")
            .bind(org)
            .fetch_optional(exec)
            .await?
            .unwrap_or(0),
    )
}

/// `(attachment_quota_mib, attachments_days)` an org set in `settings.retention`.
pub async fn org_limits(
    exec: impl PgExecutor<'_>,
    org: Uuid,
) -> Result<(Option<i64>, Option<i64>), DbError> {
    Ok(sqlx::query_as(
        "SELECT (settings #>> '{retention,attachment_quota_mib}')::bigint,
                (settings #>> '{retention,attachments_days}')::bigint
           FROM organizations WHERE id = $1",
    )
    .bind(org)
    .fetch_optional(exec)
    .await?
    .unwrap_or((None, None)))
}

// ------------------------------------------------------------------ purge (RFE-015)

/// A purged attachment whose blob still has to be removed.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Purged {
    pub id: Uuid,
    pub size_bytes: i64,
    pub blob_backend: String,
    pub blob_key: String,
}

/// Mark one attachment purged (`reason`: `retention` | `deleted`) and clear its `blob_key`, so a
/// blob that survives a failed delete is unreferenced and left to the orphan sweep. `None` when
/// it holds no bytes (already purged, or `failed`).
pub async fn purge_one(
    conn: &mut PgConnection,
    id: Uuid,
    reason: &str,
) -> Result<Option<Purged>, DbError> {
    Ok(sqlx::query_as(
        "WITH old AS (SELECT id, size_bytes, blob_backend, blob_key FROM attachments
                       WHERE id = $1 AND purged_at IS NULL AND status <> 'failed' FOR UPDATE),
              upd AS (UPDATE attachments a SET purged_at = now(), purge_reason = $2,
                             blob_key = '', updated_at = now()
                       FROM old WHERE a.id = old.id RETURNING a.id)
         SELECT old.id, old.size_bytes, old.blob_backend, old.blob_key FROM old JOIN upd USING (id)",
    )
    .bind(id)
    .bind(reason)
    .fetch_optional(conn)
    .await?)
}

/// Purge up to `limit` bound attachments of `org` older than `days` (`purge_reason =
/// retention`). `SKIP LOCKED` keeps concurrent sweeps disjoint.
pub async fn purge_expired(
    conn: &mut PgConnection,
    org: Uuid,
    days: i64,
    limit: i64,
) -> Result<Vec<Purged>, DbError> {
    Ok(sqlx::query_as(
        "WITH c AS (SELECT a.id, a.size_bytes, a.blob_backend, a.blob_key
                      FROM attachments a JOIN workspaces w ON w.id = a.workspace_id
                     WHERE w.org_id = $1 AND a.purged_at IS NULL AND a.status = 'bound'
                       AND a.created_at < now() - make_interval(days => $2::int)
                     ORDER BY a.created_at, a.id LIMIT $3 FOR UPDATE OF a SKIP LOCKED),
              u AS (UPDATE attachments a SET purged_at = now(), purge_reason = 'retention',
                           blob_key = '', updated_at = now()
                     FROM c WHERE a.id = c.id RETURNING a.id)
         SELECT c.id, c.size_bytes, c.blob_backend, c.blob_key FROM c JOIN u USING (id)",
    )
    .bind(org)
    .bind(days)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Orgs with an attachment retention window `(org_id, days)`.
pub async fn orgs_with_retention(exec: impl PgExecutor<'_>) -> Result<Vec<(Uuid, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, (settings #>> '{retention,attachments_days}')::bigint FROM organizations
         WHERE settings #>> '{retention,attachments_days}' IS NOT NULL",
    )
    .fetch_all(exec)
    .await?)
}

/// Recompute one org's counter from its attachment rows under the counter's lock; returns
/// `Some((was, now))` when it had drifted.
pub async fn reconcile_org(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Option<(i64, i64)>, DbError> {
    let was = lock_usage(&mut *conn, org).await?;
    let now: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(a.size_bytes), 0)::bigint FROM attachments a
           JOIN workspaces w ON w.id = a.workspace_id
          WHERE w.org_id = $1 AND a.purged_at IS NULL AND a.status <> 'failed'",
    )
    .bind(org)
    .fetch_one(&mut *conn)
    .await?;
    if was == now {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE org_storage_usage SET attachment_bytes = $2, updated_at = now() WHERE org_id = $1",
    )
    .bind(org)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(Some((was, now)))
}

/// Orgs that have a counter row or any attachment row.
pub async fn orgs_with_storage(exec: impl PgExecutor<'_>) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT org_id FROM org_storage_usage
         UNION SELECT w.org_id FROM attachments a JOIN workspaces w ON w.id = a.workspace_id",
    )
    .fetch_all(exec)
    .await?)
}

// ------------------------------------------------------------------ breakdown (RFE-015)

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkspaceUsage {
    pub workspace_id: Uuid,
    pub slug: String,
    pub name: String,
    pub direction: String,
    pub files: i64,
    pub bytes: i64,
}

/// Live attachment bytes of an org by workspace and direction.
pub async fn org_breakdown(
    exec: impl PgExecutor<'_>,
    org: Uuid,
) -> Result<Vec<WorkspaceUsage>, DbError> {
    Ok(sqlx::query_as(
        "SELECT w.id AS workspace_id, w.slug, w.name, a.direction, count(*)::bigint AS files,
                COALESCE(sum(a.size_bytes), 0)::bigint AS bytes
           FROM attachments a JOIN workspaces w ON w.id = a.workspace_id
          WHERE w.org_id = $1 AND a.purged_at IS NULL AND a.status <> 'failed'
          GROUP BY w.id, w.slug, w.name, a.direction ORDER BY bytes DESC, w.slug, a.direction",
    )
    .bind(org)
    .fetch_all(exec)
    .await?)
}

/// `(files that hold bytes, purged files)` of an org.
pub async fn org_file_counts(exec: impl PgExecutor<'_>, org: Uuid) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE a.purged_at IS NULL AND a.status <> 'failed')::bigint,
                count(*) FILTER (WHERE a.purged_at IS NOT NULL)::bigint
           FROM attachments a JOIN workspaces w ON w.id = a.workspace_id WHERE w.org_id = $1",
    )
    .bind(org)
    .fetch_one(exec)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrgStorage {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub attachment_bytes: i64,
    pub files: i64,
}

/// The `limit` orgs with the most attachment bytes (from the counter) and their live file counts.
pub async fn top_orgs(exec: impl PgExecutor<'_>, limit: i64) -> Result<Vec<OrgStorage>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.id AS org_id, o.slug, o.name, u.attachment_bytes,
                (SELECT count(*) FROM attachments a JOIN workspaces w ON w.id = a.workspace_id
                  WHERE w.org_id = o.id AND a.purged_at IS NULL AND a.status <> 'failed')::bigint AS files
           FROM org_storage_usage u JOIN organizations o ON o.id = u.org_id
          ORDER BY u.attachment_bytes DESC, o.slug LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// `(sum of all counters, number of orgs with a counter)`.
pub async fn usage_totals(exec: impl PgExecutor<'_>) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(sum(attachment_bytes), 0)::bigint, count(*)::bigint FROM org_storage_usage",
    )
    .fetch_one(exec)
    .await?)
}

/// `(files that hold bytes, purged files)` of the whole instance.
pub async fn instance_file_counts(exec: impl PgExecutor<'_>) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE purged_at IS NULL AND status <> 'failed')::bigint,
                count(*) FILTER (WHERE purged_at IS NOT NULL)::bigint FROM attachments",
    )
    .fetch_one(exec)
    .await?)
}

/// On-disk size (table, indexes, TOAST, partitions) of a named relation; `None` when it does not
/// exist.
pub async fn relation_size(exec: impl PgExecutor<'_>, name: &str) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT (SELECT sum(pg_total_relation_size(c.oid))::bigint FROM pg_class c
                  WHERE c.oid = r.oid OR c.oid IN (SELECT inhrelid FROM pg_inherits WHERE inhparent = r.oid))
           FROM (SELECT to_regclass($1) AS oid) r WHERE r.oid IS NOT NULL",
    )
    .bind(name)
    .fetch_optional(exec)
    .await?
    .flatten())
}

/// `(stored blob bytes, blob rows)` of the `postgres` blob backend.
pub async fn blob_totals(exec: impl PgExecutor<'_>) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(sum(octet_length(content)), 0)::bigint, count(*)::bigint FROM blobs",
    )
    .fetch_one(exec)
    .await?)
}

/// Level (`warning` | `full` | `clear`) of the org's latest quota notice row in the audit log.
pub async fn last_quota_notice(
    exec: impl PgExecutor<'_>,
    org: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT metadata ->> 'level' FROM audit_log
          WHERE org_id = $1 AND action = 'attachment.quota_notice'
          ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(org)
    .fetch_optional(exec)
    .await?
    .flatten())
}

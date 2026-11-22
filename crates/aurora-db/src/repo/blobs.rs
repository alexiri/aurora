//! Phase 0 blob backend (`postgres`): immutable `blobs` rows addressed by an opaque key.

use crate::DbError;
use sqlx::PgExecutor;

pub async fn put(exec: impl PgExecutor<'_>, key: &str, content: &[u8]) -> Result<(), DbError> {
    sqlx::query("INSERT INTO blobs (key, content) VALUES ($1, $2)")
        .bind(key)
        .bind(content)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn get(exec: impl PgExecutor<'_>, key: &str) -> Result<Option<Vec<u8>>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT content FROM blobs WHERE key = $1")
            .bind(key)
            .fetch_optional(exec)
            .await?,
    )
}

/// Idempotent: deleting a missing key is not an error.
pub async fn delete(exec: impl PgExecutor<'_>, key: &str) -> Result<(), DbError> {
    sqlx::query("DELETE FROM blobs WHERE key = $1")
        .bind(key)
        .execute(exec)
        .await?;
    Ok(())
}

/// Every column that stores a `blobs.key`: the references the orphan sweep honours (D-466, FIX-223).
/// A table that stores a blob key adds itself here and to the query of [`delete_unreferenced`] in
/// the same change; `scripts/guards/blob-references.sh` fails on a `*blob_key` column of a
/// migration that is not named below (`("table", "column")`).
pub const BLOB_REFERENCE_COLUMNS: &[(&str, &str)] =
    &[("attachments", "blob_key"), ("agents", "avatar_blob_key")];

/// Delete up to `limit` `postgres`-backend blobs older than `min_age_secs` that nothing references
/// (rows removed by a conversation/workspace/agent/org cascade leave their blobs behind). The
/// references are those of [`BLOB_REFERENCE_COLUMNS`]: an `attachments` row, or an agent's
/// `avatar_blob_key`. A deleted agent's avatar blob is unreferenced and goes (SEC-47).
/// `FOR UPDATE ... SKIP LOCKED` keeps concurrent sweeps on several replicas disjoint.
/// Returns how many were deleted.
pub async fn delete_unreferenced(
    exec: impl PgExecutor<'_>,
    min_age_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM blobs WHERE key IN (
             SELECT b.key FROM blobs b
             WHERE b.created_at < now() - make_interval(secs => $1)
               AND NOT EXISTS (SELECT 1 FROM attachments a
                               WHERE a.blob_backend = 'postgres' AND a.blob_key = b.key)
               AND NOT EXISTS (SELECT 1 FROM agents g WHERE g.avatar_blob_key = b.key)
             ORDER BY b.created_at LIMIT $2
             FOR UPDATE OF b SKIP LOCKED)",
    )
    .bind(min_age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

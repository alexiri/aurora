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

/// Delete up to `limit` `postgres`-backend blobs older than `min_age_secs` that no `attachments`
/// row references (rows removed by a conversation/workspace/agent/org cascade leave their blobs
/// behind). `FOR UPDATE ... SKIP LOCKED` keeps concurrent sweeps on several replicas disjoint.
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
             ORDER BY b.created_at LIMIT $2
             FOR UPDATE OF b SKIP LOCKED)",
    )
    .bind(min_age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

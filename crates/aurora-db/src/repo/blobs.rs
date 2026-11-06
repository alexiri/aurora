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

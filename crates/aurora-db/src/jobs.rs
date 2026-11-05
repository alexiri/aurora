//! Singleton jobs: `pg_try_advisory_lock` on a dedicated connection.

use std::future::Future;

use sqlx::PgPool;

use crate::DbError;

/// Run `f` only if the session-level advisory lock for `job_name` is free.
///
/// Returns `Ok(None)` when another holder has it. The lock lives on a dedicated
/// connection detached from the pool, so even if `f` panics or the future is
/// dropped the connection closes and Postgres releases the lock.
pub async fn try_singleton<F, Fut, T>(
    pool: &PgPool,
    job_name: &str,
    f: F,
) -> Result<Option<T>, DbError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let mut conn = pool.acquire().await?.detach();
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
        .bind(job_name)
        .fetch_one(&mut conn)
        .await?;
    if !got {
        let _ = sqlx::Connection::close(conn).await;
        return Ok(None);
    }
    let out = f().await;
    let unlocked = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(job_name)
        .execute(&mut conn)
        .await;
    if let Err(e) = unlocked {
        tracing::warn!(job = job_name, error = %e, "advisory unlock failed; closing connection");
    }
    let _ = sqlx::Connection::close(conn).await;
    Ok(Some(out))
}

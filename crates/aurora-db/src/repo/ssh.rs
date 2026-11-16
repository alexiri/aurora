//! SSH access (phase 5, P4-SSH-1, D-239, SEC-93; docs/03 §`ssh_keys`, §`ssh_mint_counters`):
//! a person's registered ed25519 public keys and the certificate-mint rate-limit ledger.
//! Parsing and canonicalizing a key is the API's job; this module only stores what it is given.
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

use crate::DbError;

/// The column list as a macro so every statement stays one static literal (sqlx 0.9 refuses
/// runtime-built SQL without an audit marker).
macro_rules! cols {
    () => {
        "id, user_id, name, algorithm, public_key, fingerprint, last_used_at, created_at, updated_at"
    };
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SshKeyRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The person's keys, newest first.
pub async fn list_keys<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Vec<SshKeyRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        cols!(),
        " FROM ssh_keys WHERE user_id = $1 ORDER BY created_at DESC, id DESC"
    ))
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// One of the person's keys (another person's id is `None`).
pub async fn get_key<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    id: Uuid,
) -> Result<Option<SshKeyRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        cols!(),
        " FROM ssh_keys WHERE user_id = $1 AND id = $2"
    ))
    .bind(user)
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// The person's key with this fingerprint, if registered.
pub async fn find_key_by_fingerprint<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    fingerprint: &str,
) -> Result<Option<SshKeyRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        cols!(),
        " FROM ssh_keys WHERE user_id = $1 AND fingerprint = $2"
    ))
    .bind(user)
    .bind(fingerprint)
    .fetch_optional(exec)
    .await?)
}

/// Serialize key registration per person (the cap check and the insert are one critical
/// section): locks the user row until the transaction ends. Returns the person's key count.
pub async fn lock_and_count_keys(conn: &mut PgConnection, user: Uuid) -> Result<i64, DbError> {
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM ssh_keys WHERE user_id = $1")
            .bind(user)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Insert a key. A repeated `(user, fingerprint)` is [`DbError::UniqueViolation`].
pub async fn insert_key<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    user: Uuid,
    name: &str,
    public_key: &str,
    fingerprint: &str,
) -> Result<SshKeyRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO ssh_keys (id, user_id, name, algorithm, public_key, fingerprint)
         VALUES ($1, $2, $3, 'ssh-ed25519', $4, $5) RETURNING ",
        cols!()
    ))
    .bind(id)
    .bind(user)
    .bind(name)
    .bind(public_key)
    .bind(fingerprint)
    .fetch_one(exec)
    .await?)
}

/// Rename one of the person's keys.
pub async fn rename_key<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    id: Uuid,
    name: &str,
) -> Result<Option<SshKeyRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE ssh_keys SET name = $3, updated_at = now()
          WHERE user_id = $1 AND id = $2 RETURNING ",
        cols!()
    ))
    .bind(user)
    .bind(id)
    .bind(name)
    .fetch_optional(exec)
    .await?)
}

/// Delete one of the person's keys; the deleted row (for the audit metadata), `None` if absent.
pub async fn delete_key<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    id: Uuid,
) -> Result<Option<SshKeyRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "DELETE FROM ssh_keys WHERE user_id = $1 AND id = $2 RETURNING ",
        cols!()
    ))
    .bind(user)
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Stamp `last_used_at` (at most once a minute: churn column).
pub async fn touch_key<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE ssh_keys SET last_used_at = now()
          WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// The mint ledger (C-791): one atomic `INSERT … ON CONFLICT … WHERE count < limit RETURNING`
/// for the current hour. `Some(count)` = the mint is allowed (and counted); `None` = the limit
/// is reached. Exact across replicas (the row lock serializes), never fails open (a database
/// error is an error, not an allow). Runs in the mint's transaction.
pub async fn bump_mint_counter<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    agent: Uuid,
    limit: i32,
) -> Result<Option<i32>, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO ssh_mint_counters (user_id, agent_id, window_start, count)
         VALUES ($1, $2, date_trunc('hour', now()), 1)
         ON CONFLICT (user_id, agent_id, window_start)
         DO UPDATE SET count = ssh_mint_counters.count + 1, updated_at = now()
            WHERE ssh_mint_counters.count < $3
         RETURNING count",
    )
    .bind(user)
    .bind(agent)
    .bind(limit)
    .fetch_optional(exec)
    .await?)
}

/// Seconds until the current hour ends (the `Retry-After` of a refused mint), at least 1.
pub async fn seconds_to_hour_end<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, DbError> {
    let s: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM date_trunc('hour', now()) + interval '1 hour' - now())::float8",
    )
    .fetch_one(exec)
    .await?;
    Ok((s.ceil() as i64).max(1))
}

/// Retention sweep: ledger rows older than 24 h.
pub async fn prune_mint_counters<'e, E: PgExecutor<'e>>(exec: E) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM ssh_mint_counters WHERE window_start < now() - interval '24 hours'",
    )
    .execute(exec)
    .await?
    .rows_affected())
}

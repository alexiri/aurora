//! Runners and their replicas (docs/11 §Registration, replicas, ownership).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunnerRow {
    pub id: Uuid,
    pub name: String,
    pub installation_fingerprint: String,
    pub backend: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub last_registered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const COLS: &str =
    "id, name, installation_fingerprint, backend, capabilities, last_registered_at, created_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReplicaRow {
    pub id: Uuid,
    pub runner_id: Uuid,
    pub advertised_host: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
    pub live: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunnerStats {
    pub runner_id: Uuid,
    pub live_replicas: i64,
    pub sandboxes: i64,
    pub running: i64,
}

pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    name: &str,
    token_hash: &[u8],
    fingerprint: &str,
) -> Result<RunnerRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO runners (id, name, token_hash, installation_fingerprint)
         VALUES ($1,$2,$3,$4) RETURNING {COLS}"
    ))
    .bind(id)
    .bind(name)
    .bind(token_hash)
    .bind(fingerprint)
    .fetch_one(exec)
    .await?)
}

/// Bootstrap upsert of a named runner with the given token (`AURORA_BOOTSTRAP_RUNNER_TOKEN`).
pub async fn upsert_named<'e, E: PgExecutor<'e>>(
    exec: E,
    name: &str,
    token_hash: &[u8],
    fingerprint: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO runners (id, name, token_hash, installation_fingerprint)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT (name) DO UPDATE
           SET token_hash = EXCLUDED.token_hash,
               installation_fingerprint = EXCLUDED.installation_fingerprint, updated_at = now()",
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(token_hash)
    .bind(fingerprint)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn set_token<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    token_hash: &[u8],
    fingerprint: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE runners SET token_hash=$2, installation_fingerprint=$3, updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(token_hash)
    .bind(fingerprint)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// Number of sandboxes that currently occupy the runner (`starting`/`running`/`stopping`).
pub async fn occupied_sandboxes<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM sandboxes WHERE runner_id=$1 AND state IN ('starting','running','stopping')",
    )
    .bind(id)
    .fetch_one(exec)
    .await?)
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM runners WHERE id=$1")
        .bind(id)
        .execute(exec)
        .await?
        .rows_affected()
        > 0)
}

pub async fn find_by_token_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8],
) -> Result<Option<RunnerRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM runners WHERE token_hash=$1"))
            .bind(hash)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<RunnerRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM runners WHERE id=$1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn list<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<RunnerRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM runners ORDER BY created_at, id"
    ))
    .fetch_all(exec)
    .await?)
}

pub async fn stats<'e, E: PgExecutor<'e>>(
    exec: E,
    online_secs: f64,
) -> Result<Vec<RunnerStats>, DbError> {
    Ok(sqlx::query_as(
        "SELECT r.id AS runner_id,
            (SELECT count(*) FROM runner_replicas p WHERE p.runner_id=r.id AND p.retired_at IS NULL
                AND p.last_seen_at > now() - make_interval(secs => $1)) AS live_replicas,
            (SELECT count(*) FROM sandboxes s WHERE s.runner_id=r.id) AS sandboxes,
            (SELECT count(*) FROM sandboxes s WHERE s.runner_id=r.id AND s.state='running') AS running
         FROM runners r",
    )
    .bind(online_secs)
    .fetch_all(exec)
    .await?)
}

pub async fn list_replicas<'e, E: PgExecutor<'e>>(
    exec: E,
    runner_id: Uuid,
    online_secs: f64,
) -> Result<Vec<ReplicaRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, runner_id, advertised_host, version, started_at, last_seen_at, retired_at,
                (retired_at IS NULL AND last_seen_at > now() - make_interval(secs => $2)) AS live
         FROM runner_replicas WHERE runner_id=$1 ORDER BY started_at, id",
    )
    .bind(runner_id)
    .bind(online_secs)
    .fetch_all(exec)
    .await?)
}

/// Record a registration: runner declared backend/capabilities, new replica row.
pub async fn register(
    conn: &mut sqlx::PgConnection,
    runner_id: Uuid,
    replica_id: Uuid,
    host: &str,
    version: &str,
    backend: &str,
    capabilities: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE runners SET backend=$2, capabilities=$3, last_registered_at=now(), updated_at=now() WHERE id=$1",
    )
    .bind(runner_id)
    .bind(backend)
    .bind(capabilities)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO runner_replicas (id, runner_id, advertised_host, version, started_at, last_seen_at)
         VALUES ($1,$2,$3,$4, now(), now())",
    )
    .bind(replica_id)
    .bind(runner_id)
    .bind(host)
    .bind(version)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Heartbeat. `false` when the replica is unknown, foreign or retired.
pub async fn touch_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    runner_id: Uuid,
    replica_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE runner_replicas SET last_seen_at=now(), updated_at=now()
         WHERE id=$1 AND runner_id=$2 AND retired_at IS NULL",
    )
    .bind(replica_id)
    .bind(runner_id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

pub async fn retire_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    runner_id: Uuid,
    replica_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE runner_replicas SET retired_at=now(), updated_at=now()
         WHERE id=$1 AND runner_id=$2 AND retired_at IS NULL",
    )
    .bind(replica_id)
    .bind(runner_id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// True only for the oldest live replica of the runner (spec fix S-08).
pub async fn is_oldest_live<'e, E: PgExecutor<'e>>(
    exec: E,
    runner_id: Uuid,
    replica_id: Uuid,
    online_secs: f64,
) -> Result<bool, DbError> {
    let oldest: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM runner_replicas
         WHERE runner_id=$1 AND retired_at IS NULL AND last_seen_at > now() - make_interval(secs => $2)
         ORDER BY started_at, id LIMIT 1",
    )
    .bind(runner_id)
    .bind(online_secs)
    .fetch_optional(exec)
    .await?;
    Ok(oldest == Some(replica_id))
}

pub async fn replica_host<'e, E: PgExecutor<'e>>(
    exec: E,
    replica_id: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT advertised_host FROM runner_replicas WHERE id=$1")
            .bind(replica_id)
            .fetch_optional(exec)
            .await?,
    )
}

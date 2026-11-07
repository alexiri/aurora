//! Connectors and their replicas (docs/13 §2–§3; schema `connectors`, `connector_replicas`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// A connector row without its token hash.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectorRow {
    pub id: String,
    pub display_name: String,
    pub version: Option<String>,
    pub contract: Option<i32>,
    /// The announced manifest. The service may keep private envelope keys (prefixed `_`) in it.
    pub manifest: Option<serde_json::Value>,
    pub manifest_digest: Option<String>,
    pub approved_manifest_digest: Option<String>,
    pub status: String,
    pub approved_by: Option<Uuid>,
    pub approved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, display_name, version, contract, manifest, manifest_digest, \
    approved_manifest_digest, status, approved_by, approved_at, created_at";

/// A connector replica (`connector_replicas`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReplicaRow {
    pub id: Uuid,
    pub connector_id: String,
    pub version: String,
    pub last_seen_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
}

/// New registration state written by `register` (all columns are replaced).
#[derive(Debug, Clone)]
pub struct Registration<'a> {
    pub version: &'a str,
    pub contract: i32,
    pub manifest: &'a serde_json::Value,
    pub manifest_digest: &'a str,
    pub status: &'a str,
    pub approved_manifest_digest: Option<&'a str>,
}

pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
    token_hash: &[u8],
    display_name: &str,
) -> Result<ConnectorRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO connectors (id, token_hash, display_name, status)
         VALUES ($1,$2,$3,'awaiting_registration') RETURNING {COLS}"
    ))
    .bind(id)
    .bind(token_hash)
    .bind(display_name)
    .fetch_one(exec)
    .await?)
}

/// Bootstrap upsert (`AURORA_BOOTSTRAP_CONNECTOR_TOKENS`): creates the connector as
/// `awaiting_registration`, or re-sets the token of an existing one. Registration state
/// (manifest, status, approval) is never touched.
pub async fn upsert_bootstrap<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
    token_hash: &[u8],
    display_name: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO connectors (id, token_hash, display_name, status)
         VALUES ($1,$2,$3,'awaiting_registration')
         ON CONFLICT (id) DO UPDATE SET token_hash = EXCLUDED.token_hash, updated_at = now()
         WHERE connectors.token_hash <> EXCLUDED.token_hash",
    )
    .bind(id)
    .bind(token_hash)
    .bind(display_name)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
) -> Result<Option<ConnectorRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM connectors WHERE id=$1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Row lock for read-modify-write of the registration state (inside a transaction).
pub async fn get_for_update<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
) -> Result<Option<ConnectorRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM connectors WHERE id=$1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

pub async fn find_by_token_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8],
) -> Result<Option<ConnectorRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM connectors WHERE token_hash=$1"
    ))
    .bind(hash)
    .fetch_optional(exec)
    .await?)
}

pub async fn list<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<ConnectorRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM connectors ORDER BY created_at, id"
    ))
    .fetch_all(exec)
    .await?)
}

/// Connectors serving the public catalogue (`active` with a manifest).
pub async fn list_active<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<ConnectorRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM connectors
         WHERE status='active' AND manifest IS NOT NULL ORDER BY display_name, id"
    ))
    .fetch_all(exec)
    .await?)
}

pub async fn set_token<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
    token_hash: &[u8],
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("UPDATE connectors SET token_hash=$2, updated_at=now() WHERE id=$1")
            .bind(id)
            .bind(token_hash)
            .execute(exec)
            .await?
            .rows_affected()
            > 0,
    )
}

/// Deletes the connector (cascades to replicas). The FK from integrations is `RESTRICT`.
pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: &str) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM connectors WHERE id=$1")
        .bind(id)
        .execute(exec)
        .await?
        .rows_affected()
        > 0)
}

/// Writes the outcome of a `register` call.
pub async fn set_registration<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
    r: &Registration<'_>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE connectors SET version=$2, contract=$3, manifest=$4, manifest_digest=$5,
                status=$6, approved_manifest_digest=$7, updated_at=now()
         WHERE id=$1",
    )
    .bind(id)
    .bind(r.version)
    .bind(r.contract)
    .bind(r.manifest)
    .bind(r.manifest_digest)
    .bind(r.status)
    .bind(r.approved_manifest_digest)
    .execute(exec)
    .await?;
    Ok(())
}

/// Marks the announced manifest approved. `manifest` is the (envelope-updated) stored value.
pub async fn set_approved<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
    digest: &str,
    manifest: &serde_json::Value,
    by: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE connectors SET status='active', approved_manifest_digest=$2, manifest=$3,
                approved_by=$4, approved_at=now(), updated_at=now()
         WHERE id=$1",
    )
    .bind(id)
    .bind(digest)
    .bind(manifest)
    .bind(by)
    .execute(exec)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- replicas

pub async fn insert_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    connector_id: &str,
    version: &str,
) -> Result<ReplicaRow, DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO connector_replicas (id, connector_id, version, last_seen_at)
         VALUES ($1,$2,$3, now())
         RETURNING id, connector_id, version, last_seen_at, retired_at",
    )
    .bind(id)
    .bind(connector_id)
    .bind(version)
    .fetch_one(exec)
    .await?)
}

/// Heartbeat: bumps `last_seen_at` when the replica belongs to the connector and is not
/// retired. `false` means unknown or retired.
pub async fn touch_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE connector_replicas SET last_seen_at=now(), updated_at=now()
         WHERE id=$1 AND connector_id=$2 AND retired_at IS NULL",
    )
    .bind(replica)
    .bind(connector_id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// Replicas of the connector that are not retired and were seen within `within_secs`.
pub async fn list_live_replicas<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    within_secs: f64,
) -> Result<Vec<ReplicaRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, connector_id, version, last_seen_at, retired_at FROM connector_replicas
         WHERE connector_id=$1 AND retired_at IS NULL
           AND last_seen_at > now() - make_interval(secs => $2)
         ORDER BY created_at, id",
    )
    .bind(connector_id)
    .bind(within_secs)
    .fetch_all(exec)
    .await?)
}

pub async fn count_live_replicas<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    within_secs: f64,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM connector_replicas
         WHERE connector_id=$1 AND retired_at IS NULL
           AND last_seen_at > now() - make_interval(secs => $2)",
    )
    .bind(connector_id)
    .bind(within_secs)
    .fetch_one(exec)
    .await?)
}

/// Marks the replica retired. `false` when it was unknown or already retired.
pub async fn retire_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE connector_replicas SET retired_at=now(), updated_at=now()
         WHERE id=$1 AND connector_id=$2 AND retired_at IS NULL",
    )
    .bind(replica)
    .bind(connector_id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

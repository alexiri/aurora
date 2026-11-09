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
    /// Added from the dashboard and run by a connector host (`13 §3.1`).
    pub managed: bool,
    pub type_id: Option<String>,
    /// Managed only: the stored non-secret settings (no defaults applied).
    pub settings: Option<serde_json::Value>,
    /// Owner id of the sealed settings (purpose `connector.settings`, nil org).
    pub secret_owner: Uuid,
    /// Field name → sealed reference column value (never a value).
    pub secret_refs: serde_json::Value,
    /// Field name → RFC 3339 time of the last write of the sealed value.
    pub secret_updated_at: serde_json::Value,
}

const COLS: &str = "id, display_name, version, contract, manifest, manifest_digest, \
    approved_manifest_digest, status, approved_by, approved_at, created_at, \
    managed, type_id, settings, secret_owner, secret_refs, secret_updated_at";

/// New managed connector (`POST /v1/admin/connectors` with `managed: true`).
#[derive(Debug, Clone)]
pub struct NewManaged<'a> {
    pub id: &'a str,
    pub display_name: &'a str,
    pub settings: &'a serde_json::Value,
    pub secret_owner: Uuid,
    pub secret_refs: &'a serde_json::Value,
    pub secret_updated_at: &'a serde_json::Value,
    /// Stored manifest (with its approved baseline) and its digest.
    pub manifest: &'a serde_json::Value,
    pub digest: &'a str,
}

pub async fn insert_managed<'e, E: PgExecutor<'e>>(
    exec: E,
    n: &NewManaged<'_>,
) -> Result<ConnectorRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO connectors (id, display_name, status, managed, type_id, settings,
                secret_owner, secret_refs, secret_updated_at, manifest, manifest_digest,
                approved_manifest_digest)
         VALUES ($1,$2,'awaiting_registration',true,$1,$3,$4,$5,$8,$6,$7,$7) RETURNING {COLS}"
    ))
    .bind(n.id)
    .bind(n.display_name)
    .bind(n.settings)
    .bind(n.secret_owner)
    .bind(n.secret_refs)
    .bind(n.manifest)
    .bind(n.digest)
    .bind(n.secret_updated_at)
    .fetch_one(exec)
    .await?)
}

/// Writes the editable state of a connector (any kind: `settings`/`secret_refs` only change for
/// managed connectors, which the caller guarantees).
pub async fn update_managed<'e, E: PgExecutor<'e>>(
    exec: E,
    id: &str,
    display_name: &str,
    status: &str,
    settings: Option<&serde_json::Value>,
    secret_refs: &serde_json::Value,
    secret_updated_at: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE connectors SET display_name=$2, status=$3, settings=$4, secret_refs=$5,
                secret_updated_at=$6, updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(display_name)
    .bind(status)
    .bind(settings)
    .bind(secret_refs)
    .bind(secret_updated_at)
    .execute(exec)
    .await?;
    Ok(())
}

/// Managed connectors that are not `disabled` and whose type is one of `types`.
pub async fn list_managed_enabled<'e, E: PgExecutor<'e>>(
    exec: E,
    types: &[String],
) -> Result<Vec<ConnectorRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM connectors
         WHERE managed AND status <> 'disabled' AND type_id = ANY($1) ORDER BY id"
    ))
    .bind(types)
    .fetch_all(exec)
    .await?)
}

/// Advances the assignments revision (C-456) in the caller's transaction and wakes the hosts'
/// long polls on commit (`aurora_ops`, payload `{"assignments": true}`).
pub async fn bump_revision<'e, E: PgExecutor<'e>>(exec: E) -> Result<(), DbError> {
    sqlx::query(
        "SELECT nextval('connector_assignments_rev'),
                pg_notify('aurora_ops', '{\"assignments\":true}')",
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Current assignments revision (0 until the first change).
pub async fn revision<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM connector_assignments_rev",
    )
    .fetch_one(exec)
    .await?)
}

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

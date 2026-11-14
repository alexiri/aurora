//! Connector hosts, their anchor tokens and per-connector status (docs/13 §3.1;
//! `connector_hosts`, `connector_host_tokens`, `connector_host_status`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HostRow {
    pub instance_id: Uuid,
    pub version: String,
    /// `[{type_id, manifest, manifest_digest}]`, verified signatures only.
    pub types: serde_json::Value,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StatusRow {
    pub connector_id: String,
    pub host_instance: Uuid,
    pub state: String,
    pub message: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Bootstrap upsert of the host token hash (`AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN`). Idempotent.
pub async fn upsert_token<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO connector_host_tokens (id, token_hash) VALUES ($1,$2)
         ON CONFLICT (token_hash) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(token_hash)
    .execute(exec)
    .await?;
    Ok(())
}

/// A live (not revoked) host token: `type_ids = None` is the shared bootstrap token.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenAuth {
    pub id: Uuid,
    pub type_ids: Option<Vec<String>>,
}

/// The live token with this hash (FIX-120).
pub async fn find_token<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<TokenAuth>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, type_ids FROM connector_host_tokens WHERE token_hash=$1 AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .fetch_optional(exec)
    .await?)
}

/// `true` when the instance has a live token bound to connector types: from then on the shared
/// bootstrap token is refused (FIX-120, D-189).
pub async fn bound_tokens_exist<'e, E: PgExecutor<'e>>(exec: E) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM connector_host_tokens
                        WHERE type_ids IS NOT NULL AND revoked_at IS NULL)",
    )
    .fetch_one(exec)
    .await?)
}

/// Records a use, at most once a minute per token (a churn column).
pub async fn touch_token<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE connector_host_tokens SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    pub id: Uuid,
    pub name: Option<String>,
    pub prefix: Option<String>,
    pub type_ids: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

const TOKEN_SEL: &str = "id, name, prefix, type_ids, created_at, last_used_at, revoked_at";

/// Every token, newest first (the shared bootstrap token has `type_ids = None`).
pub async fn list_tokens<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<TokenRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TOKEN_SEL} FROM connector_host_tokens ORDER BY created_at DESC, id"
    ))
    .fetch_all(exec)
    .await?)
}

/// Issues a token bound to `type_ids`.
pub async fn insert_bound_token<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    token_hash: &[u8],
    prefix: &str,
    name: &str,
    type_ids: &[String],
    created_by: Option<Uuid>,
) -> Result<TokenRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO connector_host_tokens (id, token_hash, prefix, name, type_ids, created_by)
         VALUES ($1,$2,$3,$4,$5,$6) RETURNING {TOKEN_SEL}"
    ))
    .bind(id)
    .bind(token_hash)
    .bind(prefix)
    .bind(name)
    .bind(type_ids)
    .bind(created_by)
    .fetch_one(exec)
    .await?)
}

/// Revokes a token (first revocation wins); `None` when it does not exist.
pub async fn revoke_token<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<TokenRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE connector_host_tokens SET revoked_at = COALESCE(revoked_at, now())
         WHERE id = $1 RETURNING {TOKEN_SEL}"
    ))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// The token that registered `instance_id`: `None` when the host is unknown, `Some(None)` for a
/// host registered by the shared bootstrap token.
pub async fn host_owner<'e, E: PgExecutor<'e>>(
    exec: E,
    instance_id: Uuid,
) -> Result<Option<Option<Uuid>>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT token_id FROM connector_hosts WHERE instance_id=$1")
            .bind(instance_id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Marks `instance_id` as registered by `token_id` unless it already has an owner.
pub async fn claim_host<'e, E: PgExecutor<'e>>(
    exec: E,
    instance_id: Uuid,
    token_id: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE connector_hosts SET token_id = $2 WHERE instance_id = $1 AND token_id IS NULL",
    )
    .bind(instance_id)
    .bind(token_id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Types announced by the live hosts that `token_id` registered, as `[{type_id, …}]` documents.
pub async fn list_seen_by_token<'e, E: PgExecutor<'e>>(
    exec: E,
    token_id: Uuid,
    within_secs: f64,
) -> Result<Vec<HostRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT instance_id, version, types, last_seen_at FROM connector_hosts
         WHERE token_id = $1 AND last_seen_at > now() - make_interval(secs => $2)
         ORDER BY last_seen_at DESC, instance_id",
    )
    .bind(token_id)
    .bind(within_secs)
    .fetch_all(exec)
    .await?)
}

pub async fn upsert_host<'e, E: PgExecutor<'e>>(
    exec: E,
    instance_id: Uuid,
    version: &str,
    types: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO connector_hosts (instance_id, version, types, last_seen_at)
         VALUES ($1,$2,$3, now())
         ON CONFLICT (instance_id) DO UPDATE
           SET version = EXCLUDED.version, types = EXCLUDED.types, last_seen_at = now()",
    )
    .bind(instance_id)
    .bind(version)
    .bind(types)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get_host<'e, E: PgExecutor<'e>>(
    exec: E,
    instance_id: Uuid,
) -> Result<Option<HostRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT instance_id, version, types, last_seen_at FROM connector_hosts WHERE instance_id=$1",
    )
    .bind(instance_id)
    .fetch_optional(exec)
    .await?)
}

/// Hosts seen within `within_secs`, newest first.
pub async fn list_seen<'e, E: PgExecutor<'e>>(
    exec: E,
    within_secs: f64,
) -> Result<Vec<HostRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT instance_id, version, types, last_seen_at FROM connector_hosts
         WHERE last_seen_at > now() - make_interval(secs => $1)
         ORDER BY last_seen_at DESC, instance_id",
    )
    .bind(within_secs)
    .fetch_all(exec)
    .await?)
}

/// Deletes hosts not seen for `older_than_secs`; returns the number removed.
pub async fn prune<'e, E: PgExecutor<'e>>(exec: E, older_than_secs: f64) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM connector_hosts WHERE last_seen_at < now() - make_interval(secs => $1)",
    )
    .bind(older_than_secs)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Replaces the status reported by `host` (C-457): upserts every reported managed connector and
/// removes the host's rows for connectors it no longer reports. Unknown or non-managed ids are
/// ignored. `reports` = `(connector id, state, message)`.
pub async fn replace_status<'e, E: PgExecutor<'e> + Copy>(
    exec: E,
    host: Uuid,
    reports: &[(String, String, Option<String>)],
    bound_types: Option<&[String]>,
) -> Result<(), DbError> {
    let ids: Vec<String> = reports.iter().map(|r| r.0.clone()).collect();
    let states: Vec<String> = reports.iter().map(|r| r.1.clone()).collect();
    let messages: Vec<Option<String>> = reports.iter().map(|r| r.2.clone()).collect();
    // (FIX-120) A bound token reports only connectors of its own types.
    sqlx::query(
        "INSERT INTO connector_host_status (connector_id, host_instance, state, message, updated_at)
         SELECT r.id, $1, r.state, r.message, now()
         FROM unnest($2::text[], $3::text[], $4::text[]) AS r(id, state, message)
         JOIN connectors c ON c.id = r.id AND c.managed
              AND ($5::text[] IS NULL OR coalesce(c.type_id, c.id) = ANY($5))
         ON CONFLICT (connector_id, host_instance) DO UPDATE
           SET state = EXCLUDED.state, message = EXCLUDED.message, updated_at = now()",
    )
    .bind(host)
    .bind(&ids)
    .bind(&states)
    .bind(&messages)
    .bind(bound_types)
    .execute(exec)
    .await?;
    sqlx::query(
        "DELETE FROM connector_host_status WHERE host_instance = $1 AND NOT (connector_id = ANY($2))",
    )
    .bind(host)
    .bind(&ids)
    .execute(exec)
    .await?;
    Ok(())
}

/// Status reports of a connector from hosts seen within `within_secs`.
pub async fn list_status<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    within_secs: f64,
) -> Result<Vec<StatusRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.connector_id, s.host_instance, s.state, s.message, s.updated_at
         FROM connector_host_status s JOIN connector_hosts h ON h.instance_id = s.host_instance
         WHERE s.connector_id = $1 AND h.last_seen_at > now() - make_interval(secs => $2)
         ORDER BY s.updated_at DESC, s.host_instance",
    )
    .bind(connector_id)
    .bind(within_secs)
    .fetch_all(exec)
    .await?)
}

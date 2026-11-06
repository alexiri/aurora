//! Channel integrations: status, ownership leases and the connector config feed
//! (docs/13 §2, §4; schema `channel_integrations`, `channel_presences`, `space_bindings`).
//!
//! `credentials_ref` values are sealed secret references; nothing here decrypts them.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Clone, sqlx::FromRow)]
pub struct IntegrationRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub connector_id: String,
    pub tenant_ref: String,
    pub tenant_label: String,
    pub config: serde_json::Value,
    /// Sealed reference (`aurora_secrets::SecretRef` text), never plaintext.
    pub credentials_ref: String,
    pub credentials_version: i64,
    pub shared_bot: Option<serde_json::Value>,
    pub status: String,
    pub status_reason: Option<String>,
    pub owner_replica_id: Option<Uuid>,
    pub owner_lease_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for IntegrationRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntegrationRow")
            .field("id", &self.id)
            .field("connector_id", &self.connector_id)
            .field("tenant_ref", &self.tenant_ref)
            .field("status", &self.status)
            .field("credentials_ref", &"[sealed]")
            .finish_non_exhaustive()
    }
}

const COLS: &str = "id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, \
    credentials_version, shared_bot, status, status_reason, owner_replica_id, owner_lease_until, \
    created_at, updated_at";

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<IntegrationRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM channel_integrations WHERE id=$1"
    ))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Number of integrations of the connector (any status).
pub async fn count_for_connector<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM channel_integrations WHERE connector_id=$1")
            .bind(connector_id)
            .fetch_one(exec)
            .await?,
    )
}

/// Integration counts per connector id.
pub async fn counts_by_connector<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT connector_id, count(*) FROM channel_integrations GROUP BY connector_id",
    )
    .fetch_all(exec)
    .await?)
}

/// Sets status and reason (for later tasks: validation outcome, needs-attention).
pub async fn set_status<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    status: &str,
    reason: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_integrations SET status=$2, status_reason=$3, updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(status)
    .bind(reason)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// The integration when `replica` currently owns it (lease not required to be live: the
/// owner may write back a rotated credential just as its lease lapses; any takeover has
/// already changed `owner_replica_id`).
pub async fn get_owned<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
    id: Uuid,
) -> Result<Option<IntegrationRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM channel_integrations
         WHERE id=$1 AND connector_id=$2 AND owner_replica_id=$3"
    ))
    .bind(id)
    .bind(connector_id)
    .bind(replica)
    .fetch_optional(exec)
    .await?)
}

/// Compare-and-set of the integration credential. Returns `(new_version, old_ref)` or `None`
/// when `expected_version` no longer matches or `replica` no longer owns the row.
pub async fn cas_credentials<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
    id: Uuid,
    expected_version: i64,
    new_ref: &str,
) -> Result<Option<(i64, String)>, DbError> {
    // The sub-select reads the pre-update row, so `old_ref` is the replaced reference.
    Ok(sqlx::query_as(
        "UPDATE channel_integrations ci
            SET credentials_ref=$5, credentials_version=ci.credentials_version+1, updated_at=now()
           FROM (SELECT id, credentials_ref FROM channel_integrations WHERE id=$1 FOR UPDATE) old
          WHERE ci.id=old.id AND ci.connector_id=$2 AND ci.owner_replica_id=$3
            AND ci.credentials_version=$4
        RETURNING ci.credentials_version, old.credentials_ref",
    )
    .bind(id)
    .bind(connector_id)
    .bind(replica)
    .bind(expected_version)
    .bind(new_ref)
    .fetch_optional(exec)
    .await?)
}

/// Compare-and-set of a dedicated presence credential; same contract as
/// [`cas_credentials`], scoped through the presence's integration.
pub async fn cas_presence_credentials<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
    id: Uuid,
    expected_version: i64,
    new_ref: &str,
) -> Result<Option<(i64, Option<String>)>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE channel_presences p
            SET credentials_ref=$5, credentials_version=p.credentials_version+1, updated_at=now()
           FROM (SELECT id, credentials_ref FROM channel_presences WHERE id=$1 FOR UPDATE) old,
                channel_integrations ci
          WHERE p.id=old.id AND ci.id=p.integration_id AND ci.connector_id=$2
            AND ci.owner_replica_id=$3 AND p.credentials_version=$4
        RETURNING p.credentials_version, old.credentials_ref",
    )
    .bind(id)
    .bind(connector_id)
    .bind(replica)
    .bind(expected_version)
    .bind(new_ref)
    .fetch_optional(exec)
    .await?)
}

/// A presence of an integration the replica owns (for credential write-back).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OwnedPresence {
    pub id: Uuid,
    pub org_id: Uuid,
    pub credentials_version: i64,
}

pub async fn get_owned_presence<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
    id: Uuid,
) -> Result<Option<OwnedPresence>, DbError> {
    Ok(sqlx::query_as(
        "SELECT p.id, ci.org_id, p.credentials_version
           FROM channel_presences p JOIN channel_integrations ci ON ci.id=p.integration_id
          WHERE p.id=$1 AND ci.connector_id=$2 AND ci.owner_replica_id=$3",
    )
    .bind(id)
    .bind(connector_id)
    .bind(replica)
    .fetch_optional(exec)
    .await?)
}

/// Releases every lease held by `replica` (clean retire).
pub async fn release_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_integrations SET owner_replica_id=NULL, owner_lease_until=NULL
         WHERE connector_id=$1 AND owner_replica_id=$2",
    )
    .bind(connector_id)
    .bind(replica)
    .execute(exec)
    .await?
    .rows_affected())
}

/// One config poll's ownership step, run inside the caller's transaction.
///
/// 1. renews this replica's leases;
/// 2. computes `quota = ceil(active integrations / live replicas)`;
/// 3. releases owned integrations above the quota (so a newly started replica can take them);
/// 4. takes lapsed or unowned active integrations up to the quota.
///
/// Returns the active integrations owned by `replica` afterwards, in a stable order.
pub async fn claim_owned(
    conn: &mut PgConnection,
    connector_id: &str,
    replica: Uuid,
    lease_secs: f64,
    live_within_secs: f64,
) -> Result<Vec<IntegrationRow>, DbError> {
    sqlx::query(
        "UPDATE channel_integrations
            SET owner_lease_until = now() + make_interval(secs => $3)
          WHERE connector_id=$1 AND owner_replica_id=$2",
    )
    .bind(connector_id)
    .bind(replica)
    .bind(lease_secs)
    .execute(&mut *conn)
    .await?;

    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connector_replicas
         WHERE connector_id=$1 AND retired_at IS NULL
           AND last_seen_at > now() - make_interval(secs => $2)",
    )
    .bind(connector_id)
    .bind(live_within_secs)
    .fetch_one(&mut *conn)
    .await?;
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channel_integrations WHERE connector_id=$1 AND status='active'",
    )
    .bind(connector_id)
    .fetch_one(&mut *conn)
    .await?;
    let live = live.max(1);
    let quota = (active + live - 1) / live;

    let owned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channel_integrations
         WHERE connector_id=$1 AND owner_replica_id=$2 AND status='active'",
    )
    .bind(connector_id)
    .bind(replica)
    .fetch_one(&mut *conn)
    .await?;

    if owned > quota {
        sqlx::query(
            "UPDATE channel_integrations SET owner_replica_id=NULL, owner_lease_until=NULL
             WHERE id IN (SELECT id FROM channel_integrations
                          WHERE connector_id=$1 AND owner_replica_id=$2 AND status='active'
                          ORDER BY created_at DESC, id DESC LIMIT $3)",
        )
        .bind(connector_id)
        .bind(replica)
        .bind(owned - quota)
        .execute(&mut *conn)
        .await?;
    } else if owned < quota {
        sqlx::query(
            "UPDATE channel_integrations
                SET owner_replica_id=$2, owner_lease_until = now() + make_interval(secs => $4)
              WHERE id IN (SELECT id FROM channel_integrations
                           WHERE connector_id=$1 AND status='active'
                             AND (owner_replica_id IS NULL OR owner_lease_until IS NULL
                                  OR owner_lease_until < now())
                           ORDER BY created_at, id LIMIT $3 FOR UPDATE SKIP LOCKED)",
        )
        .bind(connector_id)
        .bind(replica)
        .bind(quota - owned)
        .bind(lease_secs)
        .execute(&mut *conn)
        .await?;
    }

    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM channel_integrations
         WHERE connector_id=$1 AND owner_replica_id=$2 AND status='active'
         ORDER BY created_at, id"
    ))
    .bind(connector_id)
    .bind(replica)
    .fetch_all(&mut *conn)
    .await?)
}

/// An active presence with its agent, as served in the config feed.
#[derive(Clone, sqlx::FromRow)]
pub struct FeedPresence {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub org_id: Uuid,
    pub mode: String,
    pub platform_user_ref: Option<String>,
    pub handle: Option<String>,
    pub credentials_ref: Option<String>,
    pub credentials_version: i64,
    pub status: String,
    pub agent_id: Uuid,
    pub agent_handle: String,
    pub agent_name: String,
}

impl std::fmt::Debug for FeedPresence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedPresence")
            .field("id", &self.id)
            .field("integration_id", &self.integration_id)
            .field("mode", &self.mode)
            .field("credentials_ref", &"[sealed]")
            .finish_non_exhaustive()
    }
}

/// `active` presences of the given integrations.
pub async fn active_presences<'e, E: PgExecutor<'e>>(
    exec: E,
    integration_ids: &[Uuid],
) -> Result<Vec<FeedPresence>, DbError> {
    Ok(sqlx::query_as(
        "SELECT p.id, p.integration_id, ci.org_id, p.mode, p.platform_user_ref, p.handle,
                p.credentials_ref, p.credentials_version, p.status,
                a.id AS agent_id, a.handle AS agent_handle, a.name AS agent_name
           FROM channel_presences p
           JOIN channel_integrations ci ON ci.id=p.integration_id
           JOIN agents a ON a.id=p.agent_id
          WHERE p.integration_id = ANY($1) AND p.status='active'
          ORDER BY p.created_at, p.id",
    )
    .bind(integration_ids)
    .fetch_all(exec)
    .await?)
}

/// An active space binding as served in the config feed.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedBinding {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub space_ref: String,
    pub agent_id: Uuid,
    pub listen: String,
    pub followups_need_mention: bool,
}

pub async fn active_bindings<'e, E: PgExecutor<'e>>(
    exec: E,
    integration_ids: &[Uuid],
) -> Result<Vec<FeedBinding>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, integration_id, space_ref, agent_id, listen, followups_need_mention
           FROM space_bindings WHERE integration_id = ANY($1) AND active
          ORDER BY created_at, id",
    )
    .bind(integration_ids)
    .fetch_all(exec)
    .await?)
}

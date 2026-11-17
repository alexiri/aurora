//! SSH sessions, channel grants and terminator replicas (phase 5, P4-SSH-2, D-239, SEC-94,
//! SEC-95; docs/03 §`ssh_sessions`, §`ssh_stream_grants`, §`ssh_terminator_replicas`).
//!
//! A session is alive while `state = 'open'` **and** `lease_until > now()` (liveness = lease).
//! The guarded transition `open -> closed` ([`close_session`]) is the close-exactly-once gate:
//! whichever path wins it (terminator close, heartbeat detection, the sweep) writes the one
//! audit row. Nothing here stores channel bytes, command lines or certificates.
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

use crate::DbError;

macro_rules! session_cols {
    () => {
        "id, user_id, ssh_key_id, key_fingerprint, cert_serial, org_id, workspace_id, agent_id, \
         terminator_replica_id, client_address, state, lease_until, last_activity_at, \
         revoke_requested_at, revoke_reason, closed_at, close_reason, channels_total, bytes_in, \
         bytes_out, created_at"
    };
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SshSessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub ssh_key_id: Option<Uuid>,
    pub key_fingerprint: String,
    pub cert_serial: i64,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub terminator_replica_id: Uuid,
    pub client_address: Option<String>,
    pub state: String,
    pub lease_until: DateTime<Utc>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub revoke_requested_at: Option<DateTime<Utc>>,
    pub revoke_reason: Option<String>,
    pub closed_at: Option<DateTime<Utc>>,
    pub close_reason: Option<String>,
    pub channels_total: i32,
    pub bytes_in: i64,
    pub bytes_out: i64,
    pub created_at: DateTime<Utc>,
}

// ------------------------------------------------------------------------------ replicas

/// Register (or re-register) a replica: a fresh id per process start.
pub async fn insert_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    version: &str,
    host_public_key: &str,
    host_key_fingerprint: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO ssh_terminator_replicas (id, version, host_public_key, host_key_fingerprint) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(version)
    .bind(host_public_key)
    .bind(host_key_fingerprint)
    .execute(exec)
    .await?;
    Ok(())
}

/// Whether the replica is registered (an unknown or pruned replica must register again).
pub async fn replica_exists<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM ssh_terminator_replicas WHERE id = $1)")
            .bind(id)
            .fetch_one(exec)
            .await?,
    )
}

/// A heartbeat: refresh `last_seen_at` and the open-session figure.
pub async fn touch_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    sessions_open: i32,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE ssh_terminator_replicas SET last_seen_at = now(), sessions_open = $2, \
             updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(sessions_open)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReplicaRow {
    pub id: Uuid,
    pub version: String,
    pub host_public_key: String,
    pub host_key_fingerprint: String,
    pub sessions_open: i32,
    pub last_seen_at: DateTime<Utc>,
    pub live: bool,
}

/// Every replica seen in the last 24 h, newest heartbeat first, with `live` computed against
/// the session lease (`C-793` `GET /v1/admin/terminators`).
pub async fn list_replicas<'e, E: PgExecutor<'e>>(
    exec: E,
    lease_secs: i64,
) -> Result<Vec<ReplicaRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, version, host_public_key, host_key_fingerprint, sessions_open, last_seen_at, \
                last_seen_at > now() - make_interval(secs => $1) AS live \
           FROM ssh_terminator_replicas ORDER BY last_seen_at DESC, id",
    )
    .bind(lease_secs as f64)
    .fetch_all(exec)
    .await?)
}

/// `(live replica count, distinct (host public key, fingerprint) pairs of live replicas)`.
pub async fn live_host_keys<'e, E: PgExecutor<'e> + Copy>(
    exec: E,
    lease_secs: i64,
) -> Result<(i64, Vec<(String, String)>), DbError> {
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ssh_terminator_replicas \
          WHERE last_seen_at > now() - make_interval(secs => $1)",
    )
    .bind(lease_secs as f64)
    .fetch_one(exec)
    .await?;
    let keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT host_public_key, host_key_fingerprint FROM ssh_terminator_replicas \
          WHERE last_seen_at > now() - make_interval(secs => $1) \
          ORDER BY host_public_key",
    )
    .bind(lease_secs as f64)
    .fetch_all(exec)
    .await?;
    Ok((live, keys))
}

// ------------------------------------------------------------------------------ sessions

/// Lock the person's open, lease-current sessions and count them (`FOR UPDATE`, so two
/// concurrent opens over different replicas serialise on the caps).
pub async fn lock_count_open_for_user(conn: &mut PgConnection, user: Uuid) -> Result<i64, DbError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM ssh_sessions \
          WHERE user_id = $1 AND state = 'open' AND lease_until > now() ORDER BY id FOR UPDATE",
    )
    .bind(user)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.len() as i64)
}

/// The same for an agent.
pub async fn lock_count_open_for_agent(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<i64, DbError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM ssh_sessions \
          WHERE agent_id = $1 AND state = 'open' AND lease_until > now() ORDER BY id FOR UPDATE",
    )
    .bind(agent)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.len() as i64)
}

/// Insert an open session with its lease.
#[allow(clippy::too_many_arguments)]
pub async fn insert_session<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    user: Uuid,
    key_id: Uuid,
    key_fingerprint: &str,
    cert_serial: i64,
    org: Uuid,
    workspace: Uuid,
    agent: Uuid,
    replica: Uuid,
    client_address: Option<&str>,
    lease_secs: i64,
) -> Result<SshSessionRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO ssh_sessions (id, user_id, ssh_key_id, key_fingerprint, cert_serial, org_id, \
             workspace_id, agent_id, terminator_replica_id, client_address, lease_until) \
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now() + make_interval(secs => $11)) \
          RETURNING ",
        session_cols!()
    ))
    .bind(id)
    .bind(user)
    .bind(key_id)
    .bind(key_fingerprint)
    .bind(cert_serial)
    .bind(org)
    .bind(workspace)
    .bind(agent)
    .bind(replica)
    .bind(client_address)
    .bind(lease_secs as f64)
    .fetch_one(exec)
    .await?)
}

pub async fn get_session<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<SshSessionRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        session_cols!(),
        " FROM ssh_sessions WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// The sessions a heartbeat names (any state), for the revocation decision.
pub async fn get_sessions<'e, E: PgExecutor<'e>>(
    exec: E,
    ids: &[Uuid],
) -> Result<Vec<SshSessionRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        session_cols!(),
        " FROM ssh_sessions WHERE id = ANY($1)"
    ))
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// A heartbeat that keeps the session: renew the lease, stamp the activity and the totals
/// (only ever upwards). `false` when the row is not open any more.
#[allow(clippy::too_many_arguments)]
pub async fn renew_session<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    replica: Uuid,
    lease_secs: i64,
    last_activity_at: Option<DateTime<Utc>>,
    channels_total: i32,
    bytes_in: i64,
    bytes_out: i64,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE ssh_sessions SET lease_until = now() + make_interval(secs => $3), \
             last_activity_at = greatest(last_activity_at, $4), \
             channels_total = greatest(channels_total, $5), bytes_in = greatest(bytes_in, $6), \
             bytes_out = greatest(bytes_out, $7), updated_at = now() \
          WHERE id = $1 AND terminator_replica_id = $2 AND state = 'open'",
    )
    .bind(id)
    .bind(replica)
    .bind(lease_secs as f64)
    .bind(last_activity_at)
    .bind(channels_total)
    .bind(bytes_in)
    .bind(bytes_out)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// The close-exactly-once gate: `open -> closed`. `Some(row)` for the caller that won it (and
/// must write the one audit row), `None` for every later caller. Totals only move upwards.
pub async fn close_session<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    reason: &str,
    channels_total: i32,
    bytes_in: i64,
    bytes_out: i64,
) -> Result<Option<SshSessionRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE ssh_sessions SET state = 'closed', closed_at = now(), close_reason = $2, \
             channels_total = greatest(channels_total, $3), bytes_in = greatest(bytes_in, $4), \
             bytes_out = greatest(bytes_out, $5), updated_at = now() \
          WHERE id = $1 AND state = 'open' RETURNING ",
        session_cols!()
    ))
    .bind(id)
    .bind(reason)
    .bind(channels_total)
    .bind(bytes_in)
    .bind(bytes_out)
    .fetch_optional(exec)
    .await?)
}

/// Close the lease-expired open rows of this sweep tick (`SKIP LOCKED`, so replicas share the
/// work); each returned row is a transition this caller won.
pub async fn sweep_expired<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<SshSessionRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "WITH pick AS (SELECT id FROM ssh_sessions WHERE state = 'open' AND lease_until < now() \
              ORDER BY lease_until LIMIT $1 FOR UPDATE SKIP LOCKED) \
         UPDATE ssh_sessions s SET state = 'closed', closed_at = now(), \
             close_reason = 'lease_expired', updated_at = now() \
          FROM pick WHERE s.id = pick.id AND s.state = 'open' RETURNING ",
        "s.id, s.user_id, s.ssh_key_id, s.key_fingerprint, s.cert_serial, s.org_id, \
         s.workspace_id, s.agent_id, s.terminator_replica_id, s.client_address, s.state, \
         s.lease_until, s.last_activity_at, s.revoke_requested_at, s.revoke_reason, s.closed_at, \
         s.close_reason, s.channels_total, s.bytes_in, s.bytes_out, s.created_at"
    ))
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// `DELETE /v1/ssh-sessions/{id}`: ask the terminator to drop the session at its next heartbeat.
pub async fn request_revoke<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    reason: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE ssh_sessions SET revoke_requested_at = coalesce(revoke_requested_at, now()), \
             revoke_reason = coalesce(revoke_reason, $2), updated_at = now() \
          WHERE id = $1 AND state = 'open'",
    )
    .bind(id)
    .bind(reason)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// A session with the names the dashboard shows (`SshSession`, C-793).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionListRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub user_label: Option<String>,
    pub ssh_key_id: Option<Uuid>,
    pub key_name: Option<String>,
    pub key_fingerprint: String,
    pub client_address: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub channels_total: i32,
}

macro_rules! list_select {
    () => {
        "SELECT s.id, s.agent_id, a.name AS agent_name, s.workspace_id, s.user_id, \
                u.display_name AS user_label, s.ssh_key_id, k.name AS key_name, \
                s.key_fingerprint, s.client_address, s.created_at, s.last_activity_at, \
                s.channels_total \
           FROM ssh_sessions s JOIN agents a ON a.id = s.agent_id \
           JOIN users u ON u.id = s.user_id LEFT JOIN ssh_keys k ON k.id = s.ssh_key_id \
          WHERE s.state = 'open' AND s.lease_until > now() "
    };
}

pub async fn list_open_for_user<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Vec<SessionListRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        list_select!(),
        "AND s.user_id = $1 ORDER BY s.created_at DESC, s.id"
    ))
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// Open sessions on an agent; `only_user` restricts to one person's (a `can_use` member).
pub async fn list_open_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    only_user: Option<Uuid>,
) -> Result<Vec<SessionListRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        list_select!(),
        "AND s.agent_id = $1 AND ($2::uuid IS NULL OR s.user_id = $2) \
         ORDER BY s.created_at DESC, s.id"
    ))
    .bind(agent)
    .bind(only_user)
    .fetch_all(exec)
    .await?)
}

/// Number of open, lease-current sessions across the instance (Admin › Overview, C-795).
pub async fn count_open<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM ssh_sessions WHERE state = 'open' AND lease_until > now()",
    )
    .fetch_one(exec)
    .await?)
}

/// Retention: closed rows older than `retention_secs`, grants and replicas older than 24 h.
pub async fn prune<'e, E: PgExecutor<'e> + Copy>(
    exec: E,
    retention_secs: i64,
) -> Result<u64, DbError> {
    let mut n = sqlx::query(
        "DELETE FROM ssh_sessions WHERE state = 'closed' \
           AND closed_at < now() - make_interval(secs => $1)",
    )
    .bind(retention_secs as f64)
    .execute(exec)
    .await?
    .rows_affected();
    n +=
        sqlx::query("DELETE FROM ssh_stream_grants WHERE expires_at < now() - interval '24 hours'")
            .execute(exec)
            .await?
            .rows_affected();
    n += sqlx::query(
        "DELETE FROM ssh_terminator_replicas WHERE last_seen_at < now() - interval '24 hours'",
    )
    .execute(exec)
    .await?
    .rows_affected();
    Ok(n)
}

// ------------------------------------------------------------------------------ grants

#[allow(clippy::too_many_arguments)]
pub async fn insert_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    session: Uuid,
    agent: Uuid,
    sandbox: Uuid,
    boot: Uuid,
    grant_hash: &str,
    mode: &str,
    ttl_secs: i64,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO ssh_stream_grants \
             (id, session_id, agent_id, sandbox_id, boot_id, grant_hash, mode, expires_at) \
          VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(secs => $8))",
    )
    .bind(id)
    .bind(session)
    .bind(agent)
    .bind(sandbox)
    .bind(boot)
    .bind(grant_hash)
    .bind(mode)
    .bind(ttl_secs as f64)
    .execute(exec)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SshGrantRow {
    pub id: Uuid,
    pub session_id: Uuid,
    pub agent_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub mode: String,
}

/// Consume one grant exactly once, and only while its session is open and lease-current:
/// `None` when unknown, expired, already consumed or the session is gone (the redemption is
/// refused whichever it is: one answer, no oracle).
pub async fn consume_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    grant_hash: &str,
) -> Result<Option<SshGrantRow>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE ssh_stream_grants g SET consumed_at = now() \
           FROM ssh_sessions s \
          WHERE g.grant_hash = $1 AND g.consumed_at IS NULL AND g.expires_at > now() \
            AND s.id = g.session_id AND s.state = 'open' AND s.lease_until > now() \
          RETURNING g.id, g.session_id, g.agent_id, g.sandbox_id, g.boot_id, g.mode",
    )
    .bind(grant_hash)
    .fetch_optional(exec)
    .await?)
}

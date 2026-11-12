//! Preview access and relay-plane storage (phase 4, P4-PRV-2, D-146, D-147, SEC-81, SEC-82;
//! docs/03 §`preview_sessions`…§`relay_replicas`, docs/04 §Preview access, docs/16 §W17).
//!
//! This module only stores and reads; authority (the resolve decision, the grant seal, the
//! session checks) lives in the API service. Tokens are stored hashed; the `auv_` value is
//! returned once and never stored.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

// ------------------------------------------------------------ preview sessions

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PreviewSessionRow {
    pub id: Uuid,
    pub token_hash: Vec<u8>,
    pub display_prefix: String,
    pub share_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Create the preview session row for a redeemed grant (docs/04 step 3). The `expires_at`
/// is capped by the dashboard session's own absolute expiry; the caller computes it.
#[allow(clippy::too_many_arguments)]
pub async fn insert_preview_session<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    token_hash: &[u8],
    display_prefix: &str,
    share: Uuid,
    user: Uuid,
    session: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO preview_sessions \
             (id, token_hash, display_prefix, share_id, user_id, session_id, expires_at) \
          VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(token_hash)
    .bind(display_prefix)
    .bind(share)
    .bind(user)
    .bind(session)
    .bind(expires_at)
    .execute(exec)
    .await?;
    Ok(())
}

/// The live preview session of a token hash (used by `resolve`): dead when past `expires_at`
/// or idle past `PREVIEW_SESSION_IDLE`; the caller recomputes the access decision each time.
pub async fn find_live_preview_session<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
    idle_secs: i64,
) -> Result<Option<PreviewSessionRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, token_hash, display_prefix, share_id, user_id, session_id, \
                created_at, last_seen_at, expires_at \
           FROM preview_sessions \
          WHERE token_hash = $1 \
            AND expires_at > now() \
            AND last_seen_at > now() - make_interval(secs => $2)",
    )
    .bind(token_hash)
    .bind(idle_secs)
    .fetch_optional(exec)
    .await?)
}

/// Advance `last_seen_at` at most once a minute (relay resolve/report volume).
pub async fn touch_preview_session<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE preview_sessions SET last_seen_at = now() \
          WHERE id = $1 AND last_seen_at < now() - interval '60 seconds'",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete every preview session of a dashboard session (sign-out / revoke; also enforced by
/// the FK cascade — this is for explicit sweeps and tests).
pub async fn delete_preview_sessions_of_session<'e, E: PgExecutor<'e>>(
    exec: E,
    session: Uuid,
) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM preview_sessions WHERE session_id = $1")
            .bind(session)
            .execute(exec)
            .await?
            .rows_affected(),
    )
}

// ------------------------------------------------------------ grant ledgers

/// Single-use claim of a browser authorization grant: insert-on-conflict into
/// `preview_grant_uses` (`jti`); `false` means the grant was already used (400 `grant_invalid`).
pub async fn claim_grant_use<'e, E: PgExecutor<'e>>(
    exec: E,
    jti: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "INSERT INTO preview_grant_uses (jti, expires_at) VALUES ($1, $2) ON CONFLICT (jti) DO NOTHING",
    )
    .bind(jti)
    .bind(expires_at)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Mint a stream grant (`aux_`): insert the binding; consumed by one compare-and-set.
#[allow(clippy::too_many_arguments)]
pub async fn insert_stream_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
    share: Uuid,
    sandbox: Uuid,
    boot: Uuid,
    port: i32,
    session_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO preview_stream_grants \
             (token_hash, share_id, sandbox_id, boot_id, port, session_hash, expires_at) \
          VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(token_hash)
    .bind(share)
    .bind(sandbox)
    .bind(boot)
    .bind(port)
    .bind(session_hash)
    .bind(expires_at)
    .execute(exec)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StreamGrantRow {
    pub token_hash: Vec<u8>,
    pub share_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub port: i32,
    pub session_hash: Vec<u8>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

/// The runner's redemption (`POST /v1/runner/preview-streams/open`): one compare-and-set marks
/// the grant consumed; `None` when unknown, expired or already consumed (403 `grant_invalid`).
pub async fn consume_stream_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<StreamGrantRow>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE preview_stream_grants SET consumed_at = now() \
          WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now() \
          RETURNING token_hash, share_id, sandbox_id, boot_id, port, session_hash, \
                    expires_at, consumed_at",
    )
    .bind(token_hash)
    .fetch_optional(exec)
    .await?)
}

// ------------------------------------------------------------ relay plane

/// Bootstrap upsert of the relay anchor token hash (`AURORA_BOOTSTRAP_RELAY_TOKEN`). Idempotent.
pub async fn upsert_relay_token<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Uuid, DbError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO relay_tokens (id, token_hash) VALUES ($1, $2) \
         ON CONFLICT (token_hash) DO UPDATE SET token_hash = EXCLUDED.token_hash \
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(token_hash)
    .fetch_one(exec)
    .await?;
    Ok(id)
}

pub async fn find_relay_token<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM relay_tokens WHERE token_hash = $1")
            .bind(token_hash)
            .fetch_optional(exec)
            .await?,
    )
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RelayReplicaRow {
    pub id: Uuid,
    pub token_id: Uuid,
    pub version: String,
    pub streams_open: i32,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Register (or re-register) a relay replica; idempotent per replica id.
pub async fn upsert_relay_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    token_id: Uuid,
    version: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO relay_replicas (id, token_id, version) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO UPDATE \
           SET token_id = EXCLUDED.token_id, version = EXCLUDED.version, last_seen_at = now()",
    )
    .bind(id)
    .bind(token_id)
    .bind(version)
    .execute(exec)
    .await?;
    Ok(())
}

/// The relay replica must belong to the presented token (the `RelayCtx` extractor's check).
pub async fn relay_replica_owned<'e, E: PgExecutor<'e>>(
    exec: E,
    replica: Uuid,
    token_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM relay_replicas WHERE id = $1 AND token_id = $2)",
    )
    .bind(replica)
    .bind(token_id)
    .fetch_one(exec)
    .await?)
}

/// Heartbeat (`POST /v1/relay/report`): refresh the replica's liveness and stream gauge.
pub async fn touch_relay_replica<'e, E: PgExecutor<'e>>(
    exec: E,
    replica: Uuid,
    streams_open: i32,
) -> Result<(), DbError> {
    sqlx::query("UPDATE relay_replicas SET last_seen_at = now(), streams_open = $2 WHERE id = $1")
        .bind(replica)
        .bind(streams_open)
        .execute(exec)
        .await?;
    Ok(())
}

/// Relay replicas seen in the last 15 minutes (docs/14 Admin → Overview "Previews" line).
pub async fn list_relay_replicas<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<RelayReplicaRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, token_id, version, streams_open, last_seen_at, created_at \
           FROM relay_replicas \
          WHERE last_seen_at > now() - interval '15 minutes' \
          ORDER BY last_seen_at DESC",
    )
    .fetch_all(exec)
    .await?)
}

/// Prune replicas not seen for 24 h (the preview housekeeping job, docs/05).
pub async fn prune_stale_relay_replicas<'e, E: PgExecutor<'e>>(exec: E) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM relay_replicas WHERE last_seen_at < now() - interval '24 hours'")
            .execute(exec)
            .await?
            .rows_affected(),
    )
}

/// Housekeeping: expired grants and dead sessions (every 60 s, docs/05 §Background jobs).
pub async fn prune_preview_expiries<'e, E: PgExecutor<'e> + Copy>(exec: E) -> Result<u64, DbError> {
    let g = sqlx::query("DELETE FROM preview_grant_uses WHERE expires_at < now()")
        .execute(exec)
        .await?
        .rows_affected();
    let s = sqlx::query("DELETE FROM preview_stream_grants WHERE expires_at < now()")
        .execute(exec)
        .await?
        .rows_affected();
    let dead = sqlx::query(
        "DELETE FROM preview_sessions \
          WHERE expires_at < now() \
             OR last_seen_at < now() - interval '1 hour'",
    )
    .execute(exec)
    .await?
    .rows_affected();
    Ok(g + s + dead)
}

// ------------------------------------------------------------ accounting

/// `preview.session.create` is best-effort (docs/04 §Audit); the row itself is the record.
/// Stream accounting totals per share go to metrics only — the relay reports them and the API
/// writes nothing per request beyond `last_accessed_at` / `preview_active_until`.
pub async fn keep_preview_awake<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE sandboxes SET preview_active_until = now() + make_interval(secs => $2), \
             preview_awake_since = coalesce(preview_awake_since, now()), updated_at = now() \
          WHERE agent_id = $1 AND state IS NOT NULL",
    )
    .bind(agent)
    .bind(120i64)
    .execute(exec)
    .await?;
    Ok(())
}

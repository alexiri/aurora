//! Gateway credentials for agents (`agent` and BYO `context` kinds).

use crate::DbError;
use crate::bus::{self, BusError, Channel};
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Inserts the agent's `aug_` credential. `max_age_days` (RFE-013, C-311): the effective maximum
/// age, `expires_at = now() + max age` on the database clock; `None` = no expiry. Returns the id
/// and the stored `expires_at`.
pub async fn insert_agent_credential<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    token_hash: &[u8],
    display_prefix: &str,
    max_age_days: Option<u32>,
) -> Result<(Uuid, Option<DateTime<Utc>>), DbError> {
    let id = Uuid::now_v7();
    let exp: Option<DateTime<Utc>> = sqlx::query_scalar(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, expires_at)
         VALUES ($1,'agent',$2,$3,$4, now() + make_interval(days => $5))
         RETURNING expires_at",
    )
    .bind(id)
    .bind(token_hash)
    .bind(display_prefix)
    .bind(agent_id)
    .bind(max_age_days.map(|d| d as i32))
    .fetch_one(exec)
    .await?;
    Ok((id, exp))
}

/// (RFE-013, C-315) Shortens the live `agent` credentials of an org's agents to
/// `least(expires_at, greatest(created_at + max age, now() + grace))` (a null `expires_at` counts
/// as infinity); never extends one. Returns the number of credentials changed.
pub async fn retrofit_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    max_age_days: u32,
    grace_days: i32,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE gateway_credentials c
            SET expires_at = least(coalesce(c.expires_at, 'infinity'::timestamptz),
                                   greatest(c.created_at + make_interval(days => $2),
                                            now() + make_interval(days => $3))),
                updated_at = now()
           FROM agents a
          WHERE a.id = c.agent_id AND a.org_id = $1
            AND c.kind = 'agent' AND c.revoked_at IS NULL
            AND least(coalesce(c.expires_at, 'infinity'::timestamptz),
                      greatest(c.created_at + make_interval(days => $2),
                               now() + make_interval(days => $3)))
                <> coalesce(c.expires_at, 'infinity'::timestamptz)",
    )
    .bind(org)
    .bind(max_age_days as i32)
    .bind(grace_days)
    .execute(exec)
    .await?
    .rows_affected())
}

/// (RFE-013, C-315) The instance maximum applied at API start: every live `agent` credential is
/// shortened by the smaller of `instance_days` and its org's own value, with the grace.
/// Returns the number changed and the orgs they belong to.
pub async fn retrofit_instance<'e, E: PgExecutor<'e>>(
    exec: E,
    instance_days: u32,
    grace_days: i32,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "WITH eff AS (
            SELECT c.id, a.org_id,
                   least(coalesce(c.expires_at, 'infinity'::timestamptz),
                         greatest(c.created_at + make_interval(days => least($1,
                                    CASE WHEN o.governance #>> '{agent_tokens,max_age_days}' ~ '^[0-9]{1,4}$'
                                         THEN (o.governance #>> '{agent_tokens,max_age_days}')::int
                                         ELSE $1 END)),
                                  now() + make_interval(days => $2))) AS new_exp
              FROM gateway_credentials c
              JOIN agents a ON a.id = c.agent_id
              JOIN organizations o ON o.id = a.org_id
             WHERE c.kind = 'agent' AND c.revoked_at IS NULL)
         UPDATE gateway_credentials c
            SET expires_at = eff.new_exp, updated_at = now()
           FROM eff
          WHERE eff.id = c.id
            AND eff.new_exp <> coalesce(c.expires_at, 'infinity'::timestamptz)
      RETURNING eff.org_id",
    )
    .bind(instance_days as i32)
    .bind(grace_days)
    .fetch_all(exec)
    .await?)
}

/// A live BYO `agent` credential with its agent and workspace (C-313, C-314).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentTokenRow {
    pub credential_id: Uuid,
    pub agent_id: Uuid,
    pub agent_handle: String,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub display_prefix: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// `coalesce(last_used_at, created_at) < now() - AGENT_TOKEN_STALE_DAYS`.
    pub stale: bool,
    /// `expires_at <= now() + AGENT_TOKEN_EXPIRING_DAYS` (so also once expired).
    pub expiring: bool,
}

const TOKEN_SELECT: &str = "SELECT c.id AS credential_id, c.agent_id, a.handle AS agent_handle,
        a.name AS agent_name, a.workspace_id, w.slug AS workspace_slug, w.name AS workspace_name,
        c.display_prefix, c.created_at, c.expires_at, c.last_used_at,
        (coalesce(c.last_used_at, c.created_at) < now() - make_interval(days => $1)) AS stale,
        (c.expires_at IS NOT NULL AND c.expires_at <= now() + make_interval(days => $2)) AS expiring
   FROM gateway_credentials c
   JOIN agents a ON a.id = c.agent_id
   JOIN workspaces w ON w.id = a.workspace_id
  WHERE c.kind = 'agent' AND c.revoked_at IS NULL AND a.kind = 'byo'";

/// The live agent tokens in scope, newest first (C-313). `workspace` / `org` narrow the scope
/// (give exactly one); `stale` and `expiring_within_days` are ANDed filters.
#[allow(clippy::too_many_arguments)]
pub async fn list_agent_tokens<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Option<Uuid>,
    org: Option<Uuid>,
    stale: Option<bool>,
    expiring_within_days: Option<i64>,
    after: super::After,
    fetch: i64,
) -> Result<Vec<AgentTokenRow>, DbError> {
    let (t, id) = super::split(after);
    Ok(sqlx::query_as(&format!(
        "{TOKEN_SELECT}
           AND ($3::uuid IS NULL OR a.workspace_id = $3)
           AND ($4::uuid IS NULL OR w.org_id = $4)
           AND ($5::bool IS NULL OR (coalesce(c.last_used_at, c.created_at)
                                     < now() - make_interval(days => $1)) = $5)
           AND ($6::bigint IS NULL OR (c.expires_at IS NOT NULL
                                       AND c.expires_at <= now() + make_interval(days => $6::int)))
           AND ($7::timestamptz IS NULL OR (c.created_at, c.id) < ($7, $8))
         ORDER BY c.created_at DESC, c.id DESC LIMIT $9"
    ))
    .bind(aurora_core::limits::AGENT_TOKEN_STALE_DAYS as i32)
    .bind(aurora_core::limits::AGENT_TOKEN_EXPIRING_DAYS as i32)
    .bind(workspace)
    .bind(org)
    .bind(stale)
    .bind(expiring_within_days)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// The live agent token of each given BYO agent (`Agent.byo.token`, C-314).
pub async fn agent_tokens_of<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_ids: &[Uuid],
) -> Result<Vec<AgentTokenRow>, DbError> {
    if agent_ids.is_empty() {
        return Ok(vec![]);
    }
    Ok(
        sqlx::query_as(&format!("{TOKEN_SELECT} AND c.agent_id = ANY($3)"))
            .bind(aurora_core::limits::AGENT_TOKEN_STALE_DAYS as i32)
            .bind(aurora_core::limits::AGENT_TOKEN_EXPIRING_DAYS as i32)
            .bind(agent_ids)
            .fetch_all(exec)
            .await?,
    )
}

/// (RFE-013, C-316) The API's own use record on the agent-token routes: at most once a minute.
pub async fn touch_last_used<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE gateway_credentials SET last_used_at = now()
          WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn insert_context_credential<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    conversation_id: Option<Uuid>,
    token_hash: &[u8],
    display_prefix: &str,
    label: &str,
    ttl_secs: i64,
) -> Result<(Uuid, DateTime<Utc>), DbError> {
    let id = Uuid::now_v7();
    let exp: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO gateway_credentials
            (id, kind, token_hash, display_prefix, agent_id, conversation_id, context_label, expires_at)
         VALUES ($1,'context',$2,$3,$4,$5,$6,
                 -- (RFE-013, C-311) never outlives the agent token that minted it (`least`
                 -- ignores a null, i.e. a token without an expiry).
                 least(now() + make_interval(secs => $7),
                       (SELECT t.expires_at FROM gateway_credentials t
                         WHERE t.agent_id = $4 AND t.kind = 'agent' AND t.revoked_at IS NULL)))
         RETURNING expires_at",
    )
    .bind(id)
    .bind(token_hash)
    .bind(display_prefix)
    .bind(agent_id)
    .bind(conversation_id)
    .bind(label)
    .bind(ttl_secs as f64)
    .fetch_one(exec)
    .await?;
    Ok((id, exp))
}

/// Revoke live credentials of the agent: `agent`+`context` kinds, or all kinds.
pub async fn revoke_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    all_kinds: bool,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE gateway_credentials SET revoked_at = now(), updated_at = now()
         WHERE agent_id = $1 AND revoked_at IS NULL AND ($2 OR kind IN ('agent','context'))",
    )
    .bind(agent_id)
    .bind(all_kinds)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Conversation belongs to the agent.
pub async fn conversation_of_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
    agent_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND agent_id = $2)",
    )
    .bind(conversation)
    .bind(agent_id)
    .fetch_one(exec)
    .await?)
}

/// `aurora_gateway_invalidate {scope, id}` (delivered on commit inside a tx).
pub async fn notify_gateway_invalidate<'e, E: PgExecutor<'e>>(
    exec: E,
    scope: &str,
    id: Uuid,
) -> Result<(), DbError> {
    bus::notify(
        exec,
        Channel::GatewayInvalidate,
        &serde_json::json!({"scope": scope, "id": id}),
    )
    .await
    .map_err(|e| match e {
        BusError::Db(e) => e.into(),
        other => DbError::Other(sqlx::Error::Protocol(other.to_string())),
    })
}

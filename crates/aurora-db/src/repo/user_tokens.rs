//! A person's chat-platform tokens and their grants to agents (docs/13 §5.11; schema `user_tokens`,
//! `user_token_agents`; P4-UTOK, D-281..D-284). The sealed value itself lives in the secret store:
//! `token_ref` is set exactly while the token is `active`. Every end of standing goes through
//! [`end_token`] and the cancel functions here, inside one transaction of the caller.

use crate::DbError;
use crate::repo::channel_ops;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserTokenRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub integration_id: Uuid,
    pub platform_user_ref: String,
    pub platform_user_label: String,
    pub label: Option<String>,
    /// Set exactly while `status = 'active'`; never returned by the API.
    pub token_ref: Option<String>,
    pub status: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub connector_id: String,
    pub org_id: Uuid,
}

const SELECT: &str = "SELECT t.id, t.user_id, t.integration_id, t.platform_user_ref, \
    t.platform_user_label, t.label, t.token_ref, t.status, t.last_used_at, t.revoked_at, \
    t.created_at, i.connector_id, i.org_id \
    FROM user_tokens t JOIN channel_integrations i ON i.id = t.integration_id";

pub struct NewUserToken<'a> {
    pub id: Uuid,
    pub user_id: Uuid,
    pub integration_id: Uuid,
    pub platform_user_ref: &'a str,
    pub platform_user_label: &'a str,
    pub label: Option<&'a str>,
    pub token_ref: &'a str,
}

/// Serialize a person's token adds and return how many tokens they keep (`active` or `invalid`).
pub async fn lock_and_count_live(conn: &mut PgConnection, user: Uuid) -> Result<i64, DbError> {
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM user_tokens WHERE user_id = $1 AND status <> 'revoked'",
    )
    .bind(user)
    .fetch_one(&mut *conn)
    .await?)
}

/// Insert an `active` token. A second live token for the same platform user is
/// [`DbError::UniqueViolation`] on `user_tokens_live_per_platform_user_idx`.
pub async fn insert(
    conn: &mut PgConnection,
    n: &NewUserToken<'_>,
) -> Result<UserTokenRow, DbError> {
    sqlx::query(
        "INSERT INTO user_tokens (id, user_id, integration_id, platform_user_ref,
            platform_user_label, label, token_ref, status)
         VALUES ($1,$2,$3,$4,$5,$6,$7,'active')",
    )
    .bind(n.id)
    .bind(n.user_id)
    .bind(n.integration_id)
    .bind(n.platform_user_ref)
    .bind(n.platform_user_label)
    .bind(n.label)
    .bind(n.token_ref)
    .execute(&mut *conn)
    .await?;
    get(conn, n.id).await?.ok_or(DbError::NotFound)
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<UserTokenRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1")))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// The caller's own token (404 for anyone else's), row-locked.
pub async fn get_own_for_update(
    conn: &mut PgConnection,
    user: Uuid,
    id: Uuid,
) -> Result<Option<UserTokenRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.id = $1 AND t.user_id = $2 AND t.status <> 'revoked' FOR UPDATE OF t"
    )))
    .bind(id)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

pub async fn get_own<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    id: Uuid,
) -> Result<Option<UserTokenRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.id = $1 AND t.user_id = $2 AND t.status <> 'revoked'"
    )))
    .bind(id)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// The caller's tokens that are still standing (`active` or `invalid`), oldest first.
pub async fn list_for_user<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Vec<UserTokenRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.user_id = $1 AND t.status <> 'revoked' ORDER BY t.created_at, t.id"
    )))
    .bind(user)
    .fetch_all(exec)
    .await?)
}

pub async fn set_label<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    label: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE user_tokens SET label = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(label)
        .execute(exec)
        .await?;
    Ok(())
}

/// Stamp `last_used_at` (a send was claimed).
pub async fn touch_last_used<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE user_tokens SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(())
}

// ------------------------------------------------------------------ grants

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GrantRow {
    pub user_token_id: Uuid,
    pub agent_id: Uuid,
    pub agent_name: String,
}

/// Grants of several tokens, with the agents' names.
pub async fn grants_for<'e, E: PgExecutor<'e>>(
    exec: E,
    tokens: &[Uuid],
) -> Result<Vec<GrantRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT g.user_token_id, g.agent_id, a.name AS agent_name
           FROM user_token_agents g JOIN agents a ON a.id = g.agent_id
          WHERE g.user_token_id = ANY($1) ORDER BY g.granted_at, g.agent_id",
    )
    .bind(tokens)
    .fetch_all(exec)
    .await?)
}

/// Idempotent: `true` when the grant is new.
pub async fn put_grant(conn: &mut PgConnection, token: Uuid, agent: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO user_token_agents (user_token_id, agent_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(token)
    .bind(agent)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// `true` when a grant existed.
pub async fn delete_grant(
    conn: &mut PgConnection,
    token: Uuid,
    agent: Uuid,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM user_token_agents WHERE user_token_id = $1 AND agent_id = $2")
            .bind(token)
            .bind(agent)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

pub async fn has_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    token: Uuid,
    agent: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_token_agents WHERE user_token_id = $1 AND agent_id = $2)",
    )
    .bind(token)
    .bind(agent)
    .fetch_one(exec)
    .await?)
}

/// The tokens of `user` on `integration` (not revoked) that `agent` holds a grant of, `active`
/// ones first.
pub async fn granted_for<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    integration: Uuid,
    agent: Uuid,
) -> Result<Vec<UserTokenRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.user_id = $1 AND t.integration_id = $2 AND t.status <> 'revoked'
           AND EXISTS (SELECT 1 FROM user_token_agents g
                        WHERE g.user_token_id = t.id AND g.agent_id = $3)
         ORDER BY (t.status = 'active') DESC, t.created_at, t.id"
    )))
    .bind(user)
    .bind(integration)
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// Whether `agent` holds a grant of any `active` token (the tool is offered only then).
pub async fn agent_holds_active_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_token_agents g
                          JOIN user_tokens t ON t.id = g.user_token_id
                         WHERE g.agent_id = $1 AND t.status = 'active')",
    )
    .bind(agent)
    .fetch_one(exec)
    .await?)
}

// ------------------------------------------------------------- end of standing

/// Ids of the tokens still standing (`active` or `invalid`) that a trigger ends.
pub async fn live_ids_for_user(conn: &mut PgConnection, user: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM user_tokens WHERE user_id = $1 AND status <> 'revoked' FOR UPDATE",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

pub async fn live_ids_for_integration(
    conn: &mut PgConnection,
    integration: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM user_tokens WHERE integration_id = $1 AND status <> 'revoked' FOR UPDATE",
    )
    .bind(integration)
    .fetch_all(conn)
    .await?)
}

/// Tokens of one person on one integration (the person's link there is going away).
pub async fn live_ids_for_link(
    conn: &mut PgConnection,
    user: Uuid,
    integration: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM user_tokens WHERE user_id = $1 AND integration_id = $2
            AND status <> 'revoked' FOR UPDATE",
    )
    .bind(user)
    .bind(integration)
    .fetch_all(conn)
    .await?)
}

/// Tokens of one person on the integrations of one organization (a directory deprovisioning).
pub async fn live_ids_for_user_in_org(
    conn: &mut PgConnection,
    user: Uuid,
    org: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.id FROM user_tokens t JOIN channel_integrations i ON i.id = t.integration_id
          WHERE t.user_id = $1 AND i.org_id = $2 AND t.status <> 'revoked' FOR UPDATE OF t",
    )
    .bind(user)
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// Every standing token of an organization's integrations (the organization is going away).
pub async fn live_ids_for_org(conn: &mut PgConnection, org: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.id FROM user_tokens t JOIN channel_integrations i ON i.id = t.integration_id
          WHERE i.org_id = $1 AND t.status <> 'revoked' FOR UPDATE OF t",
    )
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// What an ended token leaves to discard once its transaction committed.
#[derive(Debug, Clone)]
pub struct Ended {
    pub token_id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub integration_id: Uuid,
    /// The sealed value's reference (`None` when an earlier end already took it).
    pub token_ref: Option<String>,
}

/// Move a token to `revoked` or `invalid`. The sealed reference stays in `token_ref` only until
/// the caller (after its commit) or the sweep discards the value and calls
/// [`clear_ended_ref`]; nothing uses an ended token's value. `None` when it was already ended
/// that way (or does not exist).
pub async fn end_token(
    conn: &mut PgConnection,
    id: Uuid,
    status: &str,
) -> Result<Option<Ended>, DbError> {
    debug_assert!(matches!(status, "revoked" | "invalid"));
    let row: Option<(Uuid, Uuid, Option<String>, Uuid)> = sqlx::query_as(
        "UPDATE user_tokens t
            SET status = $2::text, updated_at = now(),
                revoked_at = CASE WHEN $2::text = 'revoked' THEN now() ELSE t.revoked_at END
           FROM (SELECT x.id, x.token_ref, i.org_id FROM user_tokens x
                   JOIN channel_integrations i ON i.id = x.integration_id
                  WHERE x.id = $1 AND x.status <> 'revoked' AND x.status <> $2::text
                    FOR UPDATE OF x) o
          WHERE t.id = o.id
      RETURNING t.user_id, t.integration_id, o.token_ref, o.org_id",
    )
    .bind(id)
    .bind(status)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(
        row.map(|(user_id, integration_id, token_ref, org_id)| Ended {
            token_id: id,
            org_id,
            user_id,
            integration_id,
            token_ref,
        }),
    )
}

/// Null the reference of an ended token once its sealed value was discarded.
pub async fn clear_ended_ref<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE user_tokens SET token_ref = NULL, updated_at = now()
          WHERE id = $1 AND status <> 'active'",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// An ended token whose sealed value is still referenced.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EndedRef {
    pub id: Uuid,
    pub org_id: Uuid,
    pub token_ref: String,
}

/// Ended tokens whose sealed value was not discarded yet (the sweep retries them).
pub async fn ended_with_ref<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<EndedRef>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.id, i.org_id, t.token_ref FROM user_tokens t
           JOIN channel_integrations i ON i.id = t.integration_id
          WHERE t.status <> 'active' AND t.token_ref IS NOT NULL
          ORDER BY t.updated_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Reject the pending `send_as_me` approvals of a token (all of them, or one agent's) with the
/// error `credential_revoked`. Returns the approval ids.
pub async fn cancel_pending_sends(
    conn: &mut PgConnection,
    token: Uuid,
    agent: Option<Uuid>,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE action_approvals SET status = 'rejected', error = 'credential_revoked',
                decided_at = now(), updated_at = now()
          WHERE action_key = 'send_as_me' AND status = 'pending'
            AND payload ->> 'token_id' = $1 AND ($2::uuid IS NULL OR agent_id = $2)
      RETURNING id",
    )
    .bind(token.to_string())
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

/// Pending `send_as_me` approvals of a token (the limit's count).
pub async fn count_pending_sends(conn: &mut PgConnection, token: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM action_approvals
          WHERE action_key = 'send_as_me' AND status = 'pending' AND payload ->> 'token_id' = $1",
    )
    .bind(token.to_string())
    .fetch_one(conn)
    .await?)
}

/// A per-op sealed copy that an end of standing took away.
#[derive(Debug, Clone)]
pub struct OpCopy {
    pub op_id: Uuid,
    pub org_id: Option<Uuid>,
    pub sealed: Option<String>,
    /// The op's internal audit data (`user_meta`: agent, conversation, approval).
    pub meta: serde_json::Value,
    /// The op was still `pending` and is now failed `credential_revoked`.
    pub failed: bool,
}

/// End a token's open `message.post` ops (all of them, or one agent's): a still-`pending` op is
/// failed `credential_revoked` (it is never handed out), and a claimed one (a connector already
/// holds the value) is left to finish. The per-op copies stay referenced by their ops, which are
/// terminal or about to be, until their sealed values are discarded: right after the caller's
/// commit (the values are returned) or, if that is skipped or fails, by the custody sweep, which
/// finds every terminal op that still references a copy. A claim that comes after the revocation
/// refuses the op (the token, the grant and the link are checked at the claim).
pub async fn strip_open_copies(
    conn: &mut PgConnection,
    token: Uuid,
    agent: Option<Uuid>,
) -> Result<Vec<OpCopy>, DbError> {
    type OpenSend = (
        Uuid,
        String,
        Option<String>,
        Option<String>,
        serde_json::Value,
    );
    let rows: Vec<OpenSend> = sqlx::query_as(
        "SELECT id, status, payload->>'user_credential_ref', payload->>'user_org_id',
                COALESCE(payload->'user_meta', '{}'::jsonb)
           FROM channel_ops
          WHERE kind = 'message.post' AND status IN ('pending', 'claimed')
            AND payload->'sender'->>'user_token' = $1
            AND ($2::text IS NULL OR payload->'user_meta'->>'agent_id' = $2)
          FOR UPDATE",
    )
    .bind(token.to_string())
    .bind(agent.map(|a| a.to_string()))
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (op_id, status, sealed, org, meta) in rows {
        if status == "pending" {
            channel_ops::finish(
                &mut *conn,
                op_id,
                "failed",
                Some(&serde_json::json!({"error": {"code": "credential_revoked",
                    "message": "The person's token is no longer usable."}})),
                Some("credential_revoked"),
            )
            .await?;
        }
        out.push(OpCopy {
            op_id,
            org_id: org.and_then(|o| o.parse().ok()),
            sealed,
            meta,
            failed: status == "pending",
        });
    }
    Ok(out)
}

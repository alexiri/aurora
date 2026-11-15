//! Interactive cards posted to platforms (docs/13 §5.6; schema `channel_cards`) and the
//! single-use ledger of action tokens (`action_token_uses`, docs/13 §4.4).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CardRow {
    pub id: Uuid,
    pub subject_kind: String,
    pub subject_id: Uuid,
    pub integration_id: Uuid,
    pub post_op_id: Uuid,
    pub space_ref: String,
    pub thread_ref: String,
    pub message_ref: Option<String>,
    /// The platform user a DMed card was sent to (FIX-112); `None` for channel cards.
    pub addressee_ref: Option<String>,
    pub state: String,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, subject_kind, subject_id, integration_id, post_op_id, space_ref, \
    thread_ref, message_ref, addressee_ref, state, created_at";

pub struct NewCard<'a> {
    pub id: Uuid,
    pub subject_kind: &'a str,
    pub subject_id: Uuid,
    pub integration_id: Uuid,
    pub post_op_id: Uuid,
    pub space_ref: &'a str,
    pub thread_ref: &'a str,
    /// Set when the card is DMed to one person.
    pub addressee_ref: Option<&'a str>,
}

/// Insert a `posting` card.
pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, n: &NewCard<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO channel_cards (id, subject_kind, subject_id, integration_id, post_op_id,
             space_ref, thread_ref, addressee_ref, state)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'posting')",
    )
    .bind(n.id)
    .bind(n.subject_kind)
    .bind(n.subject_id)
    .bind(n.integration_id)
    .bind(n.post_op_id)
    .bind(n.space_ref)
    .bind(n.thread_ref)
    .bind(n.addressee_ref)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<CardRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM channel_cards WHERE id=$1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn for_subject<'e, E: PgExecutor<'e>>(
    exec: E,
    kind: &str,
    subject: Uuid,
) -> Result<Vec<CardRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM channel_cards WHERE subject_kind=$1 AND subject_id=$2
          ORDER BY created_at, id"
    ))
    .bind(kind)
    .bind(subject)
    .fetch_all(exec)
    .await?)
}

/// Cards that are not settled or failed yet, oldest first.
pub async fn unsettled<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<CardRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM channel_cards WHERE state IN ('posting','posted')
          ORDER BY created_at, id LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// `posting` -> `posted` with the platform message reference.
pub async fn set_posted<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    message_ref: &str,
    thread_ref: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_cards SET state='posted', message_ref=$2, thread_ref=$3, updated_at=now()
          WHERE id=$1 AND state='posting'",
    )
    .bind(id)
    .bind(message_ref)
    .bind(thread_ref)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

pub async fn set_failed<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_cards SET state='failed', updated_at=now()
          WHERE id=$1 AND state IN ('posting','posted')",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// Claim the right to settle: `posted` -> `settled`, exactly once.
pub async fn claim_settle<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_cards SET state='settled', updated_at=now()
          WHERE id=$1 AND state='posted'",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// Pending gateway approvals that name a conversation with at least one thread link and have no
/// card yet.
pub async fn approvals_without_card<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.id FROM approvals a
          WHERE a.status='pending' AND a.conversation_id IS NOT NULL
            AND EXISTS (SELECT 1 FROM thread_links t WHERE t.conversation_id=a.conversation_id)
            AND NOT EXISTS (SELECT 1 FROM channel_cards c
                             WHERE c.subject_kind='approval' AND c.subject_id=a.id)
          ORDER BY a.created_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Same for action approvals.
pub async fn action_approvals_without_card<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.id FROM action_approvals a
          WHERE a.status='pending' AND a.conversation_id IS NOT NULL
            AND EXISTS (SELECT 1 FROM thread_links t WHERE t.conversation_id=a.conversation_id)
            AND NOT EXISTS (SELECT 1 FROM channel_cards c
                             WHERE c.subject_kind='action_approval' AND c.subject_id=a.id)
          ORDER BY a.created_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// The most recently used thread link of a conversation (`updated_at` moves with each mirror).
pub async fn latest_thread_link<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
) -> Result<Option<crate::repo::thread_links::ThreadLinkRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, integration_id, agent_id, conversation_id, kind, space_ref, thread_ref,
                sender, revision, created_at
           FROM thread_links WHERE conversation_id=$1
          ORDER BY updated_at DESC, id DESC LIMIT 1",
    )
    .bind(conversation)
    .fetch_optional(exec)
    .await?)
}

/// Whether the token was already used.
pub async fn token_used<'e, E: PgExecutor<'e>>(exec: E, jti: Uuid) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM action_token_uses WHERE jti=$1)")
            .bind(jti)
            .fetch_one(exec)
            .await?,
    )
}

/// Record a use. `false` = it was used before (single use).
pub async fn consume_token<'e, E: PgExecutor<'e>>(
    exec: E,
    jti: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO action_token_uses (jti, used_at, expires_at) VALUES ($1, now(), $2)
         ON CONFLICT (jti) DO NOTHING",
    )
    .bind(jti)
    .bind(expires_at)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// Undo [`consume_token`] when the action failed transiently and will be retried.
pub async fn release_token<'e, E: PgExecutor<'e>>(exec: E, jti: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM action_token_uses WHERE jti=$1")
        .bind(jti)
        .execute(exec)
        .await?;
    Ok(())
}

/// Prune ledger rows of expired tokens (they can no longer verify anyway).
pub async fn prune_tokens(conn: &mut PgConnection) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM action_token_uses WHERE expires_at < now() - interval '1 hour'")
            .execute(&mut *conn)
            .await?
            .rows_affected(),
    )
}

/// A click reveals the real space of a card that was DMed to a person (`space_ref` was
/// `user:<ref>`): remember it so the card can be edited in place.
pub async fn learn_space<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    message_ref: &str,
    space_ref: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE channel_cards SET space_ref=$3, updated_at=now()
          WHERE integration_id=$1 AND message_ref=$2 AND left(space_ref, 5)='user:'",
    )
    .bind(integration)
    .bind(message_ref)
    .bind(space_ref)
    .execute(exec)
    .await?;
    Ok(())
}

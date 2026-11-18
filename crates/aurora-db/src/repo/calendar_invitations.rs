//! (P5-CALINV, D-274, C-844, SEC-108) `calendar_invitations` and the rate-limit ledger
//! `calendar_invitation_sends`. A row keeps what update, cancel and list need (ids, `UID`,
//! `SEQUENCE`, times); the description and the location are never stored.

use crate::DbError;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InvitationRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub person_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub uid: String,
    pub sequence: i32,
    pub status: String,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub all_day: bool,
    pub repeat: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, org_id, workspace_id, agent_id, person_id, conversation_id, uid, sequence, \
    status, title, starts_at, ends_at, timezone, all_day, repeat, created_at, updated_at";

fn sql(body: &str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(body.replace("{COLS}", COLS))
}

/// The event as the person sees it (everything an update replaces).
#[derive(Debug, Clone)]
pub struct Event<'a> {
    pub title: &'a str,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: &'a str,
    pub all_day: bool,
    pub repeat: Option<&'a Value>,
}

#[derive(Debug, Clone, Copy)]
pub struct Owner {
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub person_id: Uuid,
    pub conversation_id: Option<Uuid>,
}

/// Serializes one person's sends: the rate-limit count and the insert must not race. Held until
/// the transaction ends.
pub async fn lock_person(conn: &mut PgConnection, person: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('calendar_invitations:' || $1::text, 0))",
    )
    .bind(person)
    .execute(conn)
    .await?;
    Ok(())
}

/// What the ledger says about a person's recent sends (the hour for this agent, the day for all).
#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct SendWindow {
    pub hour_count: i64,
    pub hour_oldest: Option<DateTime<Utc>>,
    pub day_count: i64,
    pub day_oldest: Option<DateTime<Utc>>,
}

pub async fn send_window(
    conn: &mut PgConnection,
    person: Uuid,
    agent: Uuid,
) -> Result<SendWindow, DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE agent_id = $2 AND sent_at > now() - interval '1 hour') AS hour_count,
                min(sent_at) FILTER (WHERE agent_id = $2 AND sent_at > now() - interval '1 hour') AS hour_oldest,
                count(*) AS day_count,
                min(sent_at) AS day_oldest
           FROM calendar_invitation_sends
          WHERE person_id = $1 AND sent_at > now() - interval '1 day'",
    )
    .bind(person)
    .bind(agent)
    .fetch_one(conn)
    .await?)
}

pub async fn record_send(
    conn: &mut PgConnection,
    org: Uuid,
    agent: Uuid,
    person: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO calendar_invitation_sends (id, org_id, agent_id, person_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(agent)
    .bind(person)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    uid: &str,
    o: &Owner,
    e: &Event<'_>,
) -> Result<InvitationRow, DbError> {
    Ok(sqlx::query_as(sql(
        "INSERT INTO calendar_invitations (id, org_id, workspace_id, agent_id, person_id, \
         conversation_id, uid, sequence, status, title, starts_at, ends_at, timezone, all_day, repeat) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 0, 'sent', $8, $9, $10, $11, $12, $13) RETURNING {COLS}",
    ))
    .bind(id)
    .bind(o.org_id)
    .bind(o.workspace_id)
    .bind(o.agent_id)
    .bind(o.person_id)
    .bind(o.conversation_id)
    .bind(uid)
    .bind(e.title)
    .bind(e.starts_at)
    .bind(e.ends_at)
    .bind(e.timezone)
    .bind(e.all_day)
    .bind(e.repeat)
    .fetch_one(conn)
    .await?)
}

/// The row of this agent's invitation to this person, locked for the transaction. Another agent's
/// or person's id is `None`.
pub async fn get_own_for_update(
    conn: &mut PgConnection,
    id: Uuid,
    agent: Uuid,
    person: Uuid,
) -> Result<Option<InvitationRow>, DbError> {
    Ok(sqlx::query_as(sql(
        "SELECT {COLS} FROM calendar_invitations WHERE id = $1 AND agent_id = $2 AND person_id = $3 \
         FOR UPDATE",
    ))
    .bind(id)
    .bind(agent)
    .bind(person)
    .fetch_optional(conn)
    .await?)
}

/// An update: the new event, `SEQUENCE + 1`, status `sent` again.
pub async fn update_event(
    conn: &mut PgConnection,
    id: Uuid,
    e: &Event<'_>,
) -> Result<InvitationRow, DbError> {
    Ok(sqlx::query_as(sql(
        "UPDATE calendar_invitations SET sequence = sequence + 1, status = 'sent', title = $2, \
         starts_at = $3, ends_at = $4, timezone = $5, all_day = $6, repeat = $7, updated_at = now() \
         WHERE id = $1 RETURNING {COLS}",
    ))
    .bind(id)
    .bind(e.title)
    .bind(e.starts_at)
    .bind(e.ends_at)
    .bind(e.timezone)
    .bind(e.all_day)
    .bind(e.repeat)
    .fetch_one(conn)
    .await?)
}

pub async fn cancel(conn: &mut PgConnection, id: Uuid) -> Result<InvitationRow, DbError> {
    Ok(sqlx::query_as(sql(
        "UPDATE calendar_invitations SET sequence = sequence + 1, status = 'cancelled', \
         updated_at = now() WHERE id = $1 RETURNING {COLS}",
    ))
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// This agent's invitations to this person, newest first.
pub async fn list(
    conn: &mut PgConnection,
    agent: Uuid,
    person: Uuid,
    limit: i64,
) -> Result<Vec<InvitationRow>, DbError> {
    Ok(sqlx::query_as(sql(
        "SELECT {COLS} FROM calendar_invitations WHERE agent_id = $1 AND person_id = $2 \
         ORDER BY created_at DESC, id DESC LIMIT $3",
    ))
    .bind(agent)
    .bind(person)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Retention (the daily pass): single or cancelled invitations `age_secs` after their end (or last
/// change, whichever is later); repeating ones `repeat_age_secs` after their last change.
pub async fn prune_invitations<'e, E: sqlx::PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
    repeat_age_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM calendar_invitations WHERE id IN (
           SELECT id FROM calendar_invitations
            WHERE ((repeat IS NULL OR status = 'cancelled')
                   AND greatest(ends_at, updated_at) < now() - make_interval(secs => $1))
               OR (repeat IS NOT NULL AND updated_at < now() - make_interval(secs => $2))
            ORDER BY updated_at LIMIT $3 FOR UPDATE SKIP LOCKED)",
    )
    .bind(age_secs)
    .bind(repeat_age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Ledger rows past two days (the longest rate-limit window is one day).
pub async fn prune_sends<'e, E: sqlx::PgExecutor<'e>>(exec: E, limit: i64) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM calendar_invitation_sends WHERE id IN (
           SELECT id FROM calendar_invitation_sends
            WHERE sent_at < now() - interval '2 days' LIMIT $1 FOR UPDATE SKIP LOCKED)",
    )
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

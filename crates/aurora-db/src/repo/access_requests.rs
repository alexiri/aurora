//! (P5-UX-17, D-348, C-984, SEC-132) `agent_access_requests`: an agent's request for an app (or a
//! proposed website's token). A row is only a record: it gives no access. It is resolved by a real
//! access change (`mark_given`), by a person (`dismiss`) or by age (`expire_overdue`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RequestRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub kind: String,
    pub app_id: Option<String>,
    pub secret_id: Option<Uuid>,
    pub reason: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

const COLS: &str = "id, org_id, workspace_id, agent_id, conversation_id, turn_id, kind, app_id, \
    secret_id, reason, status, created_at, resolved_at";

/// The statement with `{COLS}` filled in (a constant list, never input).
fn sql(body: &str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(body.replace("{COLS}", COLS))
}

/// What a tool call records.
#[derive(Debug, Clone, Copy)]
pub struct NewRequest<'a> {
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    /// `app` (with `app_id`) or `website` (with `secret_id`).
    pub app_id: Option<&'a str>,
    pub secret_id: Option<Uuid>,
    pub reason: &'a str,
}

/// Creates the open request, or refreshes the agent's existing open one for the same app or
/// website (its reason, conversation and turn follow the latest ask). Returns the row and whether
/// it is new or moved to another conversation (the caller writes a card only then, so an agent
/// asking again in the same conversation does not add cards).
pub async fn open(
    tx: &mut PgConnection,
    n: &NewRequest<'_>,
) -> Result<(RequestRow, bool), DbError> {
    let existing: Option<RequestRow> = sqlx::query_as(sql(
        "SELECT {COLS} FROM agent_access_requests WHERE agent_id = $1 AND status = 'open' \
         AND ((kind = 'app' AND app_id = $2) OR (kind = 'website' AND secret_id = $3)) FOR UPDATE",
    ))
    .bind(n.agent_id)
    .bind(n.app_id)
    .bind(n.secret_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(e) = existing {
        let moved = e.conversation_id != n.conversation_id;
        let row: RequestRow = sqlx::query_as(sql(
            "UPDATE agent_access_requests SET reason = $2, conversation_id = $3, turn_id = $4, \
             updated_at = now() WHERE id = $1 RETURNING {COLS}",
        ))
        .bind(e.id)
        .bind(n.reason)
        .bind(n.conversation_id)
        .bind(n.turn_id)
        .fetch_one(&mut *tx)
        .await?;
        hint(tx, &row).await?;
        return Ok((row, moved));
    }
    let row: RequestRow = sqlx::query_as(sql(
        "INSERT INTO agent_access_requests (id, org_id, workspace_id, agent_id, conversation_id, \
         turn_id, kind, app_id, secret_id, reason) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
         RETURNING {COLS}",
    ))
    .bind(Uuid::now_v7())
    .bind(n.org_id)
    .bind(n.workspace_id)
    .bind(n.agent_id)
    .bind(n.conversation_id)
    .bind(n.turn_id)
    .bind(if n.secret_id.is_some() {
        "website"
    } else {
        "app"
    })
    .bind(n.app_id)
    .bind(n.secret_id)
    .bind(n.reason)
    .fetch_one(&mut *tx)
    .await?;
    hint(tx, &row).await?;
    Ok((row, true))
}

/// (P5-REFINE-9, C-1084) Queue the agent's timeline hint for a changed request.
async fn hint(tx: &mut PgConnection, row: &RequestRow) -> Result<(), DbError> {
    super::timeline::hint(tx, row.agent_id, super::timeline::Source::Access, row.id).await
}

/// [`hint`] for a fenced update's result.
async fn hinted(
    tx: &mut PgConnection,
    row: Option<RequestRow>,
) -> Result<Option<RequestRow>, DbError> {
    if let Some(r) = &row {
        hint(tx, r).await?;
    }
    Ok(row)
}

/// The agent's open request for an app, marked `given` (at most one per agent and app).
pub async fn mark_given_for_app(
    tx: &mut PgConnection,
    agent_id: Uuid,
    app_id: &str,
) -> Result<Option<RequestRow>, DbError> {
    let row = sqlx::query_as(sql(
        "UPDATE agent_access_requests SET status = 'given', resolved_at = now(), updated_at = now() \
         WHERE agent_id = $1 AND kind = 'app' AND app_id = $2 AND status = 'open' RETURNING {COLS}"
    ))
    .bind(agent_id)
    .bind(app_id)
    .fetch_optional(&mut *tx)
    .await?;
    hinted(tx, row).await
}

/// The agent's open request for a proposed website's secret, marked `given`.
pub async fn mark_given_for_secret(
    tx: &mut PgConnection,
    agent_id: Uuid,
    secret_id: Uuid,
) -> Result<Option<RequestRow>, DbError> {
    let row = sqlx::query_as(sql(
        "UPDATE agent_access_requests SET status = 'given', resolved_at = now(), updated_at = now() \
         WHERE agent_id = $1 AND kind = 'website' AND secret_id = $2 AND status = 'open' \
         RETURNING {COLS}"
    ))
    .bind(agent_id)
    .bind(secret_id)
    .fetch_optional(&mut *tx)
    .await?;
    hinted(tx, row).await
}

pub async fn get(db: &PgPool, id: Uuid) -> Result<Option<RequestRow>, DbError> {
    Ok(sqlx::query_as(sql(
        "SELECT {COLS} FROM agent_access_requests WHERE id = $1",
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// `open` → `dismissed`; `None` when it is not open (already resolved, or unknown).
pub async fn dismiss(
    tx: &mut PgConnection,
    id: Uuid,
    by: Option<Uuid>,
) -> Result<Option<RequestRow>, DbError> {
    let row = sqlx::query_as(sql(
        "UPDATE agent_access_requests SET status = 'dismissed', resolved_at = now(), \
         resolved_by = $2, updated_at = now() WHERE id = $1 AND status = 'open' RETURNING {COLS}",
    ))
    .bind(id)
    .bind(by)
    .fetch_optional(&mut *tx)
    .await?;
    hinted(tx, row).await
}

/// A workspace's requests of one status, newest first.
pub async fn list(
    db: &PgPool,
    workspace_id: Uuid,
    status: &str,
    limit: i64,
) -> Result<Vec<RequestRow>, DbError> {
    Ok(sqlx::query_as(sql(
        "SELECT {COLS} FROM agent_access_requests WHERE workspace_id = $1 AND status = $2 \
         ORDER BY created_at DESC, id DESC LIMIT $3",
    ))
    .bind(workspace_id)
    .bind(status)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// The host pattern of a proposed website's secret.
pub async fn secret_host(db: &PgPool, secret_id: Uuid) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT host_pattern FROM secrets WHERE id = $1")
            .bind(secret_id)
            .fetch_optional(db)
            .await?,
    )
}

/// Open requests older than `ttl_secs` → `expired`. Returns how many.
pub async fn expire_overdue(db: &PgPool, ttl_secs: f64) -> Result<u64, DbError> {
    // (P5-REFINE-9, C-1084) One timeline hint per expired request, sent with the update.
    Ok(sqlx::query(sqlx::AssertSqlSafe(format!(
        "WITH hinted AS (UPDATE agent_access_requests SET status = 'expired', resolved_at = now(), \
         updated_at = now() WHERE status = 'open' AND updated_at < now() - make_interval(secs => $1) \
         RETURNING id, agent_id) {}",
        super::timeline::hint_rows_sql(super::timeline::Source::Access)
    )))
    .bind(ttl_secs)
    .execute(db)
    .await?
    .rows_affected())
}

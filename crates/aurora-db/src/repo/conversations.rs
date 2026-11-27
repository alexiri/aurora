//! Conversations (docs/03 §6, docs/10 §Conversations).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConversationRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub kind: String,
    pub external_ref: String,
    pub direct_user_id: Option<Uuid>,
    pub title: Option<String>,
    pub origin_conversation_id: Option<Uuid>,
    pub last_seq: i64,
    pub app_turn_streak: i32,
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    /// (P5-REFINE-7, C-1073) `auto` | `person` | `automation`.
    pub title_source: String,
    /// (P5-REFINE-7, C-1074) `schedule` | `webhook` for a run thread.
    pub automation_kind: Option<String>,
    pub automation_id: Option<Uuid>,
    pub automation_deleted_at: Option<DateTime<Utc>>,
    /// (C-1098) The title before the automation's last rename; null if never renamed.
    pub previous_title: Option<String>,
}

const COLS: &str = "id, agent_id, workspace_id, kind, external_ref, direct_user_id, title, \
    origin_conversation_id, last_seq, app_turn_streak, last_activity_at, created_at, archived_at, \
    title_source, automation_kind, automation_id, automation_deleted_at, previous_title";

pub struct NewConversation<'a> {
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub kind: &'a str,
    pub external_ref: &'a str,
    pub direct_user_id: Option<Uuid>,
    pub title: Option<&'a str>,
    pub origin_conversation_id: Option<Uuid>,
}

/// (P5-REFINE-7, C-1074) The automation a run thread belongs to: `("schedule" | "webhook", id)`.
#[derive(Debug, Clone, Copy)]
pub struct Automation<'a> {
    pub kind: &'a str,
    pub id: Uuid,
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<ConversationRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM conversations WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// The existing conversations among `ids` (unknown ids are simply absent).
pub async fn get_many<'e, E: PgExecutor<'e>>(
    exec: E,
    ids: &[Uuid],
) -> Result<Vec<ConversationRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM conversations WHERE id = ANY($1)"
    )))
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// Get-or-create by `(agent, kind, external_ref)`; idempotent under concurrency (`ON CONFLICT
/// DO NOTHING` without a target absorbs both the ref and the direct-user unique indexes).
/// Returns `(row, created)`.
pub async fn get_or_create(
    conn: &mut PgConnection,
    n: &NewConversation<'_>,
) -> Result<(ConversationRow, bool), DbError> {
    get_or_create_in(conn, n, None).await
}

/// [`get_or_create`] for a run thread of an automation (P5-REFINE-7, C-1074): the row carries
/// `automation_kind`/`automation_id` and `title_source = 'automation'`.
pub async fn get_or_create_in(
    conn: &mut PgConnection,
    n: &NewConversation<'_>,
    automation: Option<Automation<'_>>,
) -> Result<(ConversationRow, bool), DbError> {
    let title_source = if automation.is_some() {
        "automation"
    } else {
        "auto"
    };
    let done = sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id,
            title, origin_conversation_id, last_activity_at, title_source, automation_kind,
            automation_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8, now(), $9, $10, $11) ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(n.agent_id)
    .bind(n.workspace_id)
    .bind(n.kind)
    .bind(n.external_ref)
    .bind(n.direct_user_id)
    .bind(n.title)
    .bind(n.origin_conversation_id)
    .bind(title_source)
    .bind(automation.map(|a| a.kind))
    .bind(automation.map(|a| a.id))
    .execute(&mut *conn)
    .await?;
    let row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM conversations WHERE agent_id = $1 AND kind = $2 AND external_ref = $3"
    )))
    .bind(n.agent_id)
    .bind(n.kind)
    .bind(n.external_ref)
    .fetch_one(&mut *conn)
    .await?;
    Ok((row, done.rows_affected() == 1))
}

// ------------------------------------------------------------------ direct (P5-REFINE-7)

/// (C-1073) The person's most recently active **non-archived** direct conversation with the
/// agent (never an archived one).
pub async fn latest_direct<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    user: Uuid,
) -> Result<Option<ConversationRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM conversations
         WHERE agent_id = $1 AND kind = 'direct' AND direct_user_id = $2 AND archived_at IS NULL
         ORDER BY last_activity_at DESC, id DESC LIMIT 1"
    )))
    .bind(agent_id)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// (C-1073) Direct conversations (active and archived) of a person with an agent.
pub async fn count_direct<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    user: Uuid,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM conversations
         WHERE agent_id = $1 AND kind = 'direct' AND direct_user_id = $2",
    )
    .bind(agent_id)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// (C-1073) Set the title and who set it.
pub async fn set_title(
    conn: &mut PgConnection,
    id: Uuid,
    title: Option<&str>,
    source: &str,
) -> Result<Option<ConversationRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE conversations SET title = $2, title_source = $3, updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(title)
    .bind(source)
    .fetch_optional(conn)
    .await?)
}

/// (C-1073) The auto title, set only while the conversation has none and nobody chose one.
/// Returns whether it was set.
pub async fn set_auto_title(
    conn: &mut PgConnection,
    id: Uuid,
    title: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE conversations SET title = $2, updated_at = now()
         WHERE id = $1 AND title IS NULL AND title_source = 'auto'",
    )
    .bind(id)
    .bind(title)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

// ------------------------------------------------------------------ automations (P5-REFINE-7)

/// (C-1074) Rename every thread of an automation (its title follows the automation's name).
pub async fn rename_automation_threads(
    conn: &mut PgConnection,
    kind: &str,
    id: Uuid,
    name: &str,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE conversations SET previous_title = title, title = $3, title_source = 'automation',
            updated_at = now()
         WHERE automation_kind = $1 AND automation_id = $2 AND title IS DISTINCT FROM $3",
    )
    .bind(kind)
    .bind(id)
    .bind(name)
    .execute(conn)
    .await?
    .rows_affected())
}

/// (C-1074) Mark every thread of a deleted automation (call before the row is deleted, in the
/// same transaction). Transcripts stay.
pub async fn mark_automation_deleted(
    conn: &mut PgConnection,
    kind: &str,
    id: Uuid,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE conversations SET automation_deleted_at = now(), updated_at = now()
         WHERE automation_kind = $1 AND automation_id = $2 AND automation_deleted_at IS NULL",
    )
    .bind(kind)
    .bind(id)
    .execute(conn)
    .await?
    .rows_affected())
}

/// (C-1074) `(id, name)` of the schedules and webhooks among `ids` that still exist.
pub async fn automation_names<'e, E: PgExecutor<'e>>(
    exec: E,
    schedule_ids: &[Uuid],
    webhook_ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    if schedule_ids.is_empty() && webhook_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT id, name FROM schedules WHERE id = ANY($1)
         UNION ALL SELECT id, name FROM webhooks WHERE id = ANY($2)",
    )
    .bind(schedule_ids)
    .bind(webhook_ids)
    .fetch_all(exec)
    .await?)
}

/// (C-1074) One automation folder of an agent's conversation list.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FolderRow {
    pub automation_kind: String,
    pub automation_id: Uuid,
    /// The automation's current name, else the newest thread's title.
    pub name: Option<String>,
    /// The automation row is gone (or its threads were marked deleted).
    pub deleted: bool,
    pub thread_count: i64,
    pub last_activity_at: DateTime<Utc>,
}

/// (C-1074) Folders of an agent by last activity (desc); `after` is `(last_activity_at, id)`.
pub async fn list_folders<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<FolderRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(
        "WITH f AS (
            SELECT automation_kind, automation_id, count(*) AS thread_count,
                   max(last_activity_at) AS last_activity_at,
                   bool_or(automation_deleted_at IS NOT NULL) AS marked
            FROM conversations
            WHERE agent_id = $1 AND automation_id IS NOT NULL AND archived_at IS NULL
            GROUP BY automation_kind, automation_id)
         SELECT f.automation_kind, f.automation_id,
                COALESCE(s.name, w.name, (SELECT c.title FROM conversations c
                    WHERE c.automation_kind = f.automation_kind AND c.automation_id = f.automation_id
                    ORDER BY c.created_at DESC LIMIT 1)) AS name,
                (f.marked OR (s.id IS NULL AND w.id IS NULL)) AS deleted,
                f.thread_count, f.last_activity_at
         FROM f
         LEFT JOIN schedules s ON f.automation_kind = 'schedule' AND s.id = f.automation_id
         LEFT JOIN webhooks w ON f.automation_kind = 'webhook' AND w.id = f.automation_id
         WHERE ($2::timestamptz IS NULL OR (f.last_activity_at, f.automation_id) < ($2, $3))
         ORDER BY f.last_activity_at DESC, f.automation_id DESC LIMIT $4",
    )
    .bind(agent_id)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// (C-1074) Automation threads beyond the newest `keep` of their automation, oldest first, with
/// their org; threads with an active turn or a pending approval are left for a later pass.
pub async fn automation_overflow(
    conn: &mut PgConnection,
    keep: i64,
    limit: i64,
) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id, w.org_id FROM (
            SELECT id, workspace_id, created_at, row_number() OVER (
                PARTITION BY automation_kind, automation_id ORDER BY created_at DESC, id DESC) AS rn
            FROM conversations WHERE automation_id IS NOT NULL) c
           JOIN workspaces w ON w.id = c.workspace_id
         WHERE c.rn > $1
           AND NOT EXISTS (SELECT 1 FROM turns t WHERE t.conversation_id = c.id AND t.role = 'turn'
                           AND t.status IN ('queued','dispatched','running'))
           AND NOT EXISTS (SELECT 1 FROM approvals a WHERE a.conversation_id = c.id AND a.status = 'pending')
           AND NOT EXISTS (SELECT 1 FROM action_approvals a WHERE a.conversation_id = c.id AND a.status = 'pending')
         ORDER BY w.org_id, c.created_at, c.id LIMIT $2",
    )
    .bind(keep)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// (C-1074) Which run threads the conversation list shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AutomationFilter {
    /// Every conversation (today's behaviour for API clients).
    #[default]
    All,
    /// No automation thread (the dashboard's main list).
    None,
    /// Only automation threads, optionally of one automation.
    Only(Option<Uuid>),
}

/// Which conversations of an agent a caller lists.
pub struct ListFilter<'a> {
    pub kind: Option<&'a str>,
    /// The caller; direct conversations of other users are excluded unless `viewer_owns`.
    pub viewer: Uuid,
    pub viewer_owns: bool,
    /// (RFE-010) `true` lists only archived conversations, `false` only the others.
    pub archived: bool,
    /// (P5-REFINE-7, C-1074)
    pub automation: AutomationFilter,
}

/// Conversations of an agent by last activity (desc); `after` is `(last_activity_at, id)`.
pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    f: &ListFilter<'_>,
    after: After,
    fetch: i64,
) -> Result<Vec<ConversationRow>, DbError> {
    let ListFilter {
        kind,
        viewer,
        viewer_owns,
        archived,
        automation,
    } = *f;
    let (t, id) = split(after);
    let (auto_mode, auto_id) = match automation {
        AutomationFilter::All => ("all", None),
        AutomationFilter::None => ("none", None),
        AutomationFilter::Only(id) => ("only", id),
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM conversations
         WHERE agent_id = $1 AND ($2::text IS NULL OR kind = $2)
           -- (P4-EVAL-2, C-860) eval conversations show only when asked for with `?kind=eval`.
           AND ($2::text IS NOT NULL OR kind <> 'eval')
           AND (kind <> 'direct' OR direct_user_id = $3 OR $7)
           AND ((archived_at IS NOT NULL) = $8)
           -- (P5-REFINE-7, C-1074) run threads: all, none, or only (of one automation).
           AND ($9 = 'all' OR ($9 = 'none' AND automation_id IS NULL)
                OR ($9 = 'only' AND automation_id IS NOT NULL
                    AND ($10::uuid IS NULL OR automation_id = $10)))
           AND ($4::timestamptz IS NULL OR (last_activity_at, id) < ($4, $5))
         ORDER BY last_activity_at DESC, id DESC LIMIT $6"
    )))
    .bind(agent_id)
    .bind(kind)
    .bind(viewer)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .bind(viewer_owns)
    .bind(archived)
    .bind(auto_mode)
    .bind(auto_id)
    .fetch_all(exec)
    .await?)
}

/// Take the conversation row lock (lock order: conversation, then its turns). Returns false when
/// the conversation does not exist.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    let r: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM conversations WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(r.is_some())
}

/// Reset the app-turn streak to zero (a person wrote, or an owner clicked "continue").
/// Caller holds the conversation lock.
pub async fn reset_app_turn_streak(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE conversations SET app_turn_streak = 0, updated_at = now()
         WHERE id = $1 AND app_turn_streak <> 0",
    )
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Count one bot/agent turn. Caller holds the conversation lock and has checked the cap.
pub async fn bump_app_turn_streak(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE conversations SET app_turn_streak = app_turn_streak + 1, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

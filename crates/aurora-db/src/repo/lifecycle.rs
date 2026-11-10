//! Conversation data lifecycle (RFE-010, docs/10 §Archive, delete, redaction, retention, export):
//! archive flag, hard delete with its cascade, transcript redaction and `session_purges`.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

// ------------------------------------------------------------------ archive

/// Set (`by = Some`) or clear the archive flag. Returns whether a row changed.
pub async fn set_archived(
    conn: &mut PgConnection,
    id: Uuid,
    archived: bool,
    by: Option<Uuid>,
) -> Result<bool, DbError> {
    let sql = if archived {
        "UPDATE conversations SET archived_at = now(), archived_by_user_id = $2, updated_at = now()
         WHERE id = $1 AND archived_at IS NULL"
    } else {
        "UPDATE conversations SET archived_at = NULL, archived_by_user_id = NULL, updated_at = now()
         WHERE id = $1 AND archived_at IS NOT NULL"
    };
    Ok(sqlx::query(sql)
        .bind(id)
        .bind(by)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

// ------------------------------------------------------------------ delete

/// What a delete removed (ids and counts only, never content).
#[derive(Debug, Clone, Default)]
pub struct Deleted {
    pub turns: i64,
    pub events: i64,
    pub attachments: i64,
    /// `(backend, key)` of every attachment blob that still has bytes to remove after commit.
    pub blobs: Vec<(String, String)>,
    pub session_purged: bool,
}

/// Number of non-terminal turns (`queued`/`dispatched`/`running`).
pub async fn has_active_turn(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM turns WHERE conversation_id = $1 AND role = 'turn'
                        AND status IN ('queued','dispatched','running'))",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// A pending gateway approval or action approval refers to the conversation.
pub async fn has_pending_approval(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM approvals WHERE conversation_id = $1 AND status = 'pending')
             OR EXISTS (SELECT 1 FROM action_approvals WHERE conversation_id = $1 AND status = 'pending')",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// Hard-delete one conversation inside the caller's transaction (which holds the conversation
/// lock). Turns, events, buffers, attachment rows, thread links, context credentials and mention
/// anchors go with the row (FK cascade); approvals keep their row with the reference nulled; the
/// stored harness session is queued for purge. Blob bytes are returned for removal after commit.
pub async fn delete_conversation(conn: &mut PgConnection, id: Uuid) -> Result<Deleted, DbError> {
    let (agent, session_ref): (Uuid, Option<String>) =
        sqlx::query_as("SELECT agent_id, harness_session_ref FROM conversations WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE conversation_id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM transcript_events WHERE conversation_id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    // Purged rows (RFE-015) have an empty blob key; deleting it is a no-op.
    let blobs: Vec<(String, String)> =
        sqlx::query_as("SELECT blob_backend, blob_key FROM attachments WHERE conversation_id = $1")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    // `approvals` (gateway) has no FK to conversations; `action_approvals` is set null by its FK.
    sqlx::query(
        "UPDATE approvals SET conversation_id = NULL, turn_id = NULL WHERE conversation_id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    let mut session_purged = false;
    if let Some(r) = session_ref.as_deref().filter(|r| !r.is_empty()) {
        enqueue_purge(&mut *conn, agent, Some(id), r).await?;
        session_purged = true;
    }
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(Deleted {
        turns,
        events,
        attachments: i64::try_from(blobs.len()).unwrap_or(i64::MAX),
        blobs,
        session_purged,
    })
}

// ------------------------------------------------------------------ session purges

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionPurgeRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub session_ref: String,
    /// (D-153) The harness that created the session; `None` = the sandbox's own.
    pub harness: Option<String>,
}

/// Queue the deletion of a stored harness session (idempotent per agent, conversation and ref).
/// The session belongs to the sandbox's own harness (`harness` stays NULL).
pub async fn enqueue_purge(
    conn: &mut PgConnection,
    agent: Uuid,
    conversation: Option<Uuid>,
    session_ref: &str,
) -> Result<(), DbError> {
    enqueue_purge_of(conn, agent, conversation, session_ref, None).await
}

/// [`enqueue_purge`] for a session of an explicit `harness` (the old one, when an agent's
/// harness is being switched).
pub async fn enqueue_purge_of(
    conn: &mut PgConnection,
    agent: Uuid,
    conversation: Option<Uuid>,
    session_ref: &str,
    harness: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO session_purges (id, agent_id, conversation_id, session_ref, harness)
         SELECT $1, $2, $3, $4, $5
         WHERE NOT EXISTS (SELECT 1 FROM session_purges WHERE agent_id = $2 AND done_at IS NULL
                           AND conversation_id IS NOT DISTINCT FROM $3 AND session_ref = $4)",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(conversation)
    .bind(session_ref)
    .bind(harness)
    .execute(conn)
    .await?;
    Ok(())
}

/// Pending purges of one agent (part of the home sync); a conversation with a live turn waits.
pub async fn pending_for_agent(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<SessionPurgeRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT p.id, p.agent_id, p.conversation_id, p.session_ref, p.harness
         FROM session_purges p JOIN agents a ON a.id = p.agent_id
         WHERE p.agent_id = $1 AND p.done_at IS NULL
           AND (p.conversation_id IS NULL
                -- (D-153) a session of another harness than the agent's is no live session
                OR (p.harness IS NOT NULL AND p.harness IS DISTINCT FROM a.harness)
                OR NOT EXISTS (
                SELECT 1 FROM turns t WHERE t.conversation_id = p.conversation_id
                  AND t.role = 'turn' AND t.status IN ('queued','dispatched','running')))
         ORDER BY p.created_at, p.id",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PurgeClaim {
    pub id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub session_ref: String,
    pub harness: Option<String>,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
}

/// Live delivery: pending purges of agents whose sandbox is running on this runner replica's
/// channel and whose conversation has no active turn. Re-sent every `lease_secs` until the
/// supervisor reports `session.purged`.
pub async fn claim_live(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    lease_secs: f64,
    limit: i64,
) -> Result<Vec<PurgeClaim>, DbError> {
    Ok(sqlx::query_as(
        "WITH cand AS (
           SELECT p.id FROM session_purges p JOIN sandboxes s ON s.agent_id = p.agent_id
           WHERE p.done_at IS NULL AND p.conversation_id IS NOT NULL
             AND (p.leased_until IS NULL OR p.leased_until < now())
             AND s.runner_id = $1 AND s.channel_replica_id = $2 AND s.state = 'running'
             AND s.boot_id IS NOT NULL AND s.home_applied_generation > 0
             AND (NOT EXISTS (SELECT 1 FROM turns t WHERE t.conversation_id = p.conversation_id
                             AND t.role = 'turn' AND t.status IN ('queued','dispatched','running'))
                  OR (p.harness IS NOT NULL
                      AND p.harness IS DISTINCT FROM (SELECT a.harness FROM agents a WHERE a.id = p.agent_id)))
           ORDER BY p.created_at LIMIT $4 FOR UPDATE OF p SKIP LOCKED)
         UPDATE session_purges p SET leased_until = now() + make_interval(secs => $3)
         FROM cand, sandboxes s WHERE p.id = cand.id AND s.agent_id = p.agent_id
         RETURNING p.id, p.conversation_id, p.session_ref, p.harness, s.id AS sandbox_id, s.boot_id",
    )
    .bind(runner)
    .bind(replica)
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// `session.purged`: `ok` stamps the row done; a failure keeps it pending (re-sent after the lease
/// and with the next home sync).
pub async fn complete_purge(
    conn: &mut PgConnection,
    agent: Uuid,
    conversation: Uuid,
    session_ref: &str,
    ok: bool,
) -> Result<bool, DbError> {
    let sql = if ok {
        "UPDATE session_purges SET done_at = now()
         WHERE agent_id = $1 AND conversation_id = $2 AND session_ref = $3 AND done_at IS NULL"
    } else {
        "UPDATE session_purges SET leased_until = NULL
         WHERE agent_id = $1 AND conversation_id = $2 AND session_ref = $3 AND done_at IS NULL"
    };
    Ok(sqlx::query(sql)
        .bind(agent)
        .bind(conversation)
        .bind(session_ref)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

/// Number of purges of an agent still pending (Computer tab, tests).
pub async fn pending_count(conn: &mut PgConnection, agent: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM session_purges WHERE agent_id = $1 AND done_at IS NULL",
    )
    .bind(agent)
    .fetch_one(conn)
    .await?)
}

// ------------------------------------------------------------------ redaction

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EventRef {
    pub seq: i64,
    pub turn_id: Option<Uuid>,
    #[sqlx(rename = "type")]
    pub ty: String,
    pub payload: serde_json::Value,
    pub redacted_at: Option<DateTime<Utc>>,
}

pub async fn event_ref(
    conn: &mut PgConnection,
    conversation: Uuid,
    seq: i64,
) -> Result<Option<EventRef>, DbError> {
    Ok(sqlx::query_as(
        "SELECT seq, turn_id, type, payload, redacted_at FROM transcript_events
         WHERE conversation_id = $1 AND seq = $2 FOR UPDATE",
    )
    .bind(conversation)
    .bind(seq)
    .fetch_optional(conn)
    .await?)
}

/// Every event of a turn (locked), ascending.
pub async fn turn_events(
    conn: &mut PgConnection,
    conversation: Uuid,
    turn: Uuid,
) -> Result<Vec<EventRef>, DbError> {
    Ok(sqlx::query_as(
        "SELECT seq, turn_id, type, payload, redacted_at FROM transcript_events
         WHERE conversation_id = $1 AND turn_id = $2 ORDER BY seq FOR UPDATE",
    )
    .bind(conversation)
    .bind(turn)
    .fetch_all(conn)
    .await?)
}

/// Replace one event's payload in place and stamp the redaction.
pub async fn write_redaction(
    conn: &mut PgConnection,
    conversation: Uuid,
    seq: i64,
    payload: &serde_json::Value,
    by: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE transcript_events SET payload = $3, redacted_at = now(), redacted_by_user_id = $4
         WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(conversation)
    .bind(seq)
    .bind(payload)
    .bind(by)
    .execute(conn)
    .await?;
    Ok(())
}

/// Scrub the stored text of a turn and its answer buffer (a redacted `turn.queued`).
pub async fn scrub_turn_text(conn: &mut PgConnection, turn: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET text = '', updated_at = now() WHERE id = $1")
        .bind(turn)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE turn_answer_buffers SET msgs = '[]'::jsonb WHERE turn_id = $1")
        .bind(turn)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// `reset_session`: force a fresh session on the next turn. Returns the previous
/// `(agent_id, session_ref)` to purge.
pub async fn reset_session(
    conn: &mut PgConnection,
    conversation: Uuid,
) -> Result<(Uuid, Option<String>), DbError> {
    Ok(sqlx::query_as(
        "UPDATE conversations c SET harness_session_epoch = c.harness_session_epoch + 1,
             harness_session_ref = NULL, updated_at = now()
         FROM (SELECT id, harness_session_ref AS old FROM conversations WHERE id = $1) o
         WHERE c.id = o.id RETURNING c.agent_id, o.old",
    )
    .bind(conversation)
    .fetch_one(conn)
    .await?)
}

/// (P4-HAR, D-122) The agent's harness changed from `old_harness`: every conversation gets a
/// fresh session on its next turn (epoch bumped, reference cleared) and the old sessions are
/// queued for purge by the old harness's adapter. The transcript is untouched. Returns the
/// number of conversations affected.
pub async fn switch_harness_sessions(
    conn: &mut PgConnection,
    agent: Uuid,
    old_harness: &str,
) -> Result<u64, DbError> {
    let old: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, harness_session_ref FROM conversations WHERE agent_id = $1 ORDER BY id",
    )
    .bind(agent)
    .fetch_all(&mut *conn)
    .await?;
    for (id, r) in &old {
        if let Some(r) = r.as_deref().filter(|r| !r.is_empty()) {
            enqueue_purge_of(&mut *conn, agent, Some(*id), r, Some(old_harness)).await?;
        }
    }
    Ok(sqlx::query(
        "UPDATE conversations SET harness_session_epoch = harness_session_epoch + 1,
             harness_session_ref = NULL, updated_at = now() WHERE agent_id = $1",
    )
    .bind(agent)
    .execute(conn)
    .await?
    .rows_affected())
}

// ------------------------------------------------------------------ retention sweep

/// Conversations of an org older than `days`, without an active turn or a pending approval.
pub async fn retention_candidates(
    conn: &mut PgConnection,
    org: Uuid,
    days: i64,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT c.id FROM conversations c JOIN workspaces w ON w.id = c.workspace_id
         WHERE w.org_id = $1 AND c.last_activity_at < now() - make_interval(days => $2::int)
           AND NOT EXISTS (SELECT 1 FROM turns t WHERE t.conversation_id = c.id AND t.role = 'turn'
                           AND t.status IN ('queued','dispatched','running'))
           AND NOT EXISTS (SELECT 1 FROM approvals a WHERE a.conversation_id = c.id AND a.status = 'pending')
           AND NOT EXISTS (SELECT 1 FROM action_approvals a WHERE a.conversation_id = c.id AND a.status = 'pending')
         ORDER BY c.last_activity_at, c.id LIMIT $3",
    )
    .bind(org)
    .bind(days)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Orgs with a conversation retention window `(org_id, days)`.
pub async fn orgs_with_retention(conn: &mut PgConnection) -> Result<Vec<(Uuid, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, (settings #>> '{retention,conversations_days}')::bigint FROM organizations
         WHERE settings #>> '{retention,conversations_days}' IS NOT NULL",
    )
    .fetch_all(conn)
    .await?)
}

// ------------------------------------------------------------------ org retention setting

pub async fn org_conversations_days(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT (settings #>> '{retention,conversations_days}')::bigint FROM organizations WHERE id = $1",
    )
    .bind(org)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// Set or clear `settings.retention.conversations_days`, keeping the other retention keys.
pub async fn set_org_conversations_days(
    conn: &mut PgConnection,
    org: Uuid,
    days: Option<i64>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE organizations SET updated_at = now(), settings = CASE
             WHEN $2::bigint IS NULL THEN settings #- '{retention,conversations_days}'
             ELSE jsonb_set(settings, '{retention}',
                  COALESCE(settings->'retention', '{}'::jsonb)
                  || jsonb_build_object('conversations_days', $2::bigint), true) END
         WHERE id = $1",
    )
    .bind(org)
    .bind(days)
    .execute(conn)
    .await?;
    Ok(())
}

/// Set or clear one integer key of `settings.retention` (RFE-015: `attachments_days`,
/// `attachment_quota_mib`), keeping the others. `key` is a code constant, never user input.
pub async fn set_org_retention_key(
    conn: &mut PgConnection,
    org: Uuid,
    key: &str,
    value: Option<i64>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE organizations SET updated_at = now(), settings = CASE
             WHEN $3::bigint IS NULL THEN settings #- ARRAY['retention', $2]
             ELSE jsonb_set(settings, '{retention}',
                  COALESCE(settings->'retention', '{}'::jsonb)
                  || jsonb_build_object($2::text, $3::bigint), true) END
         WHERE id = $1",
    )
    .bind(org)
    .bind(key)
    .bind(value)
    .execute(conn)
    .await?;
    Ok(())
}

// ------------------------------------------------------------------ export

/// `(events after seq, approximate bytes of events + turn text)`.
pub async fn export_size(
    conn: &mut PgConnection,
    conversation: Uuid,
    after_seq: i64,
) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM transcript_events WHERE conversation_id = $1 AND seq > $2),
                (SELECT COALESCE(sum(octet_length(payload::text)), 0)::bigint FROM transcript_events
                  WHERE conversation_id = $1 AND seq > $2)
              + (SELECT COALESCE(sum(octet_length(text)), 0)::bigint FROM turns WHERE conversation_id = $1)",
    )
    .bind(conversation)
    .bind(after_seq)
    .fetch_one(conn)
    .await?)
}

/// Turns of a conversation as JSON objects (content only as stored; redacted text is empty).
pub async fn export_turns(
    conn: &mut PgConnection,
    conversation: Uuid,
) -> Result<Vec<serde_json::Value>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object('id', id, 'role', role, 'status', status, 'parent_turn_id', parent_turn_id,
             'door', door, 'speaker_kind', speaker_kind, 'speaker_label', speaker_label, 'text', text,
             'error_code', error_code, 'usage', usage, 'created_at', created_at,
             'started_at', started_at, 'finished_at', finished_at)
         FROM turns WHERE conversation_id = $1 ORDER BY created_at, id",
    )
    .bind(conversation)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportAttachment {
    pub id: Uuid,
    pub filename: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub content_type: String,
    pub direction: String,
    pub blob_backend: String,
    pub blob_key: String,
    /// Bytes deleted by retention or an explicit delete (RFE-015).
    pub purged: bool,
}

pub async fn export_attachments(
    conn: &mut PgConnection,
    conversation: Uuid,
) -> Result<Vec<ExportAttachment>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, filename, size_bytes, sha256, content_type, direction, blob_backend, blob_key,
                purged_at IS NOT NULL AS purged
         FROM attachments WHERE conversation_id = $1 AND status = 'bound' ORDER BY created_at, id",
    )
    .bind(conversation)
    .fetch_all(conn)
    .await?)
}

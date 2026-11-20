//! The turn's person (P5-PERSON, C-1052, SEC-139; generalizes the calendar's SEC-108 rule).
//!
//! The person a turn runs for is the person who spoke (door `web`, `api` or `thread`), or, on a
//! schedule turn with no speaker, the schedule's creator. Webhook, watch, peer, eval and system
//! turns, and creator-less schedules, have none. A user counts only while they are not disabled,
//! are an active member of the agent's organization **and** hold `can_use` on the agent's
//! workspace (the effective view, so a workspace suspension counts). Everything is read from the
//! database at the moment of the call, never from the turn's text.

use aurora_db::repo::turns::TurnRow;
use aurora_db::repo::{members, schedules, users};
use aurora_db::{DbError, authz};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

/// The turn's person, as the account says now.
#[derive(Debug, Clone)]
pub struct TurnPerson {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    pub email_verified: bool,
    /// The person's own IANA time zone (C-1054); `None` = the workspace's.
    pub timezone: Option<String>,
}

/// Where the turn runs.
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub agent_id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
}

impl From<&aurora_db::repo::agents::AgentRow> for Scope {
    fn from(a: &aurora_db::repo::agents::AgentRow) -> Self {
        Self {
            agent_id: a.id,
            org_id: a.org_id,
            workspace_id: a.workspace_id,
        }
    }
}

/// The turn's door kind (`web`, `thread`, `schedule`, …; `system` when absent).
pub fn door_kind(turn: &TurnRow) -> &str {
    turn.door
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("system")
}

/// The schedule a `schedule` turn came from (`door.ref`).
pub fn schedule_ref(turn: &TurnRow) -> Option<Uuid> {
    (door_kind(turn) == "schedule")
        .then(|| turn.door.get("ref").and_then(Value::as_str))
        .flatten()
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// The candidate before the account checks: the person speaker of a `web`/`api`/`thread` turn,
/// or the creator of the turn's own agent's schedule.
async fn candidate(
    conn: &mut PgConnection,
    turn: &TurnRow,
    scope: Scope,
) -> Result<Option<Uuid>, DbError> {
    if turn.agent_id != scope.agent_id {
        return Ok(None);
    }
    match door_kind(turn) {
        "web" | "api" | "thread" => Ok(turn
            .speaker_user_id
            .filter(|_| turn.speaker_kind == "person")),
        "schedule" if turn.speaker_user_id.is_none() => {
            let Some(id) = schedule_ref(turn) else {
                return Ok(None);
            };
            Ok(schedules::get(&mut *conn, id)
                .await?
                .filter(|s| s.agent_id == scope.agent_id)
                .and_then(|s| s.creator_user_id))
        }
        _ => Ok(None),
    }
}

/// The turn's person (C-1052), or `None`.
pub async fn of_turn(
    conn: &mut PgConnection,
    turn: &TurnRow,
    scope: Scope,
) -> Result<Option<TurnPerson>, DbError> {
    let Some(who) = candidate(conn, turn, scope).await? else {
        return Ok(None);
    };
    let Some(user) = users::find_by_id(&mut *conn, who).await? else {
        return Ok(None);
    };
    if user.disabled_at.is_some() {
        return Ok(None);
    }
    let active = members::get(&mut *conn, scope.org_id, who)
        .await?
        .is_some_and(|m| m.status == "active");
    if !active || !authz::can_use(&mut *conn, who, scope.workspace_id).await? {
        return Ok(None);
    }
    let timezone = users::timezone(&mut *conn, who).await?;
    Ok(Some(TurnPerson {
        user_id: who,
        email_verified: user.email_verified_at.is_some() && !user.email.trim().is_empty(),
        display_name: user.display_name,
        email: user.email,
        timezone,
    }))
}

/// [`of_turn`] by turn id; a turn of another conversation has no person here.
pub async fn of_turn_id(
    conn: &mut PgConnection,
    turn: Option<Uuid>,
    conversation: Uuid,
    scope: Scope,
) -> Result<Option<TurnPerson>, DbError> {
    let Some(t) = turn else {
        return Ok(None);
    };
    let Some(row) = aurora_db::repo::turns::get(&mut *conn, t)
        .await?
        .filter(|r| r.conversation_id == conversation)
    else {
        return Ok(None);
    };
    of_turn(conn, &row, scope).await
}

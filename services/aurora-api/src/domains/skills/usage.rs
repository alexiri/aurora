//! (P5-SKILLS-1, C-1175, D-505, SEC-158) What a skill is used for: a use is the agent loading
//! it, either through the `skill_get` tool or through the harness's own skill loading as the
//! supervisor reports it. One row per turn and skill at most; kept
//! [`limits::SKILL_USE_RETENTION_DAYS`] days. Reading an agent's usage names the last
//! conversation only to its readers ([`conversation_refs`]).

use std::collections::HashMap;

use aurora_core::ApiError;
use aurora_core::limits;
use aurora_db::DbError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::skills as repo;
use sqlx::PgConnection;
use uuid::Uuid;

use super::dto::{SkillLastUse, SkillUsage};
use crate::app::AppState;
use crate::domains::conversations::reference::conversation_refs;
use crate::http::auth::Caller;

/// Records that `agent` loaded the skill of folder `name` in `conversation` (and `turn`): the
/// skill that reaches the agent **now**, after its switches ([`repo::resolve`]). A name no
/// stored skill holds (an app skill, a stray report) records nothing. With `only`, the winner
/// must be that skill (a read of a skill that does not apply is no use).
///
/// The caller has established that `conversation` and `turn` are the agent's own.
pub async fn record_loaded(
    conn: &mut PgConnection,
    agent: &AgentRow,
    conversation: Uuid,
    turn: Option<Uuid>,
    name: &str,
    only: Option<Uuid>,
) -> Result<(), DbError> {
    // The name may come from a report: only a folder name is looked up.
    if aurora_registry::skill_format::check_name(name).is_err() {
        return Ok(());
    }
    let rows =
        repo::list_reaching(conn, agent.org_id, agent.workspace_id, agent.id, Some(name)).await?;
    let Some(winner) = repo::resolve(rows).into_iter().find_map(|g| g.winner) else {
        return Ok(());
    };
    if only.is_some_and(|id| id != winner.row.id) {
        return Ok(());
    }
    repo::record_use(conn, winner.row.id, agent.id, conversation, turn).await
}

/// For each of `skills`: how many distinct agents used it in the last
/// [`limits::SKILL_USED_BY_DAYS`] days (absent: none).
pub async fn used_by_agents(
    conn: &mut PgConnection,
    skills: &[Uuid],
) -> Result<HashMap<Uuid, i64>, DbError> {
    Ok(
        repo::used_by_agents(conn, skills, limits::SKILL_USED_BY_DAYS)
            .await?
            .into_iter()
            .collect(),
    )
}

/// The agent's usage of each of `skills`, as `caller` may see it: the last use names its
/// conversation only when the caller may read it.
pub async fn of_agent(
    s: &AppState,
    caller: &Caller,
    agent: Uuid,
    skills: &[Uuid],
) -> Result<HashMap<Uuid, SkillUsage>, ApiError> {
    let rows = {
        let mut c = s.pool.acquire().await.map_err(DbError::from)?;
        repo::usage_of_agent(
            &mut c,
            agent,
            skills,
            limits::SKILL_USE_WEEK_DAYS,
            limits::SKILL_USE_RETENTION_DAYS,
        )
        .await?
    };
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.last_conversation_id).collect();
    let refs = conversation_refs(s, caller, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let conversation = r.last_conversation_id.and_then(|id| refs.get(&id).cloned());
            (
                r.skill_id,
                SkillUsage {
                    week: r.week,
                    last: Some(SkillLastUse {
                        at: r.last_at,
                        conversation,
                    }),
                },
            )
        })
        .collect())
}

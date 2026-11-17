//! Evals (phase 5, P4-EVAL-1, docs/10 §Evals): an agent's questions, its eval settings and the
//! inputs of the configuration fingerprint. Expected answers are agent-audience data (SEC-116):
//! nothing here logs or audits them.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// One eval question. Numbers are read as `float8` (the column is `numeric`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QuestionRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub position: i32,
    pub question: String,
    pub expected_kind: String,
    pub expected_number: Option<f64>,
    pub tolerance: Option<f64>,
    pub expected_text: Option<String>,
    pub criteria: Option<String>,
    pub enabled: bool,
    pub version: i32,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, agent_id, workspace_id, position, question, expected_kind, \
    expected_number::float8 AS expected_number, tolerance::float8 AS tolerance, expected_text, \
    criteria, enabled, version, created_by, created_at, updated_at";

/// A question to store (already validated). `position` is assigned: the end of the list.
#[derive(Debug, Clone)]
pub struct NewQuestion<'a> {
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub question: &'a str,
    pub expected_kind: &'a str,
    pub expected_number: Option<f64>,
    pub tolerance: Option<f64>,
    pub expected_text: Option<&'a str>,
    pub criteria: Option<&'a str>,
    pub enabled: bool,
    pub created_by: Option<Uuid>,
}

/// Serialises the writers of one agent's question list (the cap and the positions).
pub async fn lock_agent_questions(conn: &mut PgConnection, agent: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('eval_questions:' || $1::text, 0))")
        .bind(agent)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn list_questions<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<QuestionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM eval_questions WHERE agent_id = $1 ORDER BY position"
    )))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

pub async fn count_questions<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM eval_questions WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(exec)
            .await?,
    )
}

pub async fn get_question<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<QuestionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM eval_questions WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

pub async fn get_question_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<QuestionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM eval_questions WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Appends a question at the end of the agent's list.
pub async fn insert_question(
    conn: &mut PgConnection,
    q: &NewQuestion<'_>,
) -> Result<QuestionRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO eval_questions (id, agent_id, workspace_id, position, question, expected_kind, \
            expected_number, tolerance, expected_text, criteria, enabled, created_by) \
         VALUES ($1, $2, $3, \
            (SELECT COALESCE(MAX(position), 0) + 1 FROM eval_questions WHERE agent_id = $2), \
            $4, $5, $6::float8::numeric, $7::float8::numeric, $8, $9, $10, $11) \
         RETURNING {COLS}"
    )))
    .bind(Uuid::now_v7())
    .bind(q.agent_id)
    .bind(q.workspace_id)
    .bind(q.question)
    .bind(q.expected_kind)
    .bind(q.expected_number)
    .bind(q.tolerance)
    .bind(q.expected_text)
    .bind(q.criteria)
    .bind(q.enabled)
    .bind(q.created_by)
    .fetch_one(conn)
    .await?)
}

/// Writes the editable fields of `q` and bumps `version`.
pub async fn save_question(
    conn: &mut PgConnection,
    q: &QuestionRow,
) -> Result<QuestionRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE eval_questions SET question = $2, expected_number = $3::float8::numeric, \
            tolerance = $4::float8::numeric, expected_text = $5, criteria = $6, enabled = $7, \
            version = version + 1, updated_at = now() \
         WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(q.id)
    .bind(&q.question)
    .bind(q.expected_number)
    .bind(q.tolerance)
    .bind(&q.expected_text)
    .bind(&q.criteria)
    .bind(q.enabled)
    .fetch_one(conn)
    .await?)
}

/// Moves a question to `to` (1-based, already clamped to the list), shifting the ones between.
/// Does not bump `version`: the order is not part of a question.
pub async fn move_question(
    conn: &mut PgConnection,
    id: Uuid,
    agent: Uuid,
    from: i32,
    to: i32,
) -> Result<(), DbError> {
    if from == to {
        return Ok(());
    }
    sqlx::query("SET CONSTRAINTS eval_questions_position_key DEFERRED")
        .execute(&mut *conn)
        .await?;
    if to > from {
        sqlx::query(
            "UPDATE eval_questions SET position = position - 1 \
             WHERE agent_id = $1 AND position > $2 AND position <= $3",
        )
    } else {
        sqlx::query(
            "UPDATE eval_questions SET position = position + 1 \
             WHERE agent_id = $1 AND position < $2 AND position >= $3",
        )
    }
    .bind(agent)
    .bind(from)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE eval_questions SET position = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(to)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Deletes a question and closes the gap in the positions.
pub async fn delete_question(conn: &mut PgConnection, q: &QuestionRow) -> Result<(), DbError> {
    sqlx::query("SET CONSTRAINTS eval_questions_position_key DEFERRED")
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM eval_questions WHERE id = $1")
        .bind(q.id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE eval_questions SET position = position - 1 \
         WHERE agent_id = $1 AND position > $2",
    )
    .bind(q.agent_id)
    .bind(q.position)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---- settings ----

/// An agent's eval settings.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SettingsRow {
    pub judge_secret_id: Option<Uuid>,
    pub judge_model_id: Option<String>,
    pub regression_notify: bool,
    /// The judge secret's display name (for the owner's settings view only).
    pub judge_secret_name: Option<String>,
}

pub async fn get_settings<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Option<SettingsRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.eval_judge_secret_id AS judge_secret_id, \
                CASE WHEN a.eval_judge_secret_id IS NULL THEN NULL ELSE a.eval_judge_model_id END \
                    AS judge_model_id, \
                a.eval_regression_notify AS regression_notify, s.name AS judge_secret_name \
         FROM agents a LEFT JOIN secrets s ON s.id = a.eval_judge_secret_id \
         WHERE a.id = $1",
    )
    .bind(agent)
    .fetch_optional(exec)
    .await?)
}

/// Writes the settings; a `None` judge clears both columns. Does not bump the agent's
/// `home_sync_generation` (nothing here reaches the sandbox).
pub async fn set_settings(
    conn: &mut PgConnection,
    agent: Uuid,
    judge: Option<(Uuid, &str)>,
    regression_notify: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agents SET eval_judge_secret_id = $2, eval_judge_model_id = $3, \
            eval_regression_notify = $4, updated_at = now() WHERE id = $1",
    )
    .bind(agent)
    .bind(judge.map(|j| j.0))
    .bind(judge.map(|j| j.1))
    .bind(regression_notify)
    .execute(conn)
    .await?;
    Ok(())
}

// ---- fingerprint inputs ----

/// Logical ids of the policy rules compiled from the agent's grants (the rules "granted to the
/// agent", C-866), sorted.
pub async fn granted_rule_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT d.logical_id FROM derived_rules d \
         JOIN grants g ON d.source_kind = 'grant' AND d.source_id = g.id \
         WHERE g.agent_id = $1 ORDER BY d.logical_id",
    )
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

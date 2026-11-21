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
    /// (C-1101) The last test of the judge: `ok` | `failed`, null when never tested or no judge.
    pub judge_test_result: Option<String>,
    pub judge_test_reason: Option<String>,
    pub judge_test_ms: Option<i64>,
    pub judge_tested_at: Option<DateTime<Utc>>,
}

/// The stored outcome of one judge test (C-1101).
pub struct JudgeTestRow<'a> {
    pub result: &'a str,
    pub reason: Option<&'a str>,
    pub took_ms: i64,
}

/// Replaces the stored test of the judge (`None` clears it). Called in the transaction that changes the judge.
pub async fn set_judge_test(
    conn: &mut PgConnection,
    agent: Uuid,
    test: Option<JudgeTestRow<'_>>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agents SET eval_judge_test_result = $2, eval_judge_test_reason = $3, \
            eval_judge_test_ms = $4, eval_judge_tested_at = CASE WHEN $2::text IS NULL THEN NULL ELSE now() END \
         WHERE id = $1",
    )
    .bind(agent)
    .bind(test.as_ref().map(|t| t.result))
    .bind(test.as_ref().and_then(|t| t.reason))
    .bind(test.as_ref().map(|t| t.took_ms))
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn get_settings<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Option<SettingsRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.eval_judge_secret_id AS judge_secret_id, \
                CASE WHEN a.eval_judge_secret_id IS NULL THEN NULL ELSE a.eval_judge_model_id END \
                    AS judge_model_id, \
                a.eval_regression_notify AS regression_notify, s.name AS judge_secret_name, \
                CASE WHEN a.eval_judge_secret_id IS NULL THEN NULL ELSE a.eval_judge_test_result END \
                    AS judge_test_result, \
                a.eval_judge_test_reason AS judge_test_reason, a.eval_judge_test_ms AS judge_test_ms, \
                a.eval_judge_tested_at AS judge_tested_at \
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

// ---- runs (P4-EVAL-2, C-864, C-865) ----

/// One eval run. `started_by` is the person who started it (null for a scheduled run).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub trigger: String,
    pub schedule_id: Option<Uuid>,
    pub schedule_name: Option<String>,
    pub started_by_user_id: Option<Uuid>,
    pub started_by_label: Option<String>,
    pub status: String,
    pub question_set_version: String,
    pub fingerprint: serde_json::Value,
    pub fingerprint_hash: String,
    pub questions_total: i32,
    pub questions_passed: i32,
    pub questions_failed: i32,
    pub questions_errored: i32,
    pub questions_not_scored: i32,
    pub refused_writes: i32,
    pub judge_usage: Option<serde_json::Value>,
    pub regression: Option<serde_json::Value>,
    pub error_code: Option<String>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

const RUN_COLS: &str = "r.id, r.agent_id, r.workspace_id, r.trigger, r.schedule_id, \
    s.name AS schedule_name, r.started_by_user_id, u.display_name AS started_by_label, r.status, \
    r.question_set_version, r.fingerprint, r.fingerprint_hash, r.questions_total, \
    CASE WHEN live.p IS NULL THEN r.questions_passed ELSE live.p END AS questions_passed, \
    CASE WHEN live.p IS NULL THEN r.questions_failed ELSE live.f END AS questions_failed, \
    CASE WHEN live.p IS NULL THEN r.questions_errored ELSE live.e END AS questions_errored, \
    CASE WHEN live.p IS NULL THEN r.questions_not_scored ELSE live.n END AS questions_not_scored, \
    CASE WHEN live.p IS NULL THEN r.refused_writes ELSE live.w END AS refused_writes, \
    r.judge_usage, r.regression, r.error_code, r.cancel_requested_at, \
    r.created_at, r.started_at, r.finished_at";
/// While a run is `queued` or `running` its counts are read from the rows that have ended so far
/// (C-864: the stored columns are only written when the run finishes); a finished run reads them.
const RUN_FROM: &str = "FROM eval_runs r LEFT JOIN users u ON u.id = r.started_by_user_id \
    LEFT JOIN eval_schedules s ON s.id = r.schedule_id \
    LEFT JOIN LATERAL (SELECT count(*) FILTER (WHERE outcome = 'pass')::int AS p, \
        count(*) FILTER (WHERE outcome = 'fail')::int AS f, \
        count(*) FILTER (WHERE outcome = 'error')::int AS e, \
        count(*) FILTER (WHERE outcome = 'not_scored')::int AS n, \
        COALESCE(sum(refused_writes), 0)::int AS w \
        FROM eval_results er WHERE er.run_id = r.id AND er.finished_at IS NOT NULL \
          AND r.status IN ('queued','running')) live ON r.status IN ('queued','running')";

pub struct NewRun<'a> {
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub trigger: &'a str,
    pub schedule_id: Option<Uuid>,
    pub started_by: Option<Uuid>,
    pub question_set_version: &'a str,
    pub fingerprint: &'a serde_json::Value,
    pub fingerprint_hash: &'a str,
    pub questions_total: i32,
}

/// Insert a `queued` run. A second active run of the agent violates the partial unique index
/// (the caller maps it to 409 `eval_run_active`).
pub async fn insert_run(conn: &mut PgConnection, n: &NewRun<'_>) -> Result<Uuid, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO eval_runs (id, agent_id, workspace_id, trigger, schedule_id, started_by_user_id,
            question_set_version, fingerprint, fingerprint_hash, questions_total)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(id)
    .bind(n.agent_id)
    .bind(n.workspace_id)
    .bind(n.trigger)
    .bind(n.schedule_id)
    .bind(n.started_by)
    .bind(n.question_set_version)
    .bind(n.fingerprint)
    .bind(n.fingerprint_hash)
    .bind(n.questions_total)
    .execute(conn)
    .await?;
    Ok(id)
}

pub async fn get_run<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<RunRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLS} {RUN_FROM} WHERE r.id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Lock the run row (lock order: conversation, then run).
pub async fn lock_run(conn: &mut PgConnection, id: Uuid) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT status FROM eval_runs WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Runs of an agent, newest first; `after` is `(created_at, id)`.
pub async fn list_runs<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    status: Option<&str>,
    trigger: Option<&str>,
    after: Option<(DateTime<Utc>, Uuid)>,
    fetch: i64,
) -> Result<Vec<RunRow>, DbError> {
    let (t, id) = match after {
        Some((t, id)) => (Some(t), Some(id)),
        None => (None, None),
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLS} {RUN_FROM} WHERE r.agent_id = $1
           AND ($2::text IS NULL OR r.status = $2) AND ($3::text IS NULL OR r.trigger = $3)
           AND ($4::timestamptz IS NULL OR (r.created_at, r.id) < ($4, $5))
         ORDER BY r.created_at DESC, r.id DESC LIMIT $6"
    )))
    .bind(agent)
    .bind(status)
    .bind(trigger)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Run starts of the agent since the start of the current UTC day (manual and scheduled).
pub async fn runs_started_today<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM eval_runs WHERE agent_id = $1
           AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
    )
    .bind(agent)
    .fetch_one(exec)
    .await?)
}

/// How many active (`queued` or `running`) runs the agent has (at most one: the partial unique
/// index `eval_runs_active_key`; `EVAL_RUNS_ACTIVE_PER_AGENT`).
pub async fn active_run_count<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM eval_runs WHERE agent_id = $1 AND status IN ('queued','running')",
    )
    .bind(agent)
    .fetch_one(exec)
    .await?)
}

/// The agent's active run, if any.
pub async fn active_run<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM eval_runs WHERE agent_id = $1 AND status IN ('queued','running')",
    )
    .bind(agent)
    .fetch_optional(exec)
    .await?)
}

/// Claim active runs whose lease is free, under `FOR UPDATE SKIP LOCKED`; sets the lease.
pub async fn claim_runs(
    conn: &mut PgConnection,
    lease_secs: f64,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE eval_runs SET lease_until = now() + make_interval(secs => $1), updated_at = now()
         WHERE id IN (
            SELECT id FROM eval_runs WHERE status IN ('queued','running')
              AND (lease_until IS NULL OR lease_until <= now())
            ORDER BY created_at, id LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING id",
    )
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Release the claim lease at the end of a tick.
pub async fn release_lease(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE eval_runs SET lease_until = NULL WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Keep the claim for `secs` more (a judge call runs outside the tick, P4-EVAL-3).
pub async fn extend_lease(conn: &mut PgConnection, id: Uuid, secs: f64) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_runs SET lease_until = now() + make_interval(secs => $2) WHERE id = $1",
    )
    .bind(id)
    .bind(secs)
    .execute(conn)
    .await?;
    Ok(())
}

/// `queued → running` (fenced).
pub async fn mark_running(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_runs SET status = 'running', started_at = COALESCE(started_at, now()),
            updated_at = now() WHERE id = $1 AND status = 'queued'",
    )
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Ask a run to cancel (idempotent). `false` when it is not active.
pub async fn request_cancel(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE eval_runs SET cancel_requested_at = COALESCE(cancel_requested_at, now()),
            lease_until = NULL, updated_at = now()
         WHERE id = $1 AND status IN ('queued','running')",
    )
    .bind(id)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Add judge token usage to the run (`eval_runs.judge_usage`; never a turn's usage).
pub async fn add_judge_usage(
    conn: &mut PgConnection,
    id: Uuid,
    input: i64,
    output: i64,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_runs SET judge_usage = jsonb_build_object(
            'input_tokens', COALESCE((judge_usage->>'input_tokens')::bigint, 0) + $2,
            'output_tokens', COALESCE((judge_usage->>'output_tokens')::bigint, 0) + $3),
            updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(input)
    .bind(output)
    .execute(conn)
    .await?;
    Ok(())
}

/// Finish a run: status, counts recomputed from the results, the end reason, the regression.
pub async fn finish_run(
    conn: &mut PgConnection,
    id: Uuid,
    status: &str,
    error_code: Option<&str>,
    regression: Option<&serde_json::Value>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_runs r SET status = $2, error_code = $3, regression = $4,
            finished_at = now(), lease_until = NULL, updated_at = now(),
            questions_passed = c.p, questions_failed = c.f, questions_errored = c.e,
            questions_not_scored = c.n, refused_writes = c.w
         FROM (SELECT count(*) FILTER (WHERE outcome = 'pass')::int AS p,
                      count(*) FILTER (WHERE outcome = 'fail')::int AS f,
                      count(*) FILTER (WHERE outcome = 'error')::int AS e,
                      count(*) FILTER (WHERE outcome = 'not_scored')::int AS n,
                      COALESCE(sum(refused_writes), 0)::int AS w
               FROM eval_results WHERE run_id = $1) c
         WHERE r.id = $1 AND r.status IN ('queued','running')",
    )
    .bind(id)
    .bind(status)
    .bind(error_code)
    .bind(regression)
    .execute(conn)
    .await?;
    Ok(())
}

/// `(passed, failed)` over the run's results.
pub async fn pass_fail_counts(conn: &mut PgConnection, run: Uuid) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE outcome = 'pass'), count(*) FILTER (WHERE outcome = 'fail')
         FROM eval_results WHERE run_id = $1",
    )
    .bind(run)
    .fetch_one(conn)
    .await?)
}

/// Mark every result of the run that has not ended `not_scored` with `reason` (cancel, freeze,
/// access lost).
pub async fn close_open_results(
    conn: &mut PgConnection,
    run: Uuid,
    reason: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_results SET outcome = 'not_scored', reason_code = $2, finished_at = now(),
            updated_at = now() WHERE run_id = $1 AND finished_at IS NULL",
    )
    .bind(run)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(())
}

/// The previous `done` run of the same question-set version, with its pass rate (percent).
pub async fn previous_done_pass_rate(
    conn: &mut PgConnection,
    agent: Uuid,
    version: &str,
    before: Uuid,
) -> Result<Option<(Uuid, f64)>, DbError> {
    let row: Option<(Uuid, i32, i32)> = sqlx::query_as(
        "SELECT id, questions_passed, questions_failed FROM eval_runs
         WHERE agent_id = $1 AND question_set_version = $2 AND status = 'done' AND id <> $3
           AND finished_at IS NOT NULL ORDER BY finished_at DESC, id DESC LIMIT 1",
    )
    .bind(agent)
    .bind(version)
    .bind(before)
    .fetch_optional(conn)
    .await?;
    Ok(row
        .and_then(|(id, p, f)| (p + f > 0).then(|| (id, f64::from(p) * 100.0 / f64::from(p + f)))))
}

/// Delete a finished run (results cascade). The caller deletes its conversations first.
pub async fn delete_run(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM eval_runs WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Finished runs older than `secs`, oldest first.
pub async fn expired_runs<'e, E: PgExecutor<'e>>(
    exec: E,
    secs: f64,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM eval_runs WHERE finished_at IS NOT NULL
           AND finished_at < now() - make_interval(secs => $1) ORDER BY finished_at LIMIT $2",
    )
    .bind(secs)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

// ---- results ----

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ResultRow {
    pub id: Uuid,
    pub run_id: Uuid,
    pub position: i32,
    pub question_id: Option<Uuid>,
    pub question_text: String,
    pub expected_kind: String,
    pub expected_snapshot: serde_json::Value,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub outcome: String,
    pub reason_code: Option<String>,
    pub extracted_value: Option<String>,
    pub answer_text: Option<String>,
    pub judge_verdict: Option<String>,
    pub judge_rationale: Option<String>,
    pub refused_writes: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

const RESULT_COLS: &str = "id, run_id, position, question_id, question_text, expected_kind, \
    expected_snapshot, conversation_id, turn_id, outcome, reason_code, extracted_value, \
    answer_text, judge_verdict, judge_rationale, refused_writes, started_at, finished_at";

/// The enabled questions of an agent in position order, for a new run's snapshot.
pub async fn enabled_questions<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<QuestionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM eval_questions WHERE agent_id = $1 AND enabled ORDER BY position"
    )))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// The agent's questions among `ids`, in position order (a foreign or unknown id is simply absent,
/// so the caller compares lengths).
pub async fn questions_by_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    ids: &[Uuid],
) -> Result<Vec<QuestionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM eval_questions WHERE agent_id = $1 AND id = ANY($2) ORDER BY position"
    )))
    .bind(agent)
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// Create a run's result rows at start (one per question, `not_scored` with no `finished_at`
/// until the question ends), snapshotting question text and expected answer. Positions are
/// 1-based over the enabled questions at start.
pub async fn insert_results(
    conn: &mut PgConnection,
    run: Uuid,
    questions: &[QuestionRow],
) -> Result<(), DbError> {
    for (i, q) in questions.iter().enumerate() {
        let snapshot = serde_json::json!({
            "number": q.expected_number, "tolerance": q.tolerance,
            "text": q.expected_text, "criteria": q.criteria,
        });
        sqlx::query(
            "INSERT INTO eval_results (id, run_id, position, question_id, question_text,
                expected_kind, expected_snapshot) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(Uuid::now_v7())
        .bind(run)
        .bind(i32::try_from(i + 1).unwrap_or(i32::MAX))
        .bind(q.id)
        .bind(&q.question)
        .bind(&q.expected_kind)
        .bind(snapshot)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn get_result<'e, E: PgExecutor<'e>>(
    exec: E,
    run: Uuid,
    position: i32,
) -> Result<Option<ResultRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RESULT_COLS} FROM eval_results WHERE run_id = $1 AND position = $2"
    )))
    .bind(run)
    .bind(position)
    .fetch_optional(exec)
    .await?)
}

/// The first result of the run that has not ended (no `finished_at`), in position order.
pub async fn next_open_result<'e, E: PgExecutor<'e>>(
    exec: E,
    run: Uuid,
) -> Result<Option<ResultRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RESULT_COLS} FROM eval_results WHERE run_id = $1 AND finished_at IS NULL
         ORDER BY position LIMIT 1"
    )))
    .bind(run)
    .fetch_optional(exec)
    .await?)
}

pub async fn list_results<'e, E: PgExecutor<'e>>(
    exec: E,
    run: Uuid,
    after: i32,
    limit: i64,
) -> Result<Vec<ResultRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RESULT_COLS} FROM eval_results WHERE run_id = $1 AND position > $2
         ORDER BY position LIMIT $3"
    )))
    .bind(run)
    .bind(after)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// A question starts: its conversation and turn are recorded (fenced on not yet ended).
pub async fn start_result(
    conn: &mut PgConnection,
    id: Uuid,
    conversation: Uuid,
    turn: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_results SET conversation_id = $2, turn_id = $3,
            started_at = COALESCE(started_at, now()), updated_at = now()
         WHERE id = $1 AND finished_at IS NULL",
    )
    .bind(id)
    .bind(conversation)
    .bind(turn)
    .execute(conn)
    .await?;
    Ok(())
}

/// The scored outcome of a question (fenced: written once).
#[derive(Debug, Clone, Default)]
pub struct Scored {
    pub outcome: &'static str,
    pub reason_code: Option<String>,
    pub extracted_value: Option<String>,
    pub answer_text: Option<String>,
    pub judge_verdict: Option<String>,
    pub judge_rationale: Option<String>,
}

pub async fn finish_result(conn: &mut PgConnection, id: Uuid, s: &Scored) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE eval_results SET outcome = $2, reason_code = $3, extracted_value = $4,
            answer_text = $5, judge_verdict = $6, judge_rationale = $7,
            started_at = COALESCE(started_at, now()), finished_at = now(), updated_at = now()
         WHERE id = $1 AND finished_at IS NULL",
    )
    .bind(id)
    .bind(s.outcome)
    .bind(&s.reason_code)
    .bind(&s.extracted_value)
    .bind(&s.answer_text)
    .bind(&s.judge_verdict)
    .bind(&s.judge_rationale)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Count one refused tool write against the open result of the turn (platform tools).
pub async fn add_tool_refusal(conn: &mut PgConnection, turn: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_results SET refused_writes = refused_writes + 1,
            tool_refusals = tool_refusals + 1, updated_at = now()
         WHERE turn_id = $1 AND finished_at IS NULL",
    )
    .bind(turn)
    .execute(conn)
    .await?;
    Ok(())
}

/// Set every result's `refused_writes` to its tool refusals plus the gateway's `eval_read_only`
/// rows of the request log for its turn (idempotent; run after the log has been flushed).
pub async fn reconcile_refusals(conn: &mut PgConnection, run: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_results r SET refused_writes = r.tool_refusals + (
            SELECT count(*) FROM request_log l
            WHERE l.turn_id = r.turn_id AND l.decision = 'eval_read_only')::int
         WHERE r.run_id = $1 AND r.turn_id IS NOT NULL",
    )
    .bind(run)
    .execute(conn)
    .await?;
    Ok(())
}

/// When the run's last result ended (None while a result is open).
pub async fn last_result_end<'e, E: PgExecutor<'e>>(
    exec: E,
    run: Uuid,
) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT CASE WHEN bool_and(finished_at IS NOT NULL) THEN max(finished_at) END
         FROM eval_results WHERE run_id = $1",
    )
    .bind(run)
    .fetch_one(exec)
    .await?)
}

/// The conversations of a run's results (for deletion).
pub async fn run_conversations<'e, E: PgExecutor<'e>>(
    exec: E,
    run: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT conversation_id FROM eval_results WHERE run_id = $1 AND conversation_id IS NOT NULL",
    )
    .bind(run)
    .fetch_all(exec)
    .await?)
}

// ---- schedules (P4-EVAL-2, C-870) ----

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduleRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub cron: String,
    pub timezone: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub creator_user_id: Option<Uuid>,
    pub creator_label: Option<String>,
    pub anchor: String,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fire_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<String>,
    pub last_run_id: Option<Uuid>,
}

const SCHED_COLS: &str = "s.id, s.agent_id, s.name, s.cron, s.timezone, s.enabled, \
    s.disabled_reason, s.consecutive_failures, s.creator_user_id, \
    u.display_name AS creator_label, s.anchor, s.next_fire_at, s.last_fire_at, s.last_outcome, \
    s.last_run_id";
const SCHED_FROM: &str = "FROM eval_schedules s LEFT JOIN users u ON u.id = s.creator_user_id";

pub struct NewSchedule<'a> {
    pub agent_id: Uuid,
    pub name: &'a str,
    pub cron: &'a str,
    pub timezone: &'a str,
    pub enabled: bool,
    pub creator: Option<Uuid>,
    pub anchor: &'a str,
    pub next_fire_at: Option<DateTime<Utc>>,
}

pub async fn insert_schedule(
    conn: &mut PgConnection,
    n: &NewSchedule<'_>,
) -> Result<Uuid, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO eval_schedules (id, agent_id, name, cron, timezone, enabled, creator_user_id,
            anchor, next_fire_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(id)
    .bind(n.agent_id)
    .bind(n.name)
    .bind(n.cron)
    .bind(n.timezone)
    .bind(n.enabled)
    .bind(n.creator)
    .bind(n.anchor)
    .bind(n.next_fire_at)
    .execute(conn)
    .await?;
    Ok(id)
}

pub async fn get_schedule<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<ScheduleRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SCHED_COLS} {SCHED_FROM} WHERE s.id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

pub async fn list_schedules<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<ScheduleRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SCHED_COLS} {SCHED_FROM} WHERE s.agent_id = $1 ORDER BY s.created_at, s.id"
    )))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

pub async fn count_schedules<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM eval_schedules WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(exec)
            .await?,
    )
}

/// Edit a schedule (`None` keeps a field). `next_fire_at` is set when given (cron, timezone or
/// enabled changed); re-enabling clears the disable reason and the failure count.
#[allow(clippy::too_many_arguments)]
pub async fn update_schedule(
    conn: &mut PgConnection,
    id: Uuid,
    name: Option<&str>,
    cron: Option<(&str, &str)>,
    enable: Option<bool>,
    next_fire_at: Option<Option<DateTime<Utc>>>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_schedules SET name = COALESCE($2, name),
            cron = COALESCE($3, cron), timezone = COALESCE($4, timezone),
            enabled = COALESCE($5, enabled),
            disabled_reason = CASE WHEN $5 IS NULL THEN disabled_reason
                                   WHEN $5 THEN NULL ELSE 'user' END,
            consecutive_failures = CASE WHEN $5 IS TRUE THEN 0 ELSE consecutive_failures END,
            next_fire_at = CASE WHEN $6 THEN $7 ELSE next_fire_at END,
            updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(cron.map(|c| c.0))
    .bind(cron.map(|c| c.1))
    .bind(enable)
    .bind(next_fire_at.is_some())
    .bind(next_fire_at.flatten())
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_schedule(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM eval_schedules WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Claim due eval schedules under the fire lease.
pub async fn claim_due_schedules(
    conn: &mut PgConnection,
    lease_secs: f64,
    limit: i64,
) -> Result<Vec<ScheduleRow>, DbError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE eval_schedules SET fire_lease_until = now() + make_interval(secs => $1)
         WHERE id IN (
            SELECT id FROM eval_schedules
            WHERE enabled AND next_fire_at IS NOT NULL AND next_fire_at <= now()
              AND (fire_lease_until IS NULL OR fire_lease_until <= now())
            ORDER BY next_fire_at, id LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING id",
    )
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SCHED_COLS} {SCHED_FROM} WHERE s.id = ANY($1) ORDER BY s.next_fire_at, s.id"
    )))
    .bind(&ids)
    .fetch_all(conn)
    .await?)
}

/// Record a fire: the next occurrence, the outcome, the run; releases the lease.
pub async fn record_schedule_fire(
    conn: &mut PgConnection,
    id: Uuid,
    next: Option<DateTime<Utc>>,
    fired: bool,
    outcome: &str,
    run: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_schedules SET next_fire_at = $2, fire_lease_until = NULL,
            last_fire_at = CASE WHEN $3 THEN now() ELSE last_fire_at END, last_outcome = $4,
            last_run_id = COALESCE($5, last_run_id), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(next)
    .bind(fired)
    .bind(outcome)
    .bind(run)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn disable_schedule(
    conn: &mut PgConnection,
    id: Uuid,
    reason: &str,
    outcome: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE eval_schedules SET enabled = false, disabled_reason = $2, next_fire_at = NULL,
            fire_lease_until = NULL, last_outcome = COALESCE($3, last_outcome), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .bind(outcome)
    .execute(conn)
    .await?;
    Ok(())
}

/// A finished scheduled run counts toward the schedule's failure streak: `failed` increments
/// (returns the new count), anything else resets it.
pub async fn schedule_run_outcome(
    conn: &mut PgConnection,
    id: Uuid,
    failed: bool,
) -> Result<i32, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE eval_schedules SET consecutive_failures = CASE WHEN $2 THEN consecutive_failures + 1 ELSE 0 END,
            last_outcome = CASE WHEN $2 THEN 'failed' ELSE 'ok' END, updated_at = now()
         WHERE id = $1 RETURNING consecutive_failures",
    )
    .bind(id)
    .bind(failed)
    .fetch_optional(conn)
    .await?
    .unwrap_or(0))
}

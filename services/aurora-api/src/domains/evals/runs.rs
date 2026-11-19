//! Eval runs: start, list, read, cancel and delete (P4-EVAL-2; C-864, C-865, C-873). The run
//! itself is driven by the due-work arm (`engine.rs`). Authority: `can_use` of the agent's
//! workspace (a caller without it gets 404); deleting a run needs `can_own`. Expected answers
//! and answers are agent-audience data: nothing here puts them in an audit row or a log line.

use super::dto::{EvalResult, EvalRun, EvalRunDetail};
use super::service::{self, current_fingerprint, visible_agent};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::lifecycle::remove_blobs;
use crate::http::auth::{Caller, require_own};
use crate::http::pagination::{Cursor, Page, Pagination};
use aurora_core::{ApiError, limits};
use aurora_db::DbError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::evals::{self as repo, NewRun};
use aurora_db::repo::lifecycle;
use chrono::{DateTime, Duration, Utc};
use http::StatusCode;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// Results per page of `GET /eval-runs/{id}` (C-864).
const RESULTS_PAGE: i64 = 100;

pub fn run_404() -> ApiError {
    ApiError::not_found("Eval run not found.")
}

fn active_conflict() -> ApiError {
    ApiError::conflict(
        "eval_run_active",
        "This agent already has an eval run in progress.",
    )
}

/// Seconds until the next UTC midnight (`Retry-After` of 429 `eval_runs_limited`).
fn secs_to_midnight(now: DateTime<Utc>) -> u64 {
    let next = (now.date_naive() + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .map_or(now, |d| d.and_utc());
    u64::try_from((next - now).num_seconds().max(1)).unwrap_or(1)
}

/// Create a `queued` run for the agent inside the caller's transaction: the agent must not be
/// frozen (423), have enabled questions (422 `eval_no_questions`), no active run (409
/// `eval_run_active`) and a start left today (429 `eval_runs_limited`). The fingerprint and the
/// question set are snapshotted here and never change.
pub async fn start_tx(
    conn: &mut PgConnection,
    a: &AgentRow,
    fp: &super::dto::EvalFingerprint,
    trigger: &str,
    schedule: Option<Uuid>,
    started_by: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    crate::domains::freeze::service::ensure_agent_active(&mut *conn, a.id).await?;
    repo::lock_agent_questions(conn, a.id).await?;
    let questions = repo::enabled_questions(&mut *conn, a.id).await?;
    if questions.is_empty() {
        return Err(ApiError::unprocessable(
            "eval_no_questions",
            "Add at least one enabled question before starting a run.",
        ));
    }
    if repo::active_run_count(&mut *conn, a.id).await? >= limits::EVAL_RUNS_ACTIVE_PER_AGENT as i64
    {
        return Err(active_conflict());
    }
    if repo::runs_started_today(&mut *conn, a.id).await? >= limits::EVAL_RUNS_PER_AGENT_DAY as i64 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "eval_runs_limited",
            "This agent has reached its eval run limit for today.",
        )
        .with_retry_after(secs_to_midnight(Utc::now()))
        .with_details(json!({ "limit": limits::EVAL_RUNS_PER_AGENT_DAY })));
    }
    let id = repo::insert_run(
        conn,
        &NewRun {
            agent_id: a.id,
            workspace_id: a.workspace_id,
            trigger,
            schedule_id: schedule,
            started_by,
            question_set_version: &fp.question_set_version,
            fingerprint: &fp.fingerprint,
            fingerprint_hash: &fp.fingerprint_hash,
            questions_total: i32::try_from(questions.len()).unwrap_or(i32::MAX),
        },
    )
    .await
    .map_err(|e| match &e {
        DbError::UniqueViolation { .. } => active_conflict(),
        _ => e.into(),
    })?;
    repo::insert_results(conn, id, &questions).await?;
    Ok(id)
}

pub async fn start(s: &AppState, c: &Caller, agent: Uuid) -> Result<EvalRun, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    // The fingerprint is read before the transaction; nothing in it is secret.
    let fp = current_fingerprint(s, &a).await?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let id = start_tx(&mut tx, &a, &fp, "manual", None, Some(c.user_id)).await?;
    let run = repo::get_run(&mut *tx, id).await?.ok_or_else(run_404)?;
    crate::domains::conversations::service::notify_work(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        run_audit(
            AuditActor::from_caller(c),
            &a,
            id,
            "eval.run.start",
            json!({"agent_id": a.id, "questions": run.questions_total,
                   "fingerprint_hash": run.fingerprint_hash, "trigger": "manual"}),
        ),
    );
    Ok(EvalRun::new(&run, &a.name))
}

pub fn run_audit(
    actor: AuditActor,
    a: &AgentRow,
    id: Uuid,
    action: &str,
    meta: serde_json::Value,
) -> AuditEvent {
    AuditEvent::new(actor, action, Outcome::Success)
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("eval_run", id)
        .metadata(meta)
}

pub async fn list(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    status: Option<String>,
    trigger: Option<String>,
    page: (Option<u32>, Option<String>),
) -> Result<Page<EvalRun>, ApiError> {
    // Authorise before parsing the pagination (FIX-181 F7, C-1041).
    let a = visible_agent(s, c, agent).await?;
    let p = Pagination::new(page.0, page.1.as_deref())?;
    let bad = |f: &str, v: &str| service::invalid_filter(f, v);
    if let Some(v) = status.as_deref()
        && !["queued", "running", "done", "failed", "cancelled"].contains(&v)
    {
        return Err(bad("status", "queued, running, done, failed or cancelled"));
    }
    if let Some(v) = trigger.as_deref()
        && !["manual", "schedule"].contains(&v)
    {
        return Err(bad("trigger", "manual or schedule"));
    }
    let rows = repo::list_runs(
        &s.pool,
        a.id,
        status.as_deref(),
        trigger.as_deref(),
        p.after.map(|Cursor { t, id }| (t, id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Page {
        items: page
            .items
            .iter()
            .map(|r| EvalRun::new(r, &a.name))
            .collect(),
        next_cursor: page.next_cursor,
    })
}

/// The run and its agent for a caller who `can_use` the agent's workspace (404 otherwise).
async fn load(s: &AppState, c: &Caller, id: Uuid) -> Result<(repo::RunRow, AgentRow), ApiError> {
    let r = repo::get_run(&s.pool, id).await?.ok_or_else(run_404)?;
    let a = visible_agent(s, c, r.agent_id).await.map_err(|e| {
        if e.status() == StatusCode::NOT_FOUND {
            run_404()
        } else {
            e
        }
    })?;
    Ok((r, a))
}

pub async fn get(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    results_after: i32,
) -> Result<EvalRunDetail, ApiError> {
    let (r, a) = load(s, c, id).await?;
    let mut rows = repo::list_results(&s.pool, id, results_after, RESULTS_PAGE + 1).await?;
    let next = (rows.len() as i64 > RESULTS_PAGE).then(|| {
        rows.truncate(RESULTS_PAGE as usize);
        rows.last().map_or(0, |x| x.position)
    });
    Ok(EvalRunDetail {
        fingerprint: r.fingerprint.clone(),
        run: EvalRun::new(&r, &a.name),
        results: rows.into_iter().map(EvalResult::from).collect(),
        next_results_after: next,
    })
}

pub async fn cancel(s: &AppState, c: &Caller, id: Uuid) -> Result<EvalRun, ApiError> {
    let (r, a) = load(s, c, id).await?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let status = repo::lock_run(&mut tx, id).await?.ok_or_else(run_404)?;
    if !matches!(status.as_str(), "queued" | "running") {
        return Err(ApiError::conflict(
            "eval_run_finished",
            "This eval run has already finished.",
        ));
    }
    repo::request_cancel(&mut tx, id).await?;
    crate::domains::conversations::service::notify_work(&mut tx).await?;
    let run = repo::get_run(&mut *tx, id).await?.unwrap_or(r);
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        run_audit(
            AuditActor::from_caller(c),
            &a,
            id,
            "eval.run.cancel",
            json!({"agent_id": a.id}),
        ),
    );
    Ok(EvalRun::new(&run, &a.name))
}

/// Delete a finished run, its results and its eval conversations (normal delete path, session
/// purge included). Returns the blobs to remove after commit.
pub async fn delete_tx(conn: &mut PgConnection, run: Uuid) -> Result<lifecycle::Deleted, ApiError> {
    let mut agg = lifecycle::Deleted::default();
    for conv in repo::run_conversations(&mut *conn, run).await? {
        if !convs::lock(&mut *conn, conv).await? {
            continue;
        }
        crate::domains::approvals::resume::end_parked(
            conn,
            aurora_db::repo::approvals::Scope::Conversation(conv),
            "session_reset",
        )
        .await?;
        let g = lifecycle::delete_conversation(conn, conv).await?;
        agg.turns += g.turns;
        agg.events += g.events;
        agg.attachments += g.attachments;
        agg.session_purged |= g.session_purged;
        agg.blobs.extend(g.blobs);
    }
    repo::delete_run(conn, run).await?;
    Ok(agg)
}

pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let (r, a) = load(s, c, id).await?;
    let auth = c.workspace(s, a.workspace_id).await?;
    require_own(&auth)?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let status = repo::lock_run(&mut tx, id).await?.ok_or_else(run_404)?;
    if matches!(status.as_str(), "queued" | "running") {
        return Err(active_conflict());
    }
    let gone = delete_tx(&mut tx, id).await?;
    crate::domains::conversations::service::notify_work(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    remove_blobs(&s.pool, &gone.blobs).await;
    audit::record(
        s,
        run_audit(
            AuditActor::from_caller(c),
            &a,
            id,
            "eval.run.delete",
            json!({"agent_id": a.id, "questions": r.questions_total}),
        ),
    );
    Ok(())
}

/// Retention job (D-294): finished runs older than `EVAL_RUN_RETENTION` go through the same
/// delete path. Returns the number of runs deleted.
pub async fn sweep_retention(pool: &sqlx::PgPool) -> Result<usize, ApiError> {
    let ids = repo::expired_runs(pool, limits::EVAL_RUN_RETENTION.as_secs_f64(), 50).await?;
    let mut n = 0;
    for id in ids {
        let mut tx = pool.begin().await.map_err(DbError::from)?;
        match repo::lock_run(&mut tx, id).await? {
            Some(st) if !matches!(st.as_str(), "queued" | "running") => {}
            _ => continue,
        }
        let gone = delete_tx(&mut tx, id).await?;
        tx.commit().await.map_err(DbError::from)?;
        remove_blobs(pool, &gone.blobs).await;
        n += 1;
    }
    Ok(n)
}

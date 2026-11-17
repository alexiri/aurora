//! The eval run engine (P4-EVAL-2; docs/10 §Evals, C-865): a due-work arm that advances claimed
//! runs one step at a time. Each tick is idempotent: the question's conversation is
//! get-or-create, its turn is found by `source_event_id` before one is created, and a result is
//! written once (fenced on `finished_at IS NULL`), so a lost lease or a replayed claim never
//! duplicates a turn or a result. Answers and expected values are never logged or audited
//! (SEC-116).

use super::{judge, scoring};
use crate::app::AppState;
use crate::domains::conversations::service::{self as conv, NewMessage};
use aurora_core::{ApiError, limits};
use aurora_db::authz;
use aurora_db::repo::conversations::{self as convs, NewConversation};
use aurora_db::repo::evals::{self as repo, ResultRow, RunRow, Scored};
use aurora_db::repo::turns::{self, Created, TurnRow};
use aurora_db::repo::{agents, freeze, transcript};
use chrono::Utc;
use serde_json::json;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

/// Runs claimed per poll.
const ARM_LIMIT: i64 = 8;
/// Questions advanced per tick of one run (bounds the work done in one poll transaction).
const STEPS_PER_TICK: usize = 16;
/// Lease taken when a run is claimed; released when the tick ends (the tick holds the row lock).
const CLAIM_LEASE_SECS: f64 = 30.0;
/// Added to `EVAL_JUDGE_TIMEOUT` for the lease kept while a judge call runs (the call, the write).
const JUDGE_LEASE_MARGIN: std::time::Duration = std::time::Duration::from_secs(30);

/// Arm: claim active runs and advance each in its own savepoint. Judge calls (P4-EVAL-3) are not
/// made here: a run waiting for one keeps a lease that outlasts the call, and its [`judge::Job`] is
/// returned for the caller to [`judge::spawn`] once the work transaction has committed.
pub async fn arm(s: &AppState, tx: &mut PgConnection) -> Result<Vec<judge::Job>, ApiError> {
    let ids = repo::claim_runs(tx, CLAIM_LEASE_SECS, ARM_LIMIT).await?;
    let mut jobs = Vec::new();
    for id in ids {
        let mut sp = tx.begin().await.map_err(aurora_db::DbError::from)?;
        match tick(s, &mut sp, id).await {
            Ok(job) => {
                match job {
                    Some(job) => {
                        let lease = (limits::EVAL_JUDGE_TIMEOUT + JUDGE_LEASE_MARGIN).as_secs_f64();
                        repo::extend_lease(&mut sp, id, lease).await?;
                        jobs.push(job);
                    }
                    None => repo::release_lease(&mut sp, id).await?,
                }
                sp.commit().await.map_err(aurora_db::DbError::from)?;
            }
            Err(e) => {
                // Roll this run back only; the claim lease stays, so it is retried after expiry.
                tracing::error!(run = %id, code = e.code(), "eval run tick failed");
                drop(sp);
            }
        }
    }
    Ok(jobs)
}

/// What a step decided.
enum Step {
    /// A judged answer waits for the judge, whose call is made outside this transaction.
    Judge(Box<judge::Job>),
    /// The current question advanced or ended; look at the next one.
    Continue,
    /// Waiting for a turn (or for the request log); nothing more to do this tick.
    Wait,
    /// The run ended.
    Ended,
}

async fn tick(
    s: &AppState,
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<judge::Job>, ApiError> {
    let Some(run) = repo::get_run(&mut *conn, id).await? else {
        return Ok(None);
    };
    if !matches!(run.status.as_str(), "queued" | "running") {
        return Ok(None);
    }
    if run.status == "queued" {
        repo::mark_running(conn, id).await?;
    }
    for _ in 0..STEPS_PER_TICK {
        match step(s, conn, &run).await? {
            Step::Continue => {}
            Step::Wait | Step::Ended => break,
            Step::Judge(job) => return Ok(Some(*job)),
        }
    }
    Ok(None)
}

async fn step(s: &AppState, conn: &mut PgConnection, run: &RunRow) -> Result<Step, ApiError> {
    let Some(agent) = agents::get(&mut *conn, run.agent_id).await? else {
        return Ok(Step::Ended);
    };
    // Cancel: abort the question in flight, close the rest `not_scored`.
    let cancelled = repo::get_run(&mut *conn, run.id)
        .await?
        .is_some_and(|r| r.cancel_requested_at.is_some());
    if cancelled {
        abort_open_turn(conn, run.id).await?;
        return end(conn, run, "cancelled", Some("cancelled"), Some("cancelled")).await;
    }
    let Some(open) = repo::next_open_result(&mut *conn, run.id).await? else {
        return finish_done(s, conn, run).await;
    };
    let reference = format!("eval:{}:{}", run.id, open.position);
    let (c, _) = convs::get_or_create(
        conn,
        &NewConversation {
            agent_id: run.agent_id,
            workspace_id: run.workspace_id,
            kind: "eval",
            external_ref: &reference,
            direct_user_id: None,
            title: Some(&format!("Eval question {}", open.position)),
            origin_conversation_id: None,
        },
    )
    .await?;
    if !convs::lock(&mut *conn, c.id).await? {
        return Ok(Step::Wait);
    }
    let existing = turns::by_source_event(conn, c.id, &reference).await?;
    let Some(turn) = existing else {
        // Before each question: the starter's (or creator's) rights and the freeze.
        if freeze::is_agent_frozen(&mut *conn, agent.id).await? {
            return end(conn, run, "cancelled", Some("frozen"), Some("frozen")).await;
        }
        if !actor_authorized(conn, run, agent.workspace_id).await? {
            return end(
                conn,
                run,
                "cancelled",
                Some("access_lost"),
                Some("access_lost"),
            )
            .await;
        }
        let q = repo::get_result(&mut *conn, run.id, open.position)
            .await?
            .unwrap_or(open);
        let text = scoring::turn_text(&q.question_text, &q.expected_kind);
        let created = conv::create_turn_tx(
            conn,
            &c,
            &NewMessage {
                door: json!({"kind": "eval", "ref": format!("{}:{}", run.id, q.position)}),
                speaker_kind: "system",
                speaker_user_id: None,
                speaker_label: "Eval",
                text: &text,
                source_event_id: Some(&reference),
                attachment_ids: &[],
                reply_to_seq: None,
            },
        )
        .await;
        let created = match created {
            Ok(c) => c,
            Err(e) if e.code() == "frozen" => {
                return end(conn, run, "cancelled", Some("frozen"), Some("frozen")).await;
            }
            Err(e) => {
                tracing::warn!(run = %run.id, code = e.code(), "eval turn could not be created");
                return end(conn, run, "failed", Some(e.code()), Some("run_failed")).await;
            }
        };
        let (Created::Turn(t) | Created::Followup(t)) = created;
        repo::start_result(conn, q.id, c.id, t.id).await?;
        return Ok(Step::Wait);
    };
    match turn.status.as_str() {
        "done" | "failed" | "aborted" => {
            if turn.status == "done" && open.expected_kind == "judged" {
                // The judge is called outside this transaction and the conversation lock.
                let answer = transcript::answer_texts(&mut *conn, c.id, turn.id)
                    .await?
                    .join("\n\n");
                return match judge::prepare(conn, run, &open, &answer).await? {
                    judge::Prepared::Now(scored) => {
                        repo::finish_result(conn, open.id, &scored).await?;
                        Ok(Step::Continue)
                    }
                    judge::Prepared::Job(job) => Ok(Step::Judge(job)),
                };
            }
            let scored = score_turn(conn, &open, &c.id, &turn).await?;
            repo::finish_result(conn, open.id, &scored).await?;
            Ok(Step::Continue)
        }
        _ => {
            let age = Utc::now() - turn.created_at;
            if age.to_std().is_ok_and(|a| a >= limits::EVAL_TURN_TIMEOUT) {
                abort_turn(conn, &turn).await?;
                repo::finish_result(conn, open.id, &scoring::errored("timeout", None)).await?;
                return Ok(Step::Continue);
            }
            Ok(Step::Wait)
        }
    }
}

/// The scored outcome of a finished turn.
async fn score_turn(
    conn: &mut PgConnection,
    r: &ResultRow,
    conversation: &Uuid,
    t: &TurnRow,
) -> Result<Scored, ApiError> {
    if t.status != "done" {
        let code = t.error_code.clone().unwrap_or_else(|| {
            if t.status == "aborted" {
                "aborted"
            } else {
                "turn_failed"
            }
            .into()
        });
        return Ok(scoring::errored(&code, None));
    }
    let answer = transcript::answer_texts(&mut *conn, *conversation, t.id)
        .await?
        .join("\n\n");
    let snap = &r.expected_snapshot;
    let num = |k: &str| snap.get(k).and_then(serde_json::Value::as_f64);
    let txt = snap.get("text").and_then(serde_json::Value::as_str);
    Ok(scoring::score(
        &scoring::Expected {
            kind: &r.expected_kind,
            number: num("number"),
            tolerance: num("tolerance"),
            text: txt,
        },
        &answer,
    ))
}

/// Abort a turn (no caller): a queued turn ends at once, a dispatched one on the supervisor's
/// terminal event (as `POST /turns/{id}/abort`).
async fn abort_turn(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    convs::lock(&mut *conn, t.conversation_id).await?;
    if let Some(row) = turns::request_abort(conn, t.id).await? {
        if row.status == "queued" {
            conv::finish_turn(conn, t.id, "aborted", None, None, None).await?;
        } else {
            conv::notify_work(conn).await?;
        }
    }
    Ok(())
}

async fn abort_open_turn(conn: &mut PgConnection, run: Uuid) -> Result<(), ApiError> {
    let Some(open) = repo::next_open_result(&mut *conn, run).await? else {
        return Ok(());
    };
    if let Some(turn) = open.turn_id
        && let Some(t) = turns::get(&mut *conn, turn).await?
    {
        abort_turn(conn, &t).await?;
    }
    Ok(())
}

/// Whether the person (or schedule anchor) the run acts under may still use the agent.
async fn actor_authorized(
    conn: &mut PgConnection,
    run: &RunRow,
    workspace: Uuid,
) -> Result<bool, ApiError> {
    if let Some(sid) = run.schedule_id {
        return Ok(match repo::get_schedule(&mut *conn, sid).await? {
            Some(sch) => {
                authz::automation_authorized(conn, &sch.anchor, sch.creator_user_id, workspace)
                    .await?
            }
            // The schedule was deleted mid-run: the workspace still needs an active owner.
            None => authz::automation_authorized(conn, "workspace", None, workspace).await?,
        });
    }
    Ok(authz::automation_authorized(conn, "creator", run.started_by_user_id, workspace).await?)
}

/// End the run: close the open results, reconcile refusals, set the status.
async fn end(
    conn: &mut PgConnection,
    run: &RunRow,
    status: &str,
    code: Option<&str>,
    close_reason: Option<&str>,
) -> Result<Step, ApiError> {
    if let Some(reason) = close_reason {
        repo::close_open_results(conn, run.id, reason).await?;
    }
    repo::reconcile_refusals(conn, run.id).await?;
    repo::finish_run(conn, run.id, status, code, None).await?;
    after_finish(conn, run, status).await?;
    Ok(Step::Ended)
}

/// All questions ended: wait for the request log to flush (the gateway's refusals), then finish.
async fn finish_done(
    s: &AppState,
    conn: &mut PgConnection,
    run: &RunRow,
) -> Result<Step, ApiError> {
    let _ = s;
    if let Some(last) = repo::last_result_end(&mut *conn, run.id).await?
        && (Utc::now() - last)
            .to_std()
            .is_ok_and(|a| a < limits::REQUEST_LOG_FLUSH + std::time::Duration::from_secs(1))
    {
        return Ok(Step::Wait);
    }
    repo::reconcile_refusals(conn, run.id).await?;
    let (passed, failed) = repo::pass_fail_counts(conn, run.id).await?;
    let regression = regression_of(conn, run, passed, failed).await?;
    repo::finish_run(conn, run.id, "done", None, regression.as_ref()).await?;
    after_finish(conn, run, "done").await?;
    Ok(Step::Ended)
}

/// C-871: at least `EVAL_REGRESSION_DROP` points below the previous `done` run of the same
/// question-set version; only while the agent's `eval_regression_notify` is on.
async fn regression_of(
    conn: &mut PgConnection,
    run: &RunRow,
    passed: i64,
    failed: i64,
) -> Result<Option<serde_json::Value>, ApiError> {
    if passed + failed == 0 {
        return Ok(None);
    }
    let notify = repo::get_settings(&mut *conn, run.agent_id)
        .await?
        .is_none_or(|r| r.regression_notify);
    if !notify {
        return Ok(None);
    }
    let Some((from, before)) =
        repo::previous_done_pass_rate(conn, run.agent_id, &run.question_set_version, run.id)
            .await?
    else {
        return Ok(None);
    };
    #[allow(clippy::cast_precision_loss)]
    let now = passed as f64 * 100.0 / (passed + failed) as f64;
    let drop = before - now;
    // A tolerance for float noise only: 10.0 exactly is a regression, 9.0 is not.
    Ok((drop + 1e-9 >= limits::EVAL_REGRESSION_DROP)
        .then(|| json!({"from_run_id": from, "drop_points": (drop * 100.0).round() / 100.0})))
}

/// A scheduled run's end counts toward the schedule's failure streak (C-870).
async fn after_finish(conn: &mut PgConnection, run: &RunRow, status: &str) -> Result<(), ApiError> {
    let Some(sid) = run.schedule_id else {
        return Ok(());
    };
    if !matches!(status, "done" | "failed") {
        return Ok(());
    }
    let n = repo::schedule_run_outcome(conn, sid, status == "failed").await?;
    if status == "failed"
        && n >= i32::try_from(limits::SCHEDULE_FAILURE_DISABLE).unwrap_or(i32::MAX)
    {
        repo::disable_schedule(conn, sid, "failures", None).await?;
    }
    Ok(())
}

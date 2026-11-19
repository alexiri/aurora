//! Eval questions, settings and the fingerprint (P4-EVAL-1; C-861, C-863, C-866, C-883).
//! Authority: every route needs `can_use` of the agent's workspace (a caller without it gets 404);
//! changing the judge key needs `can_own`. Hosted agents only.

use super::dto::{
    CreateEvalQuestion, EvalFingerprint, EvalJudge, EvalQuestion, EvalSettings, PatchEvalQuestion,
    PutEvalSettings,
};
use super::fingerprint::{self, SkillEntry};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::hosted::model::{self, Reason};
use crate::domains::llm::platform;
use crate::domains::secrets::gov;
use crate::domains::skills::service as skills;
use crate::http::auth::{Caller, require_own};
use aurora_core::{ApiError, limits};
use aurora_db::DbError;
use aurora_db::repo::agents::{self, AgentRow};
use aurora_db::repo::evals::{self as repo, NewQuestion, QuestionRow};
use serde_json::json;
use uuid::Uuid;

fn invalid(field: &str, msg: &str) -> ApiError {
    ApiError::unprocessable("invalid", msg).with_details(json!({ "field": field }))
}

/// 422 `invalid` for a list filter with an unknown value.
pub fn invalid_filter(field: &str, allowed: &str) -> ApiError {
    invalid(field, &format!("one of: {allowed}"))
}

pub fn question_404() -> ApiError {
    ApiError::not_found("Question not found.")
}

/// The agent, for a caller who `can_use` its workspace (404 otherwise, as for a stranger) and only
/// when it is hosted (422 `eval_not_supported`).
pub async fn visible_agent(s: &AppState, c: &Caller, id: Uuid) -> Result<AgentRow, ApiError> {
    let not_found = || ApiError::not_found("Agent not found.");
    let a = agents::get(&s.pool, id).await?.ok_or_else(not_found)?;
    let auth = c.workspace(s, a.workspace_id).await.map_err(|e| {
        // A key confined to another workspace keeps its 403 `workspace_mismatch` (as elsewhere).
        if e.status() == http::StatusCode::NOT_FOUND {
            not_found()
        } else {
            e
        }
    })?;
    if !auth.can_use() {
        return Err(not_found());
    }
    if a.kind != "hosted" {
        return Err(ApiError::unprocessable(
            "eval_not_supported",
            "Evals are available for hosted agents only.",
        ));
    }
    Ok(a)
}

fn count_chars(s: &str) -> usize {
    s.chars().count()
}

fn check_question(text: &str) -> Result<(), ApiError> {
    if text.trim().is_empty() || count_chars(text) > limits::EVAL_QUESTION_CHARS {
        return Err(invalid(
            "question",
            &format!("1 to {} characters", limits::EVAL_QUESTION_CHARS),
        ));
    }
    Ok(())
}

fn check_number(field: &str, v: f64, min: Option<f64>) -> Result<(), ApiError> {
    if !v.is_finite() || min.is_some_and(|m| v < m) {
        return Err(invalid(
            field,
            if min.is_some() {
                "a finite number, 0 or more"
            } else {
                "a finite number"
            },
        ));
    }
    Ok(())
}

fn check_text(field: &str, v: &str, max: usize) -> Result<(), ApiError> {
    if v.trim().is_empty() || count_chars(v) > max {
        return Err(invalid(field, &format!("1 to {max} characters")));
    }
    Ok(())
}

/// A field that does not belong to the question's kind must be absent.
fn outside(kind: &str, field: &str, present: bool) -> Result<(), ApiError> {
    if present {
        return Err(invalid(
            field,
            &format!("not allowed for a {kind} question"),
        ));
    }
    Ok(())
}

/// The kind-specific fields of a question, checked together (used by create and by PATCH on the
/// merged row).
fn check_fields(
    kind: &str,
    number: Option<f64>,
    tolerance: Option<f64>,
    text: Option<&str>,
    criteria: Option<&str>,
) -> Result<(), ApiError> {
    match kind {
        "numeric" => {
            let n = number
                .ok_or_else(|| invalid("expected_number", "required for a numeric question"))?;
            check_number("expected_number", n, None)?;
            if let Some(t) = tolerance {
                check_number("tolerance", t, Some(0.0))?;
            }
            outside(kind, "expected_text", text.is_some())?;
            outside(kind, "criteria", criteria.is_some())
        }
        "text" => {
            let t = text.ok_or_else(|| invalid("expected_text", "required for a text question"))?;
            check_text("expected_text", t, limits::EVAL_EXPECTED_TEXT_CHARS)?;
            outside(kind, "expected_number", number.is_some())?;
            outside(kind, "tolerance", tolerance.is_some())?;
            outside(kind, "criteria", criteria.is_some())
        }
        "judged" => {
            let c =
                criteria.ok_or_else(|| invalid("criteria", "required for a judged question"))?;
            check_text("criteria", c, limits::EVAL_CRITERIA_CHARS)?;
            outside(kind, "expected_number", number.is_some())?;
            outside(kind, "tolerance", tolerance.is_some())?;
            outside(kind, "expected_text", text.is_some())
        }
        _ => Err(invalid("expected_kind", "one of numeric, text, judged")),
    }
}

fn question_audit(
    c: &Caller,
    a: &AgentRow,
    id: Uuid,
    action: &str,
    meta: serde_json::Value,
) -> AuditEvent {
    AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("eval_question", id)
        .metadata(meta)
}

pub async fn list_questions(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
) -> Result<Vec<EvalQuestion>, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    let rows = repo::list_questions(&s.pool, agent).await?;
    Ok(rows
        .into_iter()
        .map(|q| EvalQuestion::new(q, &a.name))
        .collect())
}

pub async fn create_question(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    b: CreateEvalQuestion,
) -> Result<EvalQuestion, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    check_question(&b.question)?;
    check_fields(
        &b.expected_kind,
        b.expected_number,
        b.tolerance,
        b.expected_text.as_deref(),
        b.criteria.as_deref(),
    )?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::lock_agent_questions(&mut tx, agent).await?;
    if repo::count_questions(&mut *tx, agent).await? >= limits::EVAL_QUESTIONS_PER_AGENT as i64 {
        return Err(ApiError::unprocessable(
            "eval_question_limit",
            "This agent has the most eval questions it can have.",
        )
        .with_details(json!({ "limit": limits::EVAL_QUESTIONS_PER_AGENT })));
    }
    let numeric = b.expected_kind == "numeric";
    let row = repo::insert_question(
        &mut tx,
        &NewQuestion {
            agent_id: a.id,
            workspace_id: a.workspace_id,
            question: &b.question,
            expected_kind: &b.expected_kind,
            expected_number: b.expected_number,
            tolerance: numeric.then(|| b.tolerance.unwrap_or(0.0)),
            expected_text: b.expected_text.as_deref(),
            criteria: b.criteria.as_deref(),
            enabled: b.enabled.unwrap_or(true),
            created_by: Some(c.user_id),
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    // Ids and kind only: never the question or its expected answer (SEC-116).
    audit::record(
        s,
        question_audit(
            c,
            &a,
            row.id,
            "eval.question.create",
            json!({"agent_id": a.id, "kind": row.expected_kind, "position": row.position}),
        ),
    );
    Ok(EvalQuestion::new(row, &a.name))
}

/// The question and its agent, for a caller who may use the agent.
async fn load(s: &AppState, c: &Caller, id: Uuid) -> Result<(QuestionRow, AgentRow), ApiError> {
    let q = repo::get_question(&s.pool, id)
        .await?
        .ok_or_else(question_404)?;
    let a = visible_agent(s, c, q.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            question_404()
        } else {
            e
        }
    })?;
    Ok((q, a))
}

pub async fn get_question(s: &AppState, c: &Caller, id: Uuid) -> Result<EvalQuestion, ApiError> {
    let (q, a) = load(s, c, id).await?;
    Ok(EvalQuestion::new(q, &a.name))
}

pub async fn patch_question(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    p: PatchEvalQuestion,
) -> Result<EvalQuestion, ApiError> {
    let (_, a) = load(s, c, id).await?;
    if let Some(q) = &p.question {
        check_question(q)?;
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::lock_agent_questions(&mut tx, a.id).await?;
    let cur = repo::get_question_for_update(&mut tx, id)
        .await?
        .ok_or_else(question_404)?;
    let mut next = cur.clone();
    if let Some(q) = p.question {
        next.question = q;
    }
    if let Some(n) = p.expected_number {
        next.expected_number = Some(n);
    }
    if let Some(t) = p.tolerance {
        next.tolerance = Some(t);
    }
    if let Some(t) = p.expected_text {
        next.expected_text = Some(t);
    }
    if let Some(t) = p.criteria {
        next.criteria = Some(t);
    }
    if let Some(e) = p.enabled {
        next.enabled = e;
    }
    // The stored row holds no field of another kind, so a merged field of another kind is one
    // this PATCH sent: check_fields refuses it.
    check_fields(
        &next.expected_kind,
        next.expected_number,
        next.tolerance,
        next.expected_text.as_deref(),
        next.criteria.as_deref(),
    )?;
    let changed_fields = next.question != cur.question
        || next.expected_number != cur.expected_number
        || next.tolerance != cur.tolerance
        || next.expected_text != cur.expected_text
        || next.criteria != cur.criteria
        || next.enabled != cur.enabled;
    let mut row = cur.clone();
    if changed_fields {
        row = repo::save_question(&mut tx, &next).await?;
    }
    if let Some(to) = p.position {
        let total = repo::count_questions(&mut *tx, a.id).await?;
        let to = to.clamp(1, i32::try_from(total).unwrap_or(i32::MAX).max(1));
        repo::move_question(&mut tx, id, a.id, cur.position, to).await?;
        row = repo::get_question(&mut *tx, id)
            .await?
            .ok_or_else(question_404)?;
    }
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        question_audit(
            c,
            &a,
            id,
            "eval.question.update",
            json!({"agent_id": a.id, "version": row.version, "position": row.position,
                   "enabled": row.enabled}),
        ),
    );
    Ok(EvalQuestion::new(row, &a.name))
}

pub async fn delete_question(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let (_, a) = load(s, c, id).await?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::lock_agent_questions(&mut tx, a.id).await?;
    let cur = repo::get_question_for_update(&mut tx, id)
        .await?
        .ok_or_else(question_404)?;
    repo::delete_question(&mut tx, &cur).await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        question_audit(
            c,
            &a,
            id,
            "eval.question.delete",
            json!({"agent_id": a.id, "kind": cur.expected_kind}),
        ),
    );
    Ok(())
}

// ---- settings ----

fn harness_of(s: &AppState, a: &AgentRow) -> String {
    a.harness
        .clone()
        .unwrap_or_else(|| s.ops().hosted.default_harness.clone())
}

/// Why `secret` cannot serve as the agent's judge now, per `08 §Model readiness`: `None` = usable.
enum JudgeCheck {
    Usable,
    NotGranted,
    NotPermitted,
    NotUsable,
}

async fn judge_check(
    s: &AppState,
    a: &AgentRow,
    secret: Uuid,
    model_id: &str,
) -> Result<JudgeCheck, ApiError> {
    let llm = s.llm();
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    // (D-372) Only the keys' reasons are read here: no key listing is fetched.
    let r = model::readiness_cached(
        s,
        &llm,
        &mut conn,
        a.id,
        a.org_id,
        a.workspace_id,
        &harness_of(s, a),
    )
    .await?;
    drop(conn);
    let Some(key) = r.keys.iter().find(|k| k.secret.id == secret) else {
        return Ok(JudgeCheck::NotGranted);
    };
    match key.reason {
        Some(Reason::ScopeNotPermitted) => return Ok(JudgeCheck::NotPermitted),
        Some(_) => return Ok(JudgeCheck::NotUsable),
        None => {}
    }
    let Some(ep) = key.ep else {
        return Ok(JudgeCheck::NotUsable);
    };
    // (P4-EVAL-3) `PlatformLlm` calls plain-header, Anthropic-Messages or OpenAI-chat endpoints only.
    let mode = key
        .secret
        .llm_auth_mode
        .as_deref()
        .and_then(aurora_llm::AuthMode::parse)
        .unwrap_or(aurora_llm::AuthMode::ApiKey);
    if !llm
        .provider_for(ep)
        .is_some_and(|p| platform::supported(p.as_ref(), ep, mode).is_some())
    {
        return Ok(JudgeCheck::NotUsable);
    }
    // The model is data (SEC-122): a valid id, allowed for the org on this endpoint, and on the
    // endpoint's static list when it has one.
    let eff = gov::effective_for(s, a.org_id).await?;
    if eff
        .model_allowed(&ep.id, &aurora_llm::ModelRef::Model(model_id.to_owned()))
        .is_err()
    {
        return Ok(JudgeCheck::NotPermitted);
    }
    if !ep.models.is_empty() && !ep.models.iter().any(|m| m == model_id) {
        return Ok(JudgeCheck::NotUsable);
    }
    Ok(JudgeCheck::Usable)
}

pub async fn get_settings(s: &AppState, c: &Caller, agent: Uuid) -> Result<EvalSettings, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    settings_view(s, &a).await
}

async fn settings_view(s: &AppState, a: &AgentRow) -> Result<EvalSettings, ApiError> {
    let row = repo::get_settings(&s.pool, a.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let judge = match (row.judge_secret_id, row.judge_model_id) {
        (Some(secret_id), Some(model_id)) => {
            let usable = matches!(
                judge_check(s, a, secret_id, &model_id).await?,
                JudgeCheck::Usable
            );
            Some(EvalJudge {
                secret_id,
                secret_label: row.judge_secret_name.unwrap_or_default(),
                model_id,
                usable,
            })
        }
        _ => None,
    };
    Ok(EvalSettings {
        judge,
        regression_notify: row.regression_notify,
    })
}

pub async fn put_settings(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    b: PutEvalSettings,
) -> Result<EvalSettings, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    let cur = repo::get_settings(&s.pool, a.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let cur_judge = cur.judge_secret_id.zip(cur.judge_model_id.clone());
    let new_judge = b.judge.as_ref().map(|j| (j.secret_id, j.model_id.clone()));
    let judge_changed = cur_judge != new_judge;
    if judge_changed {
        require_own(&c.workspace(s, a.workspace_id).await?)?;
    }
    if let (true, Some((secret, model_id))) = (judge_changed, &new_judge) {
        if !aurora_core::model_id::validate_model_id(model_id) {
            return Err(invalid("judge.model_id", "not a valid model id"));
        }
        match judge_check(s, &a, *secret, model_id).await? {
            JudgeCheck::Usable => {}
            JudgeCheck::NotGranted => {
                return Err(ApiError::unprocessable(
                    "eval_judge_not_granted",
                    "The judge key must be an LLM key granted to this agent.",
                ));
            }
            JudgeCheck::NotUsable => {
                return Err(ApiError::unprocessable(
                    "eval_judge_not_usable",
                    "The judge key or model cannot be used right now.",
                ));
            }
            JudgeCheck::NotPermitted => {
                return Err(ApiError::unprocessable(
                    "eval_judge_not_permitted",
                    "Your organization's model governance does not allow this key or model.",
                ));
            }
        }
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::set_settings(
        &mut tx,
        a.id,
        new_judge.as_ref().map(|(i, m)| (*i, m.as_str())),
        b.regression_notify,
    )
    .await?;
    // Ids only, never a secret value (SEC-115, SEC-116).
    audit::commit_critical(
        tx,
        &AuditEvent::new(
            AuditActor::from_caller(c),
            "eval.settings.update",
            Outcome::Success,
        )
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("agent", a.id)
        .metadata(json!({
            "judge_changed": judge_changed,
            "regression_notify": b.regression_notify,
            "change": {
                "judge_secret_id": {"from": cur.judge_secret_id, "to": new_judge.as_ref().map(|j| j.0)},
                "judge_model_id": {"from": cur.judge_model_id, "to": new_judge.as_ref().map(|j| j.1.clone())},
            },
        })),
    )
    .await?;
    settings_view(s, &a).await
}

// ---- fingerprint ----

/// What a run started now would record (C-866, C-883).
pub async fn current_fingerprint(s: &AppState, a: &AgentRow) -> Result<EvalFingerprint, ApiError> {
    let harness = harness_of(s, a);
    let llm = s.llm();
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let roles = {
        // (D-372) Role ids do not depend on a key listing: cache only.
        let r =
            model::readiness_cached(s, &llm, &mut conn, a.id, a.org_id, a.workspace_id, &harness)
                .await?;
        r.roles.map(|r| r.role_ids())
    };
    let roles = roles.unwrap_or_default();
    let effective = skills::effective(&s.pool, a).await?;
    let ids: Vec<Uuid> = effective.items.iter().map(|i| i.view.row.id).collect();
    let files = aurora_db::repo::skills::files(&mut conn, &ids).await?;
    let mut entries = Vec::with_capacity(ids.len());
    for item in &effective.items {
        let row = &item.view.row;
        let own: Vec<(String, bool, Vec<u8>)> = files
            .iter()
            .filter(|f| f.skill_id == row.id)
            .map(|f| (f.path.clone(), f.executable, f.content.clone()))
            .collect();
        entries.push(SkillEntry {
            tier: skills::tier_of(row).to_owned(),
            name: row.name.clone(),
            content_sha256: fingerprint::skill_content_sha256(&row.description, &row.body, &own),
        });
    }
    let granted = repo::granted_rule_ids(&mut *conn, a.id).await?;
    let settings = repo::get_settings(&mut *conn, a.id).await?;
    let questions = repo::list_questions(&mut *conn, a.id).await?;
    drop(conn);
    let enabled: Vec<(Uuid, i32)> = questions
        .iter()
        .filter(|q| q.enabled)
        .map(|q| (q.id, q.version))
        .collect();
    let qsv = fingerprint::question_set_version(&enabled);
    let input = fingerprint::Input {
        brief: a.brief.clone(),
        harness: a.harness.clone(),
        provider: a.model_provider.clone(),
        endpoint_id: a.model_endpoint_id.clone(),
        model_id: a.model_id.clone(),
        effort: a.effort.clone(),
        model_roles: [roles.large, roles.medium, roles.small],
        subagent_model: a.subagent_model.clone(),
        skills: entries,
        granted_rule_ids: granted,
        judge: settings.and_then(|r| r.judge_secret_id.zip(r.judge_model_id)),
        question_set_version: qsv.clone(),
    };
    let fp = fingerprint::build(&input);
    Ok(EvalFingerprint {
        fingerprint_hash: fingerprint::hash(&fp),
        fingerprint: fp,
        question_set_version: qsv,
    })
}

pub async fn get_fingerprint(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
) -> Result<EvalFingerprint, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    current_fingerprint(s, &a).await
}

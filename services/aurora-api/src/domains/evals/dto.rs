use aurora_db::repo::evals::QuestionRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct EvalAgentRef {
    pub id: Uuid,
    pub name: String,
}

/// An eval question (C-861). Expected answers are agent-audience data: this shape is served only
/// to callers who `can_use` the agent's workspace.
#[derive(Serialize, ToSchema)]
pub struct EvalQuestion {
    pub id: Uuid,
    pub agent: EvalAgentRef,
    /// 1-based, contiguous, unique per agent.
    pub position: i32,
    pub question: String,
    /// `numeric` | `text` | `judged`.
    pub expected_kind: String,
    pub expected_number: Option<f64>,
    pub tolerance: Option<f64>,
    pub expected_text: Option<String>,
    pub criteria: Option<String>,
    pub enabled: bool,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl EvalQuestion {
    pub fn new(q: QuestionRow, agent_name: &str) -> Self {
        Self {
            id: q.id,
            agent: EvalAgentRef {
                id: q.agent_id,
                name: agent_name.to_owned(),
            },
            position: q.position,
            question: q.question,
            expected_kind: q.expected_kind,
            expected_number: q.expected_number,
            tolerance: q.tolerance,
            expected_text: q.expected_text,
            criteria: q.criteria,
            enabled: q.enabled,
            version: q.version,
            created_at: q.created_at,
            updated_at: q.updated_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct EvalQuestionList {
    pub questions: Vec<EvalQuestion>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateEvalQuestion {
    pub question: String,
    /// `numeric` | `text` | `judged`; fixed for the life of the question.
    pub expected_kind: String,
    /// `numeric` only (required).
    pub expected_number: Option<f64>,
    /// `numeric` only, ≥ 0 (default 0).
    pub tolerance: Option<f64>,
    /// `text` only (required).
    pub expected_text: Option<String>,
    /// `judged` only (required).
    pub criteria: Option<String>,
    /// Default true.
    pub enabled: Option<bool>,
}

/// Any subset; a field outside the question's kind is 422 `invalid`.
#[derive(Deserialize, ToSchema)]
pub struct PatchEvalQuestion {
    pub question: Option<String>,
    pub expected_number: Option<f64>,
    pub tolerance: Option<f64>,
    pub expected_text: Option<String>,
    pub criteria: Option<String>,
    pub enabled: Option<bool>,
    /// Move to this 1-based position (clamped to the list).
    pub position: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct EvalJudge {
    pub secret_id: Uuid,
    pub model_id: String,
    pub secret_label: String,
    /// Whether the key is granted and usable now (08 §Model readiness).
    pub usable: bool,
}

/// The one real model call made when the judge is saved (C-872).
#[derive(Serialize, ToSchema)]
pub struct EvalJudgeTest {
    /// `ok` | `failed`.
    pub result: String,
    pub took_ms: i64,
    /// `rejected` | `unreachable` | `rate_limited` | `timeout` when failed, else null. Never the
    /// provider's text. `unreachable` includes an endpoint that did not answer within
    /// `EVAL_JUDGE_TEST_TIMEOUT` (C-1101); `timeout` is no longer produced.
    pub reason: Option<String>,
    /// (C-1101) When the test ran; the last one is kept and returned by `GET` too.
    pub tested_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct EvalSettings {
    pub judge: Option<EvalJudge>,
    pub regression_notify: bool,
    /// The last test of the current judge (C-1101): the one a `PUT` that changed the judge just made,
    /// kept and returned by `GET` and later `PUT`s; null with no judge or never tested.
    pub test: Option<EvalJudgeTest>,
}

#[derive(Deserialize, ToSchema)]
pub struct EvalJudgeInput {
    pub secret_id: Uuid,
    pub model_id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct PutEvalSettings {
    /// `null` clears the judge.
    pub judge: Option<EvalJudgeInput>,
    pub regression_notify: bool,
}

/// What a run started now would record (C-866, C-883).
#[derive(Serialize, ToSchema)]
pub struct EvalFingerprint {
    /// The canonical fingerprint object.
    pub fingerprint: serde_json::Value,
    pub fingerprint_hash: String,
    pub question_set_version: String,
}

// ---- runs, results, schedules (P4-EVAL-2, C-864, C-867, C-870) ----

#[derive(Serialize, ToSchema)]
pub struct EvalPersonRef {
    pub id: Uuid,
    pub label: String,
}

#[derive(Serialize, ToSchema)]
pub struct EvalScheduleRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct EvalRunCounts {
    pub total: i32,
    pub passed: i32,
    pub failed: i32,
    pub errored: i32,
    pub not_scored: i32,
    pub refused_writes: i32,
}

#[derive(Serialize, ToSchema)]
pub struct EvalJudgeUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Serialize, ToSchema)]
pub struct EvalRegression {
    pub from_run_id: Uuid,
    pub drop_points: f64,
}

/// An eval run (C-864).
#[derive(Serialize, ToSchema)]
pub struct EvalRun {
    pub id: Uuid,
    pub agent: EvalAgentRef,
    /// `manual` | `schedule`.
    pub trigger: String,
    pub schedule: Option<EvalScheduleRef>,
    pub started_by: Option<EvalPersonRef>,
    /// `queued` | `running` | `done` | `failed` | `cancelled`.
    pub status: String,
    pub error_code: Option<String>,
    pub question_set_version: String,
    pub fingerprint_hash: String,
    pub counts: EvalRunCounts,
    /// Percent, `pass ÷ (pass + fail)`; null when nothing was scored.
    pub pass_rate: Option<f64>,
    pub judge_usage: Option<EvalJudgeUsage>,
    pub regression: Option<EvalRegression>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl EvalRun {
    pub fn new(r: &aurora_db::repo::evals::RunRow, agent_name: &str) -> Self {
        let scored = r.questions_passed + r.questions_failed;
        let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_i64);
        Self {
            id: r.id,
            agent: EvalAgentRef {
                id: r.agent_id,
                name: agent_name.to_owned(),
            },
            trigger: r.trigger.clone(),
            schedule: r.schedule_id.map(|id| EvalScheduleRef {
                id,
                name: r.schedule_name.clone().unwrap_or_default(),
            }),
            started_by: r.started_by_user_id.map(|id| EvalPersonRef {
                id,
                label: r.started_by_label.clone().unwrap_or_default(),
            }),
            status: r.status.clone(),
            error_code: r.error_code.clone(),
            question_set_version: r.question_set_version.clone(),
            fingerprint_hash: r.fingerprint_hash.clone(),
            counts: EvalRunCounts {
                total: r.questions_total,
                passed: r.questions_passed,
                failed: r.questions_failed,
                errored: r.questions_errored,
                not_scored: r.questions_not_scored,
                refused_writes: r.refused_writes,
            },
            pass_rate: (scored > 0)
                .then(|| f64::from(r.questions_passed) * 100.0 / f64::from(scored)),
            judge_usage: r.judge_usage.as_ref().map(|u| EvalJudgeUsage {
                input_tokens: num(u, "input_tokens").unwrap_or(0),
                output_tokens: num(u, "output_tokens").unwrap_or(0),
            }),
            regression: r.regression.as_ref().and_then(|g| {
                Some(EvalRegression {
                    from_run_id: g.get("from_run_id")?.as_str()?.parse().ok()?,
                    drop_points: g.get("drop_points")?.as_f64()?,
                })
            }),
            created_at: r.created_at,
            started_at: r.started_at,
            finished_at: r.finished_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct EvalResultQuestion {
    pub id: Option<Uuid>,
    pub text: String,
}

/// The expected answer of a result (agent-audience data: `can_use` only).
#[derive(Serialize, ToSchema)]
pub struct EvalResultExpected {
    pub number: Option<f64>,
    pub tolerance: Option<f64>,
    pub text: Option<String>,
    pub criteria: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct EvalResultJudge {
    pub verdict: String,
    pub rationale: Option<String>,
}

/// One question's result (C-867).
#[derive(Serialize, ToSchema)]
pub struct EvalResult {
    pub position: i32,
    pub question: EvalResultQuestion,
    pub expected_kind: String,
    pub expected: Option<EvalResultExpected>,
    /// `pass` | `fail` | `error` | `not_scored`.
    pub outcome: String,
    pub reason_code: Option<String>,
    pub extracted_value: Option<String>,
    pub answer: Option<String>,
    pub judge: Option<EvalResultJudge>,
    pub refused_writes: i32,
    pub conversation_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl From<aurora_db::repo::evals::ResultRow> for EvalResult {
    fn from(r: aurora_db::repo::evals::ResultRow) -> Self {
        let s = &r.expected_snapshot;
        let f = |k: &str| s.get(k).and_then(serde_json::Value::as_f64);
        let t = |k: &str| {
            s.get(k)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        Self {
            position: r.position,
            question: EvalResultQuestion {
                id: r.question_id,
                text: r.question_text,
            },
            expected_kind: r.expected_kind,
            expected: Some(EvalResultExpected {
                number: f("number"),
                tolerance: f("tolerance"),
                text: t("text"),
                criteria: t("criteria"),
            }),
            outcome: r.outcome,
            reason_code: r.reason_code,
            extracted_value: r.extracted_value,
            answer: r.answer_text,
            judge: r.judge_verdict.map(|verdict| EvalResultJudge {
                verdict,
                rationale: r.judge_rationale,
            }),
            refused_writes: r.refused_writes,
            conversation_id: r.conversation_id,
            started_at: r.started_at,
            finished_at: r.finished_at,
        }
    }
}

/// A run with its fingerprint and one page of results (`GET /eval-runs/{id}`).
#[derive(Serialize, ToSchema)]
pub struct EvalRunDetail {
    #[serde(flatten)]
    pub run: EvalRun,
    /// The C-866 object recorded at start.
    pub fingerprint: serde_json::Value,
    pub results: Vec<EvalResult>,
    /// Position to pass as `results_after` for the next page; null on the last page.
    pub next_results_after: Option<i32>,
}

#[derive(Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct StartEvalRun {
    /// Run only these questions (1..=20, each a question of the agent); absent = every enabled
    /// question.
    pub question_ids: Option<Vec<Uuid>>,
}

/// An eval schedule (C-870).
#[derive(Serialize, ToSchema)]
pub struct EvalSchedule {
    pub id: Uuid,
    pub agent: EvalAgentRef,
    pub name: String,
    pub cron: String,
    pub timezone: String,
    pub enabled: bool,
    /// `user` | `authorization` | `failures`.
    pub disabled_reason: Option<String>,
    /// `creator` | `workspace`.
    pub anchor: String,
    pub creator: Option<EvalPersonRef>,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fire_at: Option<DateTime<Utc>>,
    /// `ok` | `failed` | `skipped_busy` | `skipped_frozen`.
    pub last_outcome: Option<String>,
    pub last_run_id: Option<Uuid>,
}

impl EvalSchedule {
    pub fn new(s: aurora_db::repo::evals::ScheduleRow, agent_name: &str) -> Self {
        Self {
            id: s.id,
            agent: EvalAgentRef {
                id: s.agent_id,
                name: agent_name.to_owned(),
            },
            name: s.name,
            cron: s.cron,
            timezone: s.timezone,
            enabled: s.enabled,
            disabled_reason: s.disabled_reason,
            anchor: s.anchor,
            creator: s.creator_user_id.map(|id| EvalPersonRef {
                id,
                label: s.creator_label.unwrap_or_default(),
            }),
            next_fire_at: s.next_fire_at,
            last_fire_at: s.last_fire_at,
            last_outcome: s.last_outcome,
            last_run_id: s.last_run_id,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct EvalScheduleList {
    pub schedules: Vec<EvalSchedule>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateEvalSchedule {
    pub name: String,
    pub cron: String,
    pub timezone: String,
    pub enabled: Option<bool>,
    /// `creator` (default) | `workspace` (`can_own` only).
    pub anchor: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchEvalSchedule {
    pub name: Option<String>,
    pub cron: Option<String>,
    pub timezone: Option<String>,
    pub enabled: Option<bool>,
}

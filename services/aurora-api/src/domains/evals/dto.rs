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

#[derive(Serialize, ToSchema)]
pub struct EvalSettings {
    pub judge: Option<EvalJudge>,
    pub regression_notify: bool,
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

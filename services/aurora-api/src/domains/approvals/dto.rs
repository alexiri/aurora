use aurora_db::repo::approvals::ApprovalRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SummaryField {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ApprovalSummary {
    pub title: String,
    pub fields: Vec<SummaryField>,
    pub text: Option<String>,
    pub link: Option<String>,
    pub group: Option<String>,
    /// `template` | `hook` | `generic`
    pub source: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApprovalAgent {
    pub id: Uuid,
    pub name: String,
    pub handle: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApprovalBatch {
    pub id: String,
    pub label: Option<String>,
    pub total: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct ApprovalDecider {
    pub id: Uuid,
    pub display_name: String,
}

/// (P5-UX-16, C-982) The sentence an approval reads as: "<agent> wants to <action> in <app>".
#[derive(Serialize, ToSchema)]
pub struct ApprovalSentence {
    pub agent: SentenceAgent,
    /// The app's approval title when the package provides one, else the tool's name, else
    /// "send a <METHOD> request to <host>". Completes "<agent> wants to …".
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<SentenceApp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<SentenceAccount>,
}

#[derive(Serialize, ToSchema)]
pub struct SentenceAgent {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct SentenceApp {
    pub app_id: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct AccountRefDto {
    /// `connection`
    #[serde(rename = "type")]
    pub kind: String,
    pub id: Uuid,
}

#[derive(Serialize, ToSchema)]
pub struct SentenceAccount {
    #[serde(rename = "ref")]
    pub reference: AccountRefDto,
    pub name: String,
}

/// (P5-UX-16, C-982) What asked the question: the agent's access, or a rule.
#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskedBy {
    Access {
        agent_id: Uuid,
        connection_id: Uuid,
        tool_id: Option<String>,
    },
    Rule {
        rule_id: Uuid,
        rule_name: String,
        /// `workspace` | `organization`
        scope: String,
    },
    /// (P5-TOOLASK, C-1043) The app's own default for the tool asked: no rule or access choice did.
    Package {
        app_id: Option<String>,
        tool_id: Option<String>,
    },
}

/// (C-982) Only on the answer of an Always allow.
#[derive(Serialize, ToSchema)]
pub struct AlwaysApplied {
    pub tool: AlwaysTool,
    /// Other pending approvals of the same agent, account and tool that went ahead too.
    pub also_approved: u32,
}

#[derive(Serialize, ToSchema)]
pub struct AlwaysTool {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct Approval {
    pub id: Uuid,
    /// (P5-UX-16, C-982)
    pub sentence: ApprovalSentence,
    /// `None` when no rule asked (nothing to name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<AskedBy>,
    /// Whether the caller may choose Always allow for this question (SEC-131).
    pub can_always_allow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub always_applied: Option<AlwaysApplied>,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent: Option<ApprovalAgent>,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub method: String,
    pub host: String,
    pub path: String,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub connection_id: Option<Uuid>,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    pub summary: ApprovalSummary,
    pub batch: Option<ApprovalBatch>,
    /// (phase 2, RFE-025) Label of the BYO context credential (a labelled `aurora run`) that
    /// made the request, if any.
    pub context_label: Option<String>,
    /// `pending` | `approved` | `denied` | `expired` | `abandoned`
    pub status: String,
    pub decided_by: Option<ApprovalDecider>,
    /// `dashboard` | `api` | `channel`
    pub decided_via: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub deadline: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// (P4-011B, C-822) The gateway refused the request at once and keeps this approval for the
    /// agent's identical repeat.
    pub parked: bool,
    /// `ready` | `consumed` | `expired` | `revoked`; only for approved parked approvals.
    pub resume_state: Option<String>,
    pub resume_expires_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
    /// Only on the decision response of an approved parked approval: `{ state: "ready", expires_at }`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<ApprovalResume>,
}

/// (C-822) What an approval of a parked request leaves the agent: one repeat before `expires_at`.
#[derive(Serialize, ToSchema)]
pub struct ApprovalResume {
    /// `ready`
    pub state: String,
    pub expires_at: DateTime<Utc>,
}

impl Approval {
    /// The decision response: carries `resume` when a parked approval was approved.
    pub fn with_resume(mut self) -> Self {
        if let (Some(state), Some(expires_at)) = (self.resume_state.clone(), self.resume_expires_at)
            && state == "ready"
        {
            self.resume = Some(ApprovalResume { state, expires_at });
        }
        self
    }
}

impl From<ApprovalRow> for Approval {
    fn from(r: ApprovalRow) -> Self {
        let summary = serde_json::from_value(r.summary).unwrap_or_else(|_| ApprovalSummary {
            title: format!("{} {}", r.method, r.host),
            fields: vec![],
            text: None,
            link: None,
            group: None,
            source: "generic".to_owned(),
        });
        let action = if summary.source == "generic" {
            format!("send a {} request to {}", r.method, r.host)
        } else {
            summary.title.clone()
        };
        let sentence = ApprovalSentence {
            agent: SentenceAgent {
                id: r.agent_id,
                name: r
                    .agent_name
                    .clone()
                    .unwrap_or_else(|| "A removed agent".to_owned()),
            },
            action,
            app: None,
            account: None,
        };
        Self {
            sentence,
            asked_by: None,
            can_always_allow: false,
            always_applied: None,
            id: r.id,
            org_id: r.org_id,
            workspace_id: r.workspace_id,
            agent: match (r.agent_name, r.agent_handle) {
                (Some(name), Some(handle)) => Some(ApprovalAgent {
                    id: r.agent_id,
                    name,
                    handle,
                }),
                _ => None,
            },
            conversation_id: r.conversation_id,
            turn_id: r.turn_id,
            method: r.method,
            host: r.host,
            path: r.path,
            app_id: r.app_id,
            tool_id: r.tool_id,
            connection_id: r.connection_id,
            rule_logical_id: r.rule_logical_id,
            rule_name: r.rule_name,
            summary,
            batch: r.batch_id.map(|id| ApprovalBatch {
                id,
                label: r.batch_label,
                total: r.batch_total,
            }),
            context_label: r.context_label,
            status: r.status,
            decided_by: r.decided_by_user_id.map(|id| ApprovalDecider {
                id,
                display_name: r.decided_by_name.unwrap_or_default(),
            }),
            decided_via: r.decided_via,
            decided_at: r.decided_at,
            deadline: r.deadline,
            created_at: r.created_at,
            parked: r.parked,
            resume_state: r.resume_state,
            resume_expires_at: r.resume_expires_at,
            consumed_at: r.consumed_at,
            resume: None,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `pending` | `approved` | `denied` | `expired` | `abandoned`
    pub status: Option<String>,
    /// (C-700) Only this agent's approvals; an unknown or foreign agent yields an empty page.
    pub agent_id: Option<Uuid>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PendingQuery {
    /// Long-poll seconds (0..=30, default 0).
    pub wait: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct PendingList {
    pub items: Vec<Approval>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Approve,
    Deny,
}

#[derive(Deserialize, ToSchema)]
pub struct DecisionBody {
    pub decision: Decision,
    /// (C-982, SEC-131) With `approve`: also stop asking for this kind of change. Needs
    /// `can_own` of the agent's workspace; only for a question the agent's access asked.
    #[serde(default)]
    pub always: Option<bool>,
}

#[derive(Deserialize, ToSchema)]
pub struct BatchDecisionBody {
    pub decision: Decision,
    pub agent_id: Uuid,
}

#[derive(Serialize, ToSchema)]
pub struct BatchDecided {
    pub decided: u32,
}

/// (FIX-252) The answer of the origin card's **Ask an approver**: how many deciders were
/// e-mailed (a second ask mails nobody — the claim keeps the notifier from repeating it).
#[derive(Serialize, ToSchema)]
pub struct AskApprovers {
    pub asked: u32,
}

/// SSE `data:` payload of `event: approval` (C-10).
#[derive(Serialize, ToSchema)]
pub struct ApprovalEvent {
    /// `created` | `decided` | `expired` | `abandoned`
    pub event: String,
    pub approval: Approval,
    /// (phase 2, RFE-011a) Only on `created`: what the dashboard needs for a browser
    /// notification and nothing more.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<ApprovalNotice>,
}

/// (phase 2, RFE-011a) The sanitized facts of a new approval: `{ id, workspace, agent, title,
/// kind, expires_at }`. No summary fields, no request body, no decision link.
#[derive(Serialize, ToSchema)]
pub struct ApprovalNotice {
    pub id: Uuid,
    pub workspace: ApprovalNoticeWorkspace,
    pub agent: Option<ApprovalAgent>,
    pub title: String,
    /// `gateway` | `action`
    pub kind: String,
    pub expires_at: DateTime<Utc>,
    /// (P4-011B, C-822) The agent has stopped waiting; deciding is for a later repeat.
    pub parked: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ApprovalNoticeWorkspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub org_slug: String,
}

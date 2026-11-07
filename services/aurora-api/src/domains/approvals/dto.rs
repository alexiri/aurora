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

#[derive(Serialize, ToSchema)]
pub struct Approval {
    pub id: Uuid,
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
    /// `pending` | `approved` | `denied` | `expired` | `abandoned`
    pub status: String,
    pub decided_by: Option<ApprovalDecider>,
    /// `dashboard` | `api` | `channel`
    pub decided_via: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub deadline: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
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
        Self {
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
            status: r.status,
            decided_by: r.decided_by_user_id.map(|id| ApprovalDecider {
                id,
                display_name: r.decided_by_name.unwrap_or_default(),
            }),
            decided_via: r.decided_via,
            decided_at: r.decided_at,
            deadline: r.deadline,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `pending` | `approved` | `denied` | `expired` | `abandoned`
    pub status: Option<String>,
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

/// SSE `data:` payload of `event: approval` (C-10).
#[derive(Serialize, ToSchema)]
pub struct ApprovalEvent {
    /// `created` | `decided` | `expired`
    pub event: String,
    pub approval: Approval,
}

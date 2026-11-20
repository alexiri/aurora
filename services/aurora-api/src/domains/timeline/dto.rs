//! (P5-REFINE-9, C-1083, C-1084) The timeline's wire shapes: structured kinds and names, never
//! sentences (the web writes the sentence from `lib/labels.ts`, D-415).

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// What a row says the agent did (C-1083).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKind {
    Answered,
    Computer,
    AskedBefore,
    Called,
    RanSchedule,
    ReceivedWebhook,
    HandedTask,
    GotTask,
    AskedForAccess,
}

impl TimelineKind {
    pub const ALL: [TimelineKind; 9] = [
        TimelineKind::Answered,
        TimelineKind::Computer,
        TimelineKind::AskedBefore,
        TimelineKind::Called,
        TimelineKind::RanSchedule,
        TimelineKind::ReceivedWebhook,
        TimelineKind::HandedTask,
        TimelineKind::GotTask,
        TimelineKind::AskedForAccess,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            TimelineKind::Answered => "answered",
            TimelineKind::Computer => "computer",
            TimelineKind::AskedBefore => "asked_before",
            TimelineKind::Called => "called",
            TimelineKind::RanSchedule => "ran_schedule",
            TimelineKind::ReceivedWebhook => "received_webhook",
            TimelineKind::HandedTask => "handed_task",
            TimelineKind::GotTask => "got_task",
            TimelineKind::AskedForAccess => "asked_for_access",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Where a row is now: `running` and `waiting` rows are updated in place by the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineState {
    Running,
    Waiting,
    Done,
    Failed,
    Blocked,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TimelineRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TimelinePerson {
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TimelineRule {
    pub logical_id: uuid::Uuid,
    pub name: String,
}

/// What a row opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineLinkType {
    Conversation,
    Approval,
    ActionApproval,
    Run,
    Delivery,
    File,
    Rule,
    App,
    PeerTask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TimelineLink {
    #[serde(rename = "type")]
    pub ty: TimelineLinkType,
    /// The conversation, approval, run, delivery, rule (logical id), app, peer task, or (for
    /// `file`) the agent.
    pub id: String,
    /// `conversation`: the event to scroll to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    /// `file`: the absolute path under `/workspace` (the Files page).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One row of an agent's timeline (C-1083).
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TimelineItem {
    /// Stable across the row's updates: `turn:<turn>`, `computer:<turn>:<seq>`,
    /// `call:<turn>:<app or host>:<outcome>`, `call:<request>`, `approval:<id>`,
    /// `action_approval:<id>`, `run:<id>`, `delivery:<id>`, `peer:<task>`, `access:<id>`.
    pub id: String,
    pub at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub state: TimelineState,
    pub kind: TimelineKind,
    /// The kind's outcome word (C-1083); null while the row is `running` or `waiting`.
    pub outcome: Option<String>,
    pub agent: TimelineRef,
    /// `answered`: who the agent answered; `asked_before`: who decided.
    pub person: Option<TimelinePerson>,
    pub app: Option<TimelineRef>,
    pub tool: Option<String>,
    pub host: Option<String>,
    pub rule: Option<TimelineRule>,
    /// The AI model's id (the turn's most used `request_log.llm_model`).
    pub model: Option<String>,
    pub schedule: Option<TimelineRef>,
    pub webhook: Option<TimelineRef>,
    pub peer: Option<TimelineRef>,
    /// `answered`, `ran_schedule`, `received_webhook`: tool calls; `called`: requests;
    /// `computer`: files written.
    pub count: Option<i64>,
    /// `computer`: the first line of the command, at most 120 characters.
    pub command: Option<String>,
    /// `computer`: at most five workspace-relative paths.
    pub files: Option<Vec<String>>,
    pub duration_ms: Option<i64>,
    pub link: Option<TimelineLink>,
}

/// `GET /v1/agents/{agent}/timeline`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TimelinePage {
    pub items: Vec<TimelineItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TimelineQuery {
    /// Page size, 1..=100 (default 50).
    pub limit: Option<u32>,
    /// Opaque cursor from a previous page's `next_cursor` (older rows).
    pub cursor: Option<String>,
    /// Comma-separated `TimelineKind`s.
    pub kind: Option<String>,
    /// Comma-separated outcome words.
    pub outcome: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StreamQuery {
    /// Also send `request` events (the Technical view is open).
    pub technical: Option<bool>,
}

/// `event: row` of the timeline stream (C-1084).
#[derive(Debug, Serialize, ToSchema)]
pub struct TimelineRowEvent {
    /// Always `upsert`.
    pub op: String,
    pub item: TimelineItem,
}

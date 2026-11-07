use aurora_db::repo::conversations::ConversationRow;
use aurora_db::repo::transcript::EventRow;
use aurora_db::repo::turns::TurnRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct Turn {
    pub id: Uuid,
    pub conversation_id: Uuid,
    /// `queued` | `dispatched` | `running` | `done` | `failed` | `aborted`
    pub status: String,
    /// `dashboard` | `api` | `channel` | `schedule` | `webhook` | `watch` | `peer` | `system`
    pub door: String,
    pub speaker_label: Option<String>,
    pub text: String,
    pub attachment_ids: Vec<Uuid>,
    pub error_code: Option<String>,
    pub abort_requested: bool,
    pub send_now_requested: bool,
    pub usage: Option<Usage>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct Followup {
    pub id: Uuid,
    pub parent_turn_id: Uuid,
    /// `joining` | `joined` | `promoted`
    pub status: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

/// Exactly one of `turn` / `followup` is non-null (C-25).
#[derive(Serialize, ToSchema)]
pub struct MessageAccepted {
    pub turn: Option<Turn>,
    pub followup: Option<Followup>,
}

/// The person a `direct` conversation belongs to (phase 1, RFE-009).
#[derive(Serialize, ToSchema)]
pub struct DirectUser {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct Conversation {
    pub id: Uuid,
    pub agent_id: Uuid,
    /// `direct` | `group` | `schedule` | `webhook` | `watch` | `peer` | `api`
    pub kind: String,
    pub external_ref: String,
    pub title: Option<String>,
    pub direct_user_id: Option<Uuid>,
    /// Set on `direct` conversations returned by the agent's conversation list to a workspace
    /// owner who is not that person (RFE-009: "visible to workspace owners"); otherwise null.
    pub direct_user: Option<DirectUser>,
    pub origin_conversation_id: Option<Uuid>,
    pub last_seq: i64,
    /// Consecutive bot/agent turns since the last person-authored turn or `continue` (C-27).
    pub app_turn_streak: i64,
    /// The streak reached `APP_TURN_CAP`: further bot/agent turns are refused.
    pub app_turn_capped: bool,
    pub active_turn: Option<Turn>,
    pub last_activity_at: Option<DateTime<Utc>>,
    /// (phase 2, RFE-010) set while the conversation is archived (hidden from default lists).
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// `seq` is null for ephemeral events (`text.delta`, `thinking.delta`, `activity`).
#[derive(Serialize, ToSchema)]
pub struct TranscriptEvent {
    pub seq: Option<i64>,
    #[serde(rename = "type")]
    pub ty: String,
    pub ts: DateTime<Utc>,
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
}

#[derive(Serialize, ToSchema)]
pub struct EventsPage {
    pub items: Vec<TranscriptEvent>,
    pub last_seq: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateApiConversation {
    pub external_ref: String,
    pub title: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct SendMessage {
    pub text: String,
    #[serde(default)]
    pub attachment_ids: Vec<Uuid>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub kind: Option<String>,
    /// (phase 2, RFE-010) `true` lists archived conversations; default `false` hides them.
    pub archived: Option<bool>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventsQuery {
    /// Return events with `seq` greater than this (default 0).
    pub after_seq: Option<i64>,
    /// 1..=500 (default 200).
    pub limit: Option<u32>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StreamQuery {
    pub after_seq: Option<i64>,
}

/// Map the stored door object to the contract's door string.
pub fn door_name(door: &serde_json::Value) -> String {
    match door.get("kind").and_then(|k| k.as_str()) {
        Some("web") => "dashboard",
        Some("thread") => "channel",
        Some(other) => other,
        None => "system",
    }
    .to_owned()
}

impl From<TurnRow> for Turn {
    fn from(t: TurnRow) -> Self {
        let usage = t.usage.as_ref().and_then(|u| {
            let o = u.as_object()?;
            let int = |k: &str| o.get(k).and_then(|v| v.as_i64());
            Some(Usage {
                input_tokens: int("input_tokens").unwrap_or(0),
                output_tokens: int("output_tokens").unwrap_or(0),
                cache_read_tokens: int("cache_read_tokens"),
                cost_usd: o.get("cost_usd").and_then(|v| v.as_f64()),
            })
        });
        Turn {
            id: t.id,
            conversation_id: t.conversation_id,
            status: t.status,
            door: door_name(&t.door),
            speaker_label: Some(t.speaker_label),
            text: t.text,
            attachment_ids: t.attachment_ids,
            error_code: t.error_code,
            abort_requested: t.abort_requested_at.is_some(),
            send_now_requested: t.send_now_requested_at.is_some(),
            usage,
            created_at: t.created_at,
            started_at: t.started_at,
            finished_at: t.finished_at,
        }
    }
}

impl From<TurnRow> for Followup {
    fn from(t: TurnRow) -> Self {
        Followup {
            id: t.id,
            parent_turn_id: t.parent_turn_id.unwrap_or(t.id),
            status: t.status,
            text: t.text,
            created_at: t.created_at,
        }
    }
}

impl Conversation {
    pub fn from_row(c: ConversationRow, active: Option<TurnRow>) -> Self {
        Conversation {
            id: c.id,
            agent_id: c.agent_id,
            kind: c.kind,
            external_ref: c.external_ref,
            title: c.title,
            direct_user_id: c.direct_user_id,
            direct_user: None,
            origin_conversation_id: c.origin_conversation_id,
            last_seq: c.last_seq,
            app_turn_streak: i64::from(c.app_turn_streak),
            app_turn_capped: u32::try_from(c.app_turn_streak)
                .is_ok_and(|n| n >= aurora_core::limits::APP_TURN_CAP),
            active_turn: active.map(Turn::from),
            last_activity_at: Some(c.last_activity_at),
            archived_at: c.archived_at,
            created_at: c.created_at,
        }
    }
}

impl From<EventRow> for TranscriptEvent {
    fn from(e: EventRow) -> Self {
        TranscriptEvent {
            seq: Some(e.seq),
            ty: e.ty,
            ts: e.created_at,
            payload: e.payload,
        }
    }
}

/// (phase 2, RFE-010) `POST .../redact` body.
#[derive(Deserialize, ToSchema, Default)]
pub struct RedactRequest {
    /// Why the content is removed (max 500 characters); recorded in the audit row.
    pub reason: Option<String>,
    /// Also start a fresh harness session on the next turn (the agent forgets the conversation).
    #[serde(default)]
    pub reset_session: bool,
}

/// (phase 2, RFE-010) Result of a redaction.
#[derive(Serialize, ToSchema)]
pub struct RedactResult {
    /// Sequence numbers whose payload was replaced by this call (empty when already redacted).
    pub redacted_seqs: Vec<i64>,
    /// A session reset was queued.
    pub session_reset: bool,
}

/// (phase 2, RFE-010) `GET/PUT /orgs/{org}/retention`.
#[derive(Serialize, ToSchema)]
pub struct Retention {
    /// Delete conversations idle for this many days (7..=3650); null = keep forever.
    pub conversations_days: Option<i64>,
    /// The instance maximum, when one is configured.
    pub conversations_days_max: Option<i64>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, ToSchema)]
pub struct PutRetention {
    /// Absent = unchanged, `null` = none.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub conversations_days: Option<Option<i64>>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportQuery {
    /// Export events with `seq` greater than this (paging past `export_too_large`).
    pub after_seq: Option<i64>,
    /// `none` (default) or `zip` (JSON plus the attachment files).
    pub attachments: Option<String>,
}

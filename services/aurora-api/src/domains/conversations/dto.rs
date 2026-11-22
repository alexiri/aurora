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
    /// (P5-MODELS-2, C-879)
    pub cache_write_tokens: Option<i64>,
    /// The provider, endpoint and default model, stamped from the turn's model choice.
    pub provider: Option<String>,
    pub endpoint_id: Option<String>,
    pub model: Option<String>,
    /// Tokens per model, subagents included; the totals above are their sum. Absent on turns recorded
    /// before per-model usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_model: Option<Vec<ModelUsage>>,
    /// Set when the harness reported models outside what the turn could have used: those tokens were
    /// folded into the default model's entry.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unverified_models: bool,
}

/// One model's share of a turn's usage (C-879).
#[derive(Serialize, ToSchema)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
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
    /// `joining` | `joined` | `promoted` | `cancelled` (FIX-225)
    pub status: String,
    pub text: String,
    /// The turn this follow-up became; non-null only when `status` is `promoted` (D-107).
    pub promoted_turn_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// (FIX-225, C-1154) Who wrote it, as the transcript labels them.
    pub speaker_label: Option<String>,
    /// (FIX-225) When its author last edited it.
    pub edited_at: Option<DateTime<Utc>>,
    /// (FIX-225) Someone asked to steer it into the running turn.
    pub steer_requested: bool,
    /// (FIX-225) `missed` | `refused`: the harness did not take the steer; it runs as the next turn.
    pub steer_error: Option<String>,
    /// (FIX-225, C-1154) What the caller may do with it now.
    pub controls: FollowupControls,
}

/// (FIX-225, C-1154) The buttons the dashboard may show for a follow-up, computed for the caller.
/// A flag is true only when the action can apply (D-470: no silent controls).
#[derive(Serialize, ToSchema, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct FollowupControls {
    pub steer: bool,
    pub send_now: bool,
    pub edit: bool,
    pub cancel: bool,
}

/// (FIX-225, C-1154) `GET /v1/conversations/{id}/followups`.
#[derive(Serialize, ToSchema)]
pub struct FollowupList {
    pub items: Vec<Followup>,
}

/// (FIX-225, C-1155) `PATCH /v1/followups/{id}`.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EditFollowup {
    pub text: String,
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
    /// (P5-REFINE-7, C-1073) Who set `title`: `auto` | `person` | `automation`.
    pub title_source: String,
    /// (P5-REFINE-7, C-1074) The schedule or webhook this run thread belongs to; null otherwise.
    pub automation: Option<ConversationAutomation>,
    /// (P5-REFINE-7, C-1074) When the run or delivery started (the thread's `created_at`) for
    /// automation threads; null otherwise.
    pub run_at: Option<DateTime<Utc>>,
}

/// (P5-REFINE-7, C-1074) The automation of a run thread.
#[derive(Serialize, ToSchema, Clone)]
pub struct ConversationAutomation {
    /// `schedule` | `webhook`
    pub kind: String,
    pub id: Uuid,
    /// The automation's current name (the stored title when it is gone).
    pub name: String,
    /// The schedule or webhook was deleted; the thread and its transcript stay.
    pub deleted: bool,
    /// (C-1098) The automation's name before its last rename, as this thread had it; null when
    /// the thread was never renamed.
    pub previous_name: Option<String>,
}

/// (P5-REFINE-7, C-1073) `PATCH /conversations/{id}` body.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateConversation {
    /// 1–200 characters after trimming; `null` goes back to the automatic title.
    pub title: Option<String>,
}

/// (P5-REFINE-7, C-1074) One automation folder of the conversation list.
#[derive(Serialize, ToSchema)]
pub struct ConversationFolder {
    /// `schedule` | `webhook`
    pub kind: String,
    pub id: Uuid,
    pub name: String,
    pub deleted: bool,
    pub thread_count: i64,
    pub last_activity_at: DateTime<Utc>,
    /// The newest run's (or delivery's) result: `running` | `answered` | `skipped_by_gate` |
    /// `failed` | `skipped` for a schedule, the delivery `result` words for a webhook.
    pub last_result: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FoldersQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
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
    /// (P2-FU-1, C-196) The attachments referenced by `items` (`attachment` events and the
    /// `attachments` of `turn.queued`) whose bytes are gone, so the transcript renders "expired"
    /// instead of a download link. Empty when none.
    pub purged_attachments: Vec<PurgedAttachment>,
}

#[derive(Serialize, ToSchema)]
pub struct PurgedAttachment {
    pub id: Uuid,
    /// `retention` (expired by the org's window) | `deleted` (removed by a person).
    pub reason: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateApiConversation {
    pub external_ref: String,
    pub title: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct SendMessage {
    /// May be empty or absent when `attachment_ids` is non-empty (C-726).
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachment_ids: Vec<Uuid>,
    /// (C-725) Quote an earlier event of this conversation.
    #[serde(default)]
    pub reply_to: Option<ReplyToRef>,
}

/// The event a message replies to (C-725): the `seq` of a `text`, `turn.queued` or `attachment`
/// event of the same conversation.
#[derive(Deserialize, ToSchema, Clone, Copy)]
pub struct ReplyToRef {
    pub seq: i64,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub kind: Option<String>,
    /// (phase 2, RFE-010) `true` lists archived conversations; default `false` hides them.
    pub archived: Option<bool>,
    /// (P5-REFINE-7, C-1074) `all` (default) | `none` (no run threads) | `only` (run threads).
    pub automation: Option<String>,
    /// (P5-REFINE-7, C-1074) With `automation=only`: the threads of this schedule or webhook.
    pub automation_id: Option<Uuid>,
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
            let text = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_owned);
            Some(Usage {
                input_tokens: int("input_tokens").unwrap_or(0),
                output_tokens: int("output_tokens").unwrap_or(0),
                cache_read_tokens: int("cache_read_tokens"),
                cost_usd: o.get("cost_usd").and_then(|v| v.as_f64()),
                cache_write_tokens: int("cache_write_tokens"),
                provider: text("provider"),
                endpoint_id: text("endpoint_id"),
                model: text("model"),
                by_model: o.get("by_model").and_then(|v| v.as_array()).map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            Some(ModelUsage {
                                model: e.get("model")?.as_str()?.to_owned(),
                                input_tokens: e.get("input_tokens")?.as_i64()?,
                                output_tokens: e.get("output_tokens")?.as_i64()?,
                                cache_read_tokens: e
                                    .get("cache_read_tokens")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0),
                                cache_write_tokens: e
                                    .get("cache_write_tokens")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0),
                            })
                        })
                        .collect()
                }),
                unverified_models: o
                    .get("unverified_models")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
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

impl Followup {
    /// The follow-up as the caller sees it; `controls` were computed for them (C-1154).
    pub fn new(t: TurnRow, controls: FollowupControls) -> Self {
        Followup {
            id: t.id,
            parent_turn_id: t.parent_turn_id.unwrap_or(t.id),
            status: t.status,
            text: t.text,
            promoted_turn_id: None,
            created_at: t.created_at,
            speaker_label: Some(t.speaker_label),
            edited_at: t.edited_at,
            steer_requested: t.steer_requested_at.is_some(),
            steer_error: t.steer_error,
            controls,
        }
    }
}

impl Conversation {
    pub fn from_row(c: ConversationRow, active: Option<TurnRow>) -> Self {
        let automation = match (c.automation_kind, c.automation_id) {
            (Some(kind), Some(id)) => Some(ConversationAutomation {
                kind,
                id,
                name: c.title.clone().unwrap_or_default(),
                deleted: c.automation_deleted_at.is_some(),
                previous_name: c.previous_title.clone(),
            }),
            _ => None,
        };
        let run_at = automation.as_ref().map(|_| c.created_at);
        Conversation {
            title_source: c.title_source,
            automation,
            run_at,
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
    /// (phase 2, RFE-015) Purge attachment bytes older than this many days (7..=3650); null =
    /// keep forever.
    pub attachments_days: Option<i64>,
    /// The instance maximum of `attachments_days` (`attachments.days_max`), when configured.
    pub attachments_days_max: Option<i64>,
    /// The org's own attachment quota in MiB (`0` = unlimited); null = the instance default.
    pub attachment_quota_mib: Option<i64>,
    /// The quota in force in MiB after the instance default and ceiling; null = unlimited.
    pub attachment_quota_mib_effective: Option<i64>,
    /// The instance ceiling of the quota in MiB (`attachments.org_quota_mib_max`); null = none.
    pub attachment_quota_mib_max: Option<i64>,
    /// (SET-5, C-783) The instance setting behind each configured ceiling, for the dashboard
    /// link (instance admins only): `conversations_days_max`, `attachments_days_max`,
    /// `attachment_quota_mib_max`. A ceiling that is not set has no entry.
    #[schema(value_type = Object)]
    pub instance_settings: serde_json::Map<String, serde_json::Value>,
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
    /// (phase 2, RFE-015) Absent = unchanged, `null` = keep forever.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub attachments_days: Option<Option<i64>>,
    /// (phase 2, RFE-015) MiB, `0` = unlimited; absent = unchanged, `null` = the instance default.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub attachment_quota_mib: Option<Option<i64>>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportQuery {
    /// Export events with `seq` greater than this (paging past `export_too_large`).
    pub after_seq: Option<i64>,
    /// `none` (default) or `zip` (JSON plus the attachment files).
    pub attachments: Option<String>,
}

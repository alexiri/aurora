use aurora_db::repo::schedules::{RunResultRow, RunRow, ScheduleRow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct ScheduleCreator {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct Schedule {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub prompt: String,
    /// 5-field cron expression; null for a one-time schedule (P5-SCHED-ONCE, C-1092).
    pub cron: Option<String>,
    /// (C-1092) A one-time schedule's local wall time in `timezone`, `YYYY-MM-DDTHH:MM[:SS]`
    /// (the request's format); null for a recurring schedule.
    pub run_at: Option<String>,
    /// (C-1092) When a one-time schedule ran (Done, RFC 3339 UTC); it is deleted
    /// `SCHEDULE_DONE_RETENTION` later. Null otherwise.
    pub completed_at: Option<DateTime<Utc>>,
    /// IANA timezone name.
    pub timezone: String,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
    pub enabled: bool,
    /// `user` | `authorization` | `failures` when disabled.
    pub disabled_reason: Option<String>,
    /// (C-1096) The person whose own pause stands (`disabled_reason = user`); null otherwise.
    pub paused_by: Option<ScheduleCreator>,
    pub consecutive_failures: i32,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fired_at: Option<DateTime<Utc>>,
    /// Provenance when `anchor = workspace`; the fire-time authority when `anchor = creator`.
    pub created_by: Option<ScheduleCreator>,
    /// `creator` | `workspace` (RFE-021): whose access authorizes each fire.
    pub anchor: String,
    /// Null when the caller cannot see that (direct) conversation; see `conversation_hidden`.
    pub origin_conversation_id: Option<Uuid>,
    /// True when `origin_conversation_id` was redacted (phase 1, RFE-009).
    pub conversation_hidden: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// (P5-REFINE-7, C-1077) The next three instants after now (RFC 3339 UTC); empty when paused.
    /// (C-1092) One instant for a pending enabled one-time schedule; empty when Done.
    pub next_runs: Vec<DateTime<Utc>>,
    /// (P5-REFINE-7, C-1077) The newest run's result (`RunResult`); null before the first run.
    pub last_result: Option<String>,
}

impl From<ScheduleRow> for Schedule {
    fn from(r: ScheduleRow) -> Self {
        let paused_by = match (&r.disabled_reason, r.disabled_by, r.enabled) {
            (Some(reason), Some(id), false) if reason == "user" => Some(ScheduleCreator {
                id,
                display_name: r.disabled_by_name.clone().unwrap_or_default(),
            }),
            _ => None,
        };
        Self {
            id: r.id,
            agent_id: r.agent_id,
            name: r.name,
            prompt: r.prompt,
            cron: r.cron,
            run_at: r.run_at.map(|at| super::once::local_of(at, &r.timezone)),
            completed_at: r.completed_at,
            timezone: r.timezone,
            gate_command: r.gate_command,
            gate_description: r.gate_description,
            enabled: r.enabled,
            disabled_reason: r.disabled_reason,
            paused_by,
            consecutive_failures: r.consecutive_failures,
            next_fire_at: r.next_fire_at,
            last_fired_at: r.last_fire_at,
            created_by: r.creator_user_id.map(|id| ScheduleCreator {
                id,
                display_name: r.creator_name.unwrap_or_default(),
            }),
            anchor: r.anchor,
            origin_conversation_id: r.origin_conversation_id,
            conversation_hidden: false,
            created_at: r.created_at,
            updated_at: r.updated_at,
            next_runs: Vec::new(),
            last_result: None,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct ScheduleRun {
    pub id: Uuid,
    pub schedule_id: Uuid,
    /// The run log stores the fire instant only; this equals `fired_at`.
    pub scheduled_for: DateTime<Utc>,
    pub fired_at: DateTime<Utc>,
    pub turn_id: Option<Uuid>,
    /// `fired` | `gated_quiet` | `gate_failed` | `skipped_unauthorized` | `coalesced` |
    /// `skipped_frozen` | `waiting_paused` (P5-SCHED-ONCE) (stored; unchanged for compatibility).
    pub outcome: String,
    pub created_at: DateTime<Utc>,
    /// (P5-REFINE-7, C-1077) The run's thread; null when it never had one or it was deleted.
    pub conversation_id: Option<Uuid>,
    /// (C-1077) `running` | `answered` | `skipped_by_gate` | `failed` | `skipped`.
    pub result: String,
    /// (C-1077) Why it failed or was skipped: the turn's error code, `gate_failed`,
    /// `unauthorized`, `frozen`, `missed`, `paused` (a one-time schedule waiting for the paused
    /// agent, C-1092); null otherwise.
    pub reason: Option<String>,
}

impl From<RunResultRow> for ScheduleRun {
    fn from(r: RunResultRow) -> Self {
        let (result, reason) = super::results::run_result(
            &r.outcome,
            r.turn_status.as_deref(),
            r.turn_error_code.as_deref(),
        );
        Self {
            id: r.id,
            schedule_id: r.schedule_id,
            scheduled_for: r.fired_at,
            fired_at: r.fired_at,
            turn_id: r.turn_id,
            outcome: r.outcome,
            created_at: r.created_at,
            conversation_id: r.conversation_id,
            result: result.to_owned(),
            reason,
        }
    }
}

impl ScheduleRun {
    /// A run that has just started in `conversation` (Run now: C-1077).
    pub fn started(r: RunRow, conversation: Uuid) -> Self {
        Self {
            id: r.id,
            schedule_id: r.schedule_id,
            scheduled_for: r.fired_at,
            fired_at: r.fired_at,
            turn_id: r.turn_id,
            outcome: r.outcome,
            created_at: r.created_at,
            conversation_id: Some(conversation),
            result: "running".to_owned(),
            reason: None,
        }
    }
}

/// (P5-REFINE-7, C-1077) `POST /agents/{agent}/schedules/preview` body.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SchedulePreviewRequest {
    /// 5-field cron expression; exactly one of `cron` / `run_at`.
    pub cron: Option<String>,
    /// (P5-SCHED-ONCE, C-1092) A local date-time `YYYY-MM-DDTHH:MM[:SS]` without an offset, read
    /// in `timezone`.
    pub run_at: Option<String>,
    /// IANA timezone; defaults to the workspace timezone.
    pub timezone: Option<String>,
}

/// (C-1077) What a cadence would do, saved nowhere.
#[derive(Serialize, ToSchema)]
pub struct SchedulePreview {
    /// The next three instants after now (RFC 3339 UTC).
    pub next: Vec<DateTime<Utc>>,
    /// The smallest gap among the next 12 instants, in seconds; null when fewer than two.
    pub interval_secs: Option<i64>,
    /// The smallest gap is below `min_interval_secs` (create and update refuse it).
    pub too_frequent: bool,
    pub min_interval_secs: i64,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub prompt: String,
    /// 5-field cron expression (minimum interval 5 minutes); exactly one of `cron` / `run_at`.
    pub cron: Option<String>,
    /// (P5-SCHED-ONCE, C-1092) Run once at this local date-time, `YYYY-MM-DDTHH:MM[:SS]` without
    /// an offset, read in `timezone`: not in the past, at most 5 years ahead.
    pub run_at: Option<String>,
    /// IANA timezone; defaults to the workspace timezone.
    pub timezone: Option<String>,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
    /// `creator` (default) or `workspace`; `workspace` needs `can_own` (RFE-021).
    pub anchor: Option<String>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateScheduleRequest {
    pub name: Option<String>,
    pub prompt: Option<String>,
    /// Makes the schedule recurring (`run_at` becomes null).
    pub cron: Option<String>,
    /// (P5-SCHED-ONCE, C-1092) Makes the schedule one-time and re-arms a Done one; at most one
    /// of `cron` / `run_at`.
    pub run_at: Option<String>,
    /// A one-time schedule keeps its local wall time in the new zone.
    pub timezone: Option<String>,
    /// `null` removes the gate.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub gate_command: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub gate_description: Option<Option<String>>,
    pub enabled: Option<bool>,
    /// `creator` | `workspace`; changing it needs `can_own` and is audited (RFE-021).
    pub anchor: Option<String>,
}

use aurora_db::repo::schedules::{RunRow, ScheduleRow};
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
    /// 5-field cron expression.
    pub cron: String,
    /// IANA timezone name.
    pub timezone: String,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
    pub enabled: bool,
    /// `user` | `authorization` | `failures` when disabled.
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_by: Option<ScheduleCreator>,
    pub origin_conversation_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ScheduleRow> for Schedule {
    fn from(r: ScheduleRow) -> Self {
        Self {
            id: r.id,
            agent_id: r.agent_id,
            name: r.name,
            prompt: r.prompt,
            cron: r.cron,
            timezone: r.timezone,
            gate_command: r.gate_command,
            gate_description: r.gate_description,
            enabled: r.enabled,
            disabled_reason: r.disabled_reason,
            consecutive_failures: r.consecutive_failures,
            next_fire_at: r.next_fire_at,
            last_fired_at: r.last_fire_at,
            created_by: r.creator_user_id.map(|id| ScheduleCreator {
                id,
                display_name: r.creator_name.unwrap_or_default(),
            }),
            origin_conversation_id: r.origin_conversation_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
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
    /// `fired` | `gated_quiet` | `gate_failed` | `skipped_unauthorized` | `coalesced`.
    pub outcome: String,
    pub created_at: DateTime<Utc>,
}

impl From<RunRow> for ScheduleRun {
    fn from(r: RunRow) -> Self {
        Self {
            id: r.id,
            schedule_id: r.schedule_id,
            scheduled_for: r.fired_at,
            fired_at: r.fired_at,
            turn_id: r.turn_id,
            outcome: r.outcome,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub prompt: String,
    /// 5-field cron expression (minimum interval 5 minutes).
    pub cron: String,
    /// IANA timezone; defaults to the workspace timezone.
    pub timezone: Option<String>,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
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
    pub cron: Option<String>,
    pub timezone: Option<String>,
    /// `null` removes the gate.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub gate_command: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub gate_description: Option<Option<String>>,
    pub enabled: Option<bool>,
}

//! Web Push wire types (C-830, C-831). Never an endpoint, host-qualified URL or key.
use aurora_db::repo::push::SubRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct PushConfig {
    /// `push.enabled` and a usable key.
    pub available: bool,
    /// base64url of the 65-byte uncompressed P-256 point of the active VAPID key.
    pub vapid_public_key: Option<String>,
    /// The endpoint hosts a subscription may name (exact, or `*.suffix`).
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushState {
    Active,
    Paused,
    Disabled,
    Expired,
    StaleKey,
}

impl PushState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Disabled => "disabled",
            Self::Expired => "expired",
            Self::StaleKey => "stale_key",
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PushSubscription {
    pub id: Uuid,
    pub label: String,
    pub show_details: bool,
    pub paused: bool,
    pub state: PushState,
    /// Registered by the session making this request.
    pub this_session: bool,
    pub created_at: DateTime<Utc>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
}

impl PushSubscription {
    pub fn from_row(r: &SubRow, current_session: Option<Uuid>) -> Self {
        Self {
            id: r.id,
            label: r.label.clone(),
            show_details: r.show_details,
            paused: r.paused_at.is_some(),
            state: super::state_of(r),
            this_session: current_session == Some(r.session_id),
            created_at: r.created_at,
            last_success_at: r.last_success_at,
            last_failure_at: r.last_failure_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PushSubscriptionList {
    pub subscriptions: Vec<PushSubscription>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePushSubscription {
    pub endpoint: String,
    pub keys: PushKeys,
    /// The browser's `expirationTime` (milliseconds since the epoch), when it has one.
    #[serde(default)]
    pub expiration_time: Option<i64>,
    /// At most 80 characters.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub show_details: Option<bool>,
    /// The application-server key the browser subscribed with (`GET /v1/push/config`).
    pub vapid_public_key: String,
    /// Re-attach this browser's registration to the current session; succeeds only when the
    /// endpoint is already registered to the caller.
    #[serde(default)]
    pub rebind: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdatePushSubscription {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub show_details: Option<bool>,
    #[serde(default)]
    pub paused: Option<bool>,
}

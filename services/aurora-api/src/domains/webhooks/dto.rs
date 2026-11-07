use aurora_db::repo::webhooks::{DeliveryRow, WebhookRow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct WebhookCreator {
    pub id: Uuid,
    pub display_name: String,
}

/// Verification as exposed: never contains the secret.
#[derive(Serialize, ToSchema)]
pub struct VerificationView {
    /// `none` | `hmac_sha256` | `bearer`.
    pub kind: String,
    pub header: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Webhook {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub instructions: String,
    pub verification: VerificationView,
    pub enabled: bool,
    /// `user` | `authorization` | `failures` when disabled.
    pub disabled_reason: Option<String>,
    /// Display prefix of the capability token (the full URL is shown once).
    pub url_prefix: String,
    pub created_by: Option<WebhookCreator>,
    pub last_delivery_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<WebhookRow> for Webhook {
    fn from(r: WebhookRow) -> Self {
        let field = |k: &str| {
            r.verification
                .as_ref()
                .and_then(|v| v.get(k))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let verification = VerificationView {
            kind: field("kind").unwrap_or_else(|| "none".into()),
            header: field("header"),
        };
        Self {
            id: r.id,
            agent_id: r.agent_id,
            name: r.name,
            instructions: r.instructions,
            verification,
            enabled: r.enabled,
            disabled_reason: r.disabled_reason,
            url_prefix: r.display_prefix,
            created_by: r.creator_user_id.map(|id| WebhookCreator {
                id,
                display_name: r.creator_name.unwrap_or_default(),
            }),
            last_delivery_at: r.last_delivery_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Returned by create and rotate: the URL embeds the token and is shown once.
#[derive(Serialize, ToSchema)]
pub struct WebhookWithUrl {
    pub webhook: Webhook,
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct WebhookDelivery {
    /// Also the `delivery_id` returned to the sender.
    pub id: Uuid,
    pub received_at: DateTime<Utc>,
    /// `accepted` | `rejected_signature` | `rejected_size` | `disabled` | `unauthorized`
    /// (`duplicate` deliveries are answered with the original id and not logged).
    pub status: String,
    pub turn_id: Option<Uuid>,
    pub size: i32,
}

impl From<DeliveryRow> for WebhookDelivery {
    fn from(r: DeliveryRow) -> Self {
        Self {
            id: r.id,
            received_at: r.created_at,
            status: api_status(&r.status, r.reason.as_deref()),
            turn_id: r.turn_id,
            size: r.size_bytes,
        }
    }
}

/// Map the stored (`accepted`|`rejected`|`paused` + reason) pair to the API status.
pub fn api_status(status: &str, reason: Option<&str>) -> String {
    match (status, reason) {
        ("accepted", _) => "accepted",
        ("paused", _) => "disabled",
        ("rejected", Some(r @ ("rejected_signature" | "rejected_size" | "unauthorized"))) => r,
        _ => "unauthorized",
    }
    .to_owned()
}

/// Write-only verification settings.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationInput {
    /// `none` | `hmac_sha256` (hex digest of the raw body) | `bearer`.
    pub kind: String,
    /// Header carrying the signature/token (defaults: `X-Signature`, `Authorization`).
    pub header: Option<String>,
    /// Shared secret; required when choosing a kind (or switching to one). Never returned.
    pub secret: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookRequest {
    pub name: String,
    pub instructions: String,
    pub verification: Option<VerificationInput>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateWebhookRequest {
    pub name: Option<String>,
    pub instructions: Option<String>,
    pub verification: Option<VerificationInput>,
    /// `true` re-enables (clears the failure count; the caller becomes the authority anchor).
    pub enabled: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct HookAccepted {
    pub delivery_id: Uuid,
}

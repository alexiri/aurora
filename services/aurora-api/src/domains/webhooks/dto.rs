use aurora_db::repo::webhooks::{DeliveryResultRow, DeliveryRow, WebhookRow};
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
    /// (P5-REFINE-7, C-1078) The effective header (`X-Signature` / `Authorization` by default);
    /// null for `none`.
    pub header: Option<String>,
    /// (C-1078) How the sender fills the header: `sha256=<hex of the HMAC-SHA256 of the raw body,
    /// keyed with the secret>` or `Bearer <secret>`; null for `none`.
    pub format: Option<String>,
}

/// (C-1078) The header value recipe of a verification kind.
pub fn verification_format(kind: &str) -> Option<&'static str> {
    match kind {
        "hmac_sha256" => {
            Some("sha256=<hex of the HMAC-SHA256 of the raw body, keyed with the secret>")
        }
        "bearer" => Some("Bearer <secret>"),
        _ => None,
    }
}

/// (C-1078) The default header of a verification kind.
pub fn default_header(kind: &str) -> Option<&'static str> {
    match kind {
        "hmac_sha256" => Some("X-Signature"),
        "bearer" => Some("Authorization"),
        _ => None,
    }
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
    /// Provenance when `anchor = workspace`; the fire-time authority when `anchor = creator`.
    pub created_by: Option<WebhookCreator>,
    /// `creator` | `workspace` (RFE-021): whose access authorizes each delivery.
    pub anchor: String,
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
        let kind = field("kind").unwrap_or_else(|| "none".into());
        let verification = VerificationView {
            header: field("header")
                .filter(|_| kind != "none")
                .or_else(|| default_header(&kind).map(str::to_owned)),
            format: verification_format(&kind).map(str::to_owned),
            kind,
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
            anchor: r.anchor,
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
    /// (P5-REFINE-7, C-1078) The secret Aurora generated on this call (`generate_secret`, or a
    /// create without `verification`); shown once, never again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// (C-1078) `POST /webhooks/{id}/rotate-secret`: the new secret, shown once.
#[derive(Serialize, ToSchema)]
pub struct WebhookSecret {
    pub webhook: Webhook,
    pub secret: String,
}

#[derive(Serialize, ToSchema)]
pub struct WebhookDelivery {
    /// Also the `delivery_id` returned to the sender.
    pub id: Uuid,
    pub received_at: DateTime<Utc>,
    /// `accepted` | `rejected_signature` | `rejected_size` | `disabled` | `unauthorized` |
    /// (P5-REFINE-7, C-1079) `duplicate`.
    pub status: String,
    pub turn_id: Option<Uuid>,
    pub size: i32,
    /// (C-1079) `answered` | `running` | `failed` | `refused_signature` | `refused_too_large` |
    /// `refused_paused` | `refused_frozen` | `refused_unauthorized` | `duplicate`.
    pub result: String,
    /// (C-1079) The delivery's own thread (a duplicate: the original's); null when none.
    pub conversation_id: Option<Uuid>,
    /// (C-1079) For a `duplicate`: the delivery it repeated.
    pub duplicate_of: Option<Uuid>,
}

impl From<DeliveryRow> for WebhookDelivery {
    fn from(r: DeliveryRow) -> Self {
        Self::from(DeliveryResultRow {
            id: r.id,
            webhook_id: r.webhook_id,
            turn_id: r.turn_id,
            status: r.status,
            size_bytes: r.size_bytes,
            reason: r.reason,
            created_at: r.created_at,
            conversation_id: None,
            turn_status: None,
            turn_error_code: None,
        })
    }
}

impl From<DeliveryResultRow> for WebhookDelivery {
    fn from(r: DeliveryResultRow) -> Self {
        let result = crate::domains::schedules::results::delivery_result(
            &r.status,
            r.reason.as_deref(),
            r.turn_status.as_deref(),
            r.turn_error_code.as_deref(),
        );
        let duplicate_of = (r.status == "duplicate")
            .then(|| r.reason.as_deref().and_then(|s| s.parse::<Uuid>().ok()))
            .flatten();
        Self {
            id: r.id,
            received_at: r.created_at,
            status: api_status(&r.status, r.reason.as_deref()),
            turn_id: r.turn_id,
            size: r.size_bytes,
            result: result.to_owned(),
            conversation_id: r.conversation_id,
            duplicate_of,
        }
    }
}

/// Map the stored (`accepted`|`rejected`|`paused`|`duplicate` + reason) pair to the API status.
pub fn api_status(status: &str, reason: Option<&str>) -> String {
    match (status, reason) {
        ("accepted", _) => "accepted",
        ("duplicate", _) => "duplicate",
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
    /// Shared secret; required when choosing a kind (or switching to one) unless
    /// `generate_secret`. Never returned.
    pub secret: Option<String>,
    /// (P5-REFINE-7, C-1078) Create only: Aurora makes the secret (32 random bytes, 64 hex
    /// characters) and returns it once. Not with `secret`; on update use `rotate-secret`.
    #[serde(default)]
    pub generate_secret: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookRequest {
    pub name: String,
    pub instructions: String,
    pub verification: Option<VerificationInput>,
    /// `creator` (default) or `workspace`; `workspace` needs `can_own` (RFE-021).
    pub anchor: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateWebhookRequest {
    pub name: Option<String>,
    pub instructions: Option<String>,
    pub verification: Option<VerificationInput>,
    /// `true` re-enables (clears the failure count; the caller becomes the authority anchor).
    pub enabled: Option<bool>,
    /// `creator` | `workspace`; changing it needs `can_own` and is audited (RFE-021).
    pub anchor: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct HookAccepted {
    pub delivery_id: Uuid,
}

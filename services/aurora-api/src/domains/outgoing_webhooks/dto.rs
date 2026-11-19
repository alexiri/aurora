//! (phase 5, P5-WEBHOOK-2, D-320, C-909, C-912) Wire shapes of the outgoing-webhook routes. The
//! configuration types are `aurora-webhook`'s own (one source of truth). Request bodies that mix the
//! configuration with edit-only keys are read as JSON by the handlers (strict, as the crate's types
//! are) and described here for the OpenAPI document.
use std::collections::BTreeMap;

use aurora_webhook::WebhookConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// How a signing secret is set on connect.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SigningInput {
    /// Connect: `generate` | `provided`. Edit: `keep` | `generate` | `provided` | `off`.
    pub mode: String,
    /// The secret, for `provided`.
    pub value: Option<String>,
}

/// `webhook` of a connect: the configuration and the secrets the person enters.
#[derive(Serialize, ToSchema)]
pub struct WebhookConnectBody {
    #[serde(flatten)]
    pub config: WebhookConfig,
    /// Secret values by name; each ≤ 4 KiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_values: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_secret: Option<SigningInput>,
}

/// The effective send limit and who lowered it.
#[derive(Serialize, ToSchema)]
pub struct EffectiveRateDto {
    pub per_minute: u32,
    pub per_day: u32,
    /// `instance` | `org` | `connection`.
    pub source: String,
}

/// `GET /v1/connections/{id}/webhook`: never a URL, a secret value or the signing secret.
#[derive(Serialize, ToSchema)]
pub struct WebhookView {
    #[serde(flatten)]
    pub config: WebhookConfig,
    pub host: String,
    pub secret_value_names: Vec<String>,
    pub effective_rate: EffectiveRateDto,
}

/// `PUT /v1/connections/{id}/webhook` request: the configuration plus the edits of the secrets.
#[derive(Serialize, ToSchema)]
pub struct PutWebhookRequest {
    #[serde(flatten)]
    pub config: WebhookConfig,
    /// A value sets, `null` deletes, an absent name is kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_values: Option<BTreeMap<String, Option<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_secret: Option<SigningInput>,
}

/// The answer of an edit: the view, and the signing secret when one was generated (once).
#[derive(Serialize, ToSchema)]
pub struct PutWebhookResponse {
    #[serde(flatten)]
    pub view: WebhookView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_secret: Option<String>,
}

/// The configuration of a preview: the configuration and `secret_value_names`, the names of the secret
/// values the person will enter.
#[derive(Serialize, ToSchema)]
pub struct PreviewConfig {
    #[serde(flatten)]
    pub config: WebhookConfig,
    pub secret_value_names: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PreviewRequest {
    pub config: PreviewConfig,
    /// The slot values.
    pub values: Value,
}

#[derive(Serialize, ToSchema)]
pub struct PreviewResponse {
    pub method: String,
    pub content_type: String,
    /// Header names only.
    pub headers: Vec<String>,
    /// The rendered body, secrets shown as `«secret:name»`.
    pub body: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TestRequest {
    #[serde(default)]
    pub values: Value,
    #[serde(default)]
    pub dry_run: bool,
}

/// The answer of a send test, or with `dry_run` the preview of the stored configuration.
#[derive(Serialize, ToSchema)]
pub struct TestResponse {
    /// The receiver's status (a real send).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub content_type: Option<String>,
    /// The receiver's answer (≤ 4 KiB, echo-checked), or the masked body of a dry run.
    pub body: Option<String>,
    pub truncated: Option<bool>,
    pub delivery_id: Option<String>,
    pub method: Option<String>,
    pub headers: Option<Vec<String>>,
}

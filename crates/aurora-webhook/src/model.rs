//! The types of C-909: the configuration of an outgoing webhook and its parts. Strict on input
//! (`deny_unknown_fields`); the API stores and serves exactly these shapes.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Highest per-minute / per-day limit a connection may set (C-912's instance ceilings). The
/// effective ceiling is lower when the instance or org sets one; the API checks that.
pub const RATE_PER_MINUTE_MAX: u32 = 600;
pub const RATE_PER_DAY_MAX: u32 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    #[serde(rename = "POST")]
    Post,
    #[serde(rename = "PUT")]
    Put,
}

impl Method {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Post => "POST",
            Self::Put => "PUT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType {
    #[serde(rename = "application/json")]
    Json,
    #[serde(rename = "application/x-www-form-urlencoded")]
    Form,
    #[serde(rename = "text/plain")]
    Text,
}

impl ContentType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Form => "application/x-www-form-urlencoded",
            Self::Text => "text/plain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotType {
    String,
    Integer,
    Number,
    Boolean,
    StringList,
    Enum,
    Json,
}

/// One typed blank of the template that the agent fills in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: SlotType,
    pub required: bool,
    #[serde(default)]
    pub description: String,
    /// `string` and `string_list` items: at most this many Unicode scalar values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    /// `string_list`: at most this many items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// `enum`: the allowed values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#enum: Option<Vec<String>>,
    /// `json`: the schema subset of C-909.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<Value>,
}

/// A header sent to the receiver: a constant value, or one with `{{secret:name}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderTemplate {
    pub name: String,
    pub value: String,
}

/// The connection's own send limits (`null` = the ceiling).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    #[serde(default)]
    pub per_minute: Option<u32>,
    #[serde(default)]
    pub per_day: Option<u32>,
}

fn de_version<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = u32::deserialize(d)?;
    if v == 1 {
        Ok(v)
    } else {
        Err(serde::de::Error::custom("version must be 1"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    #[serde(deserialize_with = "de_version")]
    pub version: u32,
    pub method: Method,
    pub content_type: ContentType,
    pub template: String,
    #[serde(default)]
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub headers: Vec<HeaderTemplate>,
    /// The person's note for the agent (≤ 1,000 characters).
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub signing: bool,
    #[serde(default)]
    pub rate: Rate,
    #[serde(default)]
    pub preset: Option<String>,
}

/// A preset file of a package (C-909): holds no secret value; the names say which ones the person
/// must enter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookPreset {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggest_for_hosts: Vec<String>,
    #[serde(default)]
    pub secret_value_names: Vec<String>,
    pub config: WebhookConfig,
}

/// One validation failure: a JSON pointer into the submitted object and a C-909 code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldError {
    pub path: String,
    pub code: &'static str,
}

impl FieldError {
    pub(crate) fn new(path: impl Into<String>, code: &'static str) -> Self {
        Self {
            path: path.into(),
            code,
        }
    }
}

/// The rule a slot value broke (C-910 `invalid_slot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Missing,
    Unknown,
    Type,
    MaxLength,
    MaxItems,
    Enum,
    Schema,
}

impl Rule {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unknown => "unknown",
            Self::Type => "type",
            Self::MaxLength => "max_length",
            Self::MaxItems => "max_items",
            Self::Enum => "enum",
            Self::Schema => "schema",
        }
    }
}

/// A slot value refused: the slot (empty when the values body is not an object) and the rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotError {
    pub slot: String,
    pub rule: Rule,
}

/// The connection's secret values (sealed in the credential), by name. Never printed.
#[derive(Clone, Default)]
pub struct SecretValues(pub std::collections::BTreeMap<String, String>);

impl std::fmt::Debug for SecretValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretValues({} names)", self.0.len())
    }
}

/// What a render produces; `headers` excludes `Content-Type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub content_type: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

/// Why a render could not produce a request. The messages carry no value or secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// The rendered body exceeds `WEBHOOK_BODY_MAX` (413 `rendered_too_large`).
    TooLarge,
    /// A secret the template names is not in the connection's secret values.
    MissingSecret(String),
    /// A secret value would put a control character into a header.
    HeaderValue(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "rendered body too large"),
            Self::MissingSecret(n) => write!(f, "secret value `{n}` is not set"),
            Self::HeaderValue(n) => write!(f, "header `{n}` cannot carry the secret value"),
        }
    }
}

impl std::error::Error for RenderError {}

//! (P5-GUIDE-6, `docs/24 §3.5`, C-651) The operations file: the model of
//! `agent/<app_id>/<file>.json` (`aurora.operations/1`). Closed schema: unknown keys anywhere are
//! errors (`deny_unknown_fields`). The linter is `validate::agent_operations_rules`; the renderer
//! re-emits the validated entries into `api.json` (`app_folder::render`).
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only accepted `format` string; anything else fails closed (`agent_operations_format`).
pub const FORMAT: &str = "aurora.operations/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationsFile {
    pub format: String,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: String,
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default)]
    pub body: Option<Body>,
    #[serde(default)]
    pub pagination: Option<Pagination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamIn {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub name: String,
    #[serde(rename = "in")]
    pub location: ParamIn,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// A top-level body field: a [`Param`] without `in`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub content_type: String,
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "style", rename_all = "snake_case", deny_unknown_fields)]
pub enum Pagination {
    LinkHeader,
    PageParam {
        param: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size_param: Option<String>,
    },
    Cursor {
        param: String,
        next: String,
    },
}

/// Why a file did not parse; maps one to one onto a violation code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Json(String),
    Format(String),
    UnknownKey(String),
}

impl ParseError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Json(_) => "agent_operations_json",
            Self::Format(_) => "agent_operations_format",
            Self::UnknownKey(_) => "agent_operations_unknown_key",
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Json(m) | Self::Format(m) | Self::UnknownKey(m) => m,
        }
    }
}

/// Parse an operations file: `format` is checked on the raw document first so that a future
/// format is `agent_operations_format`, not a schema error.
pub fn parse(bytes: &[u8]) -> Result<OperationsFile, ParseError> {
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|e| ParseError::Json(format!("not JSON: {e}")))?;
    match raw.get("format").and_then(Value::as_str) {
        Some(FORMAT) => {}
        other => {
            return Err(ParseError::Format(format!(
                "`format` must be \"{FORMAT}\", found {other:?}"
            )));
        }
    }
    serde_json::from_value(raw).map_err(|e| {
        let m = e.to_string();
        if m.contains("unknown field") {
            ParseError::UnknownKey(m)
        } else {
            ParseError::Json(m)
        }
    })
}

/// Number of operations in a stored file, `0` when it does not parse (the linter rejected it at
/// ingest, so a stored file always parses).
#[must_use]
pub fn count(bytes: &[u8]) -> usize {
    parse(bytes).map_or(0, |f| f.operations.len())
}

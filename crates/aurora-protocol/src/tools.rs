//! W7 platform tool calls. Normative: `docs/16-wire-protocols.md` §W7.
//!
//! Path: harness -> mcp-bridge -> supervisor (`tool.call` frame) -> runner ->
//! `POST /v1/runner/tool-calls` -> API tool registry.

use aurora_core::limits;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::common::{Validate, ValidationError, check_chars, check_max, json_len};
use crate::runner::Code;

/// Max serialized size of a tool call / result: it must fit one WS frame.
pub const MAX_TOOL_MESSAGE: usize = limits::WS_FRAME_MAX;
/// Max tool name length (chars). Provisional; not in the limits registry.
pub const MAX_TOOL_NAME: usize = 128;
/// Max `call_id` length (chars). Provisional; format is unspecified in `16`.
pub const MAX_CALL_ID: usize = 128;

/// `tool.call` frame body (supervisor -> runner): a call without identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallBody {
    pub call_id: String,
    pub conversation_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Uuid>,
    pub tool: String,
    pub args: Value,
}

/// `POST /v1/runner/tool-calls` request; identity is stamped by the runner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    #[serde(flatten)]
    pub call: ToolCallBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextContent {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonContent {
    pub value: Value,
}

tagged_open! {
    /// Tool result content item, tagged by `type`.
    pub enum ToolContent, mode = Internal, tag = "type" {
        Text(TextContent) = "text",
        Json(JsonContent) = "json",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolError {
    pub code: Code,
    pub message: String,
}

/// `{ call_id, ok: true, content }` or `{ call_id, ok: false, error }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolResult {
    pub call_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ToolContent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
}

impl ToolResult {
    pub fn success(call_id: impl Into<String>, content: Vec<ToolContent>) -> Self {
        Self {
            call_id: call_id.into(),
            ok: true,
            content: Some(content),
            error: None,
        }
    }

    pub fn failure(call_id: impl Into<String>, error: ToolError) -> Self {
        Self {
            call_id: call_id.into(),
            ok: false,
            content: None,
            error: Some(error),
        }
    }
}

/// One entry of the synced tool catalogue as answered by MCP `tools/list`.
/// Provisional: `16` does not define the catalogue entry shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

impl Validate for ToolCallBody {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("call_id", &self.call_id, MAX_CALL_ID)?;
        check_chars("tool", &self.tool, MAX_TOOL_NAME)?;
        check_max("tool call", json_len(self), MAX_TOOL_MESSAGE)
    }
}

impl Validate for ToolCall {
    fn validate(&self) -> Result<(), ValidationError> {
        self.call.validate()
    }
}

impl Validate for ToolResult {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("call_id", &self.call_id, MAX_CALL_ID)?;
        match (self.ok, &self.content, &self.error) {
            (true, Some(_), None) | (false, None, Some(_)) => {}
            _ => {
                return Err(ValidationError::Invalid {
                    field: "ok",
                    reason: "ok=true requires content only; ok=false requires error only",
                });
            }
        }
        check_max("tool result", json_len(self), MAX_TOOL_MESSAGE)
    }
}

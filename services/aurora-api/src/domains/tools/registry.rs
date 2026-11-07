use crate::app::AppState;
use aurora_core::{ApiError, limits};
use aurora_db::DbError;
use aurora_protocol::runner::Code;
use aurora_protocol::tools::{
    MAX_TOOL_NAME, TextContent, ToolContent, ToolDescriptor, ToolError as WireError, ToolResult,
};
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

/// Result bound (serialized), docs/12 §Platform tools.
const MAX_RESULT_BYTES: usize = 64 * 1024;

/// Agent-level facts a tool may depend on (availability, descriptions).
#[derive(Clone)]
pub struct ToolContext {
    pub pool: PgPool,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
}

/// Per-call context, derived from authenticated facts only.
#[derive(Clone)]
pub struct ToolCallContext {
    pub agent: ToolContext,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub call_id: String,
    /// Full app state for tools that need the app catalogue, governance or secrets validation.
    /// Prefer `agent.pool` for plain database access.
    pub state: AppState,
}

/// Tool failure; becomes an in-band `ok: false` result the model can read.
#[derive(Debug)]
pub enum ToolError {
    /// Arguments do not match the input schema.
    InvalidArguments(String),
    /// The tool is not available to this agent (with an explanation).
    Unavailable(String),
    /// Any other failure with a model-readable message.
    Failed(String),
}

impl From<DbError> for ToolError {
    fn from(e: DbError) -> Self {
        tracing::error!(error = %e, "tool db error");
        Self::Failed("internal error".into())
    }
}

impl From<sqlx::Error> for ToolError {
    fn from(e: sqlx::Error) -> Self {
        DbError::from(e).into()
    }
}

impl ToolError {
    fn into_wire(self) -> WireError {
        let (code, message) = match self {
            Self::InvalidArguments(m) => ("invalid_arguments", m),
            Self::Unavailable(m) => ("tool_unavailable", m),
            Self::Failed(m) => ("tool_failed", m),
        };
        WireError {
            code: code_of(code),
            message,
        }
    }
}

fn code_of(c: &str) -> Code {
    Code::new(c).unwrap_or_else(|_| unreachable!("tool error codes are short constants"))
}

#[async_trait::async_trait]
pub trait PlatformTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self, ctx: &ToolContext) -> String;
    /// JSON Schema (`type: object`).
    fn input_schema(&self) -> Value;
    fn available(&self, ctx: &ToolContext) -> bool;
    /// Availability that needs the database (e.g. messaging tools exist only while the agent
    /// has an active channel presence). Checked in addition to [`available`](Self::available)
    /// when the catalogue is rendered ([`Registry::catalogue_for`]) and at call time.
    async fn enabled(&self, _ctx: &ToolContext) -> bool {
        true
    }
    /// Explanation returned when a stale session calls a tool that is no longer available.
    fn unavailable_reason(&self) -> &'static str {
        "This tool is not available to this agent."
    }
    async fn call(&self, ctx: &ToolCallContext, args: Value)
    -> Result<Vec<ToolContent>, ToolError>;
}

pub struct Registry {
    tools: HashMap<&'static str, Box<dyn PlatformTool>>,
    order: Vec<&'static str>,
}

impl Registry {
    /// Load-time completeness: unique, well-formed names and object schemas.
    pub fn new(list: Vec<Box<dyn PlatformTool>>) -> Result<Self, String> {
        let mut tools = HashMap::new();
        let mut order = Vec::new();
        for t in list {
            let n = t.name();
            let ok = !n.is_empty()
                && n.len() <= MAX_TOOL_NAME
                && n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
            if !ok {
                return Err(format!("invalid tool name {n:?}"));
            }
            if t.input_schema().get("type").and_then(Value::as_str) != Some("object") {
                return Err(format!("tool {n}: input_schema must be an object schema"));
            }
            if tools.insert(n, t).is_some() {
                return Err(format!("duplicate tool {n}"));
            }
            order.push(n);
        }
        Ok(Self { tools, order })
    }

    /// Available tools for the agent, in registration order.
    pub fn catalogue(&self, ctx: &ToolContext) -> Vec<ToolDescriptor> {
        self.order
            .iter()
            .filter_map(|n| self.tools.get(n))
            .filter(|t| t.available(ctx))
            .map(|t| ToolDescriptor {
                name: t.name().to_owned(),
                description: Some(t.description(ctx)),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    /// [`catalogue`](Self::catalogue) plus the tools' database-backed [`PlatformTool::enabled`].
    pub async fn catalogue_for(&self, ctx: &ToolContext) -> Vec<ToolDescriptor> {
        let mut out = Vec::new();
        for t in self.order.iter().filter_map(|n| self.tools.get(n)) {
            if t.available(ctx) && t.enabled(ctx).await {
                out.push(ToolDescriptor {
                    name: t.name().to_owned(),
                    description: Some(t.description(ctx)),
                    input_schema: t.input_schema(),
                });
            }
        }
        out
    }

    /// Routes a call; every failure is an in-band `ok: false` result. Bounded by `timeout`.
    pub async fn call(
        &self,
        ctx: &ToolCallContext,
        tool: &str,
        args: Value,
        timeout: Duration,
    ) -> ToolResult {
        let id = ctx.call_id.clone();
        let fail = |e: ToolError| ToolResult::failure(id.clone(), e.into_wire());
        let Some(t) = self.tools.get(tool) else {
            return ToolResult::failure(
                id,
                WireError {
                    code: code_of("unknown_tool"),
                    message: format!("Unknown tool `{tool}`."),
                },
            );
        };
        if !t.available(&ctx.agent) || !t.enabled(&ctx.agent).await {
            return fail(ToolError::Unavailable(t.unavailable_reason().into()));
        }
        match tokio::time::timeout(timeout, t.call(ctx, args)).await {
            Err(_) => ToolResult::failure(
                id,
                WireError {
                    code: code_of("tool_timeout"),
                    message: format!("Tool `{tool}` timed out."),
                },
            ),
            Ok(Err(e)) => fail(e),
            Ok(Ok(content)) => ToolResult::success(id, bound(content)),
        }
    }
}

/// Truncates oversized results (with a note) to `MAX_RESULT_BYTES` serialized.
fn bound(content: Vec<ToolContent>) -> Vec<ToolContent> {
    let ser = serde_json::to_string(&content).unwrap_or_default();
    if ser.len() <= MAX_RESULT_BYTES {
        return content;
    }
    let mut end = MAX_RESULT_BYTES / 2; // headroom for JSON escaping
    while !ser.is_char_boundary(end) {
        end -= 1;
    }
    vec![ToolContent::Text(TextContent {
        text: format!(
            "{}\n[result truncated: {} bytes exceeded the {} byte limit]",
            &ser[..end],
            ser.len(),
            MAX_RESULT_BYTES
        ),
    })]
}

/// Process-wide registry of built-in tools.
pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| match Registry::new(super::builtin::tools()) {
        Ok(r) => r,
        // A malformed built-in set is a programming error, covered by a unit test.
        Err(e) => unreachable!("invalid built-in tool registry: {e}"),
    })
}

/// Catalogue for an agent, as rendered into the home sync inputs (BE-052).
pub async fn catalogue(pool: &PgPool, agent_id: Uuid) -> Result<Vec<ToolDescriptor>, ApiError> {
    let a = aurora_db::repo::agents::get(pool, agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Unknown agent."))?;
    let ctx = ToolContext {
        pool: pool.clone(),
        agent_id,
        workspace_id: a.workspace_id,
        org_id: a.org_id,
    };
    Ok(registry().catalogue_for(&ctx).await)
}

/// Default per-call budget.
pub const CALL_TIMEOUT: Duration = limits::TOOL_CALL_TIMEOUT;

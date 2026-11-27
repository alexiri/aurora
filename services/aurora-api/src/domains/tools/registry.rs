use crate::app::AppState;
use aurora_core::{ApiError, limits};
use aurora_db::DbError;
use aurora_protocol::runner::Code;
use aurora_protocol::tools::{
    Exposure, MAX_TOOL_NAME, TextContent, ToolContent, ToolDescriptor, ToolError as WireError,
    ToolResult,
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
    /// A failure with a code of the tool's own (C-844: `no_person`, `rate_limited`, …); the code
    /// is a short constant.
    Coded(&'static str, String),
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
            Self::Coded(c, m) => (c, m),
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
    /// (P5-GUIDE-7, C-654) The family the tool is indexed under (`24 §10b`). No default, so a new
    /// tool cannot forget it.
    fn family(&self) -> &'static str;
    /// (P5-GUIDE-7, C-654) `Core` tools are listed to the harness by the bridge; the rest are
    /// found with `aurora-agent`. Decides what is **listed**, never what is allowed.
    fn exposure(&self) -> Exposure {
        Exposure::OnDemand
    }
    fn description(&self, ctx: &ToolContext) -> String;
    /// JSON Schema (`type: object`).
    fn input_schema(&self) -> Value;
    fn available(&self, ctx: &ToolContext) -> bool;
    /// The tool sends e-mail, so it exists only while the instance mailer is configured (SMTP set,
    /// not log-only; SET-4's live flag). Decided where the app state is at hand
    /// ([`catalogue`] and [`Registry::call`]).
    fn needs_mailer(&self) -> bool {
        false
    }
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

fn descriptor(t: &dyn PlatformTool, ctx: &ToolContext) -> ToolDescriptor {
    ToolDescriptor {
        name: t.name().to_owned(),
        description: Some(t.description(ctx)),
        input_schema: t.input_schema(),
        family: Some(t.family().to_owned()),
        exposure: Some(t.exposure()),
    }
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
        // (P5-GUIDE-7, D-297) The core set stays small by review; one local tool (`send_file`)
        // is core on the supervisor's side.
        let core = tools
            .values()
            .filter(|t| t.exposure() == Exposure::Core)
            .count()
            + 1;
        if core > limits::PLATFORM_TOOLS_CORE_MAX {
            return Err(format!(
                "{core} core platform tools exceed PLATFORM_TOOLS_CORE_MAX ({})",
                limits::PLATFORM_TOOLS_CORE_MAX
            ));
        }
        Ok(Self { tools, order })
    }

    /// Available tools for the agent, in registration order.
    pub fn catalogue(&self, ctx: &ToolContext) -> Vec<ToolDescriptor> {
        self.order
            .iter()
            .filter_map(|n| self.tools.get(n))
            .filter(|t| t.available(ctx))
            .map(|t| descriptor(t.as_ref(), ctx))
            .collect()
    }

    /// [`catalogue`](Self::catalogue) plus the tools' database-backed [`PlatformTool::enabled`];
    /// `mailer` is whether the instance mailer is configured (tools that
    /// [need it](PlatformTool::needs_mailer) are absent otherwise).
    pub async fn catalogue_for(&self, ctx: &ToolContext, mailer: bool) -> Vec<ToolDescriptor> {
        let mut out = Vec::new();
        for t in self.order.iter().filter_map(|n| self.tools.get(n)) {
            if (mailer || !t.needs_mailer()) && t.available(ctx) && t.enabled(ctx).await {
                out.push(descriptor(t.as_ref(), ctx));
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
        // (P4-EVAL-2, C-868, SEC-114) In an eval turn every platform write is refused, decided by
        // the conversation's kind (never by anything the agent says): no approval, no effect.
        // Decided before availability, so no tool state can let a write through.
        if is_write_tool(tool) && is_eval_conversation(&ctx.agent.pool, ctx.conversation_id).await {
            if let Some(turn) = ctx.turn_id
                && let Ok(mut conn) = ctx.agent.pool.acquire().await
                && let Err(e) = aurora_db::repo::evals::add_tool_refusal(&mut conn, turn).await
            {
                tracing::warn!(error = %e, "eval refusal count failed");
            }
            if tool == "memory_save" {
                super::memory::refused_before_call(ctx, &args).await;
            }
            return ToolResult::failure(
                id,
                WireError {
                    code: code_of("eval_read_only"),
                    message: "This is an evaluation run: it cannot change anything. Answer from what you can read.".into(),
                },
            );
        }
        let mailer = ctx.state.auth_live.load().smtp;
        if (t.needs_mailer() && !mailer) || !t.available(&ctx.agent) || !t.enabled(&ctx.agent).await
        {
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

/// Platform tools whose effect is a write (C-868): messages and files sent as the agent or as its
/// owner, memory and skill writes, the creation or change of schedules, webhooks, peer tasks and
/// connection requests. Reads (`*_list`, `*_get`, `*_search`, `read_*`, `list_*`) are not here.
const WRITE_TOOLS: &[&str] = &[
    "send_message",
    "send_as_me",
    "edit_my_message",
    "delete_my_message",
    "memory_save",
    "memory_delete",
    "skill_create",
    "skill_update",
    "skill_delete",
    "schedule_create",
    "schedule_update",
    "schedule_delete",
    "webhook_update",
    "message_agent",
    "complete_task",
    "cancel_peer_task",
    "propose_custom_connection",
    "request_app_connection",
    "calendar_invite",
    "calendar_invite_update",
    "calendar_invite_cancel",
];

fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

/// Whether the conversation is an eval question's (`conversations.kind = 'eval'`). A lookup
/// failure refuses the write: fail closed.
async fn is_eval_conversation(pool: &PgPool, conversation: Uuid) -> bool {
    sqlx::query_scalar::<_, String>("SELECT kind FROM conversations WHERE id = $1")
        .bind(conversation)
        .fetch_optional(pool)
        .await
        .map_or(true, |k| k.as_deref() == Some("eval"))
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
pub async fn catalogue(state: &AppState, agent_id: Uuid) -> Result<Vec<ToolDescriptor>, ApiError> {
    let a = aurora_db::repo::agents::get(&state.pool, agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Unknown agent."))?;
    let ctx = ToolContext {
        pool: state.pool.clone(),
        agent_id,
        workspace_id: a.workspace_id,
        org_id: a.org_id,
    };
    let mailer = state.auth_live.load().smtp;
    Ok(registry().catalogue_for(&ctx, mailer).await)
}

/// Default per-call budget.
pub const CALL_TIMEOUT: Duration = limits::TOOL_CALL_TIMEOUT;

#[cfg(test)]
mod exposure_tests {
    use super::*;

    /// (P5-GUIDE-7, D-297) Every tool names a family; the core set is exactly the tools the
    /// bridge lists in `on_demand` mode and stays within `PLATFORM_TOOLS_CORE_MAX` (with the
    /// supervisor's local `send_file`, which is core too).
    #[test]
    fn every_tool_has_a_family_and_the_core_set_is_the_documented_one() {
        let r = registry();
        let no_family: Vec<&str> = r
            .order
            .iter()
            .copied()
            .filter(|n| r.tools[n].family().trim().is_empty())
            .collect();
        assert!(
            no_family.is_empty(),
            "tools without a family: {no_family:?}"
        );
        let mut core: Vec<&str> = r
            .order
            .iter()
            .copied()
            .filter(|n| r.tools[n].exposure() == Exposure::Core)
            .collect();
        core.sort_unstable();
        // `send_file` is the supervisor's local core tool (not in this registry).
        assert_eq!(core, vec!["complete_task"]);
        assert!(core.len() < limits::PLATFORM_TOOLS_CORE_MAX);
    }

    /// (P6-MEMLOCK, SEC-151) The memory family stays exactly these five tools: nothing
    /// agent-reachable may lock or unlock a memory, so no new memory tool can exist.
    #[test]
    fn the_memory_family_is_exactly_the_five_documented_tools() {
        let r = registry();
        let mut memory: Vec<&str> = r
            .order
            .iter()
            .copied()
            .filter(|n| r.tools[n].family() == "memory")
            .collect();
        memory.sort_unstable();
        assert_eq!(
            memory,
            vec![
                "memory_delete",
                "memory_get",
                "memory_list",
                "memory_save",
                "memory_search"
            ]
        );
    }

    #[test]
    fn a_registry_over_the_core_maximum_is_refused() {
        struct C(&'static str);
        #[async_trait::async_trait]
        impl PlatformTool for C {
            fn name(&self) -> &'static str {
                self.0
            }
            fn family(&self) -> &'static str {
                "test"
            }
            fn exposure(&self) -> Exposure {
                Exposure::Core
            }
            fn description(&self, _: &ToolContext) -> String {
                String::new()
            }
            fn input_schema(&self) -> Value {
                serde_json::json!({"type": "object"})
            }
            fn available(&self, _: &ToolContext) -> bool {
                true
            }
            async fn call(
                &self,
                _: &ToolCallContext,
                _: Value,
            ) -> Result<Vec<ToolContent>, ToolError> {
                Ok(vec![])
            }
        }
        const NAMES: [&str; 9] = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
        let many: Vec<Box<dyn PlatformTool>> = NAMES
            .iter()
            .take(limits::PLATFORM_TOOLS_CORE_MAX)
            .map(|n| Box::new(C(n)) as Box<dyn PlatformTool>)
            .collect();
        let err = Registry::new(many).err().expect("over the maximum");
        assert!(err.contains("PLATFORM_TOOLS_CORE_MAX"), "{err}");
        let ok: Vec<Box<dyn PlatformTool>> = NAMES
            .iter()
            .take(limits::PLATFORM_TOOLS_CORE_MAX - 1)
            .map(|n| Box::new(C(n)) as Box<dyn PlatformTool>)
            .collect();
        assert!(Registry::new(ok).is_ok());
    }
}

#[cfg(test)]
mod eval_classification_tests {
    use super::*;

    /// Platform tools with no effect on the world. A new built-in tool must be listed here or in
    /// `WRITE_TOOLS`; otherwise an eval turn (C-868) could reach a write the refusal does not know.
    const READ_TOOLS: &[&str] = &[
        "calendar_invite_list",
        "conversation_read",
        "conversation_search",
        "find_recipient",
        "list_agents",
        "list_apps",
        "list_my_messages",
        "list_peer_tasks",
        "memory_get",
        "memory_list",
        "memory_search",
        "read_conversation",
        "skill_get",
        "schedule_list",
        "skill_list",
        "webhook_deliveries",
        "webhook_list",
    ];

    #[test]
    fn every_registered_tool_is_classified_read_or_write_for_eval_turns() {
        let unclassified: Vec<&str> = registry()
            .order
            .iter()
            .copied()
            .filter(|n| !is_write_tool(n) && !READ_TOOLS.contains(n))
            .collect();
        assert!(
            unclassified.is_empty(),
            "classify as read (READ_TOOLS here) or write (WRITE_TOOLS): {unclassified:?}"
        );
        // No stale entries either: every name listed is a registered tool or a known future one.
        let both: Vec<&&str> = READ_TOOLS.iter().filter(|n| is_write_tool(n)).collect();
        assert!(both.is_empty(), "both read and write: {both:?}");
    }
}

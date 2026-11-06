//! Platform tool registry (docs/12 §Tool registry, W7 in docs/16).
//!
//! Each tool is a [`PlatformTool`]; the [`Registry`] is validated for completeness at load
//! time. Identity (agent, conversation, turn) is derived from authenticated facts by
//! `service::tool_call` — never from tool arguments. The catalogue ([`catalogue`]) feeds the
//! home sync render inputs (BE-052). M4 ships `list_agents` and `list_apps`; further families
//! are added by registering more tools in `builtin`.
mod builtin;
mod registry;
pub mod service;

pub use registry::{
    PlatformTool, Registry, ToolCallContext, ToolContext, ToolError, catalogue, registry,
};

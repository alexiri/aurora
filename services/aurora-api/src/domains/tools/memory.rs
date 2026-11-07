//! Memory tools (docs/12 Phase 0 catalogue): `memory_save/get/list/search/delete`.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::memory::{self, Author, Source, Write};
use aurora_core::{ApiError, limits};
use aurora_db::repo::agents;
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![
        Box::new(Save),
        Box::new(Get),
        Box::new(List),
        Box::new(Search),
        Box::new(Delete),
    ]
}

fn out(value: Value) -> Result<Vec<ToolContent>, ToolError> {
    Ok(vec![ToolContent::Json(JsonContent { value })])
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

fn fail(e: ApiError) -> ToolError {
    if e.status().is_client_error() {
        ToolError::Failed(e.message().to_owned())
    } else {
        tracing::error!(error = %e.message(), "memory tool error");
        ToolError::Failed("internal error".into())
    }
}

const KEY_NOTE: &str = "Keys are lowercase ASCII words joined by hyphens. \
Write memories in the workspace memory language.";

fn key_schema() -> Value {
    json!({"type": "string", "description": "Memory key (kebab-case, at most 64 characters)."})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyArgs {
    key: String,
}

struct Save;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    key: String,
    title: Option<String>,
    description: Option<String>,
    content: String,
}

#[async_trait::async_trait]
impl PlatformTool for Save {
    fn name(&self) -> &'static str {
        "memory_save"
    }
    fn description(&self, _: &ToolContext) -> String {
        format!(
            "Create or replace one of your memories (content at most {} characters). {KEY_NOTE}",
            limits::MEMORY_TOOL_CONTENT_MAX
        )
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["key", "content"],
            "properties": {"key": key_schema(),
                "title": {"type": "string", "maxLength": 120},
                "description": {"type": "string", "maxLength": 240,
                    "description": "One line shown in your memory index."},
                "content": {"type": "string", "description": "Markdown body."}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: SaveArgs = parse(args)?;
        let agent = agents::get(&ctx.agent.pool, ctx.agent.agent_id)
            .await?
            .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))?;
        let w = Write {
            key: &a.key,
            title: a.title.as_deref(),
            description: a.description.as_deref(),
            content: &a.content,
            max_content: limits::MEMORY_TOOL_CONTENT_MAX,
            create_only: false,
            skip_unchanged: false,
        };
        let author = Author {
            kind: "agent",
            user_id: None,
            label: agent.name,
        };
        let source = Source::Tool {
            conversation_id: ctx.conversation_id,
            turn_id: ctx.turn_id,
        };
        let (m, created) = memory::save(&ctx.agent.pool, agent.id, &w, &author, &source)
            .await
            .map_err(fail)?;
        out(json!({"key": m.key, "revision": m.revision, "created": created}))
    }
}

struct Get;

#[async_trait::async_trait]
impl PlatformTool for Get {
    fn name(&self) -> &'static str {
        "memory_get"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Read one of your memories by key.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["key"],
            "properties": {"key": key_schema()}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: KeyArgs = parse(args)?;
        let m = memory::service::get(&ctx.agent.pool, ctx.agent.agent_id, &a.key)
            .await
            .map_err(fail)?;
        out(
            json!({"key": m.key, "title": m.title, "description": m.description,
                   "content": m.content, "revision": m.revision}),
        )
    }
}

struct List;

#[async_trait::async_trait]
impl PlatformTool for List {
    fn name(&self) -> &'static str {
        "memory_list"
    }
    fn description(&self, _: &ToolContext) -> String {
        "List your memories (key, title, index line).".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        match &args {
            Value::Null => {}
            Value::Object(m) if m.is_empty() => {}
            _ => {
                return Err(ToolError::InvalidArguments(
                    "This tool takes no arguments.".into(),
                ));
            }
        }
        let rows = memory::service::list(&ctx.agent.pool, ctx.agent.agent_id)
            .await
            .map_err(fail)?;
        let items: Vec<Value> = rows
            .into_iter()
            .map(|m| {
                let line = memory::service::index_line(
                    &m.key,
                    m.title.as_deref(),
                    m.description.as_deref(),
                    &m.content,
                );
                json!({"key": m.key, "title": m.title, "index_line": line, "revision": m.revision})
            })
            .collect();
        out(json!({ "memories": items }))
    }
}

struct Search;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
}

#[async_trait::async_trait]
impl PlatformTool for Search {
    fn name(&self) -> &'static str {
        "memory_search"
    }
    fn description(&self, _: &ToolContext) -> String {
        format!(
            "Search your memories (best matches first, at most {}). Phrase the query in the \
             workspace memory language, the language your memories are written in.",
            limits::MEMORY_SEARCH_HITS
        )
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["query"],
            "properties": {"query": {"type": "string"}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: SearchArgs = parse(args)?;
        let hits = memory::search(&ctx.agent.pool, ctx.agent.agent_id, &a.query)
            .await
            .map_err(fail)?;
        let items: Vec<Value> = hits
            .into_iter()
            .map(
                |h| json!({"key": h.key, "title": h.title, "score": h.score, "snippet": h.snippet}),
            )
            .collect();
        out(json!({ "hits": items }))
    }
}

struct Delete;

#[async_trait::async_trait]
impl PlatformTool for Delete {
    fn name(&self) -> &'static str {
        "memory_delete"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Delete one of your memories and its history. Deleting its file does not delete it.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["key"],
            "properties": {"key": key_schema()}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: KeyArgs = parse(args)?;
        memory::delete(&ctx.agent.pool, ctx.agent.agent_id, &a.key)
            .await
            .map_err(fail)?;
        out(json!({"deleted": a.key}))
    }
}

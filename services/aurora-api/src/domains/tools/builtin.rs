//! M4 built-in tools: `list_agents`, `list_apps`.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use aurora_db::repo::{agents, connections, grants};
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde_json::{Value, json};

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    let mut v: Vec<Box<dyn PlatformTool>> = vec![Box::new(ListAgents), Box::new(ListApps)];
    v.extend(super::memory::tools());
    v.extend(super::skills::tools());
    v.extend(super::webhooks::tools());
    v.extend(super::schedules::tools());
    v
}

fn no_args(args: &Value) -> Result<(), ToolError> {
    match args {
        Value::Null => Ok(()),
        Value::Object(m) if m.is_empty() => Ok(()),
        _ => Err(ToolError::InvalidArguments(
            "This tool takes no arguments.".into(),
        )),
    }
}

fn json_out(value: Value) -> Vec<ToolContent> {
    vec![ToolContent::Json(JsonContent { value })]
}

struct ListAgents;

#[async_trait::async_trait]
impl PlatformTool for ListAgents {
    fn name(&self) -> &'static str {
        "list_agents"
    }
    fn description(&self, _: &ToolContext) -> String {
        "List the agents in your workspace (id, handle, name).".into()
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
        no_args(&args)?;
        let rows =
            agents::list_for_workspace(&ctx.agent.pool, ctx.agent.workspace_id, None, 200).await?;
        let list: Vec<Value> = rows
            .into_iter()
            .map(|a| {
                json!({"id": a.id, "handle": a.handle, "name": a.name, "kind": a.kind,
                       "is_self": a.id == ctx.agent.agent_id})
            })
            .collect();
        Ok(json_out(json!({ "agents": list })))
    }
}

struct ListApps;

#[async_trait::async_trait]
impl PlatformTool for ListApps {
    fn name(&self) -> &'static str {
        "list_apps"
    }
    fn description(&self, _: &ToolContext) -> String {
        "List the app connections granted to you (connection id, app, label, status).".into()
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
        no_args(&args)?;
        let pool = &ctx.agent.pool;
        let mut apps = Vec::new();
        for g in grants::list_connection_grants(pool, ctx.agent.agent_id).await? {
            let Some(c) = connections::get(pool, g.connection_id).await? else {
                continue;
            };
            if c.status == "orphaned" {
                continue;
            }
            apps.push(
                json!({"connection_id": c.id, "app_id": c.app_id, "label": c.label,
                             "status": c.status, "mode": g.mode}),
            );
        }
        Ok(json_out(json!({ "apps": apps })))
    }
}

//! Skill tools (docs/12 Phase 0 catalogue): `skill_list/get/create/update/delete`. Agent tier
//! only: an agent edits its own skills; organization skills are readable, never writable.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::skills::{NewSkill, Patch, SkillView, service};
use aurora_core::{ApiError, limits};
use aurora_db::repo::agents;
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![
        Box::new(List),
        Box::new(Get),
        Box::new(Create),
        Box::new(Update),
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
        tracing::error!(error = %e.message(), "skill tool error");
        ToolError::Failed("internal error".into())
    }
}

fn summary(v: &SkillView) -> Value {
    json!({"name": v.row.name,
        "scope": if v.row.agent_id.is_some() { "agent" } else { "organization" },
        "description": v.row.description, "enabled": v.row.enabled,
        "files": v.files.iter().map(|f| json!({"path": f.path, "size": f.size,
            "executable": f.executable})).collect::<Vec<_>>(),
        "total_size": v.total_size})
}

fn name_schema() -> Value {
    json!({"type": "string", "description": "Skill name (kebab-case, at most 64 characters; \
        the aurora- prefix is reserved)."})
}

async fn org_of(ctx: &ToolCallContext) -> Result<uuid::Uuid, ToolError> {
    Ok(agents::get(&ctx.agent.pool, ctx.agent.agent_id)
        .await?
        .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))?
        .org_id)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameArgs {
    name: String,
}

struct List;

#[async_trait::async_trait]
impl PlatformTool for List {
    fn name(&self) -> &'static str {
        "skill_list"
    }
    fn description(&self, _: &ToolContext) -> String {
        "List your skills and the organization skills (read-only for you).".into()
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
        let org = org_of(ctx).await?;
        let mut items = service::list_agent(&ctx.agent.pool, ctx.agent.agent_id)
            .await
            .map_err(fail)?;
        items.extend(
            service::list_org(&ctx.agent.pool, org)
                .await
                .map_err(fail)?,
        );
        out(json!({"skills": items.iter().map(summary).collect::<Vec<_>>()}))
    }
}

struct Get;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    name: String,
    /// Optional file to read instead of the skill body.
    path: Option<String>,
}

#[async_trait::async_trait]
impl PlatformTool for Get {
    fn name(&self) -> &'static str {
        "skill_get"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Read a skill (yours first, else the organization's): body and file list, or one text \
         file when `path` is given."
            .into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["name"],
            "properties": {"name": name_schema(),
                "path": {"type": "string", "description": "Skill file to read."}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: GetArgs = parse(args)?;
        let pool = &ctx.agent.pool;
        let v = match service::get_agent_named(pool, ctx.agent.agent_id, &a.name).await {
            Ok(v) => v,
            Err(e) if e.status() == http::StatusCode::NOT_FOUND => {
                let org = org_of(ctx).await?;
                service::get_org_named(pool, org, &a.name)
                    .await
                    .map_err(fail)?
            }
            Err(e) => return Err(fail(e)),
        };
        if let Some(p) = a.path {
            let content = service::read_file(pool, v.row.id, &p).await.map_err(fail)?;
            return out(json!({"name": v.row.name, "path": p, "content": content}));
        }
        let mut s = summary(&v);
        s["body"] = json!(v.row.body);
        out(s)
    }
}

struct Create;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    description: String,
    body: String,
}

#[async_trait::async_trait]
impl PlatformTool for Create {
    fn name(&self) -> &'static str {
        "skill_create"
    }
    fn description(&self, _: &ToolContext) -> String {
        format!(
            "Create one of your own skills: a reusable instruction bundle projected into your \
             skills directory (body at most {} characters).",
            limits::SKILL_BODY_MAX
        )
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false,
            "required": ["name", "description", "body"],
            "properties": {"name": name_schema(),
                "description": {"type": "string", "maxLength": limits::SKILL_DESCRIPTION_MAX,
                    "description": "One line saying when to use the skill."},
                "body": {"type": "string", "description": "Markdown instructions."}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: CreateArgs = parse(args)?;
        let org = org_of(ctx).await?;
        let n = NewSkill {
            name: &a.name,
            description: &a.description,
            body: &a.body,
            enabled: true,
        };
        let v = service::create(&ctx.agent.pool, org, Some(ctx.agent.agent_id), &n, "agent")
            .await
            .map_err(fail)?;
        out(summary(&v))
    }
}

struct Update;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    name: String,
    description: Option<String>,
    body: Option<String>,
    enabled: Option<bool>,
}

#[async_trait::async_trait]
impl PlatformTool for Update {
    fn name(&self) -> &'static str {
        "skill_update"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Update the description, body or enabled flag of one of your own skills.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["name"],
            "properties": {"name": name_schema(),
                "description": {"type": "string", "maxLength": limits::SKILL_DESCRIPTION_MAX},
                "body": {"type": "string"}, "enabled": {"type": "boolean"}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: UpdateArgs = parse(args)?;
        let cur = service::get_agent_named(&ctx.agent.pool, ctx.agent.agent_id, &a.name)
            .await
            .map_err(fail)?;
        let p = Patch {
            description: a.description.as_deref(),
            body: a.body.as_deref(),
            enabled: a.enabled,
        };
        let v = service::update(&ctx.agent.pool, cur.row.id, &p)
            .await
            .map_err(fail)?;
        out(summary(&v))
    }
}

struct Delete;

#[async_trait::async_trait]
impl PlatformTool for Delete {
    fn name(&self) -> &'static str {
        "skill_delete"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Delete one of your own skills and its files.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["name"],
            "properties": {"name": name_schema()}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: NameArgs = parse(args)?;
        let cur = service::get_agent_named(&ctx.agent.pool, ctx.agent.agent_id, &a.name)
            .await
            .map_err(fail)?;
        service::delete(&ctx.agent.pool, cur.row.id)
            .await
            .map_err(fail)?;
        out(json!({"deleted": a.name}))
    }
}

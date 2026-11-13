//! Webhook tools (docs/12 Phase 0 catalogue): `webhook_list`, `webhook_deliveries`,
//! `webhook_update {id, instructions?, enabled?}`. An agent sees and edits only its own
//! webhooks; URLs, tokens and verification secrets are never exposed.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::webhooks::dto::api_status;
use aurora_db::repo::webhooks::{self as repo, WebhookRow};
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const INSTRUCTIONS_MAX: usize = 20_000;

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![Box::new(List), Box::new(Deliveries), Box::new(Update)]
}

fn out(value: Value) -> Result<Vec<ToolContent>, ToolError> {
    Ok(vec![ToolContent::Json(JsonContent { value })])
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

fn view(w: &WebhookRow) -> Value {
    let kind = w
        .verification
        .as_ref()
        .and_then(|v| v.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("none");
    json!({"id": w.id, "name": w.name, "instructions": w.instructions,
        "enabled": w.enabled, "disabled_reason": w.disabled_reason,
        "verification": kind, "consecutive_failures": w.consecutive_failures,
        "last_delivery_at": w.last_delivery_at})
}

async fn own(ctx: &ToolCallContext, id: Uuid) -> Result<WebhookRow, ToolError> {
    repo::get(&ctx.agent.pool, id)
        .await?
        .filter(|w| w.agent_id == ctx.agent.agent_id)
        .ok_or_else(|| ToolError::Failed("Webhook not found.".into()))
}

struct List;

#[async_trait::async_trait]
impl PlatformTool for List {
    fn name(&self) -> &'static str {
        "webhook_list"
    }
    fn description(&self, _: &ToolContext) -> String {
        "List your webhooks (inbound capability URLs that wake you with a payload).".into()
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
        let items = repo::list_for_agent(&ctx.agent.pool, ctx.agent.agent_id).await?;
        out(json!({"webhooks": items.iter().map(view).collect::<Vec<_>>()}))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: Uuid,
}

fn id_schema() -> Value {
    json!({"type": "string", "format": "uuid", "description": "Webhook id."})
}

struct Deliveries;

#[async_trait::async_trait]
impl PlatformTool for Deliveries {
    fn name(&self) -> &'static str {
        "webhook_deliveries"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Recent deliveries of one of your webhooks (status, size, turn), newest first.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["id"],
            "properties": {"id": id_schema()}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: IdArgs = parse(args)?;
        let w = own(ctx, a.id).await?;
        let rows = repo::list_deliveries(&ctx.agent.pool, w.id, None, 50).await?;
        out(json!({"deliveries": rows.iter().map(|d| json!({
            "id": d.id, "received_at": d.created_at,
            "status": api_status(&d.status, d.reason.as_deref()),
            "turn_id": d.turn_id, "size": d.size_bytes})).collect::<Vec<_>>()}))
    }
}

struct Update;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    id: Uuid,
    instructions: Option<String>,
    enabled: Option<bool>,
}

#[async_trait::async_trait]
impl PlatformTool for Update {
    fn name(&self) -> &'static str {
        "webhook_update"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Change the instructions of one of your webhooks or pause/resume it. A webhook disabled \
         because its creator lost access cannot be re-enabled by you."
            .into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["id"],
            "properties": {"id": id_schema(),
                "instructions": {"type": "string", "maxLength": INSTRUCTIONS_MAX,
                    "description": "What to do with each delivery."},
                "enabled": {"type": "boolean"}}})
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
        let mut w = own(ctx, a.id).await?;
        if let Some(i) = a.instructions {
            if i.trim().is_empty() || i.chars().count() > INSTRUCTIONS_MAX {
                return Err(ToolError::Failed(
                    "Instructions must be 1 to 20000 characters.".into(),
                ));
            }
            w.instructions = i;
        }
        if a.enabled == Some(true)
            && !w.enabled
            && w.disabled_reason.as_deref() == Some("authorization")
        {
            return Err(ToolError::Failed(
                "This webhook was disabled because its creator lost access; a person with \
                 access must re-enable it."
                    .into(),
            ));
        }
        crate::domains::webhooks::service::apply_enabled(&mut w, a.enabled, None);
        let mut tx = ctx
            .agent
            .pool
            .begin()
            .await
            .map_err(aurora_db::DbError::from)?;
        let row = repo::update(&mut tx, &w).await?;
        tx.commit().await.map_err(aurora_db::DbError::from)?;
        out(view(&row))
    }
}

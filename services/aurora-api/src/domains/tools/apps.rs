//! Apps tools (docs/12 Phase 0 catalogue): `propose_custom_connection` and
//! `request_app_connection`. `list_apps` lives in `builtin`. Both append a durable `notice` to the
//! calling conversation so the person can act in the dashboard; neither accepts a secret value.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::apps::service as apps;
use crate::domains::conversations::service as conv;
use crate::domains::secrets::dto::InjectionRecipe;
use crate::domains::secrets::service::{self as secrets, Proposal};
use aurora_core::{ApiError, dash_paths, sanitize};
use aurora_db::repo::access_requests::{self, NewRequest};
use aurora_db::repo::{agents, conversations as convs, gateway, workspaces};
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Bound on the agent's own words in a notice (I-29).
const REASON_MAX: usize = 300;

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![
        Box::new(ProposeCustomConnection),
        Box::new(RequestAppConnection),
    ]
}

fn out(value: Value) -> Result<Vec<ToolContent>, ToolError> {
    Ok(vec![ToolContent::Json(JsonContent { value })])
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

/// Client errors keep their message (with the first offending field); anything else is internal.
fn fail(e: ApiError) -> ToolError {
    if e.status().is_client_error() {
        let field = e
            .details()
            .and_then(|d| d.get("fields"))
            .and_then(Value::as_array)
            .and_then(|f| f.first())
            .and_then(|f| {
                Some(format!(
                    " {}: {}",
                    f.get("path")?.as_str()?,
                    f.get("message")?.as_str()?
                ))
            })
            .unwrap_or_default();
        ToolError::Failed(format!("{}{field}", e.message()))
    } else {
        tracing::error!(error = %e.message(), "apps tool error");
        ToolError::Failed("internal error".into())
    }
}

/// The agent's own words, one line, bounded (I-29). Empty reasons are refused.
fn reason_of(r: &str) -> Result<String, ToolError> {
    let s = sanitize::one_line(r, REASON_MAX);
    if s.is_empty() {
        return Err(ToolError::InvalidArguments(
            "reason must not be empty.".into(),
        ));
    }
    Ok(s)
}

/// Appends a durable `notice` to the calling conversation on `tx`. Refused (`Err`) when the
/// conversation is gone, so the caller's transaction rolls back.
async fn append_notice(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ctx: &ToolCallContext,
    code: &str,
    text: &str,
    mut payload: Value,
) -> Result<(), ToolError> {
    if !convs::lock(&mut *tx, ctx.conversation_id).await? {
        return Err(ToolError::Failed(
            "The conversation is no longer available.".into(),
        ));
    }
    payload["code"] = json!(code);
    payload["text"] = json!(text);
    conv::append_event(
        &mut *tx,
        ctx.conversation_id,
        ctx.turn_id,
        "notice",
        &payload,
    )
    .await
    .map_err(fail)?;
    Ok(())
}

async fn agent_label(ctx: &ToolCallContext) -> Result<String, ToolError> {
    let a = agents::get(&ctx.agent.pool, ctx.agent.agent_id)
        .await?
        .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))?;
    Ok(sanitize::label(&a.name))
}

struct ProposeCustomConnection;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeArgs {
    name: String,
    host: String,
    path: Option<String>,
    recipe: InjectionRecipe,
    reason: String,
}

#[async_trait::async_trait]
impl PlatformTool for ProposeCustomConnection {
    fn name(&self) -> &'static str {
        "propose_custom_connection"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Ask the person to provide a secret (an API key or token) for a host you need. Never \
         send the value: the person pastes it in the dashboard. You may have at most 5 pending \
         proposals."
            .into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false,
            "required": ["name", "host", "recipe", "reason"],
            "properties": {
                "name": {"type": "string", "maxLength": 100,
                    "description": "Label for the secret, e.g. the service name."},
                "host": {"type": "string",
                    "description": "Host pattern the secret is sent to, e.g. api.example.com."},
                "path": {"type": "string", "description": "Optional path pattern."},
                "recipe": {"type": "object", "required": ["kind"],
                    "description": "How the value is injected. kind: bearer | header (name, format) | query (name, format) | basic (username) | path_template (prefix) | path_regex (pattern, replacement)."},
                "reason": {"type": "string", "maxLength": REASON_MAX,
                    "description": "Why you need it, in one line."}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: ProposeArgs = parse(args)?;
        let reason = reason_of(&a.reason)?;
        let agent = agent_label(ctx).await?;
        let mut tx = ctx.agent.pool.begin().await?;
        let id = secrets::propose(
            &ctx.state,
            &mut tx,
            Proposal {
                org: ctx.agent.org_id,
                ws: ctx.agent.workspace_id,
                agent: ctx.agent.agent_id,
                name: &a.name,
                host: &a.host,
                path: a.path.as_deref(),
                injection: &a.recipe,
            },
        )
        .await
        .map_err(fail)?;
        let text = format!(
            "{agent} asks for a secret for {host}. Paste the value in the dashboard under \
             Credentials, never in this chat. Reason (the agent's words): {reason}",
            host = sanitize::label(&a.host),
        );
        let (request, _) = access_requests::open(
            &mut tx,
            &NewRequest {
                org_id: ctx.agent.org_id,
                workspace_id: ctx.agent.workspace_id,
                agent_id: ctx.agent.agent_id,
                conversation_id: ctx.conversation_id,
                turn_id: ctx.turn_id,
                app_id: None,
                secret_id: Some(id),
                reason: &reason,
            },
        )
        .await?;
        append_notice(
            &mut tx,
            ctx,
            "secret_proposed",
            &text,
            json!({"request_id": request.id, "secret_id": id, "name": sanitize::label(&a.name),
                   "host": sanitize::label(&a.host), "reason": reason}),
        )
        .await?;
        tx.commit().await?;
        out(json!({"status": "proposed", "secret_id": id}))
    }
}

struct RequestAppConnection;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestArgs {
    app_id: String,
    reason: String,
}

#[async_trait::async_trait]
impl PlatformTool for RequestAppConnection {
    fn name(&self) -> &'static str {
        "request_app_connection"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Ask the person to connect an app for this workspace. Give the app id from the app \
         catalogue (see list_apps). The person sees a notice with a link to the connect window."
            .into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false,
            "required": ["app_id", "reason"],
            "properties": {
                "app_id": {"type": "string", "description": "App id, e.g. from list_apps."},
                "reason": {"type": "string", "maxLength": REASON_MAX,
                    "description": "Why you need the app, in one line."}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: RequestArgs = parse(args)?;
        let reason = reason_of(&a.reason)?;
        let snap = apps::snapshot(&ctx.state).await.map_err(fail)?;
        let Some((app, _)) = snap
            .as_ref()
            .and_then(|s| apps::find_app(s, &a.app_id).ok())
        else {
            return Err(ToolError::Failed(format!(
                "Unknown app `{}`: it is not in the app catalogue. Check the app id with list_apps.",
                sanitize::label(&a.app_id)
            )));
        };
        let (restricted, allowed) =
            gateway::app_availability(&ctx.agent.pool, ctx.agent.org_id, ctx.agent.workspace_id)
                .await?;
        if restricted && !allowed.iter().any(|id| id == &app.app_id) {
            return Err(ToolError::Failed(format!(
                "App `{}` is not available to your organization. Check the app id with list_apps.",
                sanitize::label(&app.app_id)
            )));
        }
        let ws = workspaces::get(&ctx.agent.pool, ctx.agent.workspace_id)
            .await?
            .ok_or_else(|| ToolError::Failed("Unknown workspace.".into()))?;
        let org_slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
            .bind(ctx.agent.org_id)
            .fetch_one(&ctx.agent.pool)
            .await?;
        let site = ctx.state.config.external_url.trim_end_matches('/');
        let agent_id = ctx.agent.agent_id.to_string();
        let agent = agent_label(ctx).await?;
        let app_name = sanitize::label(&app.about.name);
        // (FIX-154, C-897) An app with nothing to sign in to needs no connect window: the notice
        // links to this agent's Access tab (the agent page accepts an id, FIX-153). Any other app
        // links to the journey for this agent and app (C-959), from the shared table of paths.
        let free = aurora_registry::credential_free::entry_is_credential_free(app);
        let (link, text) = if free {
            let link = format!(
                "{site}{}",
                dash_paths::ws_agent_access(&org_slug, &ws.slug, &agent_id)
            );
            let text = format!(
                "{agent} asks you to give this agent access to {app_name} ({app_id}). Give access \
                 on its Access tab: {link} Reason (the agent's words): {reason}",
                app_id = app.app_id,
            );
            (link, text)
        } else {
            let link = format!(
                "{site}{}",
                dash_paths::ws_agent_connect(&org_slug, &ws.slug, &agent_id, &app.app_id)
            );
            let text = format!(
                "{agent} asks you to connect {app_name} ({app_id}). Connect it here: {link} \
                 Reason (the agent's words): {reason}",
                app_id = app.app_id,
            );
            (link, text)
        };
        let mut tx = ctx.agent.pool.begin().await?;
        // (C-984, SEC-132) A record, not access. One open request per agent and app: asking again
        // refreshes it, and a card is added only when it is new or in another conversation.
        let (request, card) = access_requests::open(
            &mut tx,
            &NewRequest {
                org_id: ctx.agent.org_id,
                workspace_id: ctx.agent.workspace_id,
                agent_id: ctx.agent.agent_id,
                conversation_id: ctx.conversation_id,
                turn_id: ctx.turn_id,
                app_id: Some(&app.app_id),
                secret_id: None,
                reason: &reason,
            },
        )
        .await?;
        if card {
            append_notice(
                &mut tx,
                ctx,
                "app_connection_requested",
                &text,
                json!({"request_id": request.id, "app_id": app.app_id, "app_name": app_name,
                       "reason": reason, "connect_url": link}),
            )
            .await?;
            // (C-984) A direct chat-platform conversation gets one line with the link.
            crate::domains::channels::mirror::post_access_request(
                &mut tx,
                ctx.conversation_id,
                request.id,
                &agent,
                &app_name,
                &link,
            )
            .await;
        }
        tx.commit().await?;
        out(json!({"status": "requested"}))
    }
}

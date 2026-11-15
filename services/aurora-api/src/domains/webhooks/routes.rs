use super::dto::*;
use super::{receive, service};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::automations::{
    dto::ReassignCreatorRequest,
    service::{self as auto, Acting, To},
};
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::{agents, webhooks::WebhookRow};
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_webhooks, create_webhook))
        .routes(routes!(get_webhook, update_webhook, delete_webhook))
        .routes(routes!(rotate_webhook))
        .routes(routes!(reassign_creator))
        .routes(routes!(list_deliveries))
        .routes(routes!(receive::receive))
}

/// Webhooks need `can_use` on the agent's workspace.
async fn visible_agent(c: &Caller, s: &AppState, id: Uuid) -> Result<agents::AgentRow, ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let ws = c.workspace(s, row.workspace_id).await?;
    if !ws.can_use() {
        return Err(ApiError::forbidden("Workspace access required."));
    }
    Ok(row)
}

async fn authorized(
    c: &Caller,
    s: &AppState,
    id: Uuid,
) -> Result<(WebhookRow, agents::AgentRow), ApiError> {
    let row = service::get(s, id).await?;
    let agent = visible_agent(c, s, row.agent_id).await?;
    Ok((row, agent))
}

fn record(c: &Caller, s: &AppState, a: &agents::AgentRow, r: &WebhookRow, action: &str) {
    audit::record(
        s,
        AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("webhook", r.id)
            .metadata(json!({"name": r.name, "agent_id": a.id})),
    );
}

/// List an agent's webhooks.
#[utoipa::path(get, path = "/agents/{agent}/webhooks", operation_id = "listWebhooks",
    tag = "webhooks", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Page<Webhook>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_webhooks(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Page<Webhook>>, ApiError> {
    visible_agent(&c, &s, agent).await?;
    let items = service::list(&s, agent).await?;
    Ok(Json(Page {
        items: items.into_iter().map(Webhook::from).collect(),
        next_cursor: None,
    }))
}

/// Create a webhook; the caller becomes its creator (fire-time authority). The URL is shown once.
#[utoipa::path(post, path = "/agents/{agent}/webhooks", operation_id = "createWebhook",
    tag = "webhooks", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreateWebhookRequest,
    responses((status = 201, body = WebhookWithUrl), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_webhook(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<(StatusCode, Json<WebhookWithUrl>), ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let anchor = match req.anchor.as_deref() {
        Some(v) => auto::parse_anchor(v)?,
        None => "creator",
    };
    if anchor == "workspace" {
        require_own(&c.workspace(&s, a.workspace_id).await?)?;
    }
    let (row, url) = service::create(
        &s,
        &a,
        service::Create {
            name: req.name,
            instructions: req.instructions,
            verification: req.verification,
            anchor,
        },
        Some(c.user_id),
        None,
    )
    .await?;
    record(&c, &s, &a, &row, "webhook.create");
    let row = service::get(&s, row.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(WebhookWithUrl {
            webhook: row.into(),
            url,
        }),
    ))
}

/// Get one webhook (never includes the URL token or the verification secret).
#[utoipa::path(get, path = "/webhooks/{id}", operation_id = "getWebhook", tag = "webhooks",
    params(("id" = Uuid, Path, description = "Webhook id")),
    responses((status = 200, body = Webhook), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_webhook(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Webhook>, ApiError> {
    let (row, _) = authorized(&c, &s, id).await?;
    Ok(Json(row.into()))
}

/// Update a webhook; `enabled: true` re-enables and clears the failure count.
#[utoipa::path(patch, path = "/webhooks/{id}", operation_id = "updateWebhook",
    tag = "webhooks", params(("id" = Uuid, Path, description = "Webhook id")),
    request_body = UpdateWebhookRequest,
    responses((status = 200, body = Webhook), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn update_webhook(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateWebhookRequest>,
) -> Result<Json<Webhook>, ApiError> {
    let (cur, a) = authorized(&c, &s, id).await?;
    // RFE-021: switching the anchor needs `can_own`; validate before anything is written.
    let new_anchor = match req.anchor.as_deref() {
        Some(v) => {
            let v = auto::parse_anchor(v)?;
            require_own(&c.workspace(&s, a.workspace_id).await?)?;
            if v == cur.anchor {
                None
            } else {
                let to = auto::anchor_target(v, cur.creator_user_id)?;
                let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
                auto::ensure_target(&mut conn, to, a.workspace_id).await?;
                Some(to)
            }
        }
        None => None,
    };
    let mut row = service::patch(
        &s,
        &a,
        cur,
        service::Patch {
            name: req.name,
            instructions: req.instructions,
            verification: req.verification,
            enabled: req.enabled,
        },
        Some(c.user_id),
    )
    .await?;
    record(&c, &s, &a, &row, "webhook.update");
    if let Some(to) = new_anchor {
        let actor = AuditActor::from_caller(&c);
        let acting = Acting {
            actor: &actor,
            org: a.org_id,
            workspace: a.workspace_id,
        };
        let mut tx = s.pool.begin().await.map_err(DbError::from)?;
        auto::reassign_webhook(&mut tx, &acting, &row, to, false).await?;
        tx.commit().await.map_err(DbError::from)?;
        row = service::get(&s, id).await?;
    }
    Ok(Json(row.into()))
}

/// Delete a webhook and its delivery log.
#[utoipa::path(delete, path = "/webhooks/{id}", operation_id = "deleteWebhook",
    tag = "webhooks", params(("id" = Uuid, Path, description = "Webhook id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_webhook(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (cur, a) = authorized(&c, &s, id).await?;
    service::delete(&s, &a, &cur).await?;
    record(&c, &s, &a, &cur, "webhook.delete");
    Ok(StatusCode::NO_CONTENT)
}

/// Replace the capability token; the old URL stops working at once. The new URL is shown once.
#[utoipa::path(post, path = "/webhooks/{id}/rotate", operation_id = "rotateWebhook",
    tag = "webhooks", params(("id" = Uuid, Path, description = "Webhook id")),
    responses((status = 200, body = WebhookWithUrl), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn rotate_webhook(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<WebhookWithUrl>, ApiError> {
    let (_, a) = authorized(&c, &s, id).await?;
    let url = service::rotate(&s, id).await?;
    let row = service::get(&s, id).await?;
    record(&c, &s, &a, &row, "webhook.rotate");
    Ok(Json(WebhookWithUrl {
        webhook: row.into(),
        url,
    }))
}

#[derive(Deserialize, IntoParams)]
struct DeliveriesQuery {
    /// Page size (default 50, max 200).
    limit: Option<u32>,
    /// Opaque cursor from a previous response's `next_cursor`.
    cursor: Option<String>,
}

/// Recent deliveries, newest first.
#[utoipa::path(get, path = "/webhooks/{id}/deliveries", operation_id = "listWebhookDeliveries",
    tag = "webhooks", params(("id" = Uuid, Path, description = "Webhook id"), DeliveriesQuery),
    responses((status = 200, body = Page<WebhookDelivery>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_deliveries(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<DeliveriesQuery>,
) -> Result<Json<Page<WebhookDelivery>>, ApiError> {
    authorized(&c, &s, id).await?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = aurora_db::repo::webhooks::list_deliveries(
        &s.pool,
        id,
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(WebhookDelivery::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Reassign the webhook's creator (fire-time authority) to an active member who can use the
/// workspace (`can_own`); `re_enable` re-enables an item disabled for `authorization`. Audited
/// `automation.reassign`.
#[utoipa::path(put, path = "/webhooks/{id}/creator", operation_id = "reassignWebhookCreator",
    tag = "webhooks", params(("id" = Uuid, Path, description = "Webhook id")),
    request_body = ReassignCreatorRequest,
    responses((status = 200, body = Webhook), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn reassign_creator(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ReassignCreatorRequest>,
) -> Result<Json<Webhook>, ApiError> {
    let (row, a) = authorized(&c, &s, id).await?;
    require_own(&c.workspace(&s, a.workspace_id).await?)?;
    let actor = AuditActor::from_caller(&c);
    let acting = Acting {
        actor: &actor,
        org: a.org_id,
        workspace: a.workspace_id,
    };
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    auto::reassign_webhook(
        &mut tx,
        &acting,
        &row,
        To::User(req.user_id),
        req.re_enable.unwrap_or(false),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Json(service::get(&s, id).await?.into()))
}

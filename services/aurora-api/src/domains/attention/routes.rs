use super::service::{self, AttentionCounts};
use crate::app::AppState;
use crate::domains::admin::dto::{AttentionList, AttentionQuery};
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use axum::extract::State;
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(workspace_attention))
        .routes(routes!(org_attention))
        .routes(routes!(attention_counts))
}

/// (P5-REFINE-8, C-1080) What needs the caller in a workspace: approvals they may decide, agents' access
/// requests (workspace owners), accounts that stopped working, and their own waiting requests (`own`,
/// never counted). Any active member; 404 otherwise.
#[utoipa::path(get, path = "/workspaces/{ws}/attention", operation_id = "workspaceAttention", tag = "attention",
    params(("ws" = Uuid, Path, description = "Workspace id"), AttentionQuery),
    responses((status = 200, body = AttentionList), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn workspace_attention(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<AttentionQuery>,
) -> Result<Json<AttentionList>, ApiError> {
    let done = service::parse_state(q.state.as_deref())?;
    Ok(Json(service::workspace_feed(&s, &c, ws, done).await?))
}

/// (C-1080) The requests the organization's owners and admins can resolve. Owners and admins; 404 for a
/// non-member, 403 for a member.
#[utoipa::path(get, path = "/orgs/{org}/attention", operation_id = "orgAttention", tag = "attention",
    params(("org" = Uuid, Path, description = "Organization id"), AttentionQuery),
    responses((status = 200, body = AttentionList), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn org_attention(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<AttentionQuery>,
) -> Result<Json<AttentionList>, ApiError> {
    let done = service::parse_state(q.state.as_deref())?;
    Ok(Json(service::org_feed(&s, &c, org, done).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CountsQuery {
    /// The organization whose rail asks.
    pub org: Option<Uuid>,
    /// The workspace whose rail asks.
    pub workspace: Option<Uuid>,
}

/// (C-1082) The rails' numbers: per level the caller can act on and asked for, the count and the topics,
/// equal to the matching feed's `count`; `null` otherwise (never 403 or 404). Any signed-in person.
#[utoipa::path(get, path = "/attention/counts", operation_id = "attentionCounts", tag = "attention",
    params(CountsQuery),
    responses((status = 200, body = AttentionCounts), (status = 400, body = ErrorEnvelope),
        (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn attention_counts(
    c: Caller,
    State(s): State<AppState>,
    Query(q): Query<CountsQuery>,
) -> Result<Json<AttentionCounts>, ApiError> {
    Ok(Json(service::counts(&s, &c, q.org, q.workspace).await?))
}

use super::dto::*;
use super::service::{self, Source};
use super::validate::Scope;
use super::{simulate, warnings};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams};
use aurora_core::ApiError;
use axum::extract::State;
use serde_json::{Value, json};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_ws_policy))
        .routes(routes!(put_ws_draft))
        .routes(routes!(publish_ws))
        .routes(routes!(rollback_ws))
        .routes(routes!(ws_generations))
        .routes(routes!(ws_warnings))
        .routes(routes!(get_org_policy))
        .routes(routes!(put_org_draft))
        .routes(routes!(publish_org))
        .routes(routes!(rollback_org))
        .routes(routes!(org_generations))
        .routes(routes!(org_warnings))
        .routes(routes!(simulate_policy))
}

/// Workspace policy reads and writes alike need `can_own` (I-27).
async fn ws_scope(caller: &Caller, state: &AppState, ws: Uuid) -> Result<Scope, ApiError> {
    let a = caller.workspace(state, ws).await?;
    require_own(&a)?;
    Ok(Scope {
        org: a.org_id,
        ws: Some(ws),
    })
}

/// Org policy reads and writes alike need owner/admin (I-27).
async fn org_scope(caller: &Caller, state: &AppState, org: Uuid) -> Result<Scope, ApiError> {
    caller.org(state, org).await?.require_admin()?;
    Ok(Scope { org, ws: None })
}

fn record(state: &AppState, caller: &Caller, action: &str, s: Scope, meta: Value) {
    let (kind, id) = match s.ws {
        Some(w) => ("workspace", w),
        None => ("organization", s.org),
    };
    let mut ev = AuditEvent::new(AuditActor::from_caller(caller), action, Outcome::Success)
        .org(s.org)
        .target(kind, id)
        .metadata(meta);
    if let Some(w) = s.ws {
        ev = ev.workspace(w);
    }
    audit::record(state, ev);
}

async fn do_put(
    state: &AppState,
    caller: &Caller,
    s: Scope,
    req: DraftRequest,
) -> Result<Json<PolicyGeneration>, ApiError> {
    let n = req.rules.len();
    let g = service::replace_draft(state, s, req).await?;
    record(
        state,
        caller,
        "policy.draft_update",
        s,
        json!({"rules": n, "draft_revision": g.draft_revision}),
    );
    Ok(Json(g))
}

async fn do_publish(
    state: &AppState,
    caller: &Caller,
    s: Scope,
    source: Source,
) -> Result<Json<PolicyDocument>, ApiError> {
    let (action, meta) = match &source {
        Source::Draft { revision } => ("policy.publish", json!({"revision": revision})),
        Source::Generation { number } => ("policy.rollback", json!({"from_generation": number})),
    };
    let number = service::publish(state, s, caller.user_id, source).await?;
    let mut meta = meta;
    meta["generation"] = json!(number);
    record(state, caller, action, s, meta);
    Ok(Json(service::document(state, s).await?))
}

/// Read the workspace policy (`can_own`; I-27).
#[utoipa::path(
    get, path = "/workspaces/{ws}/policy", operation_id = "getWorkspacePolicy", tag = "policy",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, description = "Published and draft", body = PolicyDocument),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_ws_policy(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<PolicyDocument>, ApiError> {
    let s = ws_scope(&caller, &state, ws).await?;
    Ok(Json(service::document(&state, s).await?))
}

/// Replace the workspace draft (`can_own`; C-08).
#[utoipa::path(
    put, path = "/workspaces/{ws}/policy/draft", operation_id = "putWorkspacePolicyDraft", tag = "policy",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = DraftRequest,
    responses((status = 200, description = "Draft", body = PolicyGeneration),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "stale_revision", body = ErrorEnvelope),
              (status = 422, description = "policy_invalid (details.fields)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn put_ws_draft(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<DraftRequest>,
) -> Result<Json<PolicyGeneration>, ApiError> {
    let s = ws_scope(&caller, &state, ws).await?;
    do_put(&state, &caller, s, req).await
}

/// Publish the workspace draft (`can_own`).
#[utoipa::path(
    post, path = "/workspaces/{ws}/policy/publish", operation_id = "publishWorkspacePolicy", tag = "policy",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = PublishRequest,
    responses((status = 200, description = "Document", body = PolicyDocument),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "stale_revision", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn publish_ws(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<PublishRequest>,
) -> Result<Json<PolicyDocument>, ApiError> {
    let s = ws_scope(&caller, &state, ws).await?;
    do_publish(
        &state,
        &caller,
        s,
        Source::Draft {
            revision: req.revision,
        },
    )
    .await
}

/// Publish a copy of an older workspace generation (`can_own`).
#[utoipa::path(
    post, path = "/workspaces/{ws}/policy/rollback", operation_id = "rollbackWorkspacePolicy", tag = "policy",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = RollbackRequest,
    responses((status = 200, description = "Document", body = PolicyDocument),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn rollback_ws(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<RollbackRequest>,
) -> Result<Json<PolicyDocument>, ApiError> {
    let s = ws_scope(&caller, &state, ws).await?;
    do_publish(
        &state,
        &caller,
        s,
        Source::Generation {
            number: req.generation,
        },
    )
    .await
}

/// Workspace policy history, newest first (`can_own`; I-27).
#[utoipa::path(
    get, path = "/workspaces/{ws}/policy/generations", operation_id = "listWorkspacePolicyGenerations", tag = "policy",
    params(("ws" = Uuid, Path, description = "Workspace id"), PageParams),
    responses((status = 200, description = "Generations", body = Page<PolicyGenerationSummary>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn ws_generations(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<PolicyGenerationSummary>>, ApiError> {
    let s = ws_scope(&caller, &state, ws).await?;
    Ok(Json(service::generations(&state, s, q.resolve()?).await?))
}

/// Warnings for the workspace draft (`can_own`; I-27).
#[utoipa::path(
    get, path = "/workspaces/{ws}/policy/warnings", operation_id = "getWorkspacePolicyWarnings", tag = "policy",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, description = "Warnings", body = PolicyWarnings),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn ws_warnings(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<PolicyWarnings>, ApiError> {
    let s = ws_scope(&caller, &state, ws).await?;
    Ok(Json(warnings::compute(&state, s).await?))
}

/// Read the organization policy (owner/admin; I-27).
#[utoipa::path(
    get, path = "/orgs/{org}/policy", operation_id = "getOrgPolicy", tag = "policy",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, description = "Published and draft", body = PolicyDocument),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_org_policy(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<PolicyDocument>, ApiError> {
    let s = org_scope(&caller, &state, org).await?;
    Ok(Json(service::document(&state, s).await?))
}

/// Replace the organization draft (owner/admin; C-08).
#[utoipa::path(
    put, path = "/orgs/{org}/policy/draft", operation_id = "putOrgPolicyDraft", tag = "policy",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = DraftRequest,
    responses((status = 200, description = "Draft", body = PolicyGeneration),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "stale_revision", body = ErrorEnvelope),
              (status = 422, description = "policy_invalid (details.fields)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn put_org_draft(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<DraftRequest>,
) -> Result<Json<PolicyGeneration>, ApiError> {
    let s = org_scope(&caller, &state, org).await?;
    do_put(&state, &caller, s, req).await
}

/// Publish the organization draft (owner/admin).
#[utoipa::path(
    post, path = "/orgs/{org}/policy/publish", operation_id = "publishOrgPolicy", tag = "policy",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = PublishRequest,
    responses((status = 200, description = "Document", body = PolicyDocument),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "stale_revision", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn publish_org(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<PublishRequest>,
) -> Result<Json<PolicyDocument>, ApiError> {
    let s = org_scope(&caller, &state, org).await?;
    do_publish(
        &state,
        &caller,
        s,
        Source::Draft {
            revision: req.revision,
        },
    )
    .await
}

/// Publish a copy of an older organization generation (owner/admin).
#[utoipa::path(
    post, path = "/orgs/{org}/policy/rollback", operation_id = "rollbackOrgPolicy", tag = "policy",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = RollbackRequest,
    responses((status = 200, description = "Document", body = PolicyDocument),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn rollback_org(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<RollbackRequest>,
) -> Result<Json<PolicyDocument>, ApiError> {
    let s = org_scope(&caller, &state, org).await?;
    do_publish(
        &state,
        &caller,
        s,
        Source::Generation {
            number: req.generation,
        },
    )
    .await
}

/// Organization policy history, newest first (owner/admin; I-27).
#[utoipa::path(
    get, path = "/orgs/{org}/policy/generations", operation_id = "listOrgPolicyGenerations", tag = "policy",
    params(("org" = Uuid, Path, description = "Organization id"), PageParams),
    responses((status = 200, description = "Generations", body = Page<PolicyGenerationSummary>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn org_generations(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<PolicyGenerationSummary>>, ApiError> {
    let s = org_scope(&caller, &state, org).await?;
    Ok(Json(service::generations(&state, s, q.resolve()?).await?))
}

/// Warnings for the organization draft (owner/admin; I-27).
#[utoipa::path(
    get, path = "/orgs/{org}/policy/warnings", operation_id = "getOrgPolicyWarnings", tag = "policy",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, description = "Warnings", body = PolicyWarnings),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn org_warnings(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<PolicyWarnings>, ApiError> {
    let s = org_scope(&caller, &state, org).await?;
    Ok(Json(warnings::compute(&state, s).await?))
}

/// Simulate a request for an agent against the published (or draft) policy (`can_own`: the
/// trace discloses the policy rules, I-27).
#[utoipa::path(
    post, path = "/workspaces/{ws}/policy/simulate", operation_id = "simulatePolicy", tag = "policy",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = SimulateRequest,
    responses((status = 200, description = "Verdict and trace", body = Simulation),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Workspace or agent not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn simulate_policy(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<SimulateRequest>,
) -> Result<Json<Simulation>, ApiError> {
    let s = ws_scope(&caller, &state, ws).await?;
    let agent = aurora_db::repo::agents::get(&state.pool, req.agent_id)
        .await?
        .filter(|a| a.workspace_id == ws)
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    Ok(Json(simulate::run(&state, s.org, ws, agent.id, req).await?))
}

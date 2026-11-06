use super::dto::*;
use super::service::{self, CreateScope};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use aurora_db::repo::secrets::SecretRow;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_ws_secrets, create_ws_secret))
        .routes(routes!(list_org_secrets, create_org_secret))
        .routes(routes!(get_secret, patch_secret, delete_secret))
        .routes(routes!(set_secret_value))
        .routes(routes!(accept_secret))
        .routes(routes!(reject_secret))
        .routes(routes!(list_secret_stores))
        .routes(routes!(list_llm_providers))
        .routes(routes!(secret_models))
        .routes(routes!(verify_secret))
}

fn record(
    state: &AppState,
    caller: &Caller,
    action: &str,
    row: &SecretRow,
    extra: serde_json::Value,
) {
    let mut ev = AuditEvent::new(AuditActor::from_caller(caller), action, Outcome::Success)
        .org(row.org_id)
        .target("secret", row.id)
        .metadata(json!({
            "name": row.name, "kind": row.kind, "llm_endpoint_id": row.llm_endpoint_id,
            "scope": if row.workspace_id.is_some() { "workspace" } else { "organization" },
            "extra": extra,
        }));
    if let Some(ws) = row.workspace_id {
        ev = ev.workspace(ws);
    }
    audit::record(state, ev);
}

/// List workspace-scoped secrets (`can_own`; I-27).
#[utoipa::path(
    get, path = "/workspaces/{ws}/secrets", operation_id = "listWorkspaceSecrets", tag = "secrets",
    params(("ws" = Uuid, Path, description = "Workspace id"), ListQuery),
    responses((status = 200, description = "Secrets", body = Page<Secret>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_ws_secrets(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<Secret>>, ApiError> {
    let a = caller.workspace(&state, ws).await?;
    crate::http::auth::require_own(&a)?;
    Ok(Json(service::list(&state, a.org_id, Some(ws), &q).await?))
}

/// List organization-scoped secrets (owner/admin; I-27).
#[utoipa::path(
    get, path = "/orgs/{org}/secrets", operation_id = "listOrgSecrets", tag = "secrets",
    params(("org" = Uuid, Path, description = "Organization id"), ListQuery),
    responses((status = 200, description = "Secrets", body = Page<Secret>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_org_secrets(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<Secret>>, ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    Ok(Json(service::list(&state, org, None, &q).await?))
}

const CREATE_ERRORS: &str = "llm_key_not_permitted";

/// Create a workspace secret (`can_own`).
#[utoipa::path(
    post, path = "/workspaces/{ws}/secrets", operation_id = "createWorkspaceSecret", tag = "secrets",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = CreateSecretRequest,
    responses((status = 201, description = "Created", body = Secret),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = CREATE_ERRORS, body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "name_taken", body = ErrorEnvelope),
              (status = 422, description = "llm_host_requires_llm_key | llm_key_invalid | llm_key_wrong_organization | llm_key_unverifiable | llm_endpoint_unreachable | reference_not_permitted", body = ErrorEnvelope),
              (status = 503, description = "secret_store_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn create_ws_secret(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<Secret>), ApiError> {
    let a = caller.workspace(&state, ws).await?;
    crate::http::auth::require_own(&a)?;
    let row = service::create(
        &state,
        &caller,
        CreateScope {
            org: a.org_id,
            ws: Some(ws),
        },
        req,
    )
    .await?;
    record(&state, &caller, "secret.create", &row, json!(null));
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Create an organization secret (owner/admin).
#[utoipa::path(
    post, path = "/orgs/{org}/secrets", operation_id = "createOrgSecret", tag = "secrets",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateSecretRequest,
    responses((status = 201, description = "Created", body = Secret),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = CREATE_ERRORS, body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "name_taken", body = ErrorEnvelope),
              (status = 422, description = "see workspace create", body = ErrorEnvelope),
              (status = 503, description = "secret_store_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn create_org_secret(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<Secret>), ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    let row = service::create(&state, &caller, CreateScope { org, ws: None }, req).await?;
    record(&state, &caller, "secret.create", &row, json!(null));
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Get one secret (metadata only; `can_own` / owner-admin, I-27).
#[utoipa::path(
    get, path = "/secrets/{id}", operation_id = "getSecret", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    responses((status = 200, description = "Secret", body = Secret),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_secret(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Secret>, ApiError> {
    Ok(Json(
        service::load_authorized(&caller, &state, id).await?.into(),
    ))
}

/// Update name / host / path / injection (generic secrets; LLM secrets: name only).
#[utoipa::path(
    patch, path = "/secrets/{id}", operation_id = "updateSecret", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    request_body = PatchSecretRequest,
    responses((status = 200, description = "Secret", body = Secret),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "name_taken", body = ErrorEnvelope),
              (status = 422, description = "llm_host_requires_llm_key | llm_secret_read_only", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn patch_secret(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchSecretRequest>,
) -> Result<Json<Secret>, ApiError> {
    let row = service::load_authorized(&caller, &state, id).await?;
    let out = service::patch(&state, &row, req).await?;
    record(&state, &caller, "secret.update", &out, json!(null));
    Ok(Json(out.into()))
}

/// Delete a secret (grants and their derived rules go with it).
#[utoipa::path(
    delete, path = "/secrets/{id}", operation_id = "deleteSecret", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    responses((status = 204, description = "Deleted"),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn delete_secret(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let row = service::load_authorized(&caller, &state, id).await?;
    service::delete(&state, &row).await?;
    record(&state, &caller, "secret.delete", &row, json!(null));
    Ok(StatusCode::NO_CONTENT)
}

/// Replace the value (managed `value` or `reference`); bumps `value_version`, re-verifies LLM keys.
#[utoipa::path(
    put, path = "/secrets/{id}/value", operation_id = "setSecretValue", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    request_body = SetValueRequest,
    responses((status = 200, description = "Secret", body = Secret),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden / llm_key_not_permitted", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 422, description = "as create", body = ErrorEnvelope),
              (status = 503, description = "secret_store_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn set_secret_value(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetValueRequest>,
) -> Result<Json<Secret>, ApiError> {
    let row = service::load_authorized(&caller, &state, id).await?;
    let out = service::set_value(&state, &row, req).await?;
    record(
        &state,
        &caller,
        "secret.value_update",
        &out,
        json!({"value_version": out.value_version, "value_source": out.value_source}),
    );
    Ok(Json(out.into()))
}

/// Accept an agent proposal: sets the value, activates it and grants it to the proposer.
#[utoipa::path(
    post, path = "/secrets/{id}/accept", operation_id = "acceptSecretProposal", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    request_body = AcceptRequest,
    responses((status = 200, description = "Secret", body = Secret),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "not_proposed", body = ErrorEnvelope)),
    security(("session" = []))
)]
async fn accept_secret(
    caller: SessionCaller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AcceptRequest>,
) -> Result<Json<Secret>, ApiError> {
    let row = service::load_authorized(&caller, &state, id).await?;
    let out = service::accept(&state, &caller, &row, req.value).await?;
    record(
        &state,
        &caller,
        "secret.accept",
        &out,
        json!({"granted_agent_id": out.proposed_by_agent_id}),
    );
    Ok(Json(out.into()))
}

/// Reject an agent proposal.
#[utoipa::path(
    post, path = "/secrets/{id}/reject", operation_id = "rejectSecretProposal", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    responses((status = 204, description = "Rejected"),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "not_proposed", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn reject_secret(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let row = service::load_authorized(&caller, &state, id).await?;
    service::reject(&state, &row).await?;
    record(&state, &caller, "secret.reject", &row, json!(null));
    Ok(StatusCode::NO_CONTENT)
}

/// Configured secret stores and their capabilities.
#[utoipa::path(
    get, path = "/secret-stores", operation_id = "listSecretStores", tag = "secrets",
    responses((status = 200, description = "Stores", body = SecretStores)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_secret_stores(
    _caller: Caller,
    State(state): State<AppState>,
) -> Result<Json<SecretStores>, ApiError> {
    let mut ids: Vec<&str> = vec!["inline"];
    ids.extend(state.config.secrets.stores.iter().map(|s| s.id.as_str()));
    ids.dedup();
    let reg = state.vault.registry();
    let mut seen = Vec::new();
    let items = ids
        .into_iter()
        .filter(|id| {
            let fresh = !seen.contains(id);
            seen.push(id);
            fresh
        })
        .filter_map(|id| reg.get(id))
        .map(|s| SecretStoreInfo {
            id: s.id().to_owned(),
            kind: s.kind().to_owned(),
            capabilities: StoreCaps {
                managed: s.capabilities().managed,
                references: s.capabilities().references,
            },
        })
        .collect();
    Ok(Json(SecretStores { items }))
}

/// LLM providers and endpoints; with `org_id`, adds the caller's effective governance.
#[utoipa::path(
    get, path = "/llm/providers", operation_id = "listLlmProviders", tag = "secrets",
    params(ProvidersQuery),
    responses((status = 200, description = "Providers", body = LlmProviders),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_llm_providers(
    caller: Caller,
    State(state): State<AppState>,
    Query(q): Query<ProvidersQuery>,
) -> Result<Json<LlmProviders>, ApiError> {
    let eff = match q.org_id {
        Some(org) => {
            caller.org(&state, org).await?;
            Some(super::gov::effective_for(&state, org).await?)
        }
        None => None,
    };
    let mut providers: Vec<LlmProviderInfo> = Vec::new();
    let mut endpoints = Vec::new();
    for ep in state.llm.endpoints() {
        let Some(p) = state.llm.provider_for(ep) else {
            continue;
        };
        let dialects: Vec<String> = p
            .dialects(ep)
            .into_iter()
            .map(|d| d.as_str().to_owned())
            .collect();
        match providers.iter_mut().find(|x| x.id == p.id()) {
            Some(x) => {
                for d in &dialects {
                    if !x.dialects.contains(d) {
                        x.dialects.push(d.clone());
                    }
                }
            }
            None => providers.push(LlmProviderInfo {
                id: p.id().to_owned(),
                display_name: p.display_name().to_owned(),
                auth_modes: vec!["api_key".to_owned()],
                dialects: dialects.clone(),
            }),
        }
        let e = eff.as_ref().and_then(|e| e.endpoints.get(&ep.id));
        let rule = e.map_or(ep.key_origin.as_ref(), |e| e.origin_rule.as_ref());
        endpoints.push(LlmEndpointInfo {
            id: ep.id.clone(),
            provider: ep.provider.clone(),
            display_name: ep.display_name.clone(),
            base_url: ep.base_url.clone(),
            dialects,
            origin_method: rule.map(|r| r.method.as_str().to_owned()),
            permitted: eff.as_ref().map(|_| e.is_some_and(|e| e.permitted)),
            key_scopes: eff
                .as_ref()
                .map(|_| e.map(|e| e.key_scopes.clone()).unwrap_or_default()),
        });
    }
    Ok(Json(LlmProviders {
        providers,
        endpoints,
    }))
}

/// Models available to an LLM secret (limited to the effective allow-list; `can_own` /
/// owner-admin, I-27). Agent model pickers use `GET /agents/{agent}/models` instead.
#[utoipa::path(
    get, path = "/secrets/{id}/models", operation_id = "listSecretModels", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    responses((status = 200, description = "Models", body = Models),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 502, description = "llm_endpoint_unreachable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn secret_models(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Models>, ApiError> {
    let row = service::load_authorized(&caller, &state, id).await?;
    Ok(Json(service::models(&state, &row).await?))
}

/// Re-verify the key and its origin (`can_own`).
#[utoipa::path(
    post, path = "/secrets/{id}/verify", operation_id = "verifySecret", tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    responses((status = 200, description = "Origin", body = VerifyResult),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 422, description = "llm_key_invalid | llm_endpoint_unreachable | not_llm_secret", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn verify_secret(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<VerifyResult>, ApiError> {
    let row = service::load_authorized(&caller, &state, id).await?;
    let origin = service::verify(&state, &row).await?;
    record(
        &state,
        &caller,
        "secret.verify",
        &row,
        json!({"status": origin.status}),
    );
    Ok(Json(VerifyResult { origin }))
}

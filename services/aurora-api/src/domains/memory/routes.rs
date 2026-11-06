use super::dto::*;
use super::service::{self, Author, Source, Write};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::{ApiError, limits};
use aurora_db::repo::agents;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_memories, create_memory))
        .routes(routes!(search_memories))
        .routes(routes!(get_memory, put_memory, delete_memory))
        .routes(routes!(list_revisions))
        .routes(routes!(restore_memory))
        .routes(routes!(redact_revision))
}

async fn visible_agent(c: &Caller, s: &AppState, id: Uuid) -> Result<agents::AgentRow, ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    c.workspace(s, row.workspace_id).await?;
    Ok(row)
}

fn author(c: &Caller) -> Author {
    Author {
        kind: "user",
        user_id: Some(c.user_id),
        label: c.email.clone(),
    }
}

fn source(c: &Caller) -> Source {
    if c.is_session() {
        Source::Dashboard
    } else {
        Source::Api
    }
}

fn record(c: &Caller, s: &AppState, a: &agents::AgentRow, action: &str, key: &str) {
    audit::record(
        s,
        AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("agent", a.id)
            .metadata(json!({"key": key})),
    );
}

/// List an agent's memories (workspace access).
#[utoipa::path(get, path = "/agents/{agent}/memories", operation_id = "listMemories",
    tag = "memory", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Page<Memory>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_memories(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Page<Memory>>, ApiError> {
    visible_agent(&c, &s, agent).await?;
    let items = service::list(&s.pool, agent)
        .await?
        .into_iter()
        .map(Memory::from)
        .collect();
    Ok(Json(Page {
        items,
        next_cursor: None,
    }))
}

/// Create a memory; 409 `memory_exists`, 422 `memory_limit_reached`.
#[utoipa::path(post, path = "/agents/{agent}/memories", operation_id = "createMemory",
    tag = "memory", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreateMemoryRequest,
    responses((status = 201, body = Memory), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_memory(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<CreateMemoryRequest>,
) -> Result<(StatusCode, Json<Memory>), ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let w = Write {
        key: &req.key,
        title: req.title.as_deref(),
        description: req.description.as_deref(),
        content: &req.content,
        max_content: limits::MEMORY_CONTENT_MAX,
        create_only: true,
        skip_unchanged: false,
    };
    let (row, _) = service::save(&s.pool, agent, &w, &author(&c), &source(&c)).await?;
    record(&c, &s, &a, "memory.save", &row.key);
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Search memories in the workspace memory language.
#[utoipa::path(get, path = "/agents/{agent}/memories/search", operation_id = "searchMemories",
    tag = "memory", params(("agent" = Uuid, Path, description = "Agent id"), SearchParams),
    responses((status = 200, body = MemorySearchResponse), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn search_memories(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(p): Query<SearchParams>,
) -> Result<Json<MemorySearchResponse>, ApiError> {
    visible_agent(&c, &s, agent).await?;
    let items = service::search(&s.pool, agent, &p.q)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(MemorySearchResponse { items }))
}

/// Get one memory.
#[utoipa::path(get, path = "/agents/{agent}/memories/{key}", operation_id = "getMemory",
    tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    responses((status = 200, body = Memory), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_memory(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
) -> Result<Json<Memory>, ApiError> {
    visible_agent(&c, &s, agent).await?;
    Ok(Json(service::get(&s.pool, agent, &key).await?.into()))
}

/// Replace a memory's title, description and content; creates it when the key is new.
#[utoipa::path(put, path = "/agents/{agent}/memories/{key}", operation_id = "putMemory",
    tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    request_body = PutMemoryRequest,
    responses((status = 200, body = Memory), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_memory(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
    Json(req): Json<PutMemoryRequest>,
) -> Result<Json<Memory>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let w = Write {
        key: &key,
        title: req.title.as_deref(),
        description: req.description.as_deref(),
        content: &req.content,
        max_content: limits::MEMORY_CONTENT_MAX,
        create_only: false,
        skip_unchanged: false,
    };
    let (row, _) = service::save(&s.pool, agent, &w, &author(&c), &source(&c)).await?;
    record(&c, &s, &a, "memory.save", &row.key);
    Ok(Json(row.into()))
}

/// Delete a memory and its revisions.
#[utoipa::path(delete, path = "/agents/{agent}/memories/{key}", operation_id = "deleteMemory",
    tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_memory(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    service::delete(&s.pool, agent, &key).await?;
    record(&c, &s, &a, "memory.delete", &key);
    Ok(StatusCode::NO_CONTENT)
}

/// Revisions, newest first (at most `MEMORY_REVISIONS_KEPT`).
#[utoipa::path(get, path = "/agents/{agent}/memories/{key}/revisions",
    operation_id = "listMemoryRevisions", tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    responses((status = 200, body = Page<MemoryRevision>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_revisions(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
) -> Result<Json<Page<MemoryRevision>>, ApiError> {
    visible_agent(&c, &s, agent).await?;
    Ok(Json(Page {
        items: service::revisions(&s.pool, agent, &key).await?,
        next_cursor: None,
    }))
}

/// Restore an older revision as a new revision.
#[utoipa::path(post, path = "/agents/{agent}/memories/{key}/restore",
    operation_id = "restoreMemory", tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    request_body = RestoreMemoryRequest,
    responses((status = 200, body = Memory), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn restore_memory(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
    Json(req): Json<RestoreMemoryRequest>,
) -> Result<Json<Memory>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let row = service::restore(&s.pool, agent, &key, req.revision, &author(&c)).await?;
    record(&c, &s, &a, "memory.restore", &key);
    Ok(Json(row.into()))
}

/// Redact a non-latest revision's content in place (audited).
#[utoipa::path(post, path = "/agents/{agent}/memories/{key}/revisions/{rev}/redact",
    operation_id = "redactMemoryRevision", tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key"),
        ("rev" = i32, Path, description = "Revision number")),
    responses((status = 204, description = "Redacted"), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn redact_revision(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key, rev)): Path<(Uuid, String, i32)>,
) -> Result<StatusCode, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    service::redact(&s.pool, agent, &key, rev, c.user_id).await?;
    audit::record(
        &s,
        AuditEvent::new(
            AuditActor::from_caller(&c),
            "memory.redact",
            Outcome::Success,
        )
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("agent", a.id)
        .metadata(json!({"key": key, "revision": rev})),
    );
    Ok(StatusCode::NO_CONTENT)
}

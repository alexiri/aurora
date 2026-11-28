use super::actor::{self, StoredActor};
use super::dto::*;
use super::service::{self, Author, Source, Write};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent};
use crate::domains::emergency_access::service::{ReadKind, ReadOf, record_read};
use crate::http::auth::{Caller, Principal};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::{ApiError, limits};
use aurora_db::authz::ReadAuth;
use aurora_db::repo::agents;
use aurora_db::repo::memory::{MemoryRow, RevisionRow};
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
        .routes(routes!(set_memory_locked))
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

/// [`visible_agent`] for a *read* (C-1165): the emergency reader of the workspace lists and
/// reads the agent's memory. Never for a write. The caller records what it returns
/// ([`record_read`] with this authority).
async fn visible_agent_read(
    c: &Caller,
    s: &AppState,
    id: Uuid,
) -> Result<(agents::AgentRow, ReadAuth), ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let read = c.workspace_read(s, row.workspace_id).await?;
    Ok((row, read))
}

/// The emergency record of a route that needs **own** access (revisions, search: a grant alone
/// does not open them; [`visible_agent`] has already refused that). A caller who holds own access
/// *and* an active grant reads under the grant too (D-478(b)), so what they read is recorded like
/// any other memory read (D-502): before the body, 503 `audit_unavailable` if it cannot be.
async fn record_own_read(
    c: &Caller,
    s: &AppState,
    agent: &agents::AgentRow,
    of: ReadOf,
) -> Result<(), ApiError> {
    let read = c.workspace_read(s, agent.workspace_id).await?;
    record_read(s, &AuditActor::from_caller(c), &read, ReadKind::Memory, of).await
}

fn author(c: &Caller) -> Author {
    Author {
        kind: "user",
        user_id: Some(c.user_id),
        api_key_id: match c.principal {
            Principal::ApiKey { id, .. } => Some(id),
            _ => None,
        },
        // A person's label is their display name, looked up when the revision is written.
        label: String::new(),
    }
}

/// The rows as the caller may see them: names looked up, conversations named only to their
/// readers (SEC-157).
async fn present(
    c: &Caller,
    s: &AppState,
    agent: &agents::AgentRow,
    rows: Vec<MemoryRow>,
) -> Result<Vec<Memory>, ApiError> {
    let actors: Vec<(StoredActor, StoredActor)> = rows
        .iter()
        .map(|m| {
            (
                StoredActor::parse(&m.added_by),
                StoredActor::parse(&m.changed_by),
            )
        })
        .collect();
    let used: Vec<Uuid> = rows
        .iter()
        .filter_map(|m| m.last_used_conversation_id)
        .collect();
    let names = actor::names(s, c, agent, actors.iter().flat_map(|(a, b)| [a, b]), &used).await?;
    Ok(rows
        .into_iter()
        .zip(&actors)
        .map(|(m, (added, changed))| Memory::present(m, added, changed, &names))
        .collect())
}

async fn present_one(
    c: &Caller,
    s: &AppState,
    agent: &agents::AgentRow,
    row: MemoryRow,
) -> Result<Json<Memory>, ApiError> {
    present(c, s, agent, vec![row])
        .await?
        .pop()
        .map(Json)
        .ok_or_else(|| ApiError::internal("memory vanished"))
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
        AuditEvent::by(c, action)
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("agent", a.id)
            .metadata(json!({"key": key})),
    );
}

/// List an agent's memories (workspace access).
#[utoipa::path(get, path = "/agents/{agent}/memories", operation_id = "listMemories",
    tag = "memory", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Page<Memory>), (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_memories(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Page<Memory>>, ApiError> {
    let (a, read) = visible_agent_read(&c, &s, agent).await?;
    let rows = service::list(&s.pool, agent).await?;
    let items = present(&c, &s, &a, rows).await?;
    record_read(
        &s,
        &AuditActor::from_caller(&c),
        &read,
        ReadKind::Memory,
        ReadOf::List(agent),
    )
    .await?;
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
        // (P6-MEMLOCK, C-1148) Only the create route may arrive locked.
        locked: req.locked.unwrap_or(false),
    };
    let (row, _) = service::save(&s.pool, agent, &w, &author(&c), &source(&c)).await?;
    record(&c, &s, &a, "memory.save", &row.key);
    Ok((StatusCode::CREATED, present_one(&c, &s, &a, row).await?))
}

/// Search memories in the workspace memory language.
#[utoipa::path(get, path = "/agents/{agent}/memories/search", operation_id = "searchMemories",
    tag = "memory", params(("agent" = Uuid, Path, description = "Agent id"), SearchParams),
    responses((status = 200, body = MemorySearchResponse), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn search_memories(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(p): Query<SearchParams>,
) -> Result<Json<MemorySearchResponse>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let items = service::search(&s.pool, agent, &p.q)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    record_own_read(&c, &s, &a, ReadOf::List(agent)).await?;
    Ok(Json(MemorySearchResponse { items }))
}

/// Get one memory.
#[utoipa::path(get, path = "/agents/{agent}/memories/{key}", operation_id = "getMemory",
    tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    responses((status = 200, body = Memory), (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_memory(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
) -> Result<Json<Memory>, ApiError> {
    let (a, read) = visible_agent_read(&c, &s, agent).await?;
    let row = service::get(&s.pool, agent, &key).await?;
    let id = row.id;
    let body = present_one(&c, &s, &a, row).await?;
    record_read(
        &s,
        &AuditActor::from_caller(&c),
        &read,
        ReadKind::Memory,
        ReadOf::Object(id),
    )
    .await?;
    Ok(body)
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
        // (P6-MEMLOCK) A save never changes the lock; a person's PUT on a locked memory works.
        locked: false,
    };
    let (row, _) = service::save(&s.pool, agent, &w, &author(&c), &source(&c)).await?;
    record(&c, &s, &a, "memory.save", &row.key);
    present_one(&c, &s, &a, row).await
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
    // (P6-MEMLOCK) The author decides the lock rule: an agent-authored delete of a locked memory
    // is refused; a person's delete always works.
    service::delete(&s.pool, agent, &key, &author(&c)).await?;
    record(&c, &s, &a, "memory.delete", &key);
    Ok(StatusCode::NO_CONTENT)
}

/// (P6-MEMLOCK, C-1148) Sets or clears a memory's lock. Idempotent: the same value changes
/// nothing (no audit row, no generation bump); a change writes `memory.lock` / `memory.unlock`
/// and bumps the agent's home-sync generation, with no revision. No agent-reachable tool or
/// route can call this (SEC-151).
#[utoipa::path(put, path = "/agents/{agent}/memories/{key}/locked",
    operation_id = "setMemoryLocked", tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    request_body = SetMemoryLockedRequest,
    responses((status = 200, body = Memory), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn set_memory_locked(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
    Json(req): Json<SetMemoryLockedRequest>,
) -> Result<Json<Memory>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let (row, changed) = service::set_locked(&s.pool, agent, &key, req.locked, &author(&c)).await?;
    if changed {
        let action = if req.locked {
            "memory.lock"
        } else {
            "memory.unlock"
        };
        record(&c, &s, &a, action, &key);
    }
    present_one(&c, &s, &a, row).await
}

/// Revisions, newest first (at most `MEMORY_REVISIONS_KEPT`).
#[utoipa::path(get, path = "/agents/{agent}/memories/{key}/revisions",
    operation_id = "listMemoryRevisions", tag = "memory",
    params(("agent" = Uuid, Path, description = "Agent id"), ("key" = String, Path, description = "Memory key")),
    responses((status = 200, body = Page<MemoryRevision>), (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_revisions(
    c: Caller,
    State(s): State<AppState>,
    Path((agent, key)): Path<(Uuid, String)>,
) -> Result<Json<Page<MemoryRevision>>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let (memory, rows) = service::revisions(&s.pool, agent, &key).await?;
    let actors: Vec<StoredActor> = rows.iter().map(StoredActor::of_revision).collect();
    let names = actor::names(&s, &c, &a, &actors, &[]).await?;
    let items = rows
        .into_iter()
        .zip(&actors)
        .map(|(r, who): (RevisionRow, _)| MemoryRevision::present(r, who, &names))
        .collect();
    record_own_read(&c, &s, &a, ReadOf::Object(memory)).await?;
    Ok(Json(Page {
        items,
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
    present_one(&c, &s, &a, row).await
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
    let ev = AuditEvent::by(&c, "memory.redact")
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("agent", a.id)
        .metadata(json!({"key": key, "revision": rev}));
    service::redact(&s.pool, agent, &key, rev, c.user_id, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}

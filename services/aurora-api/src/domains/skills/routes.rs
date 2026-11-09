use super::dto::*;
use super::service::{self, NewSkill, Owner, Patch};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use aurora_db::repo::{agents, skills::SkillRow};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_agent_skills, create_agent_skill))
        .routes(routes!(effective_agent_skills))
        .routes(routes!(list_org_skills, create_org_skill))
        .routes(routes!(list_workspace_skills, create_workspace_skill))
        .routes(routes!(get_skill, update_skill, delete_skill))
        .routes(routes!(put_skill_file, delete_skill_file))
}

/// Agent skills need `can_use` on the agent's workspace.
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

/// Workspace skills: `can_use` reads (404 invisible), `can_own` writes (403 for a member).
async fn workspace_auth(c: &Caller, s: &AppState, ws: Uuid, write: bool) -> Result<(), ApiError> {
    let a = c.workspace(s, ws).await?;
    if write {
        require_own(&a)
    } else if a.can_use() {
        Ok(())
    } else {
        Err(ApiError::forbidden("Workspace access required."))
    }
}

/// Agent skills follow the agent's workspace; workspace skills: `can_use` reads, `can_own`
/// writes; org skills: any active member reads, owner/admin writes.
async fn authorize(c: &Caller, s: &AppState, sk: &SkillRow, write: bool) -> Result<(), ApiError> {
    if let Some(ws) = sk.workspace_id {
        return workspace_auth(c, s, ws, write).await;
    }
    match sk.agent_id {
        Some(a) => visible_agent(c, s, a).await.map(|_| ()),
        None => {
            let org = c.org(s, sk.org_id).await?;
            if write {
                org.require_admin()?;
            }
            Ok(())
        }
    }
}

fn record(c: &Caller, s: &AppState, sk: &SkillRow, action: &str, ws: Option<Uuid>) {
    let mut ev = AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
        .org(sk.org_id)
        .target("skill", sk.id)
        .metadata(json!({"name": sk.name, "scope": service::tier_of(sk)}));
    if let Some(w) = ws {
        ev = ev.workspace(w);
    }
    audit::record(s, ev);
}

async fn workspace_of(s: &AppState, sk: &SkillRow) -> Result<Option<Uuid>, ApiError> {
    if sk.workspace_id.is_some() {
        return Ok(sk.workspace_id);
    }
    match sk.agent_id {
        Some(a) => Ok(agents::get(&s.pool, a).await?.map(|r| r.workspace_id)),
        None => Ok(None),
    }
}

async fn page(s: &AppState, items: Vec<service::SkillView>) -> Result<Json<Page<Skill>>, ApiError> {
    let mut out = Vec::with_capacity(items.len());
    for v in items {
        out.push(service::dto(&s.pool, v).await?);
    }
    Ok(Json(Page {
        items: out,
        next_cursor: None,
    }))
}

/// List an agent's own skills.
#[utoipa::path(get, path = "/agents/{agent}/skills", operation_id = "listAgentSkills",
    tag = "skills", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Page<Skill>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_agent_skills(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Page<Skill>>, ApiError> {
    visible_agent(&c, &s, agent).await?;
    page(&s, service::list_agent(&s.pool, agent).await?).await
}

/// Create an agent-tier skill; 409 `skill_exists`.
#[utoipa::path(post, path = "/agents/{agent}/skills", operation_id = "createAgentSkill",
    tag = "skills", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreateSkillRequest,
    responses((status = 201, body = Skill), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_agent_skill(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Skill>), ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let n = NewSkill {
        name: &req.name,
        description: &req.description,
        body: &req.body,
        enabled: req.enabled.unwrap_or(true),
    };
    let owner = Owner {
        agent: Some(agent),
        workspace: None,
    };
    let v = service::create(&s.pool, a.org_id, owner, &n, "user").await?;
    record(&c, &s, &v.row, "skill.create", Some(a.workspace_id));
    Ok((StatusCode::CREATED, Json(v.into())))
}

/// List an organization's skills.
#[utoipa::path(get, path = "/orgs/{org}/skills", operation_id = "listOrgSkills",
    tag = "skills", params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = Page<Skill>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_org_skills(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<Page<Skill>>, ApiError> {
    c.org(&s, org).await?;
    page(&s, service::list_org(&s.pool, org).await?).await
}

/// Create an organization-tier skill (owner/admin); 409 `skill_exists`.
#[utoipa::path(post, path = "/orgs/{org}/skills", operation_id = "createOrgSkill",
    tag = "skills", params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateSkillRequest,
    responses((status = 201, body = Skill), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_org_skill(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Skill>), ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let n = NewSkill {
        name: &req.name,
        description: &req.description,
        body: &req.body,
        enabled: req.enabled.unwrap_or(true),
    };
    let v = service::create(&s.pool, org, Owner::default(), &n, "user").await?;
    record(&c, &s, &v.row, "skill.create", None);
    Ok((StatusCode::CREATED, Json(v.into())))
}

/// List a workspace's skills (workspace tier).
#[utoipa::path(get, path = "/workspaces/{ws}/skills", operation_id = "listWorkspaceSkills",
    tag = "skills", params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = Page<Skill>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_workspace_skills(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<Page<Skill>>, ApiError> {
    workspace_auth(&c, &s, ws, false).await?;
    page(&s, service::list_workspace(&s.pool, ws).await?).await
}

/// Create a workspace-tier skill (`can_own`, org owners/admins included); 409 `skill_exists`.
#[utoipa::path(post, path = "/workspaces/{ws}/skills", operation_id = "createWorkspaceSkill",
    tag = "skills", params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = CreateSkillRequest,
    responses((status = 201, body = Skill), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_workspace_skill(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Skill>), ApiError> {
    workspace_auth(&c, &s, ws, true).await?;
    let org = aurora_db::repo::workspaces::get(&s.pool, ws)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?
        .org_id;
    let n = NewSkill {
        name: &req.name,
        description: &req.description,
        body: &req.body,
        enabled: req.enabled.unwrap_or(true),
    };
    let owner = Owner {
        agent: None,
        workspace: Some(ws),
    };
    let v = service::create(&s.pool, org, owner, &n, "user").await?;
    record(&c, &s, &v.row, "skill.create", Some(ws));
    Ok((StatusCode::CREATED, Json(service::dto(&s.pool, v).await?)))
}

/// The effective skill set an agent receives, plus every skill it does not (shadowed or
/// disabled).
#[utoipa::path(get, path = "/agents/{agent}/skills/effective",
    operation_id = "listEffectiveAgentSkills", tag = "skills",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = EffectiveSkills), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn effective_agent_skills(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<EffectiveSkills>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let e = service::effective(&s.pool, &a).await?;
    let mut items = Vec::with_capacity(e.items.len());
    for i in e.items {
        let skill = service::dto(&s.pool, i.view).await?;
        items.push(EffectiveSkill {
            tier: skill.tier,
            enabled: true,
            shadows: i
                .shadows
                .into_iter()
                .map(|(id, tier)| TierRef { id, tier })
                .collect(),
            skill,
        });
    }
    let mut shadowed = Vec::with_capacity(e.shadowed.len());
    for i in e.shadowed {
        let skill = service::dto(&s.pool, i.view).await?;
        shadowed.push(ShadowedSkill {
            tier: skill.tier,
            enabled: i.enabled,
            shadowed_by: i.shadowed_by.map(|(id, tier)| TierRef { id, tier }),
            skill,
        });
    }
    Ok(Json(EffectiveSkills { items, shadowed }))
}

/// Get one skill with its file metadata.
#[utoipa::path(get, path = "/skills/{id}", operation_id = "getSkill", tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id")),
    responses((status = 200, body = Skill), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_skill(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Skill>, ApiError> {
    let v = service::get(&s.pool, id).await?;
    authorize(&c, &s, &v.row, false).await?;
    Ok(Json(service::dto(&s.pool, v).await?))
}

/// Update description, body or enabled. The name is immutable.
#[utoipa::path(patch, path = "/skills/{id}", operation_id = "updateSkill", tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id")),
    request_body = UpdateSkillRequest,
    responses((status = 200, body = Skill), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn update_skill(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateSkillRequest>,
) -> Result<Json<Skill>, ApiError> {
    let cur = service::row(&s.pool, id).await?;
    authorize(&c, &s, &cur, true).await?;
    let p = Patch {
        description: req.description.as_deref(),
        body: req.body.as_deref(),
        enabled: req.enabled,
    };
    let v = service::update(&s.pool, id, &p).await?;
    record(
        &c,
        &s,
        &v.row,
        "skill.update",
        workspace_of(&s, &v.row).await?,
    );
    Ok(Json(service::dto(&s.pool, v).await?))
}

/// Delete a skill and its files.
#[utoipa::path(delete, path = "/skills/{id}", operation_id = "deleteSkill", tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id")),
    responses((status = 204, description = "Deleted"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_skill(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let cur = service::row(&s.pool, id).await?;
    authorize(&c, &s, &cur, true).await?;
    service::delete(&s.pool, id).await?;
    record(&c, &s, &cur, "skill.delete", workspace_of(&s, &cur).await?);
    Ok(StatusCode::NO_CONTENT)
}

/// Create or replace a skill file; the request body is the raw file content.
#[utoipa::path(put, path = "/skills/{id}/files/{*path}", operation_id = "putSkillFile",
    tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id"),
        ("path" = String, Path, description = "Relative file path"), FileParams),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses((status = 200, body = Skill), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_skill_file(
    c: Caller,
    State(s): State<AppState>,
    Path((id, path)): Path<(Uuid, String)>,
    Query(q): Query<FileParams>,
    body: Bytes,
) -> Result<Json<Skill>, ApiError> {
    let cur = service::row(&s.pool, id).await?;
    authorize(&c, &s, &cur, true).await?;
    let v = service::put_file(&s.pool, id, &path, &body, q.executable).await?;
    record(
        &c,
        &s,
        &v.row,
        "skill.file.put",
        workspace_of(&s, &v.row).await?,
    );
    Ok(Json(service::dto(&s.pool, v).await?))
}

/// Delete one skill file.
#[utoipa::path(delete, path = "/skills/{id}/files/{*path}", operation_id = "deleteSkillFile",
    tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id"),
        ("path" = String, Path, description = "Relative file path")),
    responses((status = 204, description = "Deleted"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_skill_file(
    c: Caller,
    State(s): State<AppState>,
    Path((id, path)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    let cur = service::row(&s.pool, id).await?;
    authorize(&c, &s, &cur, true).await?;
    service::delete_file(&s.pool, id, &path).await?;
    record(
        &c,
        &s,
        &cur,
        "skill.file.delete",
        workspace_of(&s, &cur).await?,
    );
    Ok(StatusCode::NO_CONTENT)
}

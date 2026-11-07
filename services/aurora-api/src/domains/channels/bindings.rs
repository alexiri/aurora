//! Space bindings of shared bots (docs/13 §5.3): dashboard listing, edit and removal by
//! workspace owners (`can_own`).

use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service::load_agent;
use crate::http::auth::{Caller, require_own};
use aurora_core::ApiError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::space_bindings::{self as repo, BindingRow};
use serde_json::json;
use uuid::Uuid;

fn view(r: &BindingRow) -> SpaceBinding {
    SpaceBinding {
        id: r.id,
        integration_id: r.integration_id,
        space_ref: r.space_ref.clone(),
        space_label: r.space_label.clone(),
        agent_id: r.agent_id,
        listen: Listen::parse(&r.listen),
        followups_need_mention: r.followups_need_mention,
        active: r.active,
        bound_by: r.bound_by_user_id.map(|id| UserRef {
            id,
            display_name: r.bound_by_name.clone().unwrap_or_default(),
        }),
        created_at: r.created_at,
    }
}

async fn owned_agent(s: &AppState, c: &Caller, agent: Uuid) -> Result<AgentRow, ApiError> {
    let a = load_agent(s, c, agent).await?;
    require_own(&c.workspace(s, a.workspace_id).await?)?;
    Ok(a)
}

pub async fn list(s: &AppState, c: &Caller, agent: Uuid) -> Result<Vec<SpaceBinding>, ApiError> {
    owned_agent(s, c, agent).await?;
    Ok(repo::list_for_agent(&s.pool, agent)
        .await?
        .iter()
        .map(view)
        .collect())
}

async fn load(s: &AppState, c: &Caller, id: Uuid) -> Result<(BindingRow, AgentRow), ApiError> {
    let not_found = || ApiError::not_found("Binding not found.");
    let row = repo::get(&s.pool, id).await?.ok_or_else(not_found)?;
    if !row.active {
        return Err(not_found());
    }
    // Bindings the caller cannot see are indistinguishable from missing ones.
    let agent = load_agent(s, c, row.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            not_found()
        } else {
            e
        }
    })?;
    require_own(&c.workspace(s, agent.workspace_id).await?)?;
    Ok((row, agent))
}

pub async fn patch(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    req: PatchBindingRequest,
) -> Result<SpaceBinding, ApiError> {
    let (_, agent) = load(s, c, id).await?;
    repo::update(
        &s.pool,
        id,
        req.listen.map(Listen::as_str),
        req.followups_need_mention,
    )
    .await?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "space_binding.update",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("space_binding", id)
        .metadata(json!({
            "listen": req.listen.map(Listen::as_str),
            "followups_need_mention": req.followups_need_mention,
        })),
    );
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Binding not found."))?;
    Ok(view(&row))
}

pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let (row, agent) = load(s, c, id).await?;
    repo::delete(&s.pool, id).await?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "space_binding.delete",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("space_binding", id)
        .metadata(json!({"integration_id": row.integration_id, "space_ref": row.space_ref})),
    );
    Ok(())
}

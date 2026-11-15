use super::dto::{AgentLink, LinkPeer, LinkPolicy, LinkSelf, SetPolicyBody};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service::load_agent;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agent_links::{self as repo, AgentLinkRow};
use aurora_db::repo::agents;
use serde_json::json;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The link as seen from `me` (one of its two agents).
pub async fn view<'e, E: PgExecutor<'e>>(
    exec: E,
    link: &AgentLinkRow,
    me: Uuid,
) -> Result<AgentLink, ApiError> {
    let other = link
        .other(me)
        .ok_or_else(|| ApiError::not_found("Link not found."))?;
    let peer = agents::get(exec, other)
        .await?
        .ok_or_else(|| ApiError::not_found("Link not found."))?;
    Ok(AgentLink {
        id: link.id,
        revision: link.revision,
        self_side: LinkSelf {
            agent_id: me,
            policy: LinkPolicy::parse(link.policy_of(me).unwrap_or("ask")),
        },
        peer: LinkPeer {
            agent_id: peer.id,
            handle: peer.handle,
            name: peer.name,
            workspace_id: peer.workspace_id,
            policy: LinkPolicy::parse(link.policy_of(other).unwrap_or("ask")),
        },
        created_at: link.created_at,
        updated_at: link.updated_at,
    })
}

pub async fn list(s: &AppState, c: &Caller, agent: Uuid) -> Result<Vec<AgentLink>, ApiError> {
    load_agent(s, c, agent).await?;
    let rows = repo::list_for_agent(&s.pool, agent).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(view(&s.pool, r, agent).await?);
    }
    Ok(out)
}

/// `PUT /agent-links/{id}/policy` (C-29): sets the named agent's side; the caller needs
/// `can_use` on that agent's workspace and cannot touch the other side.
pub async fn set_policy(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    b: SetPolicyBody,
) -> Result<AgentLink, ApiError> {
    let not_found = || ApiError::not_found("Link not found.");
    let link = repo::get(&s.pool, id).await?.ok_or_else(not_found)?;
    if link.policy_of(b.agent_id).is_none() {
        return Err(not_found());
    }
    let agent = load_agent(s, c, b.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            not_found()
        } else {
            e
        }
    })?;
    // (FIX-110, docs/07 §Agent links, RFE-012) Consenting to cross-workspace messaging (`allow`)
    // is an approver's decision; `ask` and `blocked` only restrict and stay with `can_use`.
    if b.policy == LinkPolicy::Allow {
        let auth = c.workspace(s, agent.workspace_id).await?;
        crate::http::auth::require_approve(&auth)?;
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    if cur.revision != b.revision {
        return Err(ApiError::conflict(
            "stale_revision",
            "The link changed since you loaded it.",
        ));
    }
    let row = repo::set_policy(&mut tx, id, b.agent_id, b.policy.as_str(), Some(b.revision))
        .await?
        .ok_or_else(|| {
            ApiError::conflict("stale_revision", "The link changed since you loaded it.")
        })?;
    let out = view(&mut *tx, &row, b.agent_id).await?;
    audit::commit_critical(
        tx,
        &AuditEvent::new(
            AuditActor::from_caller(c),
            "agent_link.policy",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("agent_link", id)
        .metadata(
            json!({"agent_id": b.agent_id, "from": cur.policy_of(b.agent_id),
            "to": b.policy.as_str()}),
        ),
    )
    .await?;
    Ok(out)
}

use super::dto::{
    AgentLink, CreateLinkBody, LinkCreator, LinkCreatorKind, LinkDirection, LinkPeer, LinkPolicy,
    LinkSelf, LinkableAgent, SetDirectionBody, SetPolicyBody,
};
use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::domains::action_approvals::service as approvals;
use crate::domains::conversations::service::load_agent;
use crate::domains::freeze::service::ensure_agent_active;
use crate::domains::peer_tasks::service::{self as peers, Close};
use crate::http::auth::{Caller, Principal, approver_required};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::authz::WorkspaceAuth;
use aurora_db::repo::agent_links::{self as repo, AgentLinkRow, NewPersonLink};
use aurora_db::repo::agents::{self, AgentRow};
use aurora_db::repo::peer_tasks;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// The reason a removed link gives the tasks and approvals it ends (C-1085).
const UNLINKED: &str = "the link between these agents was removed";

fn link_404() -> ApiError {
    ApiError::not_found("Link not found.")
}

fn agent_404() -> ApiError {
    ApiError::not_found("Agent not found.")
}

fn stale() -> ApiError {
    ApiError::conflict("stale_revision", "The link changed since you loaded it.")
}

/// The link as seen from `me` (one of its two agents).
pub async fn view(
    conn: &mut PgConnection,
    link: &AgentLinkRow,
    me: Uuid,
) -> Result<AgentLink, ApiError> {
    let other = link.other(me).ok_or_else(link_404)?;
    let peer = agents::get(&mut *conn, other).await?.ok_or_else(link_404)?;
    let (workspace_name, creator_name) =
        repo::view_context(&mut *conn, peer.workspace_id, link.created_by_user_id).await?;
    let created_by = if link.created_by_kind == "person" {
        LinkCreator {
            kind: LinkCreatorKind::Person,
            display_name: creator_name,
        }
    } else {
        LinkCreator {
            kind: LinkCreatorKind::Agent,
            display_name: None,
        }
    };
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
            workspace_name,
            policy: LinkPolicy::parse(link.policy_of(other).unwrap_or("ask")),
        },
        direction: LinkDirection::parse(link.direction_of(me)),
        created_by,
        created_at: link.created_at,
        updated_at: link.updated_at,
    })
}

pub async fn list(s: &AppState, c: &Caller, agent: Uuid) -> Result<Vec<AgentLink>, ApiError> {
    load_agent(s, c, agent).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::list_for_agent(&mut *conn, agent).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(view(&mut conn, r, agent).await?);
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
    let link = repo::get(&s.pool, id).await?.ok_or_else(link_404)?;
    if link.policy_of(b.agent_id).is_none() {
        return Err(link_404());
    }
    let agent = load_agent(s, c, b.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            link_404()
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
        .ok_or_else(link_404)?;
    if cur.revision != b.revision {
        return Err(stale());
    }
    let row = repo::set_policy(&mut tx, id, b.agent_id, b.policy.as_str(), Some(b.revision))
        .await?
        .ok_or_else(stale)?;
    let out = view(&mut tx, &row, b.agent_id).await?;
    audit::commit_critical(
        tx,
        &AuditEvent::by(c, "agent_link.policy")
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

// ---------------------------------------------------------------- people link agents (C-1085)

/// The caller's access to a workspace, any refusal collapsed to `None` (SEC-142: nothing about
/// an agent the caller cannot use is revealed, not even a key's `workspace_mismatch`).
async fn access(s: &AppState, c: &Caller, ws: Uuid) -> Option<WorkspaceAuth> {
    c.workspace(s, ws).await.ok()
}

/// `GET /agents/{agent}/linkable` (C-1085): same-organization agents of the workspaces the
/// caller can use; name, handle and workspace only.
pub async fn linkable(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
) -> Result<Vec<LinkableAgent>, ApiError> {
    let me = load_agent(s, c, agent).await?;
    let only_workspace = match &c.principal {
        Principal::ApiKey {
            workspace_id: Some(w),
            ..
        } => Some(*w),
        _ => None,
    };
    let rows = repo::linkable(&s.pool, c.user_id, me.org_id, me.id, only_workspace).await?;
    Ok(rows
        .into_iter()
        .map(|r| LinkableAgent {
            agent_id: r.agent_id,
            name: r.name,
            handle: r.handle,
            workspace_id: r.workspace_id,
            workspace_name: r.workspace_name,
            linked: r.linked,
            hosted: r.hosted,
            frozen: r.frozen,
        })
        .collect())
}

/// Starting policy of a side the creator links (SEC-142, FIX-110): `allow` is an approver's
/// consent; anyone else leaves the side at `ask`. The creator never sets `blocked`.
fn starting_policy(auth: &WorkspaceAuth) -> &'static str {
    if auth.can_approve() { "allow" } else { "ask" }
}

/// `POST /agents/{agent}/links` (C-1085). `(created, link)`: `false` with the existing link when
/// the pair is linked already with the same direction.
pub async fn create(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    b: CreateLinkBody,
) -> Result<(bool, AgentLink), ApiError> {
    let me = load_agent(s, c, agent).await?;
    if b.peer_agent_id == me.id {
        return Err(ApiError::conflict(
            "link_self",
            "An agent cannot be linked to itself.",
        ));
    }
    let me_auth = c.workspace(s, me.workspace_id).await?;
    let peer = agents::get(&s.pool, b.peer_agent_id)
        .await?
        .filter(|p| p.org_id == me.org_id)
        .ok_or_else(agent_404)?;
    let peer_auth = access(s, c, peer.workspace_id)
        .await
        .ok_or_else(agent_404)?;
    if me.kind != "hosted" || peer.kind != "hosted" {
        return Err(ApiError::conflict(
            "not_hosted",
            "Only agents that run on Aurora can be linked.",
        ));
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    ensure_agent_active(&mut tx, me.id).await?;
    ensure_agent_active(&mut tx, peer.id).await?;
    let direction = repo::stored_direction(me.id, peer.id, b.direction.into());
    let (policy_me, policy_peer) = (starting_policy(&me_auth), starting_policy(&peer_auth));
    let Some(row) = repo::create_by_person(
        &mut tx,
        &NewPersonLink {
            x: me.id,
            policy_x: policy_me,
            y: peer.id,
            policy_y: policy_peer,
            direction,
            user_id: c.user_id,
        },
    )
    .await?
    else {
        let cur = repo::get_pair(&mut *tx, me.id, peer.id)
            .await?
            .ok_or_else(|| ApiError::internal("Link vanished."))?;
        if cur.direction != direction {
            return Err(ApiError::conflict(
                "link_exists",
                "These agents are linked already with another direction; change it instead.",
            )
            .with_details(json!({ "link_id": cur.id })));
        }
        let out = view(&mut tx, &cur, me.id).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok((false, out));
    };
    let out = view(&mut tx, &row, me.id).await?;
    audit::commit_critical(
        tx,
        &AuditEvent::by(c, "agent_link.create")
            .org(me.org_id)
            .workspace(me.workspace_id)
            .target("agent_link", row.id)
            .metadata(json!({
                "agent_a_id": row.agent_a_id, "agent_b_id": row.agent_b_id,
                "direction": row.direction, "policy_a": row.policy_a, "policy_b": row.policy_b,
                "via": "person",
            })),
    )
    .await?;
    Ok((true, out))
}

/// Both agents of a link, for a caller who can use at least one side; `None` (404) otherwise.
struct Sides {
    a: AgentRow,
    b: AgentRow,
    a_auth: Option<WorkspaceAuth>,
    b_auth: Option<WorkspaceAuth>,
}

async fn sides(s: &AppState, c: &Caller, link: &AgentLinkRow) -> Result<Sides, ApiError> {
    let a = agents::get(&s.pool, link.agent_a_id)
        .await?
        .ok_or_else(link_404)?;
    let b = agents::get(&s.pool, link.agent_b_id)
        .await?
        .ok_or_else(link_404)?;
    let a_auth = access(s, c, a.workspace_id).await;
    let b_auth = access(s, c, b.workspace_id).await;
    if a_auth.is_none() && b_auth.is_none() {
        return Err(link_404());
    }
    Ok(Sides {
        a,
        b,
        a_auth,
        b_auth,
    })
}

/// `PUT /agent-links/{id}/direction` (C-1085). Narrowing needs `can_use` of `agent_id`'s side;
/// widening (any new flow) needs `can_approve` on both sides.
pub async fn set_direction(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    b: SetDirectionBody,
) -> Result<AgentLink, ApiError> {
    let link = repo::get(&s.pool, id).await?.ok_or_else(link_404)?;
    let Some(peer_id) = link.other(b.agent_id) else {
        return Err(link_404());
    };
    let sd = sides(s, c, &link).await?;
    let (me, me_auth, peer, peer_auth) = if b.agent_id == link.agent_a_id {
        (&sd.a, sd.a_auth, &sd.b, sd.b_auth)
    } else {
        (&sd.b, sd.b_auth, &sd.a, sd.a_auth)
    };
    let me_auth = me_auth.ok_or_else(link_404)?;
    let direction = repo::stored_direction(me.id, peer_id, b.direction.into());
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(link_404)?;
    if cur.revision != b.revision {
        return Err(stale());
    }
    if cur.direction == direction {
        let out = view(&mut tx, &cur, me.id).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(out);
    }
    if repo::direction_widens(&cur.direction, direction) {
        // It gives an agent more reach: an approver of each side consents (SEC-142).
        if !me_auth.can_approve() {
            return Err(approver_required(Some(me.org_id), Some(me.workspace_id)));
        }
        if !peer_auth.is_some_and(|a| a.can_approve()) {
            return Err(approver_required(
                Some(peer.org_id),
                Some(peer.workspace_id),
            ));
        }
    }
    let row = repo::set_direction(&mut tx, id, direction, b.revision)
        .await?
        .ok_or_else(stale)?;
    let out = view(&mut tx, &row, me.id).await?;
    audit::commit_critical(
        tx,
        &AuditEvent::by(c, "agent_link.direction")
            .org(me.org_id)
            .workspace(me.workspace_id)
            .target("agent_link", id)
            .metadata(json!({
                "agent_a_id": row.agent_a_id, "agent_b_id": row.agent_b_id,
                "from": cur.direction, "to": row.direction,
            })),
    )
    .await?;
    Ok(out)
}

/// `DELETE /agent-links/{id}` (C-1085): `can_use` on either side. Cancels the pair's queued,
/// then open, tasks with the platform's line, expires its pending `message_agent` approvals and
/// deletes the row, in one transaction under the link's row lock.
pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let link = repo::get(&s.pool, id).await?.ok_or_else(link_404)?;
    let sd = sides(s, c, &link).await?;
    let audit_side = if sd.a_auth.is_some() { &sd.a } else { &sd.b };
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    // (FIX-257) The task conversations the closes below write into come before the link.
    peers::lock_link_origins(&mut tx, id).await?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(link_404)?;
    let mut cancelled = 0u32;
    for task_id in repo::active_task_ids(&mut tx, id).await? {
        let Some(task) = peer_tasks::get(&mut *tx, task_id).await? else {
            continue;
        };
        if peers::close_with_promotion(&mut tx, &task, &Close::Cancelled(Some(UNLINKED)))
            .await?
            .is_some()
        {
            cancelled += 1;
        }
    }
    let mut expired = 0u32;
    for approval in repo::lock_pending_approval_ids(&mut tx, id).await? {
        if approvals::expire_with_reason(&mut tx, approval, Some(UNLINKED)).await? {
            expired += 1;
        }
    }
    if !repo::delete(&mut tx, id).await? {
        return Err(link_404());
    }
    audit::commit_critical(
        tx,
        &AuditEvent::by(c, "agent_link.delete")
            .org(audit_side.org_id)
            .workspace(audit_side.workspace_id)
            .target("agent_link", id)
            .metadata(json!({
                "agent_a_id": cur.agent_a_id, "agent_b_id": cur.agent_b_id,
                "cancelled_tasks": cancelled, "expired_approvals": expired,
            })),
    )
    .await?;
    Ok(())
}

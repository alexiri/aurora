//! Reach grants (docs/13; C-35): listing for `can_use`, decisions for `can_own`.

use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service::load_agent;
use crate::http::auth::{Caller, require_own};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::reach_grants::{self as repo, ReachRow};
use serde_json::json;
use uuid::Uuid;

fn view(r: &ReachRow) -> ReachGrant {
    ReachGrant {
        id: r.id,
        agent_id: r.agent_id,
        integration_id: r.integration_id,
        subject_kind: if r.subject_kind == "person" {
            SubjectKind::Person
        } else {
            SubjectKind::Space
        },
        subject_ref: r.subject_ref.clone(),
        subject_label: r.subject_label.clone(),
        state: match r.state.as_str() {
            "approved" => ReachState::Approved,
            "blocked" => ReachState::Blocked,
            "left" => ReachState::Left,
            "expired" => ReachState::Expired,
            _ => ReachState::Pending,
        },
        listen: Listen::parse(&r.listen),
        revision: r.revision,
        decided_by: r.decided_by_user_id.map(|id| UserRef {
            id,
            display_name: r.decided_by_name.clone().unwrap_or_default(),
        }),
        decided_at: r.decided_at,
        expires_at: r.expires_at,
        created_at: r.created_at,
    }
}

fn stale() -> ApiError {
    ApiError::conflict("stale_revision", "The grant changed since you loaded it.")
}

pub async fn list(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    p: Pagination,
) -> Result<Page<ReachGrant>, ApiError> {
    load_agent(s, c, agent).await?;
    let rows =
        repo::page_for_agent(&s.pool, agent, p.after.map(|a| (a.t, a.id)), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Page {
        items: page.items.iter().map(view).collect(),
        next_cursor: page.next_cursor,
    })
}

pub async fn decide(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    req: ReachDecisionRequest,
) -> Result<ReachGrant, ApiError> {
    let not_found = || ApiError::not_found("Reach grant not found.");
    let probe = repo::get(&s.pool, id).await?.ok_or_else(not_found)?;
    let agent = load_agent(s, c, probe.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            not_found()
        } else {
            e
        }
    })?;
    require_own(&c.workspace(s, agent.workspace_id).await?)?;

    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    if cur.revision != req.revision {
        return Err(stale());
    }
    let state = match req.decision {
        ReachDecision::Approve => "approved",
        ReachDecision::Block => "blocked",
    };
    let listen = req
        .listen
        .map_or_else(|| cur.listen.clone(), |l| l.as_str().to_owned());
    repo::decide(&mut *tx, id, req.revision, state, &listen, c.user_id)
        .await?
        .ok_or_else(stale)?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "reach_grant.decide",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("reach_grant", id)
        .metadata(json!({"decision": state, "listen": listen, "subject_kind": cur.subject_kind})),
    );
    let row = repo::get(&s.pool, id).await?.ok_or_else(not_found)?;
    Ok(view(&row))
}

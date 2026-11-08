//! User links and DM defaults (docs/13 §5.9; C-36, C-37).

use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, SessionCaller};
use crate::http::pagination::{Page, Pagination};
use aurora_core::token;
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::user_links::{self as repo, UserLinkRow};
use aurora_db::repo::{agents, channel_integrations as integ_repo, channel_presences, dm_defaults};
use aurora_db::repo::{link_requests, reach_grants};
use aurora_db::{DbError, authz, conflict_code};
use serde_json::json;
use uuid::Uuid;

use super::integrations::load_admin;

const LINK_CONSTRAINT: &str = "user_links_integration_id_platform_user_ref_key";
const REF_MAX: usize = 512;

fn view(r: &UserLinkRow) -> UserLink {
    UserLink {
        id: r.id,
        integration_id: r.integration_id,
        connector_id: r.connector_id.clone(),
        tenant_label: r.tenant_label.clone(),
        platform_user_ref: r.platform_user_ref.clone(),
        user: UserRef {
            id: r.user_id,
            display_name: r.user_display_name.clone(),
        },
        method: LinkMethod::parse(&r.method),
        created_at: r.created_at,
    }
}

fn already_linked() -> ApiError {
    ApiError::conflict(
        "already_linked",
        "That platform user is already linked to an Aurora user.",
    )
}

// ------------------------------------------------------------------ admin links

pub async fn list_for_integration(
    s: &AppState,
    c: &Caller,
    integration: Uuid,
    p: Pagination,
) -> Result<Page<UserLink>, ApiError> {
    load_admin(s, c, integration).await?;
    let rows = repo::page_for_integration(
        &s.pool,
        integration,
        p.after.map(|a| (a.t, a.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Page {
        items: page.items.iter().map(view).collect(),
        next_cursor: page.next_cursor,
    })
}

pub async fn admin_create(
    s: &AppState,
    c: &Caller,
    integration: Uuid,
    req: CreateUserLinkRequest,
) -> Result<UserLink, ApiError> {
    let integ = load_admin(s, c, integration).await?;
    let refl = req.platform_user_ref.trim();
    if refl.is_empty() || refl.chars().count() > REF_MAX {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new(
                "/platform_user_ref",
                "must be 1-512 characters",
            )],
        ));
    }
    let member = authz::org_membership(&s.pool, req.user_id, integ.org_id)
        .await?
        .is_some_and(|m| m.is_active());
    if !member {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new(
                "/user_id",
                "not an active member of the organization",
            )],
        ));
    }
    let mut tx = audit::begin(s).await?;
    let id = repo::insert(&mut *tx, integration, refl, req.user_id, "admin")
        .await
        .map_err(|e| conflict_code(e, &[(LINK_CONSTRAINT, "already_linked")]))?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(c),
        "user_link.create",
        Outcome::Success,
    )
    .org(integ.org_id)
    .target("user_link", id)
    .metadata(json!({"integration_id": integration, "user_id": req.user_id, "method": "admin"}));
    audit::commit_critical(tx, &ev).await?;
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("Link vanished."))?;
    Ok(view(&row))
}

pub async fn admin_delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let not_found = || ApiError::not_found("Link not found.");
    let row = repo::get(&s.pool, id).await?.ok_or_else(not_found)?;
    c.org(s, row.org_id).await?.require_admin()?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(c),
        "user_link.delete",
        Outcome::Success,
    )
    .org(row.org_id)
    .target("user_link", id)
    .metadata(json!({"integration_id": row.integration_id, "user_id": row.user_id}));
    remove(s, &row, &ev).await?;
    Ok(())
}

/// Delete a link with its DM default and its (security-critical) audit row, atomically.
async fn remove(s: &AppState, row: &UserLinkRow, ev: &AuditEvent) -> Result<(), ApiError> {
    let mut tx = audit::begin(s).await?;
    repo::delete(&mut *tx, row.id).await?;
    dm_defaults::delete(&mut *tx, row.integration_id, &row.platform_user_ref).await?;
    audit::commit_critical(tx, ev).await?;
    Ok(())
}

// --------------------------------------------------------------------- my links

pub async fn mine(s: &AppState, c: &SessionCaller) -> Result<Vec<UserLink>, ApiError> {
    Ok(repo::list_for_user(&s.pool, c.user_id)
        .await?
        .iter()
        .map(view)
        .collect())
}

pub async fn delete_mine(s: &AppState, c: &SessionCaller, id: Uuid) -> Result<(), ApiError> {
    let row = repo::get(&s.pool, id)
        .await?
        .filter(|r| r.user_id == c.user_id)
        .ok_or_else(|| ApiError::not_found("Link not found."))?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(c),
        "user_link.delete",
        Outcome::Success,
    )
    .org(row.org_id)
    .target("user_link", id)
    .metadata(json!({"integration_id": row.integration_id, "self": true}));
    remove(s, &row, &ev).await?;
    Ok(())
}

/// `POST /channel-links/confirm` (C-36): unknown, used and expired tokens all answer 410.
pub async fn confirm(
    s: &AppState,
    c: &SessionCaller,
    req: ConfirmLinkRequest,
) -> Result<UserLink, ApiError> {
    let expired = || ApiError::gone("link_expired", "This link has expired or was already used.");
    if !req.token.starts_with("aui_") || req.token.len() > 256 {
        return Err(expired());
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let lr = link_requests::get_for_update_by_hash(&mut tx, &token::hash(&req.token))
        .await?
        .ok_or_else(expired)?;
    if lr.used || lr.expired {
        return Err(expired());
    }
    let integ = integ_repo::get(&mut *tx, lr.integration_id)
        .await?
        .ok_or_else(expired)?;
    // Only members of the integration's organization can link into it.
    c.org(s, integ.org_id).await?;
    let existing = repo::user_link_for(&mut *tx, lr.integration_id, &lr.platform_user_ref).await?;
    let link_id = match existing {
        Some(l) if l.user_id == c.user_id => l.id,
        Some(_) => return Err(already_linked()),
        None => repo::insert(
            &mut *tx,
            lr.integration_id,
            &lr.platform_user_ref,
            c.user_id,
            "sign_in_link",
        )
        .await
        .map_err(|e| conflict_code(e, &[(LINK_CONSTRAINT, "already_linked")]))?,
    };
    link_requests::mark_used(&mut *tx, lr.id, c.user_id).await?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(c),
        "user_link.create",
        Outcome::Success,
    )
    .org(integ.org_id)
    .target("user_link", link_id)
    .metadata(json!({"integration_id": integ.id, "method": "sign_in_link"}));
    audit::commit_critical(tx, &ev).await?;
    let row = repo::get(&s.pool, link_id)
        .await?
        .ok_or_else(|| ApiError::internal("Link vanished."))?;
    Ok(view(&row))
}

// ------------------------------------------------------------------ DM defaults

pub async fn dm_defaults_list(s: &AppState, c: &SessionCaller) -> Result<Vec<DmDefault>, ApiError> {
    let mut out = Vec::new();
    for l in repo::list_for_user(&s.pool, c.user_id).await? {
        let d = dm_defaults::get(&s.pool, l.integration_id, &l.platform_user_ref).await?;
        out.push(DmDefault {
            integration_id: l.integration_id,
            platform_user_ref: l.platform_user_ref,
            agent_id: d.map(|d| d.agent_id),
        });
    }
    Ok(out)
}

/// `PUT /me/dm-defaults` (C-37). Applies to every platform user the caller has linked on the
/// integration; answers with the first.
pub async fn dm_default_set(
    s: &AppState,
    c: &SessionCaller,
    req: SetDmDefaultRequest,
) -> Result<DmDefault, ApiError> {
    let links = repo::list_for_user_in(&s.pool, c.user_id, req.integration_id).await?;
    if links.is_empty() {
        return Err(ApiError::not_found("No link on that integration."));
    }
    let unreachable = || {
        ApiError::unprocessable(
            "agent_not_reachable",
            "That agent cannot be reached through this integration.",
        )
    };
    let agent = agents::get(&s.pool, req.agent_id)
        .await?
        .ok_or_else(unreachable)?;
    let can_use = authz::workspace_access(&s.pool, c.user_id, agent.workspace_id)
        .await?
        .is_some();
    let refs: Vec<String> = links.iter().map(|l| l.platform_user_ref.clone()).collect();
    let reachable = can_use
        || reach_grants::approved_person_reach(&s.pool, agent.id, req.integration_id, &refs)
            .await?;
    if !reachable
        || channel_presences::active_shared(&s.pool, agent.id, req.integration_id)
            .await?
            .is_none()
    {
        return Err(unreachable());
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    for l in &links {
        dm_defaults::upsert(&mut *tx, l.integration_id, &l.platform_user_ref, agent.id).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(DmDefault {
        integration_id: req.integration_id,
        platform_user_ref: links[0].platform_user_ref.clone(),
        agent_id: Some(agent.id),
    })
}

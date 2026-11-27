//! (FIX-234, D-477, C-1161) The self-grant notice. An administrator who gives themselves access
//! to a shared workspace they are not already a person of is audited critical
//! (`workspace_access.self_grant`, written by the route in the change's transaction) and every
//! other active own-right owner of the workspace is told — when there is none, every other
//! active org owner. One e-mail and one Web Push per recipient, no content: the fixed sentence
//! with names and the role, and a link to the workspace's People page.
//!
//! The routes that give a person access through their **direct** binding (add or raise; restore
//! or remove a suspended one, D-501) share [`require_session`], [`audit_tx`] and
//! [`notify_committed`].

use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::domains::auth::mail::Mail;
use crate::http::auth::Caller;
use aurora_core::{ApiError, sanitize};
use aurora_db::{DbError, PgPool};
use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// A self-grant needs a signed-in session, never an API key.
pub fn require_session(c: &Caller) -> Result<(), ApiError> {
    if c.is_session() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "session_required",
        "Adding yourself to a workspace needs a signed-in session, not an API key.",
    ))
}

/// The critical `workspace_access.self_grant` row of a caller's own direct binding, in the
/// change's transaction (which the change's own row commits as critical). `via` says which act
/// gave the access (`user`, `restore`, `remove`).
pub async fn audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    c: &Caller,
    org: Uuid,
    ws: Uuid,
    role: &str,
    via: &str,
) -> Result<(), ApiError> {
    let ev = AuditEvent::by(c, "workspace_access.self_grant")
        .org(org)
        .workspace(ws)
        .target("user", c.user_id)
        .metadata(json!({"role": role, "via": via}));
    audit::insert_tx(tx, &ev).await?;
    Ok(())
}

/// [`notify_for_user`] after the change committed: a failing notice is logged and the change
/// stands.
pub async fn notify_committed(state: &AppState, org: Uuid, ws: Uuid, actor: &Caller, role: &str) {
    if let Err(e) = notify_for_user(state, org, ws, actor, role).await {
        tracing::warn!(
            error = %e,
            workspace = %ws,
            "self-grant notice after the change failed; the change stands"
        );
    }
}

const NAME_MAX: usize = 80;

fn name(s: &str) -> String {
    sanitize::one_line(s, NAME_MAX)
}

/// One person to tell.
struct Recipient {
    user_id: Uuid,
    email: String,
}

/// The other active own-right owners of the workspace; when there is none, the other active
/// org owners (C-1161). People tables only (the `own_workspace_access` view).
async fn recipients(
    pool: &PgPool,
    org: Uuid,
    ws: Uuid,
    actor: Uuid,
) -> Result<Vec<Recipient>, ApiError> {
    let own: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT o.user_id, u.email::text \
         FROM own_workspace_access o \
         JOIN org_members m ON m.org_id = o.org_id AND m.user_id = o.user_id \
                              AND m.status = 'active' \
         JOIN users u ON u.id = o.user_id AND u.disabled_at IS NULL \
         WHERE o.workspace_id = $1 AND o.role = 'owner' AND o.user_id <> $2 \
         ORDER BY u.display_name, o.user_id",
    )
    .bind(ws)
    .bind(actor)
    .fetch_all(pool)
    .await
    .map_err(DbError::from)?;
    if !own.is_empty() {
        return Ok(own
            .into_iter()
            .map(|(user_id, email)| Recipient { user_id, email })
            .collect());
    }
    let org_owners: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT m.user_id, u.email::text \
         FROM org_members m \
         JOIN users u ON u.id = m.user_id AND u.disabled_at IS NULL \
         WHERE m.org_id = $1 AND m.role = 'owner' AND m.status = 'active' \
           AND m.user_id <> $2 \
         ORDER BY u.display_name, m.user_id",
    )
    .bind(org)
    .bind(actor)
    .fetch_all(pool)
    .await
    .map_err(DbError::from)?;
    Ok(org_owners
        .into_iter()
        .map(|(user_id, email)| Recipient { user_id, email })
        .collect())
}

/// The fixed sentence (C-1161): "<Name> (organization admin) gave themselves <role> access to
/// <Workspace>." The org role says which administrator acted.
fn sentence(actor_name: &str, org_role: &str, role: &str, ws_name: &str) -> String {
    format!(
        "{} ({}) gave themselves {} access to {}.",
        name(actor_name),
        org_role,
        role,
        name(ws_name)
    )
}

/// The People page of the workspace (C-1161's link).
fn people_link(external_url: &str, org_slug: &str, ws_slug: &str) -> String {
    format!(
        "{}/o/{}/w/{}/people",
        external_url.trim_end_matches('/'),
        org_slug,
        ws_slug
    )
}

/// Tell the owners. Fire-and-forget per recipient (a failed send is logged, as in the other
/// notifiers); called after the self-grant's transaction committed.
#[allow(clippy::too_many_arguments)]
pub async fn notify(
    state: &AppState,
    org: Uuid,
    ws: Uuid,
    org_slug: &str,
    ws_slug: &str,
    ws_name: &str,
    actor: Uuid,
    actor_name: &str,
    org_role: &str,
    role: &str,
) -> Result<(), ApiError> {
    let people = recipients(&state.pool, org, ws, actor).await?;
    if people.is_empty() {
        return Ok(());
    }
    let text = sentence(actor_name, org_role, role, ws_name);
    let link = people_link(&state.config.external_url, org_slug, ws_slug);
    for p in &people {
        let mail = Mail::new(
            &p.email,
            &format!(
                "{} gave themselves access to {}",
                name(actor_name),
                name(ws_name)
            ),
            &format!(
                "{text}\n\nOpen the workspace's People page to review it. You need to be signed \
                 in to manage it."
            ),
            &link,
        );
        if let Err(e) = state.mailer.send(mail).await {
            tracing::error!(error = %e, "self-grant e-mail send failed");
        }
        crate::domains::push::deliver::notify_self_grant(state, p.user_id, &text, &link).await;
    }
    Ok(())
}

/// The same notice for a self-grant that already knows the caller is the target (e.g.
/// by-email): fetches the names, slugs and the actor's org role, then sends. Called after the
/// commit.
pub async fn notify_for_user(
    state: &AppState,
    org: Uuid,
    ws: Uuid,
    actor: &crate::http::auth::Caller,
    role: &str,
) -> Result<(), ApiError> {
    let (org_slug, ws_slug, ws_name): (String, String, String) = sqlx::query_as(
        "SELECT o.slug, w.slug, w.name FROM workspaces w \
         JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(ws)
    .fetch_one(&state.pool)
    .await
    .map_err(DbError::from)?;
    let (actor_name,): (String,) = sqlx::query_as("SELECT display_name FROM users WHERE id = $1")
        .bind(actor.user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(DbError::from)?;
    let org_role = aurora_db::authz::effective_org_role(&state.pool, actor.user_id, org)
        .await?
        .map(|r| r.as_str())
        .unwrap_or("admin")
        .to_owned();
    notify(
        state,
        org,
        ws,
        &org_slug,
        &ws_slug,
        &ws_name,
        actor.user_id,
        &actor_name,
        &org_role,
        role,
    )
    .await
}

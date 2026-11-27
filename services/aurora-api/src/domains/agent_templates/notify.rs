//! (FIX-251, D-488(a)) The update notice. When a version becomes the offered one (an approval,
//! or a re-offer after a withdrawal), the people who can apply an update to an agent made from
//! that template — the same right the apply route requires, `Caller::workspace` on the agent's
//! workspace — are told: ONE in-app notice per workspace and version (the
//! `agent_template_update_notices` row is the dedup; a withdrawal and a re-offer never send
//! again), plus e-mail and Web Push like the other notices. Never the person who approved,
//! never a person without that right, never an e-mail address in a body. Everything runs after
//! the commit; a failure is logged and never fails or undoes the approval.

use aurora_core::{ApiError, sanitize};
use uuid::Uuid;

use crate::app::AppState;
use crate::domains::auth::mail::Mail;
use aurora_db::repo::agent_templates::{self as repo};

const NAME_MAX: usize = 80;

fn name(s: &str) -> String {
    sanitize::one_line(s, NAME_MAX)
}

/// One workspace to tell: its ids, slugs and names, and how many of its agents made from the
/// template are behind the offered version.
struct Workspace {
    id: Uuid,
    org_slug: String,
    ws_slug: String,
    ws_name: String,
    behind: i64,
}

/// The hook for the approve and re-offer routes: called after their commit. Never fails.
pub async fn version_offered(state: &AppState, template_id: &str, actor: Option<Uuid>) {
    if let Err(e) = tell(state, template_id, actor).await {
        tracing::error!(error = %e.message(), "template update notice failed");
    }
}

async fn tell(state: &AppState, template_id: &str, actor: Option<Uuid>) -> Result<(), ApiError> {
    // The offered version decides: a withdrawn template, or one whose newest approved version
    // is not the one just approved, tells nobody (the dedup row of an already-told version
    // keeps it quiet anyway).
    let Some(t) = repo::template_by_template_id(&state.pool, template_id)
        .await?
        .filter(|t| t.offered)
    else {
        return Ok(());
    };
    let Some(offered) = super::admin::offered_version(&state.pool, t.id).await? else {
        return Ok(());
    };
    let workspaces: Vec<Workspace> = sqlx::query_as(
        "SELECT w.id, o.slug, w.slug, w.name, \
                count(*) FILTER (WHERE a.template_version IS DISTINCT FROM $2)::bigint AS behind \
         FROM agents a \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN organizations o ON o.id = w.org_id \
         WHERE a.template_id = $1 \
           AND NOT EXISTS (SELECT 1 FROM org_disabled_agent_templates d \
                            WHERE d.org_id = w.org_id AND d.template_id = $1) \
         GROUP BY w.id, o.slug, w.slug, w.name \
         ORDER BY w.id",
    )
    .bind(template_id)
    .bind(&offered.version)
    .fetch_all(&state.pool)
    .await
    .map_err(aurora_db::DbError::from)?
    .into_iter()
    .map(|(id, org_slug, ws_slug, ws_name, behind)| Workspace {
        id,
        org_slug,
        ws_slug,
        ws_name,
        behind,
    })
    .collect();
    for ws in workspaces {
        if ws.behind <= 0 {
            continue;
        }
        // The dedup: the first run for this workspace and version tells the people, every
        // later one (a re-offer, a second approval) is silent.
        if !repo::record_update_notice(&state.pool, ws.id, template_id, &offered.version).await? {
            continue;
        }
        let people = recipients(state, ws.id, actor).await?;
        tell_workspace(state, template_id, &t.name, &offered.version, &ws, &people).await;
    }
    Ok(())
}

/// The people who can apply an update in `ws` (the right the apply route requires: an active
/// member with an effective workspace access — own, group- or IdP-derived, or an effective org
/// owner or admin's implicit own access), except the person who approved.
async fn recipients(
    state: &AppState,
    ws: Uuid,
    actor: Option<Uuid>,
) -> Result<Vec<(Uuid, String, Option<String>)>, aurora_db::DbError> {
    sqlx::query_as(
        "SELECT DISTINCT e.user_id, u.email::text, u.timezone \
         FROM effective_workspace_access e \
         JOIN effective_org_members eo ON eo.org_id = e.org_id AND eo.user_id = e.user_id \
         JOIN org_members m ON m.org_id = e.org_id AND m.user_id = e.user_id \
                               AND m.status = 'active' \
         JOIN users u ON u.id = e.user_id AND u.disabled_at IS NULL \
         WHERE e.workspace_id = $1 AND e.user_id IS DISTINCT FROM $2 \
         ORDER BY e.user_id",
    )
    .bind(ws)
    .bind(actor)
    .fetch_all(&state.pool)
    .await
    .map_err(aurora_db::DbError::from)
}

/// The e-mails and pushes of one workspace's notice. Fire-and-forget per recipient (a failed
/// send is logged, as in the other notifiers); never fails.
async fn tell_workspace(
    state: &AppState,
    template_id: &str,
    template_name: &str,
    version: &str,
    ws: &Workspace,
    people: &[(Uuid, String, Option<String>)],
) {
    let template = name(template_name);
    let workspace = name(&ws.ws_name);
    let agents = if ws.behind == 1 {
        "1 agent".to_owned()
    } else {
        format!("{} agents", ws.behind)
    };
    let title = format!("{template} {version} is available for {agents} in {workspace}");
    let text = format!(
        "{title}. Applying an update keeps your own changes where they can be merged; \
         memories are never changed."
    );
    let link = format!(
        "{}/o/{}/w/{}/agents?needs_update=1",
        state.config.external_url.trim_end_matches('/'),
        ws.org_slug,
        ws.ws_slug
    );
    for (user_id, email, _) in people {
        let mail = Mail::new(
            email,
            &format!("{template} {version} is available in {workspace}"),
            &format!("{text}\n\nOpen the Agents page to review the updates."),
            &link,
        );
        if let Err(e) = state.mailer.send(mail).await {
            tracing::error!(error = %e, "template update notice e-mail send failed");
        }
        crate::domains::push::deliver::notify_template_update(state, *user_id, &title, &link).await;
    }
    tracing::debug!(template = %template_id, version, workspace = %ws.id, people = people.len(), "template update notice sent");
}

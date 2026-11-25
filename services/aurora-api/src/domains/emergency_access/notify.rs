//! (FIX-236, D-478(c), C-1165) The opening notices. The workspace's people **in their own
//! right** (for a personal workspace, its person) and the organization's other active owners are
//! told at once by e-mail and Web Push; never the opener. Notices go out **after** the grant's
//! transaction (with its critical audit row) committed: a failed send is logged and must not
//! undo or fail the grant. A person's e-mail is never a label — display names only.

use crate::app::AppState;
use crate::domains::auth::mail::Mail;
use aurora_core::{ApiError, sanitize};
use aurora_db::{DbError, PgPool};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use uuid::Uuid;

const NAME_MAX: usize = 80;

fn name(s: &str) -> String {
    sanitize::one_line(s, NAME_MAX)
}

/// One person to tell: their e-mail and their own time zone (C-1165: "until <time, recipient's
/// timezone>").
struct Recipient {
    user_id: Uuid,
    email: String,
    timezone: Option<String>,
}

/// The recipients: every active person with access to the workspace in their own right, plus
/// every other active org owner; the opener is never told (C-1165). People tables only.
async fn recipients(
    pool: &PgPool,
    org: Uuid,
    ws: Uuid,
    opener: Uuid,
) -> Result<Vec<Recipient>, ApiError> {
    let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT t.user_id, u.email::text, u.timezone \
         FROM (SELECT o.user_id FROM own_workspace_access o \
                 JOIN org_members m ON m.org_id = o.org_id AND m.user_id = o.user_id \
                                      AND m.status = 'active' \
                WHERE o.workspace_id = $1 \
               UNION \
               SELECT m2.user_id FROM org_members m2 \
                WHERE m2.org_id = $2 AND m2.role = 'owner' AND m2.status = 'active') t \
         JOIN users u ON u.id = t.user_id AND u.disabled_at IS NULL \
         WHERE t.user_id <> $3 \
         ORDER BY u.display_name, t.user_id",
    )
    .bind(ws)
    .bind(org)
    .bind(opener)
    .fetch_all(pool)
    .await
    .map_err(DbError::from)?;
    Ok(rows
        .into_iter()
        .map(|(user_id, email, timezone)| Recipient {
            user_id,
            email,
            timezone,
        })
        .collect())
}

/// "until 18:00" in the recipient's time zone; UTC when they set none or an unknown one.
fn until(expires_at: DateTime<Utc>, timezone: Option<&str>) -> String {
    let tz: Tz = timezone
        .and_then(|t| t.parse().ok())
        .unwrap_or(chrono_tz::UTC);
    expires_at.with_timezone(&tz).format("%H:%M").to_string()
}

/// The same notice for a route that has just committed the grant: fetches the slugs and names,
/// then sends. Call after the commit; a lookup or send failure is the caller's to log (it must
/// never undo or fail the grant).
pub async fn notify_open_for(
    state: &AppState,
    org: Uuid,
    ws: Uuid,
    opener: Uuid,
    reason: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), ApiError> {
    let (org_slug, ws_slug, ws_name, org_name): (String, String, String, String) = sqlx::query_as(
        "SELECT o.slug, w.slug, w.name, o.name FROM workspaces w \
         JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(ws)
    .fetch_one(&state.pool)
    .await
    .map_err(DbError::from)?;
    let (opener_name,): (String,) = sqlx::query_as("SELECT display_name FROM users WHERE id = $1")
        .bind(opener)
        .fetch_one(&state.pool)
        .await
        .map_err(DbError::from)?;
    notify_open(
        state,
        org,
        ws,
        &org_slug,
        &ws_slug,
        &ws_name,
        &org_name,
        opener,
        &opener_name,
        reason,
        expires_at,
    )
    .await
}

/// The fixed body (C-1165): "<Name>, an owner of <Org>, opened read access to <Workspace>'s
/// conversations until <time>. Reason: <reason>. Everything they open is recorded."
fn body(opener_name: &str, org_name: &str, ws_name: &str, until: &str, reason: &str) -> String {
    format!(
        "{}, an owner of {}, opened read access to {}'s conversations until {}. Reason: {}. \
         Everything they open is recorded.",
        name(opener_name),
        name(org_name),
        name(ws_name),
        until,
        reason
    )
}

/// Tell the workspace's people and the other owners. Fire-and-forget per recipient (a failed
/// send is logged, as in the other notifiers); called after the grant's commit. A failure here
/// never fails or undoes the grant (the grant and its audit row are the commit).
#[allow(clippy::too_many_arguments)]
pub async fn notify_open(
    state: &AppState,
    org: Uuid,
    ws: Uuid,
    org_slug: &str,
    ws_slug: &str,
    ws_name: &str,
    org_name: &str,
    opener: Uuid,
    opener_name: &str,
    reason: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), ApiError> {
    let people = recipients(&state.pool, org, ws, opener).await?;
    if people.is_empty() {
        return Ok(());
    }
    let link = format!(
        "{}/o/{}/w/{}",
        state.config.external_url.trim_end_matches('/'),
        org_slug,
        ws_slug
    );
    let subject = format!("Emergency read access to {}", name(ws_name));
    for p in &people {
        let text = body(
            opener_name,
            org_name,
            ws_name,
            &until(expires_at, p.timezone.as_deref()),
            reason,
        );
        let mail = Mail::new(
            &p.email,
            &subject,
            &format!("{text}\n\nOpen the workspace."),
            &link,
        );
        if let Err(e) = state.mailer.send(mail).await {
            tracing::error!(error = %e, "emergency-access e-mail send failed");
        }
        crate::domains::push::deliver::notify_emergency_access(state, p.user_id, &subject, &link)
            .await;
    }
    Ok(())
}

//! (FIX-244, D-486) The lifecycle notices. When a workspace or an organization is deleted,
//! frozen or unfrozen, its owners are told by e-mail and Web Push: what happened, to what
//! (display name), who did it (an instance administrator is never named), when (the recipient's
//! time zone), and what it means. Never the actor, never an ordinary member; no e-mail address,
//! secret, endpoint or path appears in a body. A delete prepares its notice **before** its
//! transaction (the cascade removes the recipients' rows) and sends after the commit; a freeze
//! or unfreeze prepares after its commit (the rows are still there). A failed send is logged
//! and never fails or undoes the action — a lookup failure means no notice, also never a
//! failure.

use crate::app::AppState;
use crate::audit::AuditActor;
use crate::domains::auth::mail::Mail;
use crate::domains::freeze::service::Target;
use crate::domains::push::deliver::Kind;
use aurora_core::sanitize;
use aurora_db::{DbError, PgPool};
use chrono::Utc;
use chrono_tz::Tz;
use uuid::Uuid;

const NAME_MAX: usize = 80;

fn name(s: &str) -> String {
    sanitize::one_line(s, NAME_MAX)
}

/// What happened to the workspace or organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Deleted,
    Frozen,
    Unfrozen,
}

impl Action {
    /// "… was <participle>" (the e-mail subject).
    fn participle(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Frozen => "frozen",
            Self::Unfrozen => "unfrozen",
        }
    }

    /// The verb of the body's sentence.
    fn verb(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Frozen => "froze",
            Self::Unfrozen => "lifted the freeze of",
        }
    }

    /// What the change means for the people the notice goes to.
    fn meaning(self, org: bool) -> &'static str {
        match (self, org) {
            (Self::Deleted, false) => "Its agents, conversations and credentials are gone.",
            (Self::Deleted, true) => "The organization and all of its workspaces are gone.",
            (Self::Frozen, false) => "No one can run its agents until it is unfrozen.",
            (Self::Frozen, true) => {
                "No one can run agents in any of its workspaces until it is unfrozen."
            }
            (Self::Unfrozen, _) => "Its agents can run again.",
        }
    }
}

/// One person to tell: their e-mail and their own time zone (the "when" of the body).
struct Recipient {
    user_id: Uuid,
    email: String,
    timezone: Option<String>,
}

/// A workspace's other active own-right owners; when there is none, the organization's other
/// effective owners (the self-grant notice's rule, C-1161: group- or IdP-derived owners are
/// told like direct ones).
async fn ws_recipients(
    pool: &PgPool,
    org: Uuid,
    ws: Uuid,
    actor: Option<Uuid>,
) -> Result<Vec<Recipient>, DbError> {
    let own: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT o.user_id, u.email::text, u.timezone \
         FROM own_workspace_access o \
         JOIN org_members m ON m.org_id = o.org_id AND m.user_id = o.user_id \
                               AND m.status = 'active' \
         JOIN users u ON u.id = o.user_id AND u.disabled_at IS NULL \
         WHERE o.workspace_id = $1 AND o.role = 'owner' \
           AND o.user_id IS DISTINCT FROM $2 \
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
            .map(|(user_id, email, timezone)| Recipient {
                user_id,
                email,
                timezone,
            })
            .collect());
    }
    org_recipients(pool, org, actor).await
}

/// An organization's other effective active owners (`effective_org_members`, the view the
/// freeze route's own check uses).
async fn org_recipients(
    pool: &PgPool,
    org: Uuid,
    actor: Option<Uuid>,
) -> Result<Vec<Recipient>, DbError> {
    let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT e.user_id, u.email::text, u.timezone \
         FROM effective_org_members e \
         JOIN users u ON u.id = e.user_id AND u.disabled_at IS NULL \
         WHERE e.org_id = $1 AND e.role = 'owner' AND e.user_id IS DISTINCT FROM $2 \
         ORDER BY u.display_name, e.user_id",
    )
    .bind(org)
    .bind(actor)
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

/// The actor as the body names them: an instance administrator is never named, everyone else by
/// display name. A name that cannot be read degrades to "an administrator" (logged) — the
/// notice still goes out.
async fn actor_label(pool: &PgPool, actor: &AuditActor) -> String {
    if actor.kind == "instance_admin" {
        return "an instance administrator".to_owned();
    }
    if let Some(id) = actor.id
        && let Ok((n,)) =
            sqlx::query_as::<_, (String,)>("SELECT display_name FROM users WHERE id = $1")
                .bind(id)
                .fetch_one(pool)
                .await
    {
        return name(&n);
    }
    tracing::warn!("lifecycle notice: the actor's display name is unavailable");
    "an administrator".to_owned()
}

/// "14:05 UTC on 2026-11-25" in the recipient's time zone (UTC when they set none or an unknown
/// one), as in the emergency-access notice.
fn now_in(timezone: Option<&str>) -> String {
    let tz: Tz = timezone
        .and_then(|t| t.parse().ok())
        .unwrap_or(chrono_tz::UTC);
    Utc::now()
        .with_timezone(&tz)
        .format("%H:%M %Z on %Y-%m-%d")
        .to_string()
}

/// One prepared notice: everything the send names was read while its rows still existed.
pub struct Notice {
    action: Action,
    org_scope: bool,
    scope_name: String,
    who: String,
    link: String,
    recipients: Vec<Recipient>,
}

/// A workspace's notice. Call **before** a delete's transaction (the cascade removes the
/// recipients' rows); for a freeze or unfreeze, after the commit. A lookup failure is logged
/// and means no notice — it never fails the action.
pub async fn prepare_workspace(
    state: &AppState,
    org: Uuid,
    ws: Uuid,
    actor: &AuditActor,
    action: Action,
) -> Option<Notice> {
    let row: Result<Option<(String, String, String)>, DbError> = sqlx::query_as(
        "SELECT w.name, w.slug, o.slug \
         FROM workspaces w JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(ws)
    .fetch_optional(&state.pool)
    .await
    .map_err(DbError::from);
    let (ws_name, ws_slug, org_slug) = match row {
        Ok(Some(r)) => r,
        Ok(None) | Err(_) => {
            tracing::error!("lifecycle notice: reading the workspace failed");
            return None;
        }
    };
    let who = actor_label(&state.pool, actor).await;
    let recipients = match ws_recipients(&state.pool, org, ws, actor.id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "lifecycle notice: reading the recipients failed");
            Vec::new()
        }
    };
    // A deleted scope has no page to open; anything else opens it.
    let link = if action == Action::Deleted {
        state.config.external_url.trim_end_matches('/').to_owned()
    } else {
        format!(
            "{}/o/{}/w/{}",
            state.config.external_url.trim_end_matches('/'),
            org_slug,
            ws_slug
        )
    };
    Some(Notice {
        action,
        org_scope: false,
        scope_name: ws_name,
        who,
        link,
        recipients,
    })
}

/// An organization's notice. Call **before** a delete's transaction; for a freeze or unfreeze,
/// after the commit. A lookup failure is logged and means no notice.
pub async fn prepare_org(
    state: &AppState,
    org: Uuid,
    actor: &AuditActor,
    action: Action,
) -> Option<Notice> {
    let row: Result<Option<(String, String)>, DbError> =
        sqlx::query_as("SELECT name, slug FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(&state.pool)
            .await
            .map_err(DbError::from);
    let (org_name, org_slug) = match row {
        Ok(Some(r)) => r,
        Ok(None) | Err(_) => {
            tracing::error!("lifecycle notice: reading the organization failed");
            return None;
        }
    };
    let who = actor_label(&state.pool, actor).await;
    let recipients = match org_recipients(&state.pool, org, actor.id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "lifecycle notice: reading the recipients failed");
            Vec::new()
        }
    };
    let link = if action == Action::Deleted {
        state.config.external_url.trim_end_matches('/').to_owned()
    } else {
        format!(
            "{}/o/{}",
            state.config.external_url.trim_end_matches('/'),
            org_slug
        )
    };
    Some(Notice {
        action,
        org_scope: true,
        scope_name: org_name,
        who,
        link,
        recipients,
    })
}

impl Notice {
    /// Send it: one e-mail and one Web Push per recipient, fire-and-forget per recipient (a
    /// failed send is logged, as in the other notifiers). Never fails.
    pub async fn send(self, state: &AppState) {
        if self.recipients.is_empty() {
            return;
        }
        let kind_word = if self.org_scope {
            "Organization"
        } else {
            "Workspace"
        };
        let subject = format!(
            "{} {} was {}",
            kind_word,
            name(&self.scope_name),
            self.action.participle()
        );
        let push_kind = if self.org_scope {
            Kind::OrgLifecycle
        } else {
            Kind::WorkspaceLifecycle
        };
        for p in &self.recipients {
            let text = format!(
                "{} {} the {} {} at {}. {}",
                self.who,
                self.action.verb(),
                kind_word.to_lowercase(),
                name(&self.scope_name),
                now_in(p.timezone.as_deref()),
                self.action.meaning(self.org_scope),
            );
            let mail = Mail::new(
                &p.email,
                &subject,
                &format!("{text}\n\nSign in to Aurora for details."),
                &self.link,
            );
            if let Err(e) = state.mailer.send(mail).await {
                tracing::error!(error = %e, "lifecycle notice e-mail send failed");
            }
            crate::domains::push::deliver::notify_lifecycle(
                state, push_kind, p.user_id, &text, &self.link,
            )
            .await;
        }
    }
}

/// The freeze hook: the freeze (created, or raised to a higher authority) just committed. Called
/// after the commit; a failing notice never fails the freeze. The agent scope has no notice of
/// its own — its people are told by the workspace's and organization's own changes.
pub async fn freeze_committed(state: &AppState, target: &Target, actor: &AuditActor) {
    scoped(state, target, actor, Action::Frozen).await;
}

/// The unfreeze hook: the freeze was actually lifted and the change committed.
pub async fn unfreeze_committed(state: &AppState, target: &Target, actor: &AuditActor) {
    scoped(state, target, actor, Action::Unfrozen).await;
}

async fn scoped(state: &AppState, target: &Target, actor: &AuditActor, action: Action) {
    use aurora_db::repo::freeze::Scope;
    let notice = match target.scope {
        Scope::Workspace => prepare_workspace(state, target.org_id, target.id, actor, action).await,
        Scope::Org => prepare_org(state, target.id, actor, action).await,
        Scope::Agent => None,
    };
    if let Some(n) = notice {
        n.send(state).await;
    }
}

/// The store's name as the dashboard shows it: the id, the built-in one by its plain name. The
/// ids are lowercase letters, digits, `_` or `-` — never a secret, an endpoint or a path.
fn store_name(id: &str) -> String {
    if id.is_empty() || id == "inline" {
        "the built-in store".to_owned()
    } else {
        name(id)
    }
}

/// (FIX-244, D-486) The default secret store changed — an instance-wide setting (there is no
/// per-organization default), so every organization's effective owners are told who changed it,
/// when, and from which store to which store. Called after the commit; no secret value, endpoint
/// or path appears; a failing send is logged and never fails the save.
pub async fn default_store_changed(state: &AppState, actor: &AuditActor, from: &str, to: &str) {
    let rows: Vec<(Uuid, String, Option<String>)> = match sqlx::query_as(
        "SELECT e.user_id, u.email::text, u.timezone \
         FROM effective_org_members e \
         JOIN users u ON u.id = e.user_id AND u.disabled_at IS NULL \
         WHERE e.role = 'owner' AND e.user_id IS DISTINCT FROM $1 \
         ORDER BY u.display_name, e.user_id",
    )
    .bind(actor.id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "default-store notice: reading the recipients failed");
            return;
        }
    };
    // A person owning several organizations gets one notice, not one per organization.
    let mut seen = std::collections::BTreeSet::new();
    let recipients: Vec<(Uuid, String, Option<String>)> = rows
        .into_iter()
        .filter(|(id, _, _)| seen.insert(*id))
        .collect();
    if recipients.is_empty() {
        return;
    }
    let who = actor_label(&state.pool, actor).await;
    let subject = "The default secret store changed".to_owned();
    for (user_id, email, timezone) in &recipients {
        let text = format!(
            "{} changed the instance's default secret store at {}. Managed values now go to {} \
             instead of {}.",
            who,
            now_in(timezone.as_deref()),
            store_name(to),
            store_name(from),
        );
        let mail = Mail::new(
            email,
            &subject,
            &format!("{text}\n\nSign in to Aurora for details."),
            state.config.external_url.trim_end_matches('/'),
        );
        if let Err(e) = state.mailer.send(mail).await {
            tracing::error!(error = %e, "default-store notice e-mail send failed");
        }
        crate::domains::push::deliver::notify_lifecycle(
            state,
            Kind::DefaultStore,
            *user_id,
            &text,
            state.config.external_url.trim_end_matches('/'),
        )
        .await;
    }
}

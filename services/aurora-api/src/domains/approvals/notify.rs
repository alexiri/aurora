//! Approval watchers and the e-mail notifier (phase 2, RFE-011a, D-56, docs/07 §Approvals).
//!
//! * An open `approvals/stream` (SSE) or `approvals/pending?wait=` long-poll registers a
//!   [`WatchGuard`]; the replica's heartbeat upserts `approval_watchers` rows for the workspaces
//!   its user may decide, which makes that user a *live delivery surface*.
//! * Every [`NOTIFY_SWEEP_EVERY`] the notifier looks for approvals still `pending`
//!   `APPROVAL_NOTIFY_DELAY` after creation with no live surface (no delivered channel card, no
//!   fresh watcher) and e-mails each decider once: sanitized title and a dashboard link only,
//!   never a decision link, never a request body or summary field.

use crate::app::AppState;
use crate::domains::auth::mail::Mail;
use aurora_core::limits::{
    APPROVAL_EMAIL_PER_USER_HOUR, APPROVAL_NOTIFY_DELAY, APPROVAL_WATCHER_TTL,
};
use aurora_core::{ApiError, sanitize};
use aurora_db::repo::approval_notify as repo;
use aurora_db::repo::approvals;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(15);
pub const NOTIFY_SWEEP_EVERY: Duration = Duration::from_secs(10);
/// Approvals handled per sweep.
const SWEEP_LIMIT: i64 = 100;
/// Watcher rows older than this are pruned (twice the liveness window).
const WATCHER_PRUNE_AFTER: Duration = Duration::from_secs(2 * APPROVAL_WATCHER_TTL.as_secs());
/// `approval_notifications` claims are kept this long.
pub const CLAIM_RETENTION_DAYS: i32 = 7;
const TITLE_MAX: usize = 120;

/// What an open connection watches: its user and the API-key confinement (sessions: none).
type WatchKey = (Uuid, Option<Uuid>, Option<Uuid>);

/// The open approvals connections of this replica, counted per user (and key scope).
pub struct WatchRegistry {
    replica_id: Uuid,
    open: Mutex<HashMap<WatchKey, usize>>,
}

impl Default for WatchRegistry {
    fn default() -> Self {
        Self {
            replica_id: Uuid::now_v7(),
            open: Mutex::default(),
        }
    }
}

/// Keeps one connection registered until dropped.
pub struct WatchGuard {
    registry: Arc<WatchRegistry>,
    key: WatchKey,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        let mut open = self.registry.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = open.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(&self.key);
            }
        }
    }
}

impl WatchRegistry {
    pub fn replica_id(&self) -> Uuid {
        self.replica_id
    }

    fn register(self: &Arc<Self>, key: WatchKey) -> WatchGuard {
        *self
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_default() += 1;
        WatchGuard {
            registry: Arc::clone(self),
            key,
        }
    }

    fn snapshot(&self) -> Vec<WatchKey> {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect()
    }
}

/// Register an open stream / long-poll of `user` and record it right away (best effort), so the
/// connection counts as a live surface before the next heartbeat.
pub async fn watch(s: &AppState, user: Uuid, org: Option<Uuid>, ws: Option<Uuid>) -> WatchGuard {
    let key = (user, org, ws);
    let guard = s.approval_watch.register(key);
    if let Err(e) = beat(s, &[key]).await {
        tracing::warn!(error = %e, "approval watcher registration failed");
    }
    guard
}

async fn beat(s: &AppState, keys: &[WatchKey]) -> Result<(), aurora_db::DbError> {
    let mut pairs = Vec::new();
    for (user, org, ws) in keys {
        for w in repo::watched_workspaces(&s.pool, *user, *org, *ws).await? {
            pairs.push((*user, w));
        }
    }
    repo::heartbeat_watchers(&s.pool, s.approval_watch.replica_id(), &pairs).await
}

/// One watcher heartbeat: upsert this replica's rows, prune stale ones.
pub async fn heartbeat_once(s: &AppState) -> Result<(), aurora_db::DbError> {
    let keys = s.approval_watch.snapshot();
    beat(s, &keys).await?;
    repo::prune_watchers(&s.pool, WATCHER_PRUNE_AFTER.as_secs_f64()).await?;
    Ok(())
}

fn clean(s: &str) -> String {
    sanitize::one_line(s, TITLE_MAX)
}

/// `<external>/o/<org>/w/<ws>/approvals?focus=<id>`: the dashboard approvals page, never a link
/// that decides anything (deciding needs a signed-in session).
pub fn dashboard_link(external_url: &str, u: &repo::Unseen) -> String {
    format!(
        "{}/o/{}/w/{}/approvals?focus={}",
        external_url.trim_end_matches('/'),
        u.org_slug,
        u.workspace_slug,
        u.id
    )
}

/// Platform-voice mail for one approval (plus `more` further approvals of its batch).
pub fn approval_mail(external_url: &str, to: &str, u: &repo::Unseen, more: usize) -> Mail {
    let title = clean(&u.title);
    let agent = clean(&u.agent_name);
    let workspace = clean(&u.workspace_name);
    let mut intro = format!(
        "{agent} in the workspace \"{workspace}\" is waiting for an approval.\n\nWhat: {title}"
    );
    if more > 0 {
        intro.push_str(&format!("\n(and {more} more in the same batch)"));
    }
    if let Some(rule) = u.rule_name.as_deref().filter(|r| !r.is_empty()) {
        intro.push_str(&format!("\nRule: {}", clean(rule)));
    }
    intro.push_str(&format!(
        "\nExpires: {}\n\nOpen the approvals page to review it. You need to be signed in to decide.",
        u.expires_at.format("%Y-%m-%d %H:%M UTC")
    ));
    Mail::new(
        to,
        &format!("Approval needed: {title}"),
        &intro,
        &dashboard_link(external_url, u),
    )
}

fn digest_mail(external_url: &str, to: &str, u: &repo::Unseen, waiting: usize) -> Mail {
    let n = waiting.max(1);
    let plural = if n == 1 {
        "approval is"
    } else {
        "approvals are"
    };
    Mail::new(
        to,
        &format!("{n} more approval(s) waiting"),
        &format!(
            "{n} more {plural} waiting for you in Aurora. To keep your inbox quiet they are not \
             e-mailed one by one.\n\nOpen the approvals page to review them. You need to be \
             signed in to decide."
        ),
        &format!(
            "{}/o/{}/w/{}/approvals",
            external_url.trim_end_matches('/'),
            u.org_slug,
            u.workspace_slug
        ),
    )
}

/// One notifier sweep; returns the number of e-mails handed to the mailer.
pub async fn notify_once(s: &AppState) -> Result<usize, ApiError> {
    // `approvals.email_notifications = false` switches the channel off instance-wide.
    if !s.config.approvals.email_notifications {
        return Ok(0);
    }
    let unseen = repo::unseen(
        &s.pool,
        APPROVAL_NOTIFY_DELAY.as_secs_f64(),
        APPROVAL_WATCHER_TTL.as_secs_f64(),
        SWEEP_LIMIT,
    )
    .await?;
    let ext = s.config.external_url.clone();
    let mut sent = 0usize;
    for u in &unseen {
        let deciders = repo::deciders(&s.pool, u.org_id, u.workspace_id, u.conversation_id).await?;
        for d in &deciders {
            // Approvals sharing a batch produce one e-mail per user.
            if let Some(b) = u.batch_id.as_deref()
                && repo::batch_served(&s.pool, d.user_id, u.agent_id, b).await?
            {
                repo::claim(&s.pool, u.id, d.user_id, &u.kind, "batch").await?;
                continue;
            }
            let (mails, digest_sent) = repo::hourly(&s.pool, d.user_id).await?;
            if mails < i64::from(APPROVAL_EMAIL_PER_USER_HOUR) {
                if !repo::claim(&s.pool, u.id, d.user_id, &u.kind, "mail").await? {
                    continue; // another replica won
                }
                let mut more = 0;
                if let Some(b) = u.batch_id.as_deref() {
                    for sib in repo::batch_siblings(&s.pool, u.agent_id, b, u.id).await? {
                        if repo::claim(&s.pool, sib, d.user_id, &u.kind, "batch").await? {
                            more += 1;
                        }
                    }
                }
                deliver(s, approval_mail(&ext, &d.email, u, more)).await;
                sent += 1;
            } else if digest_sent {
                repo::claim(&s.pool, u.id, d.user_id, &u.kind, "digest").await?;
            } else if repo::claim(&s.pool, u.id, d.user_id, &u.kind, "digest_mail").await? {
                let waiting = approvals::list_pending_for(&s.pool, d.user_id, None, None, 200)
                    .await?
                    .len();
                deliver(s, digest_mail(&ext, &d.email, u, waiting)).await;
                sent += 1;
            }
        }
    }
    Ok(sent)
}

async fn deliver(s: &AppState, mail: Mail) {
    if let Err(e) = s.mailer.send(mail).await {
        tracing::error!(error = %e, "approval e-mail send failed");
    }
}

/// Start the heartbeat and (unless `approvals.email_notifications = false`) the notifier.
pub fn spawn(state: AppState) {
    let s = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = heartbeat_once(&s).await {
                tracing::warn!(error = %e, "approval watcher heartbeat failed");
            }
        }
    });
    if !state.config.approvals.email_notifications {
        tracing::info!(
            "approval e-mail notifications are disabled (approvals.email_notifications)"
        );
        return;
    }
    if state.config.email.smtp_url.is_none() {
        tracing::warn!("approval e-mails disabled: email.smtp_url is unset, mails are only logged");
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(NOTIFY_SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match notify_once(&state).await {
                Ok(n) if n > 0 => tracing::info!(sent = n, "approval e-mail sweep"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "approval e-mail sweep failed"),
            }
        }
    });
}

//! What needs an instance admin's attention (FIX-167 Part B, D-370, C-1022..C-1024; docs/14
//! §Administration › Overview, docs/07 §E-mail): `GET /v1/admin/attention`, and the notice rows
//! plus the e-mail sweeper that tell the administrators once when a *decision* item first appears.
//!
//! Titles and details are fixed sentences filled with names and versions only (sanitized, one
//! line); no manifest body, no setting value, no secret. The same sentences feed the dashboard,
//! the e-mail and (when the device asked for names) the push.

use super::dto::{AttentionItem, AttentionKind, AttentionLevel, AttentionList};
use crate::app::AppState;
use crate::domains::auth::mail::Mail;
use crate::domains::connectors::manifest;
use aurora_core::{ApiError, limits, sanitize};
use aurora_db::repo::admin_attention as repo;
use aurora_db::repo::orgs;
use sqlx::PgConnection;
use std::time::Duration;

const NAME_MAX: usize = 80;
const DETAIL_MAX: usize = 300;
/// Notices handled per sweep.
const SWEEP_LIMIT: i64 = 100;
pub const SWEEP_EVERY: Duration = Duration::from_secs(10);
/// Notices are kept this long, then pruned.
pub const RETENTION_DAYS: i32 = 30;
/// A push for an item older than this is not sent any more (a replica that was down).
pub const PUSH_MAX_AGE: Duration = Duration::from_secs(3600);

fn name(s: &str) -> String {
    sanitize::one_line(s, NAME_MAX)
}

fn detail(s: &str) -> String {
    sanitize::one_line(s, DETAIL_MAX)
}

fn versioned(n: &str, version: Option<&str>) -> String {
    match version.map(name).filter(|v| !v.is_empty()) {
        Some(v) => format!("{} {v}", name(n)),
        None => name(n),
    }
}

// ---------------------------------------------------------------------------------------------
// The sentences (docs/14 §Needs your attention)
// ---------------------------------------------------------------------------------------------

/// A connector's first registration (`update = false`) or a widened update that waits.
pub fn connector_text(
    display_name: &str,
    version: Option<&str>,
    update: bool,
    widening: &[String],
) -> (String, String) {
    let what = if widening.is_empty() {
        String::new()
    } else {
        format!(
            " It adds: {}.",
            widening
                .iter()
                .map(|w| name(w))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if update {
        (
            format!(
                "{} is waiting for approval",
                versioned(display_name, version)
            ),
            detail(&format!(
                "It keeps running at the level you approved; only what is new waits.{what}"
            )),
        )
    } else {
        (
            format!("{} needs your approval", name(display_name)),
            // The widening of a first registration is only "awaiting first approval": no list.
            "This connector registered for the first time and does nothing until you approve it."
                .to_owned(),
        )
    }
}

/// The detail of a held package version, by its reason.
fn package_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("first_install") => {
            "A new app package. Review what it asks for before it is switched on."
        }
        Some("addon_update") => {
            "An update from an add-on source. Review it before it is switched on."
        }
        Some("permissions_widened") => {
            "It asks for more than the approved version. The approved version keeps running."
        }
        Some("publisher_changed") => "Its publisher differs from the approved version.",
        Some("builtin_id_reserved") => "It uses the id of a built-in app package.",
        Some("version_downgrade") => "It is older than the approved version.",
        Some("breaks_connections") => {
            "Switching it on would make some connections sign in again. The approved version keeps running."
        }
        Some("converts_grants") => {
            "Switching it on would convert some agents' full access grants to custom ones. The approved version keeps running."
        }
        _ => "It waits for an instance admin.",
    }
}

pub fn package_text(package_name: &str, version: &str, reason: Option<&str>) -> (String, String) {
    if reason == Some("claim_conflict") {
        return (
            format!(
                "{} conflicts with another app package",
                versioned(package_name, Some(version))
            ),
            "Two app packages claim the same host. One of them waits until you choose which wins."
                .to_owned(),
        );
    }
    (
        format!(
            "{} needs your approval",
            versioned(package_name, Some(version))
        ),
        package_reason(reason).to_owned(),
    )
}

pub fn key_changed_text(source_name: &str) -> (String, String) {
    (
        format!("{}: signing key changed", name(source_name)),
        "The source now signs with a different key. Nothing from it is ingested until you confirm the new key."
            .to_owned(),
    )
}

fn down_text(display_name: &str, errored: bool) -> (String, String) {
    (
        format!("{} is not running", name(display_name)),
        if errored {
            "Its connector host reports an error.".to_owned()
        } else {
            format!(
                "No connector host has reported it for more than {} seconds.",
                limits::CONNECTOR_HOST_STALE.as_secs()
            )
        },
    )
}

fn ownerless_text(slug: &str, members: i64) -> (String, String) {
    (
        format!("The organization {} has no owner", name(slug)),
        format!(
            "Its {members} active member(s) cannot use the gateway until someone is made an owner."
        ),
    )
}

/// The dashboard page that resolves a package or source item (the names of `web/src/lib/routes.ts`).
pub fn package_href(package_row_id: &str, conflict: bool) -> String {
    if conflict {
        "/admin/apps?tab=advanced#host-conflicts".to_owned()
    } else {
        format!("/admin/apps/packages/{package_row_id}")
    }
}

// ---------------------------------------------------------------------------------------------
// Notices (written in the transaction of the state change)
// ---------------------------------------------------------------------------------------------

/// A connector reached `pending_approval` (first registration) or holds a new widened update.
/// `digest` is the announced manifest's digest: a new digest is a new subject.
pub async fn notify_connector(
    conn: &mut PgConnection,
    id: &str,
    display_name: &str,
    version: Option<&str>,
    digest: &str,
    update: bool,
    widening: &[String],
) -> Result<(), aurora_db::DbError> {
    let (t, d) = connector_text(display_name, version, update, widening);
    repo::record_notices(conn, "connector_approval", id, digest, &t, &d).await?;
    Ok(())
}

/// A package version reached `pending_approval`. A claim conflict is a `host_claim_conflict`.
pub async fn notify_package(
    conn: &mut PgConnection,
    row_id: uuid::Uuid,
    package_name: &str,
    version: &str,
    reason: Option<&str>,
) -> Result<(), aurora_db::DbError> {
    let (t, d) = package_text(package_name, version, reason);
    let kind = if reason == Some("claim_conflict") {
        "host_claim_conflict"
    } else {
        "package_approval"
    };
    repo::record_notices(conn, kind, &row_id.to_string(), version, &t, &d).await?;
    Ok(())
}

/// An addon source's signing key changed (`fingerprint` is the new one).
pub async fn notify_key_changed(
    conn: &mut PgConnection,
    source_id: uuid::Uuid,
    source_name: &str,
    fingerprint: &str,
) -> Result<(), aurora_db::DbError> {
    let (t, d) = key_changed_text(source_name);
    repo::record_notices(
        conn,
        "addon_key_changed",
        &source_id.to_string(),
        fingerprint,
        &t,
        &d,
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// GET /v1/admin/attention (C-1022)
// ---------------------------------------------------------------------------------------------

/// (P5-REFINE-8, C-1080) The rail topic of a C-1022 kind.
fn topic_of(kind: AttentionKind) -> &'static str {
    use crate::domains::attention::service::{TOPIC_APPS, TOPIC_CHAT, TOPIC_PEOPLE};
    match kind {
        AttentionKind::ConnectorApproval | AttentionKind::ConnectorDown => TOPIC_CHAT,
        AttentionKind::OwnerlessOrg => TOPIC_PEOPLE,
        _ => TOPIC_APPS,
    }
}

fn item(
    kind: AttentionKind,
    subject_id: String,
    text: (String, String),
    since: chrono::DateTime<chrono::Utc>,
    href: String,
) -> AttentionItem {
    // (C-1080) "Review" for a decision, "Open" for an item that only informs; the href is C-1022's.
    let label = if kind.is_decision() { "Review" } else { "Open" };
    AttentionItem::new(
        kind,
        AttentionLevel::Administration,
        topic_of(kind),
        subject_id,
        text,
        since,
        href.clone(),
    )
    .action(label, href)
}

/// `GET /v1/admin/attention` (C-1022, C-1080): open, the six kinds and the requests of the
/// Administration level; `done`, the requests closed in the last 30 days (the six kinds have no
/// "closed" record, C-1080's corrections).
pub async fn list(s: &AppState, done: bool) -> Result<AttentionList, ApiError> {
    let mut items = if done {
        Vec::new()
    } else {
        open_items(s).await?
    };
    items.extend(crate::domains::attention::service::admin_request_items(s, done).await?);
    Ok(crate::domains::attention::service::list_of(items, done))
}

/// The six C-1022 kinds, computed on read from existing tables: decisions first (oldest first within a
/// kind group), then the items that are only informative.
pub async fn open_items(s: &AppState) -> Result<Vec<AttentionItem>, ApiError> {
    let mut items = Vec::new();
    for c in repo::connector_candidates(&s.pool).await? {
        let stored = c.manifest.as_ref();
        let update = stored.is_some_and(manifest::update_pending);
        if c.status != "pending_approval" && !update {
            continue;
        }
        let widening = stored.map(manifest::stored_widening).unwrap_or_default();
        items.push(item(
            AttentionKind::ConnectorApproval,
            c.id.clone(),
            connector_text(&c.display_name, c.version.as_deref(), update, &widening),
            c.since,
            "/admin/chat-platforms".to_owned(),
        ));
    }
    for p in repo::held_packages(&s.pool).await? {
        let conflict = p.status_reason.as_deref() == Some("claim_conflict");
        items.push(item(
            if conflict {
                AttentionKind::HostClaimConflict
            } else {
                AttentionKind::PackageApproval
            },
            p.id.to_string(),
            package_text(&p.name, &p.version, p.status_reason.as_deref()),
            p.since,
            package_href(&p.id.to_string(), conflict),
        ));
    }
    for k in repo::key_changed_sources(&s.pool).await? {
        items.push(item(
            AttentionKind::AddonKeyChanged,
            k.id.to_string(),
            key_changed_text(&k.name),
            k.since,
            format!("/admin/apps?tab=advanced#{}", k.id),
        ));
    }
    // Decisions first, oldest first; the stable sort keeps the kind order within a tie.
    items.sort_by_key(|i| (!i.decision, i.since));
    let mut info = Vec::new();
    for d in repo::down_connectors(&s.pool, limits::CONNECTOR_HOST_STALE.as_secs_f64()).await? {
        info.push(item(
            AttentionKind::ConnectorDown,
            d.id,
            down_text(&d.display_name, d.errored),
            d.since,
            "/admin/chat-platforms".to_owned(),
        ));
    }
    for o in orgs::list_ownerless(&s.pool).await? {
        info.push(item(
            AttentionKind::OwnerlessOrg,
            o.slug.clone(),
            ownerless_text(&o.slug, o.members),
            o.created_at,
            "/admin/people?tab=organizations".to_owned(),
        ));
    }
    info.sort_by_key(|i| i.since);
    items.extend(info);
    Ok(items)
}

// ---------------------------------------------------------------------------------------------
// The e-mail sweeper (07 §E-mail)
// ---------------------------------------------------------------------------------------------

/// The e-mail of one decision item. Names the item and what it adds; links `/admin`; never a
/// secret or a setting value.
pub fn notice_mail(external_url: &str, to: &str, title: &str, detail: &str) -> Mail {
    Mail::new(
        to,
        &format!("Aurora needs your decision: {title}"),
        &format!(
            "{title}\n\n{detail}\n\nOpen Administration to review it. You need to be signed in to decide."
        ),
        &format!("{}/admin", external_url.trim_end_matches('/')),
    )
}

/// One sweep: claim the open e-mail halves, send to administrators who did not opt out (an
/// opted-out or no-longer-admin recipient is marked without a send). Returns the mails handed to
/// the mailer. With SMTP unset the mailer logs only.
pub async fn notify_once(s: &AppState) -> Result<usize, ApiError> {
    let ext = s.config.external_url.clone();
    let mut sent = 0;
    for n in repo::claim_mail(&s.pool, SWEEP_LIMIT).await? {
        if !n.wants_email {
            continue;
        }
        if let Err(e) = s
            .mailer
            .send(notice_mail(&ext, &n.email, &n.title, &n.detail))
            .await
        {
            tracing::error!(error = %e, "administration attention e-mail send failed");
        } else {
            sent += 1;
        }
    }
    Ok(sent)
}

pub fn spawn(state: AppState) {
    if !state.auth().smtp {
        tracing::warn!(
            "administration attention e-mails: email.smtp_url is unset, mails are only logged"
        );
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sweeps = 0u32;
        loop {
            tick.tick().await;
            // Hourly: a sent or abandoned notice is history after `RETENTION_DAYS`.
            if sweeps.is_multiple_of(360)
                && let Err(e) = repo::prune(&state.pool, RETENTION_DAYS).await
            {
                tracing::warn!(error = %e, "administration attention notices could not be pruned");
            }
            sweeps = sweeps.wrapping_add(1);
            match notify_once(&state).await {
                Ok(n) if n > 0 => tracing::info!(sent = n, "administration attention e-mails"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "attention e-mail sweep failed"),
            }
        }
    });
}

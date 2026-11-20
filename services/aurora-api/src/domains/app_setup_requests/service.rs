//! App setup requests (P5-POLISH-4, D-386, C-1030, SEC-135).
//!
//! A person who cannot connect an app because it needs an organization owner or admin first (no sign-in
//! app, no server, switched off) asks them. The request is a record and a notice: it changes no
//! availability, OAuth client or access, and it is visible only to the people who can resolve it at its
//! level (C-1080: the organization's owners and admins, or instance admins) and to the asker (their own
//! ask). It closes by itself when the app becomes ready for its askers ([`reevaluate`]), when the last
//! asker withdraws, or when an admin of its level declines it.
//!
//! (P5-REFINE-8, C-1081, SEC-135 amended) The two free texts — the asker's `reason` and the decliner's
//! `message` — go through [`free_text`]: control and invisible formatting characters removed, line breaks
//! kept, at most [`FREE_TEXT_MAX`] characters (400 above). They are shown to the people C-1081 names,
//! quoted in a notice only as its `detail` (never its title), with links defused for mail and push, and
//! never given to an agent, put in a prompt or audited.

use super::dto::{
    AppSetupRequest, Asker, ClosedBy, ClosedByRole, NewRequest, RequestLevel, Requested,
};
use super::level;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::apps::service as apps;
use crate::domains::apps::setup::{self, Facts, ReadinessState, SetupNeed, SetupView};
use crate::domains::auth::mail::Mail;
use crate::http::auth::{Caller, require_own};
use aurora_core::{ApiError, dash_paths, ids, sanitize};
use aurora_db::DbError;
use aurora_db::repo::app_setup_requests::{self as repo, Closing, RequestRow};
use aurora_db::repo::contacts;
use chrono::Utc;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// Notices handled per sweep.
const SWEEP_LIMIT: i64 = 100;
/// The periodic re-evaluation and mail sweep (heals a missed trigger).
pub const SWEEP_EVERY: Duration = Duration::from_secs(600);
/// Closed requests are purged after this many days (03 §app_setup_requests).
pub const RETENTION_DAYS: i32 = 90;
/// (P5-REFINE-8, C-1080/C-1081) Closed requests are *shown* (history, `state=done`) for this many days.
pub const REQUEST_HISTORY_DAYS: i32 = 30;
/// A push for a request older than this is noise.
pub const PUSH_MAX_AGE: Duration = Duration::from_secs(3600);
/// (C-1081) The most characters a reason or a decline message holds.
pub const FREE_TEXT_MAX: usize = 300;
/// (C-1081) The most characters of a reason or a message a notice (mail, push) quotes.
pub const NOTICE_QUOTE_MAX: usize = 140;
/// The most rows a history list returns.
const HISTORY_LIMIT: i64 = 200;

fn need_str(n: SetupNeed) -> &'static str {
    match n {
        SetupNeed::OauthClient => "oauth_client",
        SetupNeed::Instance => "instance",
        SetupNeed::AdminMaterial => "admin_material",
    }
}

/// (C-1081, SEC-135 amended) A reason or a decline message: `None` when absent or empty after cleaning;
/// control and invisible formatting characters (`sanitize::is_stripped`) removed, runs of spaces
/// collapsed, line breaks kept (at most two in a row), trimmed; 400 `invalid_request` at `field` when it
/// is longer than [`FREE_TEXT_MAX`] characters.
pub fn free_text(field: &str, raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let clean = sanitize::block(raw, usize::MAX);
    let clean = clean.trim();
    if clean.is_empty() {
        return Ok(None);
    }
    if clean.chars().count() > FREE_TEXT_MAX {
        return Err(ApiError::invalid_field(
            "Invalid request.",
            field,
            format!("at most {FREE_TEXT_MAX} characters"),
        ));
    }
    Ok(Some(clean.to_owned()))
}

/// (C-1081) A free text quoted in a notice's detail (mail and push): one line, the first
/// [`NOTICE_QUOTE_MAX`] characters, and written so no mail client or push turns an address into a link.
pub fn quote(text: &str) -> String {
    sanitize::break_preview_urls(&sanitize::one_line(text, NOTICE_QUOTE_MAX))
}

/// Who reads a request (C-1081): what they may see of the askers and the decliner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    /// The asker: only their own ask, never the decliner's name.
    Asker(Uuid),
    /// The organization's owners and admins: every ask; the decliner when an owner or admin declined.
    OrgAdmin,
    /// Instance admins (Administration): every ask and the decliner.
    InstanceAdmin,
}

pub(crate) fn needs_of(v: &serde_json::Value) -> Vec<SetupNeed> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| match x.as_str()? {
            "oauth_client" => Some(SetupNeed::OauthClient),
            "instance" => Some(SetupNeed::Instance),
            "admin_material" => Some(SetupNeed::AdminMaterial),
            _ => None,
        })
        .collect()
}

const SUBJECT_APP: &str = "app";
const SUBJECT_CHAT: &str = "chat_platform";
const SUBJECT_KEY: &str = "ai_key";

/// "A chat platform", or the connector type id named for people (first letter capitalised).
fn chat_label(connector: &str) -> String {
    let mut c = connector.chars();
    match c.next() {
        None => "A chat platform".to_owned(),
        Some(f) => format!("{}{}", f.to_uppercase(), c.as_str()),
    }
}

fn name(s: &str) -> String {
    sanitize::one_line(s, 80)
}

/// An app's name from a snapshot read once (a list reads the registry snapshot once, not per row).
pub(crate) fn app_name_in(
    snap: Option<&aurora_registry::snapshot::Snapshot>,
    app_id: &str,
) -> String {
    snap.and_then(|s| apps::find_app(s, app_id).ok())
        .map_or_else(
            || sanitize::label(app_id),
            |(a, _)| sanitize::label(&a.about.name),
        )
}

/// The registry snapshot when some of `rows` are apps (one read per list).
pub(crate) async fn snapshot_for(
    state: &AppState,
    rows: &[RequestRow],
) -> Option<aurora_registry::snapshot::Snapshot> {
    if rows.iter().any(|r| r.subject == SUBJECT_APP) {
        apps::snapshot(state).await.ok().flatten()
    } else {
        None
    }
}

/// One sentence of what is needed (SEC-135: fixed words and the app name only).
fn need_sentence(app: &str, needs: &[SetupNeed], unavailable: bool) -> String {
    if unavailable {
        return format!("{app} is switched off for the organization; it needs to be switched on.");
    }
    match needs.first() {
        Some(SetupNeed::OauthClient) => format!(
            "A sign-in app for {app} needs to be registered once; it then works for everyone."
        ),
        Some(SetupNeed::Instance) => {
            format!("A {app} server needs to be registered for the organization.")
        }
        Some(SetupNeed::AdminMaterial) | None => {
            format!("{app} needs set-up that only an owner or admin can do.")
        }
    }
}

struct Names {
    org_slug: String,
    org_name: String,
}

async fn org_names(state: &AppState, org: Uuid) -> Result<Names, ApiError> {
    let (org_slug, _) = contacts::slugs(&state.pool, org, None)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    let org_name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(org)
        .fetch_one(&state.pool)
        .await
        .map_err(DbError::from)?;
    Ok(Names { org_slug, org_name })
}

/// The label of a request's thing in a sentence: the app's name, "a chat platform" or the connector's
/// name, "an AI key".
pub(crate) fn thing_in(
    snap: Option<&aurora_registry::snapshot::Snapshot>,
    subject: &str,
    key: &str,
) -> String {
    match subject {
        SUBJECT_CHAT if key.is_empty() => "a chat platform".to_owned(),
        SUBJECT_CHAT => chat_label(key),
        SUBJECT_KEY => "an AI key".to_owned(),
        _ => app_name_in(snap, key),
    }
}

/// "\n\nThey wrote: …" for a notice's detail (C-1081), or nothing.
fn they_wrote(text: Option<&str>) -> String {
    text.map_or_else(String::new, |t| format!("\n\nThey wrote: {}", quote(t)))
}

async fn to_dto(
    state: &AppState,
    rows: Vec<RequestRow>,
    viewer: Viewer,
) -> Result<Vec<AppSetupRequest>, ApiError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let askers = repo::askers(&state.pool, &ids).await?;
    let only = match viewer {
        Viewer::Asker(u) => Some(u),
        _ => None,
    };
    let snap = snapshot_for(state, &rows).await;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let (app_id, connector_type, label) = match r.subject.as_str() {
            SUBJECT_CHAT => (
                String::new(),
                Some(r.app_id.clone()).filter(|t| !t.is_empty()),
                chat_label(&r.app_id),
            ),
            SUBJECT_KEY => (String::new(), None, "An AI key".to_owned()),
            _ => (
                r.app_id.clone(),
                None,
                app_name_in(snap.as_ref(), &r.app_id),
            ),
        };
        let role = r.closed_by_role.as_deref().and_then(ClosedByRole::parse);
        // The decliner's name: never for the asker; for the organization only when one of its owners or
        // admins declined (C-981: an instance admin is not named outside Administration).
        let closed_by = match (viewer, role) {
            (Viewer::InstanceAdmin, Some(ClosedByRole::OrgAdmin | ClosedByRole::InstanceAdmin))
            | (Viewer::OrgAdmin, Some(ClosedByRole::OrgAdmin)) => {
                r.closed_by_name.as_deref().map(|n| ClosedBy {
                    display_name: name(n),
                })
            }
            _ => None,
        };
        out.push(AppSetupRequest {
            id: r.id,
            org_id: r.org_id,
            level: level::of(&r),
            subject: r.subject,
            connector_type,
            app_name: label,
            app_id,
            status: r.status,
            askers: askers
                .iter()
                .filter(|a| a.request_id == r.id && only.is_none_or(|u| u == a.user_id))
                .map(|a| Asker {
                    user_id: a.user_id,
                    display_name: a.display_name.clone(),
                    workspace_id: a.workspace_id,
                    asked_at: a.asked_at,
                    reason: a.reason.clone(),
                })
                .collect(),
            needs: needs_of(&r.needs),
            unavailable: r.unavailable,
            created_at: r.created_at,
            closed_at: r.closed_at,
            message: r.closed_message,
            closed_by_role: role,
            closed_by,
        });
    }
    Ok(out)
}

fn audit_event(
    actor: AuditActor,
    action: &'static str,
    org: Uuid,
    ws: Option<Uuid>,
    subject: &str,
    app: &str,
    request: Uuid,
) -> AuditEvent {
    audit_event_with(
        actor,
        action,
        org,
        ws,
        subject,
        app,
        request,
        serde_json::json!({}),
    )
}

/// [`audit_event`] with more metadata (C-1081: `has_reason`, `has_message`, `closed_by_role`; never the
/// text itself).
#[allow(clippy::too_many_arguments)]
fn audit_event_with(
    actor: AuditActor,
    action: &'static str,
    org: Uuid,
    ws: Option<Uuid>,
    subject: &str,
    app: &str,
    request: Uuid,
    extra: serde_json::Value,
) -> AuditEvent {
    let target = if app.is_empty() {
        request.to_string()
    } else {
        app.to_owned()
    };
    let mut meta = match subject {
        SUBJECT_APP => serde_json::json!({ "app_id": app, "request_id": request }),
        SUBJECT_CHAT => serde_json::json!({ "subject": subject,
            "connector_type": (!app.is_empty()).then_some(app), "request_id": request }),
        _ => serde_json::json!({ "subject": subject, "request_id": request }),
    };
    if let (Some(m), serde_json::Value::Object(x)) = (meta.as_object_mut(), extra) {
        m.extend(x);
    }
    let ev = AuditEvent::new(actor, action, Outcome::Success)
        .org(org)
        .target(subject, &target)
        .metadata(meta);
    match ws {
        Some(w) => ev.workspace(w),
        None => ev,
    }
}

/// `POST /workspaces/{ws}/apps/{app}/setup-request`: asks the organization's owners and admins to make
/// `app_id` ready. Idempotent per person; (C-1081) a new `reason` replaces the person's.
pub async fn ask(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    app_id: &str,
    reason: Option<&str>,
) -> Result<AppSetupRequest, ApiError> {
    let reason = free_text("/reason", reason)?;
    let auth = caller.workspace(state, ws).await?;
    require_own(&auth)?;
    let org = auth.org_id;
    let snap = apps::snapshot(state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let (app, pkg) = apps::find_app(&snap, app_id)?;
    let available =
        crate::domains::connect::service::available_in_workspace(state, org, ws, app_id).await?;
    let org_admin = auth.org_role.is_admin();
    // An app switched off is not visible to a person who cannot switch it on.
    if !available && !org_admin {
        return Err(ApiError::not_found("App not found."));
    }
    let names = org_names(state, org).await?;
    let view = SetupView {
        org,
        workspace: Some(ws),
        org_admin,
    };
    let mut facts = Facts::load(state, view, names.org_slug.clone(), Some(app_id)).await?;
    let readiness = setup::readiness(&mut facts, app, pkg, available).await;
    if readiness.state == ReadinessState::Ready {
        return Err(ApiError::conflict(
            "app_ready",
            "This app is ready: connect it from the journey.",
        ));
    }
    let needs: Vec<&str> = readiness.needs.iter().map(|n| need_str(*n)).collect();
    let needs_json = serde_json::json!(needs);
    let unavailable = !available;
    let app_label = sanitize::label(&app.about.name);
    let asker =
        name(&aurora_db::repo::app_setup_requests::asker_name(&state.pool, caller.user_id).await?);

    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let mut req = repo::open_for_update(&mut *tx, org, SUBJECT_APP, app_id).await?;
    let mut opened = false;
    if req.is_none() {
        opened = repo::insert(
            &mut *tx,
            ids::new_id(),
            org,
            SUBJECT_APP,
            app_id,
            &needs_json,
            unavailable,
        )
        .await?;
        req = repo::open_for_update(&mut *tx, org, SUBJECT_APP, app_id).await?;
    }
    let req = req.ok_or_else(|| ApiError::internal("The request vanished."))?;
    repo::set_needs(&mut *tx, req.id, &needs_json, unavailable).await?;
    let new_asker =
        repo::add_asker(&mut *tx, req.id, caller.user_id, ws, reason.as_deref()).await?;
    let sentence = need_sentence(&app_label, &readiness.needs, unavailable);
    let href = format!(
        "{}&app={app_id}",
        dash_paths::org_apps(&names.org_slug, "apps")
    );
    if opened || new_asker {
        let mut kind = "asked".to_owned();
        let (title, detail) = if opened {
            (
                format!("{asker} wants {app_label} in {}", name(&names.org_name)),
                format!(
                    "{asker} asked for {app_label}.{}\n\n{sentence}",
                    they_wrote(reason.as_deref())
                ),
            )
        } else {
            // Others joined an open request: one digest per hour (the hour is part of the claim).
            kind = format!("more:{}", Utc::now().format("%Y%m%d%H"));
            let who = repo::askers(&mut *tx, &[req.id]).await?;
            let list: Vec<String> = who.iter().take(10).map(|a| name(&a.display_name)).collect();
            (
                format!(
                    "{asker} also wants {app_label} in {}",
                    name(&names.org_name)
                ),
                format!(
                    "{} now want {app_label}: {}.{}\n\n{sentence}",
                    who.len(),
                    list.join(", "),
                    they_wrote(reason.as_deref())
                ),
            )
        };
        for (admin, _) in repo::org_admins(&mut *tx, org).await? {
            repo::insert_notice(&mut *tx, req.id, admin, &kind, &title, &detail, &href).await?;
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    if new_asker {
        audit::record(
            state,
            audit_event_with(
                AuditActor::from_caller(caller),
                "app_setup_request.ask",
                org,
                Some(ws),
                SUBJECT_APP,
                app_id,
                req.id,
                serde_json::json!({ "has_reason": reason.is_some() }),
            ),
        );
    }
    let row = repo::open_for_update(&state.pool, org, SUBJECT_APP, app_id)
        .await?
        .unwrap_or(req);
    // The asker sees only their own ask (an owner or admin sees them all).
    let viewer = if org_admin {
        Viewer::OrgAdmin
    } else {
        Viewer::Asker(caller.user_id)
    };
    to_dto(state, vec![row], viewer)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("The request vanished."))
}

/// `DELETE /workspaces/{ws}/apps/{app}/setup-request`: withdraws the caller's ask.
pub async fn withdraw(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    app_id: &str,
) -> Result<(), ApiError> {
    let auth = caller.workspace(state, ws).await?;
    withdraw_row(state, caller, ws, auth.org_id, SUBJECT_APP, app_id).await
}

async fn withdraw_row(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    org: Uuid,
    subject: &str,
    key: &str,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if let Some(req) = repo::open_for_update(&mut *tx, org, subject, key).await?
        && let Some(left) = repo::remove_asker(&mut *tx, req.id, caller.user_id).await?
    {
        if left == 0 {
            repo::close(
                &mut *tx,
                req.id,
                Closing {
                    status: "withdrawn",
                    by: None,
                    role: None,
                    message: None,
                },
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        audit::record(
            state,
            audit_event(
                AuditActor::from_caller(caller),
                "app_setup_request.withdraw",
                org,
                Some(ws),
                subject,
                key,
                req.id,
            ),
        );
    }
    Ok(())
}

/// `DELETE /workspaces/{ws}/requests/{id}` (C-1047): withdraws the caller's ask of any subject. An unknown
/// request, or one the caller did not ask, changes nothing and reveals nothing.
pub async fn withdraw_by_id(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    id: Uuid,
) -> Result<(), ApiError> {
    let auth = caller.workspace(state, ws).await?;
    if let Some(req) = repo::get(&state.pool, id)
        .await?
        .filter(|r| r.org_id == auth.org_id && r.status == "open")
    {
        withdraw_row(state, caller, ws, auth.org_id, &req.subject, &req.app_id).await?;
    }
    Ok(())
}

/// `GET /workspaces/{ws}/requests` (C-1047): the caller's own open requests, each with only their ask.
/// (C-1081) `status=history`: their own requests that ended in the last 30 days — declined with their
/// ask on it, or resolved for them (the request may still be open for others: it reads `resolved` with
/// the time it ended for them) — with the message and the role, never the admin's name.
pub async fn list_mine(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    status: Option<&str>,
) -> Result<Vec<AppSetupRequest>, ApiError> {
    let history = open_or_history(status)?;
    let auth = caller.workspace(state, ws).await?;
    let me = Viewer::Asker(caller.user_id);
    if !history {
        let rows = repo::open_of_asker(&state.pool, auth.org_id, caller.user_id).await?;
        return to_dto(state, rows, me).await;
    }
    let done = repo::history_of_asker(
        &state.pool,
        auth.org_id,
        caller.user_id,
        REQUEST_HISTORY_DAYS,
        HISTORY_LIMIT,
    )
    .await?;
    let rows = done
        .into_iter()
        .map(|d| {
            let mut row = d.row;
            if d.resolved_for_me {
                row.status = "resolved".to_owned();
                row.closed_by_role = Some(ClosedByRole::System.as_str().to_owned());
                row.closed_message = None;
            }
            row.closed_at = Some(d.done_at);
            row
        })
        .collect();
    to_dto(state, rows, me).await
}

/// `open` (default) or `history` (C-1081); 400 for anything else.
pub(crate) fn open_or_history(status: Option<&str>) -> Result<bool, ApiError> {
    match status.unwrap_or("open") {
        "open" => Ok(false),
        "history" => Ok(true),
        _ => Err(ApiError::invalid_field(
            "Unknown status.",
            "/status",
            "open or history",
        )),
    }
}

/// `POST /workspaces/{ws}/requests` (C-1047): the setup request with a subject.
pub async fn ask_subject(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    body: NewRequest,
) -> Result<AppSetupRequest, ApiError> {
    let bad = |m: &str, f: &str| {
        ApiError::invalid_request("Invalid request.", vec![aurora_core::FieldError::new(f, m)])
    };
    let reason = free_text("/reason", body.reason.as_deref())?;
    let (subject, key) = match body.subject.as_str() {
        SUBJECT_APP => {
            let app = body
                .app_id
                .filter(|a| !a.is_empty())
                .ok_or_else(|| bad("required for an app", "/app_id"))?;
            return ask(state, caller, ws, &app, reason.as_deref()).await;
        }
        SUBJECT_CHAT => {
            let t = body.connector_type.unwrap_or_default();
            let ok = t.is_empty()
                || (t.len() <= 32
                    && t.starts_with(|c: char| c.is_ascii_lowercase())
                    && t.len() >= 2
                    && t.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            if !ok {
                return Err(bad(
                    "a connector type id: lowercase letters, digits and dashes",
                    "/connector_type",
                ));
            }
            (SUBJECT_CHAT, t)
        }
        SUBJECT_KEY => (SUBJECT_KEY, String::new()),
        _ => return Err(bad("app, chat_platform or ai_key", "/subject")),
    };
    let auth = caller.workspace(state, ws).await?;
    let org = auth.org_id;
    let connector = Some(key.as_str()).filter(|t| !t.is_empty());
    let exists = if subject == SUBJECT_CHAT {
        repo::has_chat_platform(&state.pool, org, connector).await?
    } else {
        repo::has_ai_key(&state.pool, org, ws).await?
    };
    if exists {
        return Err(ApiError::conflict(
            "already_available",
            "This is already set up: it is ready to use.",
        ));
    }
    let names = org_names(state, org).await?;
    let label = if subject == SUBJECT_CHAT {
        if key.is_empty() {
            "a chat platform".to_owned()
        } else {
            chat_label(&key)
        }
    } else {
        "an AI key".to_owned()
    };
    let asker = name(&repo::asker_name(&state.pool, caller.user_id).await?);
    let org_name = name(&names.org_name);
    let to_instance =
        subject == SUBJECT_CHAT && !repo::has_active_connector(&state.pool, connector).await?;

    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let none = serde_json::json!([]);
    let mut req = repo::open_for_update(&mut *tx, org, subject, &key).await?;
    let mut opened = false;
    if req.is_none() {
        opened = repo::insert(&mut *tx, ids::new_id(), org, subject, &key, &none, false).await?;
        req = repo::open_for_update(&mut *tx, org, subject, &key).await?;
    }
    let req = req.ok_or_else(|| ApiError::internal("The request vanished."))?;
    let new_asker =
        repo::add_asker(&mut *tx, req.id, caller.user_id, ws, reason.as_deref()).await?;
    if opened || new_asker {
        let (kind, title, detail) = if opened {
            (
                "asked".to_owned(),
                format!("{asker} wants {label} in {org_name}"),
                format!(
                    "{asker} asked for {label}.{}\n\n{}",
                    they_wrote(reason.as_deref()),
                    fix_sentence(subject)
                ),
            )
        } else {
            let who = repo::askers(&mut *tx, &[req.id]).await?;
            let list: Vec<String> = who.iter().take(10).map(|a| name(&a.display_name)).collect();
            (
                format!("more:{}", Utc::now().format("%Y%m%d%H")),
                format!("{asker} also wants {label} in {org_name}"),
                format!(
                    "{} now want {label}: {}.{}\n\n{}",
                    who.len(),
                    list.join(", "),
                    they_wrote(reason.as_deref()),
                    fix_sentence(subject)
                ),
            )
        };
        let admin_href = if subject == SUBJECT_CHAT {
            org_href(&names.org_slug, "chat-platforms")
        } else {
            dash_paths::org_ai_models(&names.org_slug)
        };
        for (admin, _) in repo::org_admins(&mut *tx, org).await? {
            repo::insert_notice(&mut *tx, req.id, admin, &kind, &title, &detail, &admin_href)
                .await?;
        }
        if to_instance {
            for admin in repo::instance_admins(&mut *tx).await? {
                repo::insert_notice(
                    &mut *tx,
                    req.id,
                    admin,
                    &kind,
                    &title,
                    &detail,
                    "/admin/chat-platforms",
                )
                .await?;
            }
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    if new_asker {
        audit::record(
            state,
            audit_event_with(
                AuditActor::from_caller(caller),
                "app_setup_request.ask",
                org,
                Some(ws),
                subject,
                &key,
                req.id,
                serde_json::json!({ "has_reason": reason.is_some() }),
            ),
        );
    }
    let row = repo::open_for_update(&state.pool, org, subject, &key)
        .await?
        .unwrap_or(req);
    let viewer = if auth.org_role.is_admin() {
        Viewer::OrgAdmin
    } else {
        Viewer::Asker(caller.user_id)
    };
    to_dto(state, vec![row], viewer)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("The request vanished."))
}

fn org_href(org_slug: &str, page: &str) -> String {
    format!("/o/{org_slug}/{page}")
}

fn fix_sentence(subject: &str) -> &'static str {
    if subject == SUBJECT_CHAT {
        "A chat platform needs to be connected for the organization."
    } else {
        "The organization needs an AI key its agents can use."
    }
}

/// The organization's owners and admins only, like every organization admin route: a person who is not
/// a member gets the 404 of an organization they cannot see, a plain member the 403 of the role (neither
/// reveals whether any request exists, SEC-135).
async fn require_org_admin(state: &AppState, caller: &Caller, org: Uuid) -> Result<(), ApiError> {
    caller.org(state, org).await?.require_admin()
}

pub async fn list(
    state: &AppState,
    caller: &Caller,
    org: Uuid,
    status: Option<&str>,
) -> Result<Vec<AppSetupRequest>, ApiError> {
    require_org_admin(state, caller, org).await?;
    let status = status.unwrap_or("open");
    if status == "history" {
        // (C-1081) Closed in the display window, every level of the organization, newest first.
        let rows = repo::closed_within(&state.pool, Some(org), REQUEST_HISTORY_DAYS, HISTORY_LIMIT)
            .await?;
        return to_dto(state, rows, Viewer::OrgAdmin).await;
    }
    if !["open", "resolved", "declined", "withdrawn"].contains(&status) {
        return Err(ApiError::invalid_request(
            "Unknown status.",
            vec![aurora_core::FieldError::new(
                "/status",
                "open, resolved, declined, withdrawn or history",
            )],
        ));
    }
    let rows = repo::list_org(&state.pool, org, Some(status)).await?;
    to_dto(state, rows, Viewer::OrgAdmin).await
}

/// (C-1081) `GET /admin/requests?status=open|history` (instance admins): the requests whose level is
/// `administration`, of every organization.
pub async fn admin_list(
    state: &AppState,
    status: Option<&str>,
) -> Result<Vec<AppSetupRequest>, ApiError> {
    let rows = if open_or_history(status)? {
        repo::closed_within(&state.pool, None, REQUEST_HISTORY_DAYS, HISTORY_LIMIT).await?
    } else {
        repo::open_requests(&state.pool, None).await?
    };
    let rows = rows
        .into_iter()
        .filter(|r| level::of(r) == RequestLevel::Administration)
        .take(HISTORY_LIMIT as usize)
        .collect();
    to_dto(state, rows, Viewer::InstanceAdmin).await
}

/// `POST /orgs/{org}/app-setup-requests/{id}/decline`: "Not now". Tells each asker once. (C-1080) A request
/// of the Administration level is not the organization's to decline: 404.
pub async fn decline(
    state: &AppState,
    caller: &Caller,
    org: Uuid,
    id: Uuid,
    message: Option<&str>,
) -> Result<AppSetupRequest, ApiError> {
    let message = free_text("/message", message)?;
    require_org_admin(state, caller, org).await?;
    let req = repo::get(&state.pool, id)
        .await?
        .filter(|r| r.org_id == org)
        .filter(|r| level::of(r) == RequestLevel::Organization)
        .ok_or_else(|| ApiError::not_found("Request not found."))?;
    close_declined(
        state,
        caller,
        req,
        ClosedByRole::OrgAdmin,
        message,
        AuditActor::from_caller(caller),
        Viewer::OrgAdmin,
    )
    .await
}

/// (C-1081) `POST /admin/requests/{id}/decline` (instance admins): a request of the Administration level;
/// 404 for any other.
pub async fn admin_decline(
    state: &AppState,
    caller: &Caller,
    id: Uuid,
    message: Option<&str>,
) -> Result<AppSetupRequest, ApiError> {
    let message = free_text("/message", message)?;
    let req = repo::get(&state.pool, id)
        .await?
        .filter(|r| level::of(r) == RequestLevel::Administration)
        .ok_or_else(|| ApiError::not_found("Request not found."))?;
    close_declined(
        state,
        caller,
        req,
        ClosedByRole::InstanceAdmin,
        message,
        AuditActor::instance_admin(caller),
        Viewer::InstanceAdmin,
    )
    .await
}

/// Declines an open request as `role`: each asker gets one notice naming the role (D-407), with the
/// message as "They wrote: …" in its detail (C-1081).
async fn close_declined(
    state: &AppState,
    caller: &Caller,
    req: RequestRow,
    role: ClosedByRole,
    message: Option<String>,
    actor: AuditActor,
    viewer: Viewer,
) -> Result<AppSetupRequest, ApiError> {
    let org = req.org_id;
    let id = req.id;
    let snap = snapshot_for(state, std::slice::from_ref(&req)).await;
    let thing = thing_in(snap.as_ref(), &req.subject, &req.app_id);
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let locked = repo::open_for_update(&mut *tx, org, &req.subject, &req.app_id)
        .await?
        .filter(|r| r.id == id);
    let Some(locked) = locked else {
        return Err(ApiError::conflict(
            "already_closed",
            "This request is already closed.",
        ));
    };
    let askers = repo::askers(&mut *tx, &[locked.id]).await?;
    repo::close(
        &mut *tx,
        locked.id,
        Closing {
            status: "declined",
            by: Some(caller.user_id),
            role: Some(role.as_str()),
            message: message.as_deref(),
        },
    )
    .await?;
    let (title, detail) = declined_text(&thing, message.as_deref());
    for a in &askers {
        let ws_slug = contacts::slugs(&state.pool, org, Some(a.workspace_id))
            .await?
            .and_then(|(o, w)| w.map(|w| (o, w)));
        let href = ws_slug.map_or_else(|| "/".to_owned(), |(o, w)| format!("/o/{o}/w/{w}/apps"));
        repo::insert_notice(
            &mut *tx, locked.id, a.user_id, "declined", &title, &detail, &href,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        state,
        audit_event_with(
            actor,
            "app_setup_request.decline",
            org,
            None,
            &req.subject,
            &req.app_id,
            id,
            serde_json::json!({ "has_message": message.is_some(), "closed_by_role": role.as_str() }),
        ),
    );
    let row = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("The request vanished."))?;
    to_dto(state, vec![row], viewer)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("The request vanished."))
}

/// (C-1081, D-407) The asker's notice of a decline: the role, never the decliner's name; the message's
/// first 140 characters in the detail only.
pub fn declined_text(thing: &str, message: Option<&str>) -> (String, String) {
    (
        format!("An admin isn't setting up {thing} now"),
        format!(
            "An admin isn't setting up {thing} for the organization now.{}",
            they_wrote(message)
        ),
    )
}

/// A request that closes by itself: the system resolved it (C-1081 `closed_by_role: "system"`).
const RESOLVED: Closing<'static> = Closing {
    status: "resolved",
    by: None,
    role: Some("system"),
    message: None,
};

/// The catalogue's `requested` for every app with an open request: only whether the caller asked and
/// how many people did.
pub async fn requested(
    state: &AppState,
    org: Uuid,
    user: Uuid,
) -> Result<HashMap<String, Requested>, ApiError> {
    Ok(repo::open_counts(&state.pool, org, user)
        .await?
        .into_iter()
        .map(|(app, count, by_me)| (app, Requested { by_me, count }))
        .collect())
}

/// Re-evaluates the open requests of `org` (all organizations with `None`): an asker for whom the app
/// is now `ready` is told once and leaves the request; a request left without askers closes as
/// `resolved`. Called after the commit of whatever can change readiness (an OAuth client, a server,
/// availability, a package approval) and by the periodic sweep; a failure here never undoes that change.
pub async fn reevaluate(state: &AppState, org: Option<Uuid>) {
    if let Err(e) = reevaluate_inner(state, org).await {
        tracing::warn!(error = %e.message(), "app setup requests could not be re-evaluated");
    }
}

async fn reevaluate_inner(state: &AppState, org: Option<Uuid>) -> Result<(), ApiError> {
    let open = repo::open_requests(&state.pool, org).await?;
    if open.is_empty() {
        return Ok(());
    }
    let (open_apps, others): (Vec<_>, Vec<_>) =
        open.into_iter().partition(|r| r.subject == SUBJECT_APP);
    for req in &others {
        reevaluate_other(state, req).await?;
    }
    let Some(snap) = apps::snapshot(state).await? else {
        return Ok(());
    };
    for req in open_apps {
        let Ok((app, pkg)) = apps::find_app(&snap, &req.app_id) else {
            continue;
        };
        let askers = repo::askers(&state.pool, &[req.id]).await?;
        let mut ready = Vec::new();
        let mut last_needs: Option<(Vec<SetupNeed>, bool)> = None;
        for a in &askers {
            let Some((org_slug, _)) = contacts::slugs(&state.pool, req.org_id, None).await? else {
                continue;
            };
            let available = crate::domains::connect::service::available_in_workspace(
                state,
                req.org_id,
                a.workspace_id,
                &req.app_id,
            )
            .await?;
            let view = SetupView {
                org: req.org_id,
                workspace: Some(a.workspace_id),
                org_admin: false,
            };
            let mut facts = Facts::load(state, view, org_slug, Some(&req.app_id)).await?;
            let r = setup::readiness(&mut facts, app, pkg, available).await;
            if r.state == ReadinessState::Ready {
                ready.push(a);
            } else {
                last_needs = Some((r.needs, !available));
            }
        }
        if ready.is_empty() {
            if let Some((needs, off)) = last_needs {
                let j = serde_json::json!(needs.iter().map(|n| need_str(*n)).collect::<Vec<_>>());
                repo::set_needs(&state.pool, req.id, &j, off).await?;
            }
            continue;
        }
        let app_label = sanitize::label(&app.about.name);
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        if repo::open_for_update(&mut *tx, req.org_id, SUBJECT_APP, &req.app_id)
            .await?
            .is_none_or(|r| r.id != req.id)
        {
            continue;
        }
        let mut left = askers.len() as i64;
        for a in &ready {
            if repo::remove_asker(&mut *tx, req.id, a.user_id)
                .await?
                .is_some()
            {
                left -= 1;
            }
            let ws_slug = contacts::slugs(&state.pool, req.org_id, Some(a.workspace_id))
                .await?
                .and_then(|(o, w)| w.map(|w| (o, w)));
            let href =
                ws_slug.map_or_else(|| "/".to_owned(), |(o, w)| format!("/o/{o}/w/{w}/apps"));
            repo::insert_notice(
                &mut *tx,
                req.id,
                a.user_id,
                "resolved",
                &format!("{app_label} is ready"),
                &format!("{app_label} is ready. Give it to your agent from its Access tab."),
                &href,
            )
            .await?;
        }
        if left <= 0 {
            repo::close(&mut *tx, req.id, RESOLVED).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        audit::record(
            state,
            audit_event(
                AuditActor::system("app-setup-requests"),
                "app_setup_request.resolve",
                req.org_id,
                None,
                SUBJECT_APP,
                &req.app_id,
                req.id,
            ),
        );
    }
    Ok(())
}

/// (C-1047) A chat platform or AI key request: an asker for whom the thing now exists is told once and
/// leaves the request; a request left without askers closes as `resolved`.
async fn reevaluate_other(state: &AppState, req: &RequestRow) -> Result<(), ApiError> {
    let connector = Some(req.app_id.as_str()).filter(|t| !t.is_empty());
    let askers = repo::askers(&state.pool, &[req.id]).await?;
    let mut ready = Vec::new();
    for a in &askers {
        let ok = if req.subject == SUBJECT_CHAT {
            repo::has_chat_platform(&state.pool, req.org_id, connector).await?
        } else {
            repo::has_ai_key(&state.pool, req.org_id, a.workspace_id).await?
        };
        if ok {
            ready.push(a);
        }
    }
    if ready.is_empty() {
        return Ok(());
    }
    let (title, detail) = if req.subject == SUBJECT_CHAT {
        (
            "Your organization connected a chat platform",
            "A chat platform is connected. Add your agent to a channel from its Channels tab.",
        )
    } else {
        (
            "An AI key was added",
            "Your organization added an AI key. Your agents can use it once it is given to them.",
        )
    };
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if repo::open_for_update(&mut *tx, req.org_id, &req.subject, &req.app_id)
        .await?
        .is_none_or(|r| r.id != req.id)
    {
        return Ok(());
    }
    let mut left = askers.len() as i64;
    for a in &ready {
        if repo::remove_asker(&mut *tx, req.id, a.user_id)
            .await?
            .is_some()
        {
            left -= 1;
        }
        let href = contacts::slugs(&state.pool, req.org_id, Some(a.workspace_id))
            .await?
            .and_then(|(o, w)| w.map(|w| dash_paths::ws_overview(&o, &w)))
            .unwrap_or_else(|| "/".to_owned());
        repo::insert_notice(
            &mut *tx, req.id, a.user_id, "resolved", title, detail, &href,
        )
        .await?;
    }
    if left <= 0 {
        repo::close(&mut *tx, req.id, RESOLVED).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        state,
        audit_event(
            AuditActor::system("app-setup-requests"),
            "app_setup_request.resolve",
            req.org_id,
            None,
            &req.subject,
            &req.app_id,
            req.id,
        ),
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The e-mail sweeper
// ---------------------------------------------------------------------------------------------

/// The e-mail of one notice: the sentence and a dashboard link; never anything else (SEC-135).
pub fn notice_mail(external_url: &str, to: &str, title: &str, detail: &str, href: &str) -> Mail {
    Mail::new(
        to,
        title,
        detail,
        &format!("{}{href}", external_url.trim_end_matches('/')),
    )
}

/// One sweep: claim the open mail halves and send them (claim, then send). Switched off with
/// `approvals.email_notifications = false`. Returns the mails handed to the mailer.
pub async fn notify_once(s: &AppState) -> Result<usize, ApiError> {
    let ext = s.config.external_url.clone();
    let mut sent = 0;
    for n in repo::claim_mail(&s.pool, SWEEP_LIMIT).await? {
        if !n.active || !s.ops().approvals.email_notifications {
            continue;
        }
        match s
            .mailer
            .send(notice_mail(&ext, &n.email, &n.title, &n.detail, &n.href))
            .await
        {
            Ok(()) => sent += 1,
            Err(e) => tracing::error!(error = %e, "app setup request e-mail send failed"),
        }
    }
    Ok(sent)
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sweeps = 0u32;
        loop {
            tick.tick().await;
            // Every ten minutes: re-evaluate (heals a missed trigger); hourly: purge closed requests.
            if sweeps.is_multiple_of((SWEEP_EVERY.as_secs() / 10) as u32) {
                reevaluate(&state, None).await;
            }
            if sweeps.is_multiple_of(360)
                && let Err(e) = repo::purge_closed(&state.pool, RETENTION_DAYS).await
            {
                tracing::warn!(error = %e, "closed app setup requests could not be purged");
            }
            sweeps = sweeps.wrapping_add(1);
            match notify_once(&state).await {
                Ok(n) if n > 0 => tracing::info!(sent = n, "app setup request e-mails"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "app setup e-mail sweep failed"),
            }
        }
    });
}

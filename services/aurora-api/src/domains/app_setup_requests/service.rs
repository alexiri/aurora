//! App setup requests (P5-POLISH-4, D-386, C-1030, SEC-135).
//!
//! A person who cannot connect an app because it needs an organization owner or admin first (no sign-in
//! app, no server, switched off) asks them. The request is a record and a notice: it changes no
//! availability, OAuth client or access, it holds no free text, and it is visible only to the
//! organization's owners and admins (a person sees their own ask and the count). It closes by itself
//! when the app becomes ready for its askers ([`reevaluate`]), when the last asker withdraws, or when an
//! owner or admin declines it.

use super::dto::{AppSetupRequest, Asker, Requested};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::apps::service as apps;
use crate::domains::apps::setup::{self, Facts, ReadinessState, SetupNeed, SetupView};
use crate::domains::auth::mail::Mail;
use crate::http::auth::{Caller, require_own};
use aurora_core::{ApiError, dash_paths, ids, sanitize};
use aurora_db::DbError;
use aurora_db::repo::app_setup_requests::{self as repo, RequestRow};
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
/// A push for a request older than this is noise.
pub const PUSH_MAX_AGE: Duration = Duration::from_secs(3600);

fn need_str(n: SetupNeed) -> &'static str {
    match n {
        SetupNeed::OauthClient => "oauth_client",
        SetupNeed::Instance => "instance",
        SetupNeed::AdminMaterial => "admin_material",
    }
}

fn needs_of(v: &serde_json::Value) -> Vec<SetupNeed> {
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

fn name(s: &str) -> String {
    sanitize::one_line(s, 80)
}

async fn app_name(state: &AppState, app_id: &str) -> String {
    match apps::snapshot(state).await {
        Ok(Some(s)) => apps::find_app(&s, app_id)
            .map(|(a, _)| sanitize::label(&a.about.name))
            .unwrap_or_else(|_| sanitize::label(app_id)),
        _ => sanitize::label(app_id),
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

async fn to_dto(
    state: &AppState,
    rows: Vec<RequestRow>,
    only: Option<Uuid>,
) -> Result<Vec<AppSetupRequest>, ApiError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let askers = repo::askers(&state.pool, &ids).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(AppSetupRequest {
            id: r.id,
            org_id: r.org_id,
            app_name: app_name(state, &r.app_id).await,
            app_id: r.app_id,
            status: r.status,
            askers: askers
                .iter()
                .filter(|a| a.request_id == r.id && only.is_none_or(|u| u == a.user_id))
                .map(|a| Asker {
                    user_id: a.user_id,
                    display_name: a.display_name.clone(),
                    workspace_id: a.workspace_id,
                    asked_at: a.asked_at,
                })
                .collect(),
            needs: needs_of(&r.needs),
            unavailable: r.unavailable,
            created_at: r.created_at,
            closed_at: r.closed_at,
        });
    }
    Ok(out)
}

fn audit_event(
    actor: AuditActor,
    action: &'static str,
    org: Uuid,
    ws: Option<Uuid>,
    app: &str,
    request: Uuid,
) -> AuditEvent {
    let ev = AuditEvent::new(actor, action, Outcome::Success)
        .org(org)
        .target("app", app)
        .metadata(serde_json::json!({ "app_id": app, "request_id": request }));
    match ws {
        Some(w) => ev.workspace(w),
        None => ev,
    }
}

/// `POST /workspaces/{ws}/apps/{app}/setup-request`: asks the organization's owners and admins to make
/// `app_id` ready. Idempotent per person.
pub async fn ask(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    app_id: &str,
) -> Result<AppSetupRequest, ApiError> {
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
    let mut req = repo::open_for_update(&mut *tx, org, app_id).await?;
    let mut opened = false;
    if req.is_none() {
        opened = repo::insert(
            &mut *tx,
            ids::new_id(),
            org,
            app_id,
            &needs_json,
            unavailable,
        )
        .await?;
        req = repo::open_for_update(&mut *tx, org, app_id).await?;
    }
    let req = req.ok_or_else(|| ApiError::internal("The request vanished."))?;
    repo::set_needs(&mut *tx, req.id, &needs_json, unavailable).await?;
    let new_asker = repo::add_asker(&mut *tx, req.id, caller.user_id, ws).await?;
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
                format!("{asker} asked for {app_label}.\n\n{sentence}"),
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
                    "{} now want {app_label}: {}.\n\n{sentence}",
                    who.len(),
                    list.join(", ")
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
            audit_event(
                AuditActor::from_caller(caller),
                "app_setup_request.ask",
                org,
                Some(ws),
                app_id,
                req.id,
            ),
        );
    }
    let row = repo::open_for_update(&state.pool, org, app_id)
        .await?
        .unwrap_or(req);
    // The asker sees only their own ask (an owner or admin sees them all).
    let only = (!org_admin).then_some(caller.user_id);
    to_dto(state, vec![row], only)
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
    let org = auth.org_id;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if let Some(req) = repo::open_for_update(&mut *tx, org, app_id).await?
        && let Some(left) = repo::remove_asker(&mut *tx, req.id, caller.user_id).await?
    {
        if left == 0 {
            repo::close(&mut *tx, req.id, "withdrawn", None).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        audit::record(
            state,
            audit_event(
                AuditActor::from_caller(caller),
                "app_setup_request.withdraw",
                org,
                Some(ws),
                app_id,
                req.id,
            ),
        );
    }
    Ok(())
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
    if !["open", "resolved", "declined", "withdrawn"].contains(&status) {
        return Err(ApiError::invalid_request(
            "Unknown status.",
            vec![aurora_core::FieldError::new(
                "/status",
                "open, resolved, declined or withdrawn",
            )],
        ));
    }
    let rows = repo::list_org(&state.pool, org, Some(status)).await?;
    to_dto(state, rows, None).await
}

/// `POST /orgs/{org}/app-setup-requests/{id}/decline`: "Not now". Tells each asker once.
pub async fn decline(
    state: &AppState,
    caller: &Caller,
    org: Uuid,
    id: Uuid,
) -> Result<AppSetupRequest, ApiError> {
    require_org_admin(state, caller, org).await?;
    let req = repo::get(&state.pool, id)
        .await?
        .filter(|r| r.org_id == org)
        .ok_or_else(|| ApiError::not_found("Request not found."))?;
    let app_label = app_name(state, &req.app_id).await;
    let decliner = name(&repo::asker_name(&state.pool, caller.user_id).await?);
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let locked = repo::open_for_update(&mut *tx, org, &req.app_id)
        .await?
        .filter(|r| r.id == id);
    let Some(locked) = locked else {
        return Err(ApiError::conflict(
            "already_closed",
            "This request is already closed.",
        ));
    };
    let askers = repo::askers(&mut *tx, &[locked.id]).await?;
    repo::close(&mut *tx, locked.id, "declined", Some(caller.user_id)).await?;
    for a in &askers {
        let ws_slug = contacts::slugs(&state.pool, org, Some(a.workspace_id))
            .await?
            .and_then(|(o, w)| w.map(|w| (o, w)));
        let href = ws_slug.map_or_else(|| "/".to_owned(), |(o, w)| format!("/o/{o}/w/{w}/apps"));
        repo::insert_notice(
            &mut *tx,
            locked.id,
            a.user_id,
            "declined",
            &format!("{decliner} isn't setting up {app_label} now"),
            &format!("{decliner} isn't setting up {app_label} for the organization now."),
            &href,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        state,
        audit_event(
            AuditActor::from_caller(caller),
            "app_setup_request.decline",
            org,
            None,
            &req.app_id,
            id,
        ),
    );
    let row = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("The request vanished."))?;
    to_dto(state, vec![row], None)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("The request vanished."))
}

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
    let Some(snap) = apps::snapshot(state).await? else {
        return Ok(());
    };
    for req in open {
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
        if repo::open_for_update(&mut *tx, req.org_id, &req.app_id)
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
            repo::close(&mut *tx, req.id, "resolved", None).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        audit::record(
            state,
            audit_event(
                AuditActor::system("app-setup-requests"),
                "app_setup_request.resolve",
                req.org_id,
                None,
                &req.app_id,
                req.id,
            ),
        );
    }
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

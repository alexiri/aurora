//! The admin templates list, the review and the approve/reject/withdraw routes' service
//! (C-1108). Everything is built from the stored rows, never a live read; the offered version
//! is one rule over the stored versions ([`super::offered_version`]), and an approval is bound
//! to the `content_sha256` the review showed.

use std::collections::BTreeSet;

use aurora_core::{ApiError, sanitize};
use aurora_db::repo::agent_templates::{self as repo};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::agent_templates::dto::{
    self, AdminTemplateList, AdminTemplateSourceRef, AdminTemplateView, ApproveRequest,
    DecidedByView, NeedsAppView, NeedsLibraryView, TemplateVersionDetail, TemplateVersionListItem,
    UpdateTemplateRequest, VersionSourceView,
};

fn not_found() -> ApiError {
    ApiError::not_found("Template not found.")
}

fn version_not_found() -> ApiError {
    ApiError::not_found("Template version not found.")
}

fn content_changed() -> ApiError {
    ApiError::conflict(
        "template_content_changed",
        "This version's content changed since it was reviewed. Reload the review.",
    )
}

fn bad_state(status: &str) -> ApiError {
    ApiError::conflict(
        "invalid_state",
        format!("This version is {status}; only one waiting for review can be decided."),
    )
}

/// The offered version of a template (C-1108): its newest `approved` version while `offered`,
/// the semver-newest (rows come newest-first, so a tie keeps the newest stored one). One
/// function for every reader — the admin list, the org switches and the catalogue.
pub(crate) async fn offered_version<'e, E: sqlx::PgExecutor<'e>>(
    exec: E,
    agent_template_id: Uuid,
) -> Result<Option<repo::VersionRow>, ApiError> {
    Ok(newest_approved(
        repo::approved_versions(exec, agent_template_id).await?,
    ))
}

/// [`offered_version`] with FOR SHARE on the version rows (SEC-146 B): the create's
/// transaction re-reads the offer under the row lock, so what it verifies against is what it
/// writes with.
pub(crate) async fn offered_version_share<'e, E: sqlx::PgExecutor<'e>>(
    exec: E,
    agent_template_id: Uuid,
) -> Result<Option<repo::VersionRow>, ApiError> {
    Ok(newest_approved(
        repo::approved_versions_share(exec, agent_template_id).await?,
    ))
}

/// The offered version of a set read newest-first: the semver-newest (rows come newest-first,
/// so a tie keeps the newest stored one), one rule for every reader (C-1108).
fn newest_approved(versions: Vec<repo::VersionRow>) -> Option<repo::VersionRow> {
    let mut best: Option<repo::VersionRow> = None;
    let mut best_key = aurora_registry::manifest::SemVer::default();
    for v in versions {
        let key = aurora_registry::manifest::parse_semver(&v.version).unwrap_or_default();
        if best.is_none() || key > best_key {
            best_key = key;
            best = Some(v);
        }
    }
    best
}

/// The app ids the instance currently serves (the active packages' `apps[].about.id`).
async fn installed_app_ids(state: &AppState) -> Result<BTreeSet<String>, ApiError> {
    let mut out = BTreeSet::new();
    for (_, m) in aurora_db::repo::plugins::active_manifests(&state.pool).await? {
        if let Ok(m) = serde_json::from_value::<aurora_registry::Manifest>(m) {
            for a in m.apps {
                out.insert(a.about.id);
            }
        }
    }
    Ok(out)
}

/// Whether a template's library reference has a library added here (C-1108's `added_here`):
/// the C-1142 candidate rule's address half — same normalized url, same folder, and the
/// reference's `ref` (when it gives one) equal to the library's.
fn lib_added(
    libs: &[(Option<String>, Option<String>, String)],
    url: &str,
    reference: Option<&str>,
    path: &str,
) -> bool {
    libs.iter().any(|(lurl, lref, lpath)| {
        lurl.as_deref() == Some(url)
            && lpath == path
            && (reference.is_none() || lref.as_deref() == reference)
    })
}

/// The library references of the content (`meta.skill_refs`, kind `library`), as
/// `(url, ref, path)` with the path defaulting to the root.
fn library_refs_of(content: &Value) -> Vec<(String, Option<String>, String)> {
    content
        .get("meta")
        .and_then(|m| m.get("skill_refs"))
        .and_then(Value::as_array)
        .map(|refs| {
            refs.iter()
                .filter(|r| r.get("kind").and_then(Value::as_str) == Some("library"))
                .filter_map(|r| {
                    Some((
                        r.get("address")?.as_str()?.to_owned(),
                        r.get("ref")
                            .and_then(Value::as_str)
                            .filter(|s: &&str| !s.is_empty())
                            .map(str::to_owned),
                        r.get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The app ids the content's apps entries name (`app`, or every id of `any_of`).
fn app_ids_of(content: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        if !id.is_empty() && !out.iter().any(|x| x == id) {
            out.push(id.to_owned());
        }
    };
    for a in content
        .get("meta")
        .and_then(|m| m.get("apps"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(app) = a.get("app").and_then(Value::as_str) {
            push(app);
        }
        for app in a
            .get("any_of")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            push(app);
        }
    }
    out
}

/// The list view of one template row: versions, the offered version and what it needs
/// (C-1108: the needs come from the current version, else the newest pending one).
async fn template_view(
    state: &AppState,
    r: &repo::TemplateWithSourceRow,
    installed: &BTreeSet<String>,
    libs: &[(Option<String>, Option<String>, String)],
) -> Result<AdminTemplateView, ApiError> {
    let versions = repo::versions_with_decider(&state.pool, r.template.id).await?;
    let current = offered_version(&state.pool, r.template.id).await?;
    let needs_content = match current.as_ref().and_then(|v| v.content.as_ref()) {
        Some(c) => Some(c.clone()),
        None => versions
            .iter()
            .find(|v| v.version.status == "pending_approval")
            .and_then(|v| v.version.content.clone()),
    };
    let (needs_apps, needs_libraries) = match &needs_content {
        Some(c) => (
            app_ids_of(c)
                .into_iter()
                .map(|app_id| NeedsAppView {
                    installed: installed.contains(&app_id),
                    app_id,
                })
                .collect(),
            library_refs_of(c)
                .into_iter()
                .map(|(url, reference, path)| NeedsLibraryView {
                    added_here: lib_added(libs, &url, reference.as_deref(), &path),
                    url,
                    reference,
                    path,
                })
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    Ok(AdminTemplateView {
        template_id: r.template.template_id.clone(),
        name: r.template.name.clone(),
        summary: r.template.summary.clone(),
        source: AdminTemplateSourceRef {
            id: r.template.source_id,
            name: r.source_name.clone(),
        },
        offered: r.template.offered,
        removed_upstream: r.template.removed_upstream,
        head_version: r.template.head_version.clone(),
        head_state: r.template.head_state.clone(),
        head_problems: r.template.head_problems.clone(),
        current_version: current.map(|v| v.version),
        versions: versions
            .iter()
            .map(|v| TemplateVersionListItem {
                version: v.version.version.clone(),
                status: v.version.status.clone(),
                status_reason: v.version.status_reason.clone(),
                commit_sha: v.version.commit_sha.clone(),
                size_bytes: v.version.size_bytes,
                created_at: v.version.created_at,
                last_attempt_at: v.version.last_attempt_at,
                decided_at: v.version.decided_at,
                decided_by: v
                    .decided_by_name
                    .clone()
                    .zip(v.version.decided_by)
                    .map(|(display_name, id)| DecidedByView { id, display_name }),
            })
            .collect(),
        needs_apps,
        needs_libraries,
    })
}

/// `GET /admin/agent-templates` (C-1108): every template of every source, from the stored rows.
pub async fn list_templates(state: &AppState) -> Result<AdminTemplateList, ApiError> {
    let rows = repo::all_templates_with_source(&state.pool).await?;
    let installed = installed_app_ids(state).await?;
    let libs = aurora_db::repo::skill_libraries::all_addresses(&state.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        items.push(template_view(state, r, &installed, &libs).await?);
    }
    Ok(AdminTemplateList { items })
}

/// The template row and the version row behind the review routes' path (404s included).
async fn target(
    state: &AppState,
    template_id: &str,
    version: &str,
) -> Result<(repo::TemplateRow, repo::VersionRow), ApiError> {
    let t = repo::template_by_template_id(&state.pool, template_id)
        .await?
        .ok_or_else(not_found)?;
    let v = repo::get_version(&state.pool, t.id, version)
        .await?
        .ok_or_else(version_not_found)?;
    Ok((t, v))
}

/// The detail view of a version row (the source from the stored rows, the content decoded).
async fn detail(
    state: &AppState,
    t: &repo::TemplateRow,
    v: &repo::VersionRow,
) -> Result<TemplateVersionDetail, ApiError> {
    let s = repo::get_source(&state.pool, t.source_id)
        .await?
        .ok_or_else(not_found)?;
    Ok(TemplateVersionDetail {
        template_id: t.template_id.clone(),
        version: v.version.clone(),
        status: v.status.clone(),
        status_reason: v.status_reason.clone(),
        commit_sha: v.commit_sha.clone(),
        source: VersionSourceView {
            id: s.id,
            name: s.name.clone(),
            repo: aurora_core::git_url::parse(&s.url)
                .map(|a| a.repo_display)
                .unwrap_or_default(),
            reference: s.reference.clone(),
            path: s.path.clone(),
        },
        content_sha256: v.content_sha256.clone(),
        archive_sha256: v.archive_sha256.clone(),
        size_bytes: v.size_bytes,
        content: v
            .content
            .as_ref()
            .and_then(dto::TemplateContentView::from_json),
    })
}

/// `GET …/versions/{version}` (C-1108): the review's data, everything in full.
pub async fn version_detail(
    state: &AppState,
    template_id: &str,
    version: &str,
) -> Result<TemplateVersionDetail, ApiError> {
    let (t, v) = target(state, template_id, version).await?;
    detail(state, &t, &v).await
}

/// The version's avatar PNG (C-1108's avatar route), from the stored content.
pub async fn avatar(
    state: &AppState,
    template_id: &str,
    version: &str,
) -> Result<Vec<u8>, ApiError> {
    let (_, v) = target(state, template_id, version).await?;
    let content = v.content.as_ref().ok_or_else(version_not_found)?;
    dto::TemplateContentView::avatar_bytes(content).ok_or_else(version_not_found)
}

/// Approve a version (C-1108): only from `pending_approval`, bound to the `content_sha256`
/// the review showed. Critical `agent_template.approve` with the write.
pub async fn approve(
    state: &AppState,
    actor: &AuditActor,
    template_id: &str,
    version: &str,
    req: &ApproveRequest,
) -> Result<TemplateVersionDetail, ApiError> {
    let sha = sanitize::one_line(&req.content_sha256, 128);
    let (t, v) = target(state, template_id, version).await?;
    if v.status != "pending_approval" {
        return Err(bad_state(&v.status));
    }
    if v.content_sha256.as_deref() != Some(sha.as_str()) {
        return Err(content_changed());
    }
    // The review approves what it read: the full content view. If the stored content cannot
    // be rendered back into it (a shape this encoder never wrote — e.g. a corrupted base64
    // file), the admin has not reviewed what is stored; refuse (SEC-146).
    let content = v.content.as_ref().ok_or_else(content_changed)?;
    if dto::TemplateContentView::from_json(content).is_none() {
        return Err(ApiError::conflict(
            "template_content_changed",
            "This version's stored content cannot be rendered into the review view.",
        ));
    }
    let mut tx = audit::begin(state).await?;
    if !repo::approve(&mut tx, v.id, &sha, actor.id.unwrap_or_default()).await? {
        // A concurrent decide (or ingest) changed the row between the read and the write.
        return Err(content_changed());
    }
    let ev = AuditEvent::new(actor.clone(), "agent_template.approve", Outcome::Success)
        .target("agent_template", t.id)
        .metadata(json!({"template": t.template_id, "version": v.version}));
    audit::commit_critical(tx, &ev).await?;
    // (FIX-251, D-488(a)) The people who can apply an update are told, after the commit; a
    // failing notice never fails or undoes the approval.
    super::notify::version_offered(state, &t.template_id, actor.id).await;
    let (_, v) = target(state, template_id, version).await?;
    detail(state, &t, &v).await
}

/// Reject a version (C-1108): only from `pending_approval`; that version string is never
/// ingested again. Critical `agent_template.reject` with the write.
pub async fn reject(
    state: &AppState,
    actor: &AuditActor,
    template_id: &str,
    version: &str,
) -> Result<TemplateVersionDetail, ApiError> {
    let (t, v) = target(state, template_id, version).await?;
    if v.status != "pending_approval" {
        return Err(bad_state(&v.status));
    }
    let mut tx = audit::begin(state).await?;
    if !repo::reject(&mut tx, v.id, actor.id.unwrap_or_default()).await? {
        return Err(bad_state("pending_approval"));
    }
    let ev = AuditEvent::new(actor.clone(), "agent_template.reject", Outcome::Success)
        .target("agent_template", t.id)
        .metadata(json!({"template": t.template_id, "version": v.version}));
    audit::commit_critical(tx, &ev).await?;
    let (_, v) = target(state, template_id, version).await?;
    detail(state, &t, &v).await
}

/// Withdraw or offer again (C-1108's PATCH): the offered version is a rule over the stored
/// versions, so this changes nothing stored. Critical `agent_template.update` with the write.
pub async fn set_offered(
    state: &AppState,
    actor: &AuditActor,
    template_id: &str,
    req: &UpdateTemplateRequest,
) -> Result<AdminTemplateView, ApiError> {
    let t = repo::template_by_template_id(&state.pool, template_id)
        .await?
        .ok_or_else(not_found)?;
    let mut tx = audit::begin(state).await?;
    repo::set_offered(&mut tx, t.id, req.offered).await?;
    let ev = AuditEvent::new(actor.clone(), "agent_template.update", Outcome::Success)
        .target("agent_template", t.id)
        .metadata(json!({"template": t.template_id, "offered": req.offered}));
    audit::commit_critical(tx, &ev).await?;
    // (FIX-251, D-488(a)) Offering the template again tells the people of the offered version,
    // once per workspace and version; a failing notice never fails the change.
    if req.offered {
        super::notify::version_offered(state, &t.template_id, actor.id).await;
    }
    let installed = installed_app_ids(state).await?;
    let libs = aurora_db::repo::skill_libraries::all_addresses(&state.pool).await?;
    let r = repo::all_templates_with_source(&state.pool)
        .await?
        .into_iter()
        .find(|r| r.template.id == t.id)
        .ok_or_else(not_found)?;
    template_view(state, &r, &installed, &libs).await
}

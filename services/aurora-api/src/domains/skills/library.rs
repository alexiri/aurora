//! Library skills (phase 4, P4-GSL, D-148, SEC-83; docs/10 §Git skill library): adding a
//! library skill to a tier, the update review (diff, approve, reject) and detach. Every write here
//! is a person's act with the tier's write authority (agent `can_use`, workspace `can_own`, org
//! owner/admin: SEC-56 unchanged); approve, reject and detach are session-only.

use std::collections::{BTreeMap, HashMap};

use aurora_core::limits;
use aurora_core::sanitize::one_line;
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::skill_libraries::{self as repo, Source, UpdateRow};
use aurora_db::repo::skills::{self as skills_repo, SkillRow};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::dto::{FromLibrary, Skill};
use super::routes::authorize;
use super::service::{self, Owner, SkillView};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::skill_libraries::content::{self, LoadedFile, LoadedSkill};
use crate::domains::skill_libraries::dto::TierRef;
use crate::domains::skill_libraries::service as lib_service;
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_skill_library))
        .routes(routes!(approve_update))
        .routes(routes!(reject_update))
        .routes(routes!(detach_skill))
        .routes(routes!(org_updates))
        .routes(routes!(workspace_updates))
        .routes(routes!(agent_updates))
}

fn tier_name(owner: Owner) -> &'static str {
    match (owner.agent, owner.workspace) {
        (Some(_), _) => "agent",
        (None, Some(_)) => "workspace",
        (None, None) => "org",
    }
}

// ---- budgets --------------------------------------------------------------------------------

struct Limits {
    bytes: usize,
    skills: usize,
}

fn limits_of(state: &AppState, owner: Owner) -> Limits {
    match tier_name(owner) {
        "agent" => Limits {
            bytes: limits::LIBRARY_BYTES_PER_AGENT
                .min(state.config.skills_library.bytes_per_agent as usize),
            skills: limits::LIBRARY_SKILLS_PER_AGENT,
        },
        "workspace" => Limits {
            bytes: limits::LIBRARY_BYTES_PER_WORKSPACE,
            skills: limits::LIBRARY_SKILLS_PER_WORKSPACE,
        },
        _ => Limits {
            bytes: limits::LIBRARY_BYTES_PER_ORG,
            skills: limits::LIBRARY_SKILLS_PER_ORG,
        },
    }
}

/// Library-sourced bytes (body + files, binary as base64 length) and count of the tier, leaving
/// out `exclude`.
async fn usage(
    conn: &mut PgConnection,
    org: Uuid,
    owner: Owner,
    exclude: Option<Uuid>,
) -> Result<(usize, usize), ApiError> {
    let rows: Vec<SkillRow> = repo::tier_library_skills(conn, org, owner)
        .await?
        .into_iter()
        .filter(|r| Some(r.id) != exclude)
        .collect();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let files = skills_repo::files(conn, &ids).await?;
    let bytes = rows.iter().map(|r| r.body.chars().count()).sum::<usize>()
        + files
            .iter()
            .map(|f| service::content_size(&f.content))
            .sum::<usize>();
    Ok((bytes, rows.len()))
}

/// 422 `library_budget_exceeded` when adding `add_size` characters (one more skill unless
/// `exclude` replaces an existing one) would pass the tier's budget.
async fn check_budget(
    state: &AppState,
    conn: &mut PgConnection,
    org: Uuid,
    owner: Owner,
    add_size: usize,
    exclude: Option<Uuid>,
) -> Result<(), ApiError> {
    let l = limits_of(state, owner);
    let (bytes, count) = usage(conn, org, owner, exclude).await?;
    if bytes + add_size > l.bytes || count + 1 > l.skills {
        return Err(ApiError::unprocessable(
            "library_budget_exceeded",
            format!(
                "This {} would exceed its library skill budget ({} of {} characters, {} of {} skills).",
                tier_name(owner),
                bytes + add_size,
                l.bytes,
                count + 1,
                l.skills
            ),
        )
        .with_details(json!({
            "tier": tier_name(owner),
            "bytes_used": bytes, "bytes_adding": add_size, "bytes_limit": l.bytes,
            "skills_used": count, "skills_limit": l.skills,
        })));
    }
    Ok(())
}

async fn lock_budget(conn: &mut PgConnection, org: Uuid, owner: Owner) -> Result<(), ApiError> {
    let key = format!(
        "skill_library_budget:{org}:{:?}:{:?}",
        owner.agent, owner.workspace
    );
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

// ---- add ------------------------------------------------------------------------------------

/// Adds a library skill to a tier. The caller has already been authorized for that tier's write
/// authority. Reads **exactly** `from.commit_sha`; stores the skill pinned and read-only.
pub async fn add(
    state: &AppState,
    caller: &Caller,
    org: Uuid,
    owner: Owner,
    workspace: Option<Uuid>,
    from: &FromLibrary,
) -> Result<SkillView, ApiError> {
    lib_service::ensure_enabled(state)?;
    let commit = from.commit_sha.as_deref().ok_or_else(|| {
        ApiError::unprocessable(
            "commit_required",
            "Quote the commit you reviewed: from_library.commit_sha is required.",
        )
    })?;
    aurora_registry::library::check_hex_id(commit, "sha").map_err(|_| {
        ApiError::invalid_request(
            "Invalid skill.",
            vec![FieldError::new(
                "/from_library/commit_sha",
                "a full commit id (40 to 64 lowercase hex characters)",
            )],
        )
    })?;
    if from.path.is_empty()
        || from.path.len() > 100
        || !from
            .path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(ApiError::invalid_request(
            "Invalid skill.",
            vec![FieldError::new(
                "/from_library/path",
                "the skill's directory name as the library lists it",
            )],
        ));
    }
    let lib = lib_service::get(state, from.library_id).await?;
    if lib.org_id != org {
        return Err(ApiError::not_found("Skill library not found."));
    }
    let loaded = lib_service::load_for_add(state, &lib, &from.path, commit).await?;
    let blobs =
        serde_json::to_value(&loaded.blobs).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut tx = audit::begin(state).await?;
    lock_budget(&mut tx, org, owner).await?;
    check_budget(state, &mut tx, org, owner, loaded.size, None).await?;
    let src = Source {
        library_id: lib.id,
        label: &lib.label,
        repo: &lib.repo,
        path: &from.path,
        commit_sha: commit,
        blobs: &blobs,
        content_sha256: &loaded.content_sha256,
        pinned_by: caller.user_id,
    };
    let row = repo::insert_library_skill(
        &mut tx,
        org,
        owner,
        &loaded.name,
        &loaded.description,
        &loaded.body,
        &src,
    )
    .await
    .map_err(|e| match e {
        DbError::UniqueViolation { .. } => ApiError::conflict(
            "skill_exists",
            "A skill with this name already exists in this scope.",
        ),
        e => e.into(),
    })?;
    for f in &loaded.files {
        skills_repo::upsert_file(&mut tx, row.id, &f.path, &f.content, Some(f.executable)).await?;
    }
    service::bump(&mut tx, &row).await?;
    repo::forget_head(&mut tx, lib.id, commit).await?;
    let mut ev = AuditEvent::new(
        AuditActor::from_caller(caller),
        "skill.library.add",
        Outcome::Success,
    )
    .org(org)
    .target("skill", row.id)
    .metadata(
        json!({"name": row.name, "tier": service::tier_of(&row), "library_id": lib.id,
            "repo": lib.repo, "path": from.path, "commit_sha": commit,
            "content_sha256": content::hex(&loaded.content_sha256)}),
    );
    if let Some(w) = workspace {
        ev = ev.workspace(w);
    }
    audit::commit_critical(tx, &ev).await?;
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    service::view(&mut c, row).await
}

/// `{ directory name: [where it is added] }` for a library (the browse list's "already added").
pub async fn added_by_path(
    state: &AppState,
    library: Uuid,
) -> Result<HashMap<String, Vec<TierRef>>, ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    let mut out: HashMap<String, Vec<TierRef>> = HashMap::new();
    for s in repo::library_skills(&mut c, library).await? {
        out.entry(s.library_path.clone().unwrap_or_default())
            .or_default()
            .push(TierRef {
                tier: service::tier_of(&s),
                id: s.id,
            });
    }
    Ok(out)
}

// ---- the review (GET /skills/{id}/library) ---------------------------------------------------

#[derive(Serialize, ToSchema)]
pub struct LibraryRef {
    pub id: Uuid,
    pub label: String,
    pub repo: String,
    pub path: String,
}

#[derive(Serialize, ToSchema)]
#[schema(as = LibraryTextChange)]
pub struct TextChange {
    pub old: String,
    pub new: String,
    pub changed: bool,
}

#[derive(Serialize, ToSchema)]
#[schema(as = LibraryFileChange)]
pub struct FileChange {
    pub path: String,
    /// `added` | `removed` | `modified` | `unchanged`.
    pub change: &'static str,
    pub old: Option<String>,
    pub new: Option<String>,
    pub old_executable: Option<bool>,
    pub new_executable: Option<bool>,
    /// Binary files are compared by content but not shown.
    pub binary: bool,
}

/// The whole stored-versus-proposed content: never a summary and never truncated.
#[derive(Serialize, ToSchema)]
#[schema(as = LibraryDiff)]
pub struct Diff {
    pub description: TextChange,
    pub body: TextChange,
    pub files: Vec<FileChange>,
}

#[derive(Serialize, ToSchema)]
#[schema(as = LibraryUpdateView)]
pub struct UpdateView {
    pub id: Uuid,
    /// `changed` | `removed` | `invalid`.
    pub kind: String,
    pub commit_sha: String,
    pub created_at: DateTime<Utc>,
    /// What an approval must quote (lowercase hex SHA-256 of the proposed content).
    pub content_sha256: String,
    /// Approving would exceed the tier's library budget.
    pub over_budget: bool,
    /// Why an `invalid` proposal cannot be applied (repository text: untrusted).
    pub reason: Option<String>,
    pub diff: Option<Diff>,
}

#[derive(Serialize, ToSchema)]
pub struct SkillLibraryInfo {
    pub library: Option<LibraryRef>,
    pub commit_sha: String,
    pub pinned_at: Option<DateTime<Utc>>,
    pub pinned_by: Option<Uuid>,
    /// The library link is gone (detach or library deletion).
    pub detached: bool,
    pub update: Option<UpdateView>,
}

#[derive(Deserialize, ToSchema)]
#[schema(as = ApproveLibraryUpdateRequest)]
pub struct ApproveRequest {
    /// The proposal's `content_sha256`: the content the person reviewed.
    pub content_sha256: String,
}

#[derive(Serialize, ToSchema)]
#[schema(as = PendingLibrarySkill)]
pub struct PendingSkill {
    pub id: Uuid,
    pub name: String,
    pub tier: &'static str,
    pub agent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
#[schema(as = PendingUpdateLibrary)]
pub struct PendingLibrary {
    pub id: Uuid,
    pub label: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[schema(as = PendingLibraryUpdate)]
pub struct PendingUpdate {
    pub id: Uuid,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub skill: PendingSkill,
    pub library: Option<PendingLibrary>,
}

fn text_of(c: &[u8]) -> Option<String> {
    std::str::from_utf8(c).ok().map(str::to_owned)
}

fn stored_files(files: Vec<skills_repo::FileRow>) -> Vec<LoadedFile> {
    files
        .into_iter()
        .map(|f| LoadedFile {
            path: f.path,
            content: f.content,
            executable: f.executable,
        })
        .collect()
}

fn diff(
    old_description: &str,
    old_body: &str,
    old_files: &[LoadedFile],
    new: Option<(&str, &str, &[LoadedFile])>,
) -> Diff {
    let (nd, nb, nf) = new.unwrap_or(("", "", &[]));
    let mut paths: Vec<&str> = old_files
        .iter()
        .map(|f| f.path.as_str())
        .chain(nf.iter().map(|f| f.path.as_str()))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    let files = paths
        .into_iter()
        .map(|p| {
            let o = old_files.iter().find(|f| f.path == p);
            let n = nf.iter().find(|f| f.path == p);
            let change = match (o, n) {
                (None, Some(_)) => "added",
                (Some(_), None) => "removed",
                (Some(a), Some(b)) if a.content == b.content && a.executable == b.executable => {
                    "unchanged"
                }
                _ => "modified",
            };
            let binary = o.is_some_and(|f| text_of(&f.content).is_none())
                || n.is_some_and(|f| text_of(&f.content).is_none());
            FileChange {
                path: p.to_owned(),
                change,
                old: o.and_then(|f| text_of(&f.content)),
                new: n.and_then(|f| text_of(&f.content)),
                old_executable: o.map(|f| f.executable),
                new_executable: n.map(|f| f.executable),
                binary,
            }
        })
        .collect();
    Diff {
        description: TextChange {
            old: old_description.to_owned(),
            new: nd.to_owned(),
            changed: old_description != nd,
        },
        body: TextChange {
            old: old_body.to_owned(),
            new: nb.to_owned(),
            changed: old_body != nb,
        },
        files,
    }
}

/// `(description, body, files, blobs)` of a proposal.
type ProposedParts = (String, String, Vec<LoadedFile>, BTreeMap<String, String>);

/// The proposal's content as stored files, from its `proposed` JSON.
fn proposed_parts(u: &UpdateRow) -> Result<ProposedParts, ApiError> {
    let bad = || {
        ApiError::unprocessable(
            "library_skill_invalid",
            "The proposed update is unreadable; reject it.",
        )
    };
    let p = u.proposed.as_ref().ok_or_else(bad)?;
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    let description = s("description").ok_or_else(bad)?;
    let body = s("body").ok_or_else(bad)?;
    let mut files = Vec::new();
    for f in p.get("files").and_then(|v| v.as_array()).ok_or_else(bad)? {
        let path = f.get("path").and_then(|v| v.as_str()).ok_or_else(bad)?;
        let b64 = f
            .get("content_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(bad)?;
        files.push(LoadedFile {
            path: path.to_owned(),
            content: base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| bad())?,
            executable: f
                .get("executable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        });
    }
    let blobs: BTreeMap<String, String> = p
        .get("blobs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or_else(bad)?;
    Ok((description, body, files, blobs))
}

async fn update_view(
    state: &AppState,
    conn: &mut PgConnection,
    skill: &SkillRow,
    u: UpdateRow,
) -> Result<UpdateView, ApiError> {
    let files = stored_files(skills_repo::files(conn, &[skill.id]).await?);
    let (diff, over_budget, reason) = match u.kind.as_str() {
        "changed" => match proposed_parts(&u) {
            Ok((d, b, f, _)) => {
                let size = b.chars().count()
                    + f.iter()
                        .map(|x| service::content_size(&x.content))
                        .sum::<usize>();
                let owner = Owner {
                    agent: skill.agent_id,
                    workspace: skill.workspace_id,
                };
                let over = check_budget(state, conn, skill.org_id, owner, size, Some(skill.id))
                    .await
                    .is_err();
                (
                    Some(diff(
                        &skill.description,
                        &skill.body,
                        &files,
                        Some((&d, &b, &f)),
                    )),
                    over,
                    None,
                )
            }
            Err(_) => (
                None,
                false,
                Some("the proposed update is unreadable".to_owned()),
            ),
        },
        "removed" => (
            Some(diff(&skill.description, &skill.body, &files, None)),
            false,
            None,
        ),
        _ => (None, false, u.reason.as_deref().map(|r| one_line(r, 300))),
    };
    Ok(UpdateView {
        id: u.id,
        kind: u.kind,
        commit_sha: u.commit_sha,
        created_at: u.created_at,
        content_sha256: u
            .content_sha256
            .as_deref()
            .map(content::hex)
            .unwrap_or_default(),
        over_budget,
        reason,
        diff,
    })
}

/// The library link of a skill, its pin and any open proposed update with the whole diff.
#[utoipa::path(get, path = "/skills/{id}/library", operation_id = "getSkillLibraryInfo",
    tag = "skills", params(("id" = Uuid, Path, description = "Skill id")),
    responses((status = 200, body = SkillLibraryInfo), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_skill_library(
    c: Caller,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SkillLibraryInfo>, ApiError> {
    let row = service::row(&s.pool, id).await?;
    authorize(&c, &s, &row, false).await?;
    let Some(commit) = row.library_commit_sha.clone() else {
        return Err(ApiError::not_found(
            "This skill was not added from a library.",
        ));
    };
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let update = match repo::pending_update(&mut conn, id).await? {
        Some(u) => Some(update_view(&s, &mut conn, &row, u).await?),
        None => None,
    };
    Ok(Json(SkillLibraryInfo {
        library: row.library_id.map(|lid| LibraryRef {
            id: lid,
            label: row.library_label.clone().unwrap_or_default(),
            repo: row.library_repo.clone().unwrap_or_default(),
            path: row.library_path.clone().unwrap_or_default(),
        }),
        commit_sha: commit,
        pinned_at: row.library_pinned_at,
        pinned_by: row.library_pinned_by_user_id,
        detached: row.library_id.is_none(),
        update,
    }))
}

// ---- decisions (session only) ---------------------------------------------------------------

fn superseded() -> ApiError {
    ApiError::conflict(
        "update_superseded",
        "This update changed or was decided in the meantime; reload and review it again.",
    )
}

/// Locks the skill and the update and checks they belong together and are still open.
async fn lock_pending(
    conn: &mut PgConnection,
    skill: Uuid,
    uid: Uuid,
) -> Result<(SkillRow, UpdateRow), ApiError> {
    let row = skills_repo::get_for_update(conn, skill)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill not found."))?;
    let u = repo::get_update_for_update(conn, uid)
        .await?
        .filter(|u| u.skill_id == skill)
        .ok_or_else(|| ApiError::not_found("Update not found."))?;
    if u.status != "pending" || row.library_id.is_none() {
        return Err(superseded());
    }
    Ok((row, u))
}

/// Approve: applies exactly the quoted content, re-validated, in one transaction.
#[utoipa::path(post, path = "/skills/{id}/library/updates/{uid}/approve",
    operation_id = "approveSkillLibraryUpdate", tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id"), ("uid" = Uuid, Path, description = "Update id")),
    request_body = ApproveRequest,
    responses((status = 200, body = Skill), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn approve_update(
    c: SessionCaller,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path((id, uid)): Path<(Uuid, Uuid)>,
    Json(req): Json<ApproveRequest>,
) -> Result<Json<Skill>, ApiError> {
    lib_service::ensure_enabled(&s)?;
    let cur = service::row(&s.pool, id).await?;
    authorize(&c, &s, &cur, true).await?;
    let quoted = content::parse_hex(&req.content_sha256).ok_or_else(|| {
        ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new(
                "/content_sha256",
                "64 hexadecimal characters",
            )],
        )
    })?;
    let mut tx = audit::begin(&s).await?;
    let (row, u) = lock_pending(&mut tx, id, uid).await?;
    if u.content_sha256.as_deref() != Some(quoted.as_slice()) {
        return Err(superseded());
    }
    let owner = Owner {
        agent: row.agent_id,
        workspace: row.workspace_id,
    };
    let workspace = match (row.workspace_id, row.agent_id) {
        (Some(w), _) => Some(w),
        (None, Some(a)) => aurora_db::repo::agents::get(&s.pool, a)
            .await?
            .map(|r| r.workspace_id),
        _ => None,
    };
    let mut meta = json!({"name": row.name, "tier": service::tier_of(&row), "kind": u.kind,
        "library_id": row.library_id, "from_commit": row.library_commit_sha,
        "to_commit": u.commit_sha, "content_sha256": content::hex(&quoted)});
    let result = match u.kind.as_str() {
        "invalid" => {
            return Err(ApiError::unprocessable(
                "library_skill_invalid",
                "An update the repository broke can only be rejected.",
            ));
        }
        "removed" => {
            // The skill goes away with its (cascading) proposal rows; the audit row remains.
            service::bump(&mut tx, &row).await?;
            skills_repo::delete(&mut tx, id).await?;
            None
        }
        _ => {
            let (description, body, files, blobs) = proposed_parts(&u)?;
            let loaded: LoadedSkill =
                content::finish(row.name.clone(), description, body, files, blobs).map_err(
                    |e| match e {
                        content::LoadError::Invalid(r) => ApiError::unprocessable(
                            "library_skill_invalid",
                            format!("This update cannot be applied: {}.", one_line(&r, 200)),
                        ),
                        content::LoadError::Read(_) => ApiError::internal("unreachable"),
                    },
                )?;
            if loaded.content_sha256.as_slice() != quoted.as_slice() {
                return Err(ApiError::unprocessable(
                    "library_skill_invalid",
                    "The stored proposal does not match its hash; reject it.",
                ));
            }
            lock_budget(&mut tx, row.org_id, owner).await?;
            check_budget(&s, &mut tx, row.org_id, owner, loaded.size, Some(row.id)).await?;
            let blobs = serde_json::to_value(&loaded.blobs)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let updated = repo::apply_library_content(
                &mut tx,
                id,
                &loaded.description,
                &loaded.body,
                &u.commit_sha,
                &blobs,
                &loaded.content_sha256,
                c.user_id,
            )
            .await?;
            let files: Vec<(String, Vec<u8>, bool)> = loaded
                .files
                .iter()
                .map(|f| (f.path.clone(), f.content.clone(), f.executable))
                .collect();
            repo::replace_files(&mut tx, id, &files).await?;
            repo::decide_update(&mut tx, uid, "applied", c.user_id).await?;
            service::bump(&mut tx, &updated).await?;
            Some(updated)
        }
    };
    if let Some(o) = meta.as_object_mut() {
        o.insert("removed".into(), json!(result.is_none()));
    }
    let mut ev = AuditEvent::new(
        AuditActor::from_caller(&c),
        "skill.library.update_apply",
        Outcome::Success,
    )
    .org(row.org_id)
    .target("skill", id)
    .metadata(meta);
    if let Some(w) = workspace {
        ev = ev.workspace(w);
    }
    audit::commit_critical(tx, &ev).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    match result {
        Some(r) => Ok(Json(service::view(&mut conn, r).await?.into())),
        None => {
            // A removed skill: answer with its last stored state.
            let v = service::view(&mut conn, row).await?;
            Ok(Json(v.into()))
        }
    }
}

/// Reject: that repository state is not proposed again; a later change is.
#[utoipa::path(post, path = "/skills/{id}/library/updates/{uid}/reject",
    operation_id = "rejectSkillLibraryUpdate", tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id"), ("uid" = Uuid, Path, description = "Update id")),
    responses((status = 204, description = "Rejected"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn reject_update(
    c: SessionCaller,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path((id, uid)): Path<(Uuid, Uuid)>,
) -> Result<axum::http::StatusCode, ApiError> {
    lib_service::ensure_enabled(&s)?;
    let cur = service::row(&s.pool, id).await?;
    authorize(&c, &s, &cur, true).await?;
    let mut tx = audit::begin(&s).await?;
    let (row, u) = lock_pending(&mut tx, id, uid).await?;
    repo::decide_update(&mut tx, uid, "rejected", c.user_id).await?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(&c),
        "skill.library.update_reject",
        Outcome::Success,
    )
    .org(row.org_id)
    .target("skill", id)
    .metadata(
        json!({"name": row.name, "tier": service::tier_of(&row), "kind": u.kind,
            "commit_sha": u.commit_sha}),
    );
    audit::commit_critical(tx, &ev).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Detach: the skill becomes an ordinary editable skill (the one-way fork).
#[utoipa::path(post, path = "/skills/{id}/library/detach", operation_id = "detachSkillFromLibrary",
    tag = "skills", params(("id" = Uuid, Path, description = "Skill id")),
    responses((status = 200, body = Skill), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn detach_skill(
    c: SessionCaller,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Skill>, ApiError> {
    let cur = service::row(&s.pool, id).await?;
    authorize(&c, &s, &cur, true).await?;
    let mut tx = audit::begin(&s).await?;
    let row = skills_repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill not found."))?;
    if row.library_id.is_none() {
        return Err(ApiError::conflict(
            "not_a_library_skill",
            "This skill is not linked to a library.",
        ));
    }
    repo::detach_skill(&mut tx, id).await?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(&c),
        "skill.library.detach",
        Outcome::Success,
    )
    .org(row.org_id)
    .target("skill", id)
    .metadata(json!({"name": row.name, "tier": service::tier_of(&row),
            "library_id": row.library_id, "commit_sha": row.library_commit_sha}));
    audit::commit_critical(tx, &ev).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let row = skills_repo::get(&mut conn, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill not found."))?;
    Ok(Json(service::view(&mut conn, row).await?.into()))
}

// ---- pending proposals by scope -------------------------------------------------------------

fn pending_items(rows: Vec<repo::PendingRow>) -> Vec<PendingUpdate> {
    rows.into_iter()
        .map(|r| PendingUpdate {
            id: r.update_id,
            kind: r.kind,
            created_at: r.created_at,
            skill: PendingSkill {
                id: r.skill_id,
                name: r.name,
                tier: match (r.agent_id, r.workspace_id) {
                    (Some(_), _) => "agent",
                    (None, Some(_)) => "workspace",
                    _ => "org",
                },
                agent_id: r.agent_id,
                workspace_id: r.workspace_id,
            },
            library: r.library_id.map(|id| PendingLibrary {
                id,
                label: r.library_label,
            }),
        })
        .collect()
}

#[derive(Serialize, ToSchema)]
#[schema(as = PendingLibraryUpdates)]
pub struct PendingUpdates {
    pub items: Vec<PendingUpdate>,
}

/// Pending proposed updates of the organization-tier skills (org owner/admin).
#[utoipa::path(get, path = "/orgs/{org}/skill-library-updates",
    operation_id = "listOrgSkillLibraryUpdates", tag = "skills",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = PendingUpdates), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn org_updates(
    c: Caller,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<PendingUpdates>, ApiError> {
    lib_service::ensure_enabled(&s)?;
    c.org(&s, org).await?.require_admin()?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    // Every tier of the org is visible to an org owner/admin (they hold every authority).
    Ok(Json(PendingUpdates {
        items: pending_items(repo::pending_for_org(&mut conn, org).await?),
    }))
}

/// Pending proposed updates of a workspace's skills (`can_own`).
#[utoipa::path(get, path = "/workspaces/{ws}/skill-library-updates",
    operation_id = "listWorkspaceSkillLibraryUpdates", tag = "skills",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = PendingUpdates), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn workspace_updates(
    c: Caller,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<PendingUpdates>, ApiError> {
    lib_service::ensure_enabled(&s)?;
    let a = c.workspace(&s, ws).await?;
    crate::http::auth::require_own(&a)?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(Json(PendingUpdates {
        items: pending_items(repo::pending_for_workspace(&mut conn, ws).await?),
    }))
}

/// Pending proposed updates of an agent's own skills (`can_use`).
#[utoipa::path(get, path = "/agents/{agent}/skill-library-updates",
    operation_id = "listAgentSkillLibraryUpdates", tag = "skills",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = PendingUpdates), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn agent_updates(
    c: Caller,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<PendingUpdates>, ApiError> {
    lib_service::ensure_enabled(&s)?;
    super::routes::visible_agent(&c, &s, agent).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(Json(PendingUpdates {
        items: pending_items(repo::pending_for_agent(&mut conn, agent).await?),
    }))
}

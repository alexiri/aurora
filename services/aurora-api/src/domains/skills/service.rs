use aurora_core::{ApiError, FieldError, limits};
use aurora_db::DbError;
use aurora_db::repo::agents;
pub use aurora_db::repo::skills::Owner;
use aurora_db::repo::skills::{self as repo, SkillRow};
use base64::Engine as _;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

const PATH_MAX_DEPTH: usize = 4;
const PATH_MAX_LEN: usize = 200;

pub struct FileMeta {
    pub path: String,
    pub size: usize,
    pub executable: bool,
}

pub struct SkillView {
    pub row: SkillRow,
    pub files: Vec<FileMeta>,
    pub total_size: usize,
    /// (phase 4, P4-GSL) a library skill with an open proposed update.
    pub update_pending: bool,
}

/// 409 `library_skill_readonly`: library skills change only by a person's approval of a proposed
/// update (docs/10 §Git skill library).
pub fn readonly() -> ApiError {
    ApiError::conflict(
        "library_skill_readonly",
        "This skill is managed by a skill library and is read-only. Detach it to edit it.",
    )
}

pub struct NewSkill<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub body: &'a str,
    pub enabled: bool,
}

#[derive(Default)]
pub struct Patch<'a> {
    pub description: Option<&'a str>,
    pub body: Option<&'a str>,
    pub enabled: Option<bool>,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid skill.", vec![FieldError::new(path, msg)])
}

fn not_found() -> ApiError {
    ApiError::not_found("Skill not found.")
}

pub fn validate_name(name: &str) -> Result<(), ApiError> {
    let ok = !name.is_empty()
        && name.len() <= limits::SKILL_NAME_MAX
        && name.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
    if !ok {
        return Err(bad(
            "/name",
            "lowercase letters and digits joined by single hyphens, at most 64 characters",
        ));
    }
    if name.starts_with("aurora-") || name == "aurora" {
        return Err(bad("/name", "the aurora- prefix is reserved"));
    }
    Ok(())
}

fn validate_description(d: &str) -> Result<(), ApiError> {
    if d.trim().is_empty() {
        return Err(bad("/description", "must not be empty"));
    }
    if d.contains(['\r', '\n']) {
        return Err(bad("/description", "must be a single line"));
    }
    if d.chars().count() > limits::SKILL_DESCRIPTION_MAX {
        return Err(bad(
            "/description",
            &format!("at most {} characters", limits::SKILL_DESCRIPTION_MAX),
        ));
    }
    Ok(())
}

fn validate_body(b: &str) -> Result<(), ApiError> {
    if b.chars().count() > limits::SKILL_BODY_MAX {
        return Err(bad(
            "/body",
            &format!("at most {} characters", limits::SKILL_BODY_MAX),
        ));
    }
    Ok(())
}

pub fn validate_path(path: &str) -> Result<(), ApiError> {
    let chars_ok = path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));
    let segs: Vec<&str> = path.split('/').collect();
    let ok = !path.is_empty()
        && path.len() <= PATH_MAX_LEN
        && chars_ok
        && segs.len() <= PATH_MAX_DEPTH
        && segs
            .iter()
            .all(|s| !s.is_empty() && *s != "." && *s != "..")
        // (FIX-110) `SKILL.md` in any segment (`skill.md/x` too), not only the last.
        && !segs.iter().any(|s| s.eq_ignore_ascii_case("skill.md"));
    if ok {
        Ok(())
    } else {
        Err(bad(
            "/path",
            "relative path of [A-Za-z0-9._/-], depth at most 4, no . or .. segments, \
             and not named SKILL.md",
        ))
    }
}

/// Characters for text, base64 length for binary content.
pub(crate) fn content_size(c: &[u8]) -> usize {
    match std::str::from_utf8(c) {
        Ok(s) => s.chars().count(),
        Err(_) => base64::engine::general_purpose::STANDARD.encode(c).len(),
    }
}

fn check_total(body: &str, files: &[repo::FileRow]) -> Result<(), ApiError> {
    let total = body.chars().count()
        + files
            .iter()
            .map(|f| content_size(&f.content))
            .sum::<usize>();
    if total > limits::SKILL_TOTAL_MAX {
        return Err(ApiError::unprocessable(
            "skill_too_large",
            format!(
                "A skill's body and files can total at most {} characters.",
                limits::SKILL_TOTAL_MAX
            ),
        ));
    }
    Ok(())
}

/// `agent` | `workspace` | `org`.
pub fn tier_of(r: &SkillRow) -> &'static str {
    if r.agent_id.is_some() {
        "agent"
    } else if r.workspace_id.is_some() {
        "workspace"
    } else {
        "org"
    }
}

/// Precedence rank: lower wins (agent > workspace > org).
fn rank(r: &SkillRow) -> u8 {
    match tier_of(r) {
        "agent" => 0,
        "workspace" => 1,
        _ => 2,
    }
}

/// Bumps the sync generation of the agent (agent tier), of the workspace (workspace tier,
/// reaching only its agents) or of every agent in the org.
pub(crate) async fn bump(conn: &mut PgConnection, s: &SkillRow) -> Result<(), DbError> {
    match (s.agent_id, s.workspace_id) {
        (Some(a), _) => repo::bump_agent(conn, a).await,
        (None, Some(w)) => repo::bump_workspace(conn, w).await,
        (None, None) => repo::bump_org(conn, s.org_id).await,
    }
}

pub struct EffectiveItem {
    pub view: SkillView,
    /// Every lower-tier skill of the name (enabled or not): `(id, tier)`.
    pub shadows: Vec<(Uuid, &'static str)>,
}

pub struct ShadowedItem {
    pub view: SkillView,
    pub enabled: bool,
    pub shadowed_by: Option<(Uuid, &'static str)>,
}

pub struct Effective {
    pub items: Vec<EffectiveItem>,
    pub shadowed: Vec<ShadowedItem>,
}

/// The effective set of an agent (RFE-020): per name the highest *enabled* tier wins
/// (agent > workspace > org); a disabled skill never vetoes a lower tier.
pub async fn effective(pool: &PgPool, agent: &agents::AgentRow) -> Result<Effective, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::list_reaching(&mut c, agent.org_id, agent.workspace_id, agent.id).await?;
    let mut views = view_many(&mut c, rows).await?;
    // Group by name (rows are ordered by name); within a name order by tier rank.
    views.sort_by(|a, b| {
        (a.row.name.as_str(), rank(&a.row)).cmp(&(b.row.name.as_str(), rank(&b.row)))
    });
    let mut out = Effective {
        items: Vec::new(),
        shadowed: Vec::new(),
    };
    let mut it = views.into_iter().peekable();
    while let Some(first) = it.next() {
        let mut group = vec![first];
        while it.peek().is_some_and(|n| n.row.name == group[0].row.name) {
            group.extend(it.next());
        }
        match group.iter().position(|v| v.row.enabled) {
            Some(w) => {
                let win = group.remove(w);
                let by = (win.row.id, tier_of(&win.row));
                let shadows = group
                    .iter()
                    .filter(|v| rank(&v.row) > rank(&win.row))
                    .map(|v| (v.row.id, tier_of(&v.row)))
                    .collect();
                out.items.push(EffectiveItem { view: win, shadows });
                for v in group {
                    let enabled = v.row.enabled;
                    out.shadowed.push(ShadowedItem {
                        view: v,
                        enabled,
                        shadowed_by: Some(by),
                    });
                }
            }
            None => {
                for v in group {
                    out.shadowed.push(ShadowedItem {
                        view: v,
                        enabled: false,
                        shadowed_by: None,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// The enabled agent-tier skills of the workspace's agents that override a workspace skill.
pub async fn overridden_in(
    pool: &PgPool,
    row: &SkillRow,
) -> Result<Option<Vec<super::dto::OverriddenAgent>>, ApiError> {
    let Some(ws) = row.workspace_id else {
        return Ok(None);
    };
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::agent_overrides(&mut c, ws, &row.name).await?;
    Ok(Some(
        rows.into_iter()
            .map(|(id, handle, name, skill_id)| super::dto::OverriddenAgent {
                agent: super::dto::AgentRef { id, handle, name },
                skill_id,
            })
            .collect(),
    ))
}

/// A DTO with `overridden_in` filled (workspace tier).
pub async fn dto(pool: &PgPool, v: SkillView) -> Result<super::dto::Skill, ApiError> {
    let ov = overridden_in(pool, &v.row).await?;
    let mut s = super::dto::Skill::from(v);
    s.overridden_in = ov;
    Ok(s)
}

fn views(
    rows: Vec<SkillRow>,
    files: Vec<repo::FileRow>,
    pending: &std::collections::HashSet<Uuid>,
) -> Vec<SkillView> {
    rows.into_iter()
        .map(|row| {
            let mine: Vec<&repo::FileRow> = files.iter().filter(|f| f.skill_id == row.id).collect();
            let metas: Vec<FileMeta> = mine
                .iter()
                .map(|f| FileMeta {
                    path: f.path.clone(),
                    size: content_size(&f.content),
                    executable: f.executable,
                })
                .collect();
            let total_size = row.body.chars().count() + metas.iter().map(|m| m.size).sum::<usize>();
            SkillView {
                update_pending: pending.contains(&row.id),
                row,
                files: metas,
                total_size,
            }
        })
        .collect()
}

/// Ids of the library skills (among `rows`) that have an open proposed update.
async fn pending_of(
    conn: &mut PgConnection,
    rows: &[SkillRow],
) -> Result<std::collections::HashSet<Uuid>, ApiError> {
    let ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.library_id.is_some())
        .map(|r| r.id)
        .collect();
    if ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    Ok(
        aurora_db::repo::skill_libraries::pending_updates_of(conn, &ids)
            .await?
            .into_iter()
            .map(|u| u.skill_id)
            .collect(),
    )
}

pub(crate) async fn view(conn: &mut PgConnection, row: SkillRow) -> Result<SkillView, ApiError> {
    let files = repo::files(conn, &[row.id]).await?;
    let pending = pending_of(conn, std::slice::from_ref(&row)).await?;
    Ok(views(vec![row], files, &pending).remove(0))
}

async fn view_many(
    conn: &mut PgConnection,
    rows: Vec<SkillRow>,
) -> Result<Vec<SkillView>, ApiError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let files = repo::files(conn, &ids).await?;
    let pending = pending_of(conn, &rows).await?;
    Ok(views(rows, files, &pending))
}

pub async fn list_agent(pool: &PgPool, agent: Uuid) -> Result<Vec<SkillView>, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::list_agent(&mut c, agent).await?;
    view_many(&mut c, rows).await
}

pub async fn list_org(pool: &PgPool, org: Uuid) -> Result<Vec<SkillView>, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::list_org(&mut c, org).await?;
    view_many(&mut c, rows).await
}

pub async fn list_workspace(pool: &PgPool, ws: Uuid) -> Result<Vec<SkillView>, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::list_workspace(&mut c, ws).await?;
    view_many(&mut c, rows).await
}

/// Raw row lookup (authorization is the caller's job).
pub async fn row(pool: &PgPool, id: Uuid) -> Result<SkillRow, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    repo::get(&mut c, id).await?.ok_or_else(not_found)
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<SkillView, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let r = repo::get(&mut c, id).await?.ok_or_else(not_found)?;
    view(&mut c, r).await
}

pub async fn get_agent_named(
    pool: &PgPool,
    agent: Uuid,
    name: &str,
) -> Result<SkillView, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let r = repo::get_agent_named(&mut c, agent, name)
        .await?
        .ok_or_else(not_found)?;
    view(&mut c, r).await
}

pub async fn get_org_named(pool: &PgPool, org: Uuid, name: &str) -> Result<SkillView, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let r = repo::get_org_named(&mut c, org, name)
        .await?
        .ok_or_else(not_found)?;
    view(&mut c, r).await
}

/// Text content of one file (UTF-8 only).
pub async fn read_file(pool: &PgPool, id: Uuid, path: &str) -> Result<String, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let f = repo::file(&mut c, id, path)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill file not found."))?;
    String::from_utf8(f.content)
        .map_err(|_| ApiError::unprocessable("binary_file", "This file is binary."))
}

pub async fn create(
    pool: &PgPool,
    org: Uuid,
    owner: Owner,
    n: &NewSkill<'_>,
    created_by: &str,
) -> Result<SkillView, ApiError> {
    validate_name(n.name)?;
    validate_description(n.description)?;
    validate_body(n.body)?;
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    // (FIX-110) The per-agent cap is checked under the agent row lock (the pattern
    // `schedules::service::create` uses), so concurrent creates cannot overshoot it.
    if let Some(agent) = owner.agent {
        sqlx::query("SELECT 1 FROM agents WHERE id = $1 FOR UPDATE")
            .bind(agent)
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        if repo::count_agent_own(&mut tx, agent).await? >= limits::SKILL_PER_AGENT_MAX {
            return Err(ApiError::conflict(
                "skill_limit_reached",
                format!(
                    "An agent can have at most {} skills of its own.",
                    limits::SKILL_PER_AGENT_MAX
                ),
            ));
        }
    }
    let row = repo::insert(
        &mut tx,
        org,
        owner,
        n.name,
        n.description,
        n.body,
        n.enabled,
        created_by,
    )
    .await
    .map_err(|e| match e {
        DbError::UniqueViolation { .. } => ApiError::conflict(
            "skill_exists",
            "A skill with this name already exists in this scope.",
        ),
        e => e.into(),
    })?;
    bump(&mut tx, &row).await?;
    let v = view(&mut tx, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(v)
}

pub async fn update(pool: &PgPool, id: Uuid, p: &Patch<'_>) -> Result<SkillView, ApiError> {
    if let Some(d) = p.description {
        validate_description(d)?;
    }
    if let Some(b) = p.body {
        validate_body(b)?;
    }
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    // (P4-GSL) enable/disable stays allowed; the content changes only by an approved update.
    if cur.library_id.is_some() && (p.description.is_some() || p.body.is_some()) {
        return Err(readonly());
    }
    let body = p.body.unwrap_or(&cur.body);
    if p.body.is_some() {
        let files = repo::files(&mut tx, &[id]).await?;
        check_total(body, &files)?;
    }
    let row = repo::update(
        &mut tx,
        id,
        p.description.unwrap_or(&cur.description),
        body,
        p.enabled.unwrap_or(cur.enabled),
    )
    .await?;
    bump(&mut tx, &row).await?;
    let v = view(&mut tx, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(v)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    repo::delete(&mut tx, id).await?;
    bump(&mut tx, &cur).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

pub async fn put_file(
    pool: &PgPool,
    id: Uuid,
    path: &str,
    content: &[u8],
    executable: Option<bool>,
) -> Result<SkillView, ApiError> {
    validate_path(path)?;
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    if cur.library_id.is_some() {
        return Err(readonly());
    }
    let mut files = repo::files(&mut tx, &[id]).await?;
    // (FIX-110) At most SKILL_FILES_MAX files per skill (a replaced path adds none); the skill
    // row is locked above, so the count is race-free.
    if !files.iter().any(|f| f.path == path) && files.len() as i64 >= limits::SKILL_FILES_MAX {
        return Err(ApiError::conflict(
            "skill_files_limit_reached",
            format!(
                "A skill can have at most {} files.",
                limits::SKILL_FILES_MAX
            ),
        ));
    }
    files.retain(|f| f.path != path);
    files.push(repo::FileRow {
        skill_id: id,
        path: path.to_owned(),
        content: content.to_vec(),
        executable: false,
    });
    check_total(&cur.body, &files)?;
    repo::upsert_file(&mut tx, id, path, content, executable).await?;
    repo::touch(&mut tx, id).await?;
    bump(&mut tx, &cur).await?;
    let row = repo::get(&mut tx, id).await?.ok_or_else(not_found)?;
    let v = view(&mut tx, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(v)
}

pub async fn delete_file(pool: &PgPool, id: Uuid, path: &str) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let cur = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    if cur.library_id.is_some() {
        return Err(readonly());
    }
    if !repo::delete_file(&mut tx, id, path).await? {
        return Err(ApiError::not_found("Skill file not found."));
    }
    repo::touch(&mut tx, id).await?;
    bump(&mut tx, &cur).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

use aurora_core::{ApiError, limits};
use aurora_db::DbError;
use aurora_db::repo::agents;
pub use aurora_db::repo::skills::Owner;
use aurora_db::repo::skills::{self as repo, SkillRow};
use aurora_registry::skill_format;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

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
    /// The display name; blank means none.
    pub title: Option<&'a str>,
    pub description: &'a str,
    pub body: &'a str,
    pub enabled: bool,
}

#[derive(Default)]
pub struct Patch<'a> {
    /// `Some("")` clears the display name.
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub body: Option<&'a str>,
    pub enabled: Option<bool>,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid skill.", path, msg)
}

fn not_found() -> ApiError {
    ApiError::not_found("Skill not found.")
}

pub fn validate_name(name: &str) -> Result<(), ApiError> {
    match skill_format::check_name(name) {
        Ok(()) => Ok(()),
        Err(skill_format::NameError::Grammar) => Err(bad(
            "/name",
            "lowercase letters and digits joined by single hyphens, at most 64 characters",
        )),
        Err(skill_format::NameError::Reserved) => {
            Err(bad("/name", "the aurora- prefix is reserved"))
        }
    }
}

/// The stored form of a display name (C-1173): trimmed plain text of at most
/// [`limits::SKILL_TITLE_MAX`] characters on one line; blank is none.
pub(crate) fn clean_title(t: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(t) = t.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if t.chars().any(char::is_control) {
        return Err(bad("/title", "must be a single line of plain text"));
    }
    if t.chars().count() > limits::SKILL_TITLE_MAX {
        return Err(bad(
            "/title",
            &format!("at most {} characters", limits::SKILL_TITLE_MAX),
        ));
    }
    Ok(Some(t.to_owned()))
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
    if skill_format::path_ok(path) {
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
    skill_format::content_size(c)
}

fn files_limit() -> ApiError {
    ApiError::conflict(
        "skill_files_limit_reached",
        format!(
            "A skill can have at most {} files.",
            limits::SKILL_FILES_MAX
        ),
    )
}

/// The limits of an edit, applied to a whole skill composed elsewhere (a template merge's
/// result, FIX-251): the description's and body's own rules, the file count, the total and each
/// text's home-sync entry — refused with the same errors. `files` are `(path, content)`.
pub(crate) fn check_composed(
    name: &str,
    description: &str,
    body: &str,
    files: &[(&str, &[u8])],
) -> Result<(), ApiError> {
    validate_description(description)?;
    validate_body(body)?;
    if files.len() as i64 > limits::SKILL_FILES_MAX {
        return Err(files_limit());
    }
    check_total(body, files.iter().map(|(_, c)| *c))?;
    check_skill_md_entry(name, description, body)?;
    for (path, content) in files {
        skill_format::check_file_entry(path, content).map_err(entry_too_large)?;
    }
    Ok(())
}

/// (D-513) 422 `file_too_large`: a text that would not fit one home-sync entry would be stored
/// and then missing from the agent's home.
fn entry_too_large(e: skill_format::SkillError) -> ApiError {
    let mut message = e.message;
    if let Some(first) = message.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    ApiError::unprocessable("file_too_large", format!("{message}."))
}

fn check_skill_md_entry(name: &str, description: &str, body: &str) -> Result<(), ApiError> {
    let md = skill_format::projected_skill_md(name, description, body);
    skill_format::check_entry("SKILL.md", &md).map_err(entry_too_large)
}

fn check_total<'a>(body: &str, files: impl Iterator<Item = &'a [u8]>) -> Result<(), ApiError> {
    let total = body.chars().count() + files.map(content_size).sum::<usize>();
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
    /// The skill's own switch.
    pub enabled: bool,
    /// (C-1174) Turned off for this agent.
    pub off_for_agent: bool,
    pub shadowed_by: Option<(Uuid, &'static str)>,
}

pub struct Effective {
    pub items: Vec<EffectiveItem>,
    pub shadowed: Vec<ShadowedItem>,
}

/// The effective set of an agent (RFE-020, C-1174): [`repo::resolve`], the one rule, with each
/// skill's files. A skill turned off for the agent is never among `items`.
pub async fn effective(pool: &PgPool, agent: &agents::AgentRow) -> Result<Effective, ApiError> {
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    let rows =
        repo::list_reaching(&mut c, agent.org_id, agent.workspace_id, agent.id, None).await?;
    let all: Vec<SkillRow> = rows.iter().map(|r| r.row.clone()).collect();
    let mut views: std::collections::HashMap<Uuid, SkillView> = view_many(&mut c, all)
        .await?
        .into_iter()
        .map(|v| (v.row.id, v))
        .collect();
    let mut out = Effective {
        items: Vec::new(),
        shadowed: Vec::new(),
    };
    for group in repo::resolve(rows) {
        let by = group.winner.as_ref().map(|w| (w.row.id, tier_of(&w.row)));
        if let Some(win) = &group.winner {
            let shadows = group
                .others
                .iter()
                .filter(|o| repo::rank(&o.row) > repo::rank(&win.row))
                .map(|o| (o.row.id, tier_of(&o.row)))
                .collect();
            if let Some(view) = views.remove(&win.row.id) {
                out.items.push(EffectiveItem { view, shadows });
            }
        }
        for o in group.others {
            if let Some(view) = views.remove(&o.row.id) {
                out.shadowed.push(ShadowedItem {
                    view,
                    enabled: o.row.enabled,
                    off_for_agent: o.off_for_agent,
                    shadowed_by: by,
                });
            }
        }
    }
    Ok(out)
}

/// What a change of the per-agent switch did.
pub enum SwitchOutcome {
    Changed(Box<SkillRow>),
    Unchanged,
}

/// (C-1174, D-499) Turns an inherited skill off (or back on) for one agent, inside `tx`. The
/// caller has established that the person may edit the agent. The skill must reach the agent (a
/// workspace or organization skill of its workspace or organization): anything else is the same
/// 404 as an unknown id, so another tenant's skill is never revealed; the agent's own skill is
/// 409 `skill_not_inherited`.
///
/// Lock order: the skill's row first, then the agent rows the bump updates, like a change or
/// delete of an inherited skill.
pub async fn switch_for_agent(
    tx: &mut PgConnection,
    agent: &agents::AgentRow,
    skill: Uuid,
    off: bool,
    by: Uuid,
) -> Result<SwitchOutcome, ApiError> {
    // A skill's owner never changes, so reach is decided on an unlocked read: no row that does
    // not reach the agent is ever locked.
    let seen = repo::get(tx, skill).await?.ok_or_else(not_found)?;
    let reaches = seen.org_id == agent.org_id
        && match (seen.agent_id, seen.workspace_id) {
            (Some(a), _) => a == agent.id,
            (None, Some(w)) => w == agent.workspace_id,
            (None, None) => true,
        };
    if !reaches {
        return Err(not_found());
    }
    let row = repo::get_for_update(tx, skill)
        .await?
        .ok_or_else(not_found)?;
    if row.agent_id.is_some() {
        return Err(ApiError::conflict(
            "skill_not_inherited",
            "Only a workspace or organization skill can be turned off for one agent. This is the \
             agent's own skill: switch the skill itself.",
        ));
    }
    let changed = if off {
        repo::set_off(tx, agent.id, skill, by).await?
    } else {
        repo::clear_off(tx, agent.id, skill).await?
    };
    if !changed {
        return Ok(SwitchOutcome::Unchanged);
    }
    repo::bump_agent(tx, agent.id).await?;
    Ok(SwitchOutcome::Changed(Box::new(row)))
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

/// A DTO with `overridden_in` (workspace tier) and `used_by_agents` (workspace and
/// organization tiers) filled.
pub async fn dto(pool: &PgPool, v: SkillView) -> Result<super::dto::Skill, ApiError> {
    let ov = overridden_in(pool, &v.row).await?;
    let used_by = if v.row.agent_id.is_none() {
        let mut c = pool.acquire().await.map_err(DbError::from)?;
        let used = super::usage::used_by_agents(&mut c, &[v.row.id]).await?;
        Some(used.get(&v.row.id).copied().unwrap_or(0))
    } else {
        None
    };
    let mut s = super::dto::Skill::from(v);
    s.overridden_in = ov;
    s.used_by_agents = used_by;
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

/// One file of the skill, bytes as stored (C-1099): the path is validated as for a write, and the lookup is
/// by (skill id, path), so another skill's file is never reachable.
pub async fn file(pool: &PgPool, id: Uuid, path: &str) -> Result<repo::FileRow, ApiError> {
    validate_path(path)?;
    let mut c = pool.acquire().await.map_err(DbError::from)?;
    repo::file(&mut c, id, path)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill file not found."))
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
    clean_title(n.title)?;
    validate_description(n.description)?;
    validate_body(n.body)?;
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let v = create_in_tx(&mut tx, org, owner, n, created_by).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(v)
}

/// The transaction body of [`create`] (P6-TPL-3's from-template route calls it inside its own
/// bigger transaction): the per-agent cap under the agent row lock, the insert (its unique
/// violation is 409 `skill_exists`) and the sync bump. The caller has validated the text.
pub(crate) async fn create_in_tx(
    tx: &mut PgConnection,
    org: Uuid,
    owner: Owner,
    n: &NewSkill<'_>,
    created_by: &str,
) -> Result<SkillView, ApiError> {
    // (FIX-110) The per-agent cap is checked under the agent row lock (the pattern
    // `schedules::service::create` uses), so concurrent creates cannot overshoot it.
    if let Some(agent) = owner.agent {
        agents::lock_for_write(&mut *tx, agent).await?;
        if repo::count_agent_own(&mut *tx, agent).await? >= limits::SKILL_PER_AGENT_MAX {
            return Err(ApiError::conflict(
                "skill_limit_reached",
                format!(
                    "An agent can have at most {} skills of its own.",
                    limits::SKILL_PER_AGENT_MAX
                ),
            ));
        }
    }
    check_skill_md_entry(n.name, n.description, n.body)?;
    let title = clean_title(n.title)?;
    let row = repo::insert(
        tx,
        org,
        owner,
        n.name,
        n.description,
        n.body,
        n.enabled,
        created_by,
        title.as_deref(),
    )
    .await
    .map_err(|e| match e {
        DbError::UniqueViolation { .. } => ApiError::conflict(
            "skill_exists",
            "A skill with this name already exists in this scope.",
        ),
        e => e.into(),
    })?;
    bump(tx, &row).await?;
    view(tx, row).await
}

/// Locks a skill for a change: the owning agent's row first (agent-tier skills), then the
/// skill's own row. A template apply holds the agent's row while it writes skills, so every
/// change of an existing skill takes the two in this order.
pub(crate) async fn lock_for_edit(conn: &mut PgConnection, id: Uuid) -> Result<SkillRow, ApiError> {
    // The owner of a skill never changes, so this unlocked read is enough to find it.
    let owner = repo::get(conn, id).await?.ok_or_else(not_found)?.agent_id;
    if let Some(agent) = owner {
        agents::lock_for_write(&mut *conn, agent).await?;
    }
    repo::get_for_update(conn, id).await?.ok_or_else(not_found)
}

pub async fn update(pool: &PgPool, id: Uuid, p: &Patch<'_>) -> Result<SkillView, ApiError> {
    if let Some(d) = p.description {
        validate_description(d)?;
    }
    if let Some(b) = p.body {
        validate_body(b)?;
    }
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let cur = lock_for_edit(&mut tx, id).await?;
    // (P4-GSL) enable/disable stays allowed; the content changes only by an approved update.
    if cur.library_id.is_some()
        && (p.title.is_some() || p.description.is_some() || p.body.is_some())
    {
        return Err(readonly());
    }
    let title = match p.title {
        Some(t) => clean_title(Some(t))?,
        None => cur.title.clone(),
    };
    let body = p.body.unwrap_or(&cur.body);
    if p.body.is_some() {
        let files = repo::files(&mut tx, &[id]).await?;
        check_total(body, files.iter().map(|f| f.content.as_slice()))?;
    }
    if p.body.is_some() || p.description.is_some() {
        check_skill_md_entry(&cur.name, p.description.unwrap_or(&cur.description), body)?;
    }
    let row = repo::update(
        &mut tx,
        id,
        p.description.unwrap_or(&cur.description),
        body,
        p.enabled.unwrap_or(cur.enabled),
        title.as_deref(),
    )
    .await?;
    bump(&mut tx, &row).await?;
    let v = view(&mut tx, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(v)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let cur = lock_for_edit(&mut tx, id).await?;
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
    let cur = lock_for_edit(&mut tx, id).await?;
    if cur.library_id.is_some() {
        return Err(readonly());
    }
    let mut files = repo::files(&mut tx, &[id]).await?;
    // (FIX-110) At most SKILL_FILES_MAX files per skill (a replaced path adds none); the skill
    // row is locked above, so the count is race-free.
    if !files.iter().any(|f| f.path == path) && files.len() as i64 >= limits::SKILL_FILES_MAX {
        return Err(files_limit());
    }
    files.retain(|f| f.path != path);
    files.push(repo::FileRow {
        skill_id: id,
        path: path.to_owned(),
        content: content.to_vec(),
        executable: false,
    });
    check_total(&cur.body, files.iter().map(|f| f.content.as_slice()))?;
    skill_format::check_file_entry(path, content).map_err(entry_too_large)?;
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
    let cur = lock_for_edit(&mut tx, id).await?;
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

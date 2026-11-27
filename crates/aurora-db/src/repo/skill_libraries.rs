//! Git skill libraries (phase 4, P4-GSL, D-148, D-149; docs/03 §skill_libraries,
//! §skill_library_updates, §skills library columns). A library is a *source*: only skills people
//! add are stored (pinned, read-only); a refresh writes proposals into `skill_library_updates`
//! and never touches `skills`.

use crate::DbError;
use crate::repo::skills::{COLS as SKILL_COLS, Owner, SkillRow};
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

const LIB_COLS: &str = "id, org_id, label, url, repo, ref, path, status, \
    last_head_sha, last_refresh_at, next_refresh_at, refresh_failures, last_error, \
    refresh_lease_until, lease_owner, check_requested_at, created_by_user_id, created_at, \
    updated_at, head_epoch, token_ref, token_updated_at, skills_offered, skills_unavailable";

#[derive(Clone, sqlx::FromRow)]
pub struct LibraryRow {
    pub id: Uuid,
    /// (P5-SKLIB-2, C-1128) Null = an instance library (managed by instance admins, visible to
    /// every organization).
    pub org_id: Option<Uuid>,
    pub label: String,
    /// (P5-SKLIB-1, C-1122) The normalized https clone URL; null only for a library migrated
    /// without one (C-1127).
    pub url: Option<String>,
    /// The display form `host/path` (null only with `url`).
    pub repo: Option<String>,
    #[sqlx(rename = "ref")]
    pub reference: Option<String>,
    pub path: String,
    pub status: String,
    pub last_head_sha: Option<String>,
    pub last_refresh_at: Option<DateTime<Utc>>,
    pub next_refresh_at: DateTime<Utc>,
    pub refresh_failures: i32,
    pub last_error: Option<String>,
    pub refresh_lease_until: Option<DateTime<Utc>>,
    pub lease_owner: Option<Uuid>,
    pub check_requested_at: Option<DateTime<Utc>>,
    pub created_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Bumped whenever `last_head_sha` is cleared by an add (FIX-096): a refresh that read an
    /// older epoch must not write its head back.
    pub head_epoch: i32,
    /// (P5-SKLIB-1, C-1126) The sealed token's reference: opened only by the repository
    /// reader, never returned (the `Debug` below leaves it out).
    pub token_ref: Option<String>,
    pub token_updated_at: Option<DateTime<Utc>>,
    /// (P5-SKILLS-1, C-1176) Valid and invalid skill folders at the last read.
    pub skills_offered: Option<i32>,
    pub skills_unavailable: Option<i32>,
}

impl std::fmt::Debug for LibraryRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryRow")
            .field("id", &self.id)
            .field("org_id", &self.org_id)
            .field("label", &self.label)
            .field("url", &self.url)
            .field("status", &self.status)
            .field("token_set", &self.token_ref.is_some())
            .finish_non_exhaustive()
    }
}

pub struct NewLibrary<'a> {
    /// The new row's id (chosen first: the token is sealed with it as owner).
    pub id: Uuid,
    /// `None` creates an instance library.
    pub org_id: Option<Uuid>,
    pub label: &'a str,
    pub url: &'a str,
    pub repo: &'a str,
    pub reference: Option<&'a str>,
    pub path: &'a str,
    pub token_ref: Option<&'a str>,
    pub head_sha: &'a str,
    pub created_by: Uuid,
    pub next_refresh_in_secs: f64,
}

pub async fn insert_library(
    conn: &mut PgConnection,
    n: &NewLibrary<'_>,
) -> Result<LibraryRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO skill_libraries (id, org_id, label, url, repo, ref, path, token_ref,
            token_updated_at, last_head_sha, last_refresh_at, next_refresh_at, created_by_user_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8, CASE WHEN $8::text IS NULL THEN NULL ELSE now() END,
                 $9, now(), now() + make_interval(secs => $10), $11)
         RETURNING {LIB_COLS}"
    )))
    .bind(n.id)
    .bind(n.org_id)
    .bind(n.label)
    .bind(n.url)
    .bind(n.repo)
    .bind(n.reference)
    .bind(n.path)
    .bind(n.token_ref)
    .bind(n.head_sha)
    .bind(n.next_refresh_in_secs)
    .bind(n.created_by)
    .fetch_one(conn)
    .await?)
}

/// Whether the scope (an organization, or `None` = the instance) already has a library of this
/// url + ref + path (C-1122's 409).
pub async fn url_taken(
    conn: &mut PgConnection,
    org: Option<Uuid>,
    url: &str,
    reference: Option<&str>,
    path: &str,
    except: Option<Uuid>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM skill_libraries WHERE org_id IS NOT DISTINCT FROM $1
           AND url = $2 AND coalesce(ref, '') = coalesce($3, '') AND path = $4
           AND id IS DISTINCT FROM $5)",
    )
    .bind(org)
    .bind(url)
    .bind(reference)
    .bind(path)
    .bind(except)
    .fetch_one(conn)
    .await?)
}

pub async fn get_library(conn: &mut PgConnection, id: Uuid) -> Result<Option<LibraryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {LIB_COLS} FROM skill_libraries WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn get_library_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<LibraryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {LIB_COLS} FROM skill_libraries WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn list_libraries(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Vec<LibraryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {LIB_COLS} FROM skill_libraries WHERE org_id = $1 ORDER BY label, id"
    )))
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// Instance libraries by label (every status; the caller filters for its audience).
pub async fn list_instance_libraries(conn: &mut PgConnection) -> Result<Vec<LibraryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {LIB_COLS} FROM skill_libraries WHERE org_id IS NULL ORDER BY label, id"
    )))
    .fetch_all(conn)
    .await?)
}

/// `(url, ref, path)` of every library that has a normalized address (P6-TPL-2: the template
/// views' `added_here` and C-1142's resolution).
pub async fn all_addresses<'e, E: sqlx::PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<(Option<String>, Option<String>, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT url, ref, path FROM skill_libraries WHERE url IS NOT NULL ORDER BY id",
    )
    .fetch_all(exec)
    .await?)
}

/// Libraries of the scope: an organization's, or (`None`) the instance's.
pub async fn count_libraries(conn: &mut PgConnection, org: Option<Uuid>) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM skill_libraries WHERE org_id IS NOT DISTINCT FROM $1",
    )
    .bind(org)
    .fetch_one(conn)
    .await?)
}

/// How many organizations have at least one skill added from the library.
pub async fn orgs_using(conn: &mut PgConnection, library: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(DISTINCT org_id) FROM skills WHERE library_id = $1")
            .bind(library)
            .fetch_one(conn)
            .await?,
    )
}

/// `(organization, skills detached)` for every organization with a skill added from the library
/// (the per-organization audit rows of an instance library's deletion).
pub async fn affected_orgs(
    conn: &mut PgConnection,
    library: Uuid,
) -> Result<Vec<(Uuid, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT org_id, count(*) FROM skills WHERE library_id = $1 GROUP BY org_id ORDER BY org_id",
    )
    .bind(library)
    .fetch_all(conn)
    .await?)
}

// ---- per-organization budget state (P5-SKLIB-2, C-1128) ---------------------------------------

/// Whether this organization's open proposals of the library are over its budget.
pub async fn org_over_budget(
    conn: &mut PgConnection,
    library: Uuid,
    org: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM skill_library_org_state WHERE library_id = $1 AND org_id = $2)",
    )
    .bind(library)
    .bind(org)
    .fetch_one(conn)
    .await?)
}

/// The libraries over budget for an organization.
pub async fn org_over_budget_libraries(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT library_id FROM skill_library_org_state WHERE org_id = $1")
            .bind(org)
            .fetch_all(conn)
            .await?,
    )
}

/// Whether any organization is over budget for the library (an unchanged head is then compared
/// again: space may be free).
pub async fn has_org_state(conn: &mut PgConnection, library: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM skill_library_org_state WHERE library_id = $1)",
    )
    .bind(library)
    .fetch_one(conn)
    .await?)
}

/// After a full pass: exactly `over` are over budget (rows added, rows of organizations that fit
/// again or no longer use the library deleted; `over_budget_since` of a kept row stays).
pub async fn set_org_states(
    conn: &mut PgConnection,
    library: Uuid,
    over: &[Uuid],
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM skill_library_org_state WHERE library_id = $1 AND org_id <> ALL($2)")
        .bind(library)
        .bind(over)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO skill_library_org_state (library_id, org_id)
         SELECT $1, o FROM unnest($2::uuid[]) AS o ON CONFLICT DO NOTHING",
    )
    .bind(library)
    .bind(over)
    .execute(conn)
    .await?;
    Ok(())
}

/// Changes an admin may make; `None` keeps the value. `reference`/`token_ref`: `Some(None)`
/// clears.
#[derive(Default)]
pub struct LibraryPatch<'a> {
    pub label: Option<&'a str>,
    pub status: Option<&'a str>,
    /// Only while the stored url is null (C-1122); with `repo`.
    pub url: Option<(&'a str, &'a str)>,
    pub reference: Option<Option<&'a str>>,
    pub path: Option<&'a str>,
    /// A new sealed token (`Some(Some)`) or its removal (`Some(None)`).
    pub token_ref: Option<Option<&'a str>>,
    /// Set with a re-read (url, ref, path or token change): the new head and a clean state.
    pub head_sha: Option<&'a str>,
}

pub async fn patch_library(
    conn: &mut PgConnection,
    id: Uuid,
    p: &LibraryPatch<'_>,
) -> Result<LibraryRow, DbError> {
    let (set_ref, ref_val) = match p.reference {
        Some(v) => (true, v),
        None => (false, None),
    };
    let (set_token, token_val) = match p.token_ref {
        Some(v) => (true, v),
        None => (false, None),
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE skill_libraries SET
            label = COALESCE($2, label),
            status = CASE WHEN $8 IS NOT NULL AND COALESCE($3, status) <> 'paused' THEN 'active'
                          ELSE COALESCE($3, status) END,
            url = COALESCE($4, url),
            repo = COALESCE($5, repo),
            ref = CASE WHEN $6 THEN $7 ELSE ref END,
            path = COALESCE($9, path),
            token_ref = CASE WHEN $10 THEN $11 ELSE token_ref END,
            token_updated_at = CASE WHEN $10 THEN (CASE WHEN $11::text IS NULL THEN NULL ELSE now() END)
                                    ELSE token_updated_at END,
            last_head_sha = COALESCE($8, last_head_sha),
            last_error = CASE WHEN $8 IS NOT NULL OR $3 = 'active' THEN NULL ELSE last_error END,
            refresh_failures = CASE WHEN $8 IS NOT NULL OR $3 = 'active' THEN 0
                                    ELSE refresh_failures END,
            next_refresh_at = CASE WHEN $8 IS NOT NULL OR $3 = 'active' THEN now()
                                   ELSE next_refresh_at END,
            updated_at = now()
         WHERE id = $1 RETURNING {LIB_COLS}"
    )))
    .bind(id)
    .bind(p.label)
    .bind(p.status)
    .bind(p.url.map(|u| u.0))
    .bind(p.url.map(|u| u.1))
    .bind(set_ref)
    .bind(ref_val)
    .bind(p.head_sha)
    .bind(p.path)
    .bind(set_token)
    .bind(token_val)
    .fetch_one(conn)
    .await?)
}

/// Stores what a read of the library found (C-1176). `updated_at` stays: this is the
/// repository's content, not a change of the library.
pub async fn set_offered(
    conn: &mut PgConnection,
    id: Uuid,
    offered: i32,
    unavailable: i32,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE skill_libraries SET skills_offered = $2, skills_unavailable = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(offered)
    .bind(unavailable)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_library(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM skill_libraries WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// "Check now": `Ok(false)` when one was requested within `min_gap_secs`; otherwise the library
/// becomes due at once.
pub async fn request_check(
    conn: &mut PgConnection,
    id: Uuid,
    min_gap_secs: f64,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE skill_libraries SET check_requested_at = now(), next_refresh_at = now()
         WHERE id = $1 AND (check_requested_at IS NULL
            OR check_requested_at < now() - make_interval(secs => $2))",
    )
    .bind(id)
    .bind(min_gap_secs)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Per tier, the number of skills added from the library; `org` limits the count to one
/// organization's skills (an instance library's per-organization view), `None` counts them all.
pub async fn skill_counts(
    conn: &mut PgConnection,
    library: Uuid,
    org: Option<Uuid>,
) -> Result<(i64, i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE agent_id IS NOT NULL),
                count(*) FILTER (WHERE workspace_id IS NOT NULL),
                count(*) FILTER (WHERE agent_id IS NULL AND workspace_id IS NULL)
         FROM skills WHERE library_id = $1 AND ($2::uuid IS NULL OR org_id = $2)",
    )
    .bind(library)
    .bind(org)
    .fetch_one(conn)
    .await?)
}

/// Open proposals of the library; `org` counts only those of one organization's skills (the
/// skill's organization, not the library's).
pub async fn pending_count(
    conn: &mut PgConnection,
    library: Uuid,
    org: Option<Uuid>,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM skill_library_updates u JOIN skills s ON s.id = u.skill_id
         WHERE u.library_id = $1 AND u.status = 'pending' AND ($2::uuid IS NULL OR s.org_id = $2)",
    )
    .bind(library)
    .bind(org)
    .fetch_one(conn)
    .await?)
}

/// Skills currently linked to the library (the read-only, pinned ones); `org` limits them to one
/// organization's skills.
pub async fn library_skills(
    conn: &mut PgConnection,
    library: Uuid,
    org: Option<Uuid>,
) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SKILL_COLS} FROM skills WHERE library_id = $1 AND ($2::uuid IS NULL OR org_id = $2)
         ORDER BY name, id"
    )))
    .bind(library)
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// Library-sourced skills of one tier of an org (budget accounting).
pub async fn tier_library_skills(
    conn: &mut PgConnection,
    org: Uuid,
    owner: Owner,
) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SKILL_COLS} FROM skills WHERE org_id = $1 AND library_id IS NOT NULL
           AND agent_id IS NOT DISTINCT FROM $2 AND workspace_id IS NOT DISTINCT FROM $3
         ORDER BY name, id"
    )))
    .bind(org)
    .bind(owner.agent)
    .bind(owner.workspace)
    .fetch_all(conn)
    .await?)
}

/// What a stored library skill remembers about its source.
#[derive(Debug, Clone)]
pub struct Source<'a> {
    pub library_id: Uuid,
    pub label: &'a str,
    pub repo: &'a str,
    pub path: &'a str,
    pub commit_sha: &'a str,
    pub blobs: &'a serde_json::Value,
    pub content_sha256: &'a [u8],
    pub pinned_by: Uuid,
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_library_skill(
    conn: &mut PgConnection,
    org: Uuid,
    owner: Owner,
    name: &str,
    description: &str,
    body: &str,
    src: &Source<'_>,
) -> Result<SkillRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO skills (id, org_id, agent_id, workspace_id, name, description, body, enabled,
            created_by_kind, library_id, library_label, library_repo, library_path,
            library_commit_sha, library_blobs, library_content_sha256, library_pinned_at,
            library_pinned_by_user_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,true,'user',$8,$9,$10,$11,$12,$13,$14,now(),$15)
         RETURNING {SKILL_COLS}"
    )))
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(owner.agent)
    .bind(owner.workspace)
    .bind(name)
    .bind(description)
    .bind(body)
    .bind(src.library_id)
    .bind(src.label)
    .bind(src.repo)
    .bind(src.path)
    .bind(src.commit_sha)
    .bind(src.blobs)
    .bind(src.content_sha256)
    .bind(src.pinned_by)
    .fetch_one(conn)
    .await?)
}

/// An approved update replaces the stored content and moves the pin.
#[allow(clippy::too_many_arguments)]
pub async fn apply_library_content(
    conn: &mut PgConnection,
    skill: Uuid,
    description: &str,
    body: &str,
    commit_sha: &str,
    blobs: &serde_json::Value,
    content_sha256: &[u8],
    pinned_by: Uuid,
) -> Result<SkillRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE skills SET description = $2, body = $3, library_commit_sha = $4,
            library_blobs = $5, library_content_sha256 = $6, library_pinned_at = now(),
            library_pinned_by_user_id = $7, updated_at = now()
         WHERE id = $1 AND library_id IS NOT NULL RETURNING {SKILL_COLS}"
    )))
    .bind(skill)
    .bind(description)
    .bind(body)
    .bind(commit_sha)
    .bind(blobs)
    .bind(content_sha256)
    .bind(pinned_by)
    .fetch_one(conn)
    .await?)
}

/// Replaces every file of a skill (approved update).
pub async fn replace_files(
    conn: &mut PgConnection,
    skill: Uuid,
    files: &[(String, Vec<u8>, bool)],
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM skill_files WHERE skill_id = $1")
        .bind(skill)
        .execute(&mut *conn)
        .await?;
    for (path, content, executable) in files {
        sqlx::query(
            "INSERT INTO skill_files (skill_id, path, content, executable) VALUES ($1,$2,$3,$4)",
        )
        .bind(skill)
        .bind(path)
        .bind(content)
        .bind(executable)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Detach one skill: it becomes an ordinary editable skill (text provenance stays), and its
/// pending update is removed.
pub async fn detach_skill(conn: &mut PgConnection, skill: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE skills SET library_id = NULL, library_blobs = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(skill)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM skill_library_updates WHERE skill_id = $1 AND status = 'pending'")
        .bind(skill)
        .execute(conn)
        .await?;
    Ok(())
}

/// Library deletion: detaches every skill of the library; returns how many.
pub async fn detach_all(conn: &mut PgConnection, library: Uuid) -> Result<u64, DbError> {
    let n = sqlx::query(
        "UPDATE skills SET library_id = NULL, library_blobs = NULL, updated_at = now()
         WHERE library_id = $1",
    )
    .bind(library)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM skill_library_updates WHERE library_id = $1 AND status = 'pending'")
        .bind(library)
        .execute(conn)
        .await?;
    Ok(n)
}

// ---- proposed updates ------------------------------------------------------------------------

const UPD_COLS: &str = "id, skill_id, library_id, kind, status, commit_sha, proposed, \
    content_sha256, reason, bytes, created_at, decided_at, decided_by_user_id";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpdateRow {
    pub id: Uuid,
    pub skill_id: Uuid,
    pub library_id: Option<Uuid>,
    pub kind: String,
    pub status: String,
    pub commit_sha: String,
    pub proposed: Option<serde_json::Value>,
    pub content_sha256: Option<Vec<u8>>,
    pub reason: Option<String>,
    pub bytes: i32,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by_user_id: Option<Uuid>,
}

pub struct NewUpdate<'a> {
    pub skill_id: Uuid,
    pub library_id: Uuid,
    pub kind: &'a str,
    pub commit_sha: &'a str,
    pub proposed: Option<&'a serde_json::Value>,
    pub content_sha256: &'a [u8],
    pub reason: Option<&'a str>,
    pub bytes: i32,
}

/// Records a proposal; a still-pending older one for the skill is superseded in the same step.
pub async fn insert_update(
    conn: &mut PgConnection,
    n: &NewUpdate<'_>,
) -> Result<UpdateRow, DbError> {
    sqlx::query(
        "UPDATE skill_library_updates SET status = 'superseded', decided_at = now()
         WHERE skill_id = $1 AND status = 'pending'",
    )
    .bind(n.skill_id)
    .execute(&mut *conn)
    .await?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha,
            proposed, content_sha256, reason, bytes)
         VALUES ($1,$2,$3,$4,'pending',$5,$6,$7,$8,$9) RETURNING {UPD_COLS}"
    )))
    .bind(Uuid::now_v7())
    .bind(n.skill_id)
    .bind(n.library_id)
    .bind(n.kind)
    .bind(n.commit_sha)
    .bind(n.proposed)
    .bind(n.content_sha256)
    .bind(n.reason)
    .bind(n.bytes)
    .fetch_one(conn)
    .await?)
}

pub async fn get_update(conn: &mut PgConnection, id: Uuid) -> Result<Option<UpdateRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPD_COLS} FROM skill_library_updates WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn get_update_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<UpdateRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPD_COLS} FROM skill_library_updates WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn pending_update(
    conn: &mut PgConnection,
    skill: Uuid,
) -> Result<Option<UpdateRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPD_COLS} FROM skill_library_updates WHERE skill_id = $1 AND status = 'pending'"
    )))
    .bind(skill)
    .fetch_optional(conn)
    .await?)
}

/// Pending proposals of a set of skills.
pub async fn pending_updates_of(
    conn: &mut PgConnection,
    skills: &[Uuid],
) -> Result<Vec<UpdateRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPD_COLS} FROM skill_library_updates
         WHERE skill_id = ANY($1) AND status = 'pending'"
    )))
    .bind(skills)
    .fetch_all(conn)
    .await?)
}

/// Bytes of the org's open proposals (`LIBRARY_PENDING_BYTES_PER_ORG`).
pub async fn pending_bytes_org(conn: &mut PgConnection, org: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(sum(u.bytes), 0)::bigint FROM skill_library_updates u
         JOIN skills s ON s.id = u.skill_id WHERE s.org_id = $1 AND u.status = 'pending'",
    )
    .bind(org)
    .fetch_one(conn)
    .await?)
}

/// The most recent rejected proposal of the skill with this hash: the repository state is not
/// proposed again.
/// The repository went back to the pinned state: an open proposal no longer applies.
pub async fn supersede_pending(conn: &mut PgConnection, skill: Uuid) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE skill_library_updates SET status = 'superseded', decided_at = now()
         WHERE skill_id = $1 AND status = 'pending'",
    )
    .bind(skill)
    .execute(conn)
    .await?
    .rows_affected())
}

/// All open proposals (the `aurora_skill_library_pending_updates` gauge).
pub async fn pending_total(conn: &mut PgConnection) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM skill_library_updates WHERE status = 'pending'")
            .fetch_one(conn)
            .await?,
    )
}

/// An add pinned a commit other than the library's last compared head: compare at the next
/// refresh whatever the head is.
pub async fn forget_head(
    conn: &mut PgConnection,
    library: Uuid,
    commit: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE skill_libraries SET last_head_sha = NULL, head_epoch = head_epoch + 1
         WHERE id = $1 AND last_head_sha IS DISTINCT FROM $2",
    )
    .bind(library)
    .bind(commit)
    .execute(conn)
    .await?;
    Ok(())
}

/// Heads kept per library (the current one and the most recent ones before it).
pub const HEADS_KEPT: i64 = 20;

/// Remembers that `commit` was the library ref's head (refresh, browse, create, patch): browse
/// and add accept only the current head and these (D-178 Q-R5-3). Keeps the newest `HEADS_KEPT`.
pub async fn record_head(
    conn: &mut PgConnection,
    library: Uuid,
    commit: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO skill_library_heads (library_id, commit_sha) VALUES ($1, $2)
         ON CONFLICT (library_id, commit_sha) DO UPDATE SET seen_at = now(), updated_at = now()",
    )
    .bind(library)
    .bind(commit)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM skill_library_heads WHERE library_id = $1 AND commit_sha NOT IN
           (SELECT commit_sha FROM skill_library_heads WHERE library_id = $1
            ORDER BY seen_at DESC, commit_sha LIMIT $2)",
    )
    .bind(library)
    .bind(HEADS_KEPT)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether `commit` is a head this library has recorded.
pub async fn is_recorded_head(
    conn: &mut PgConnection,
    library: Uuid,
    commit: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM skill_library_heads WHERE library_id = $1 AND commit_sha = $2)",
    )
    .bind(library)
    .bind(commit)
    .fetch_one(conn)
    .await?)
}

pub async fn was_rejected(
    conn: &mut PgConnection,
    skill: Uuid,
    content_sha256: &[u8],
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM skill_library_updates
           WHERE skill_id = $1 AND status = 'rejected' AND content_sha256 = $2)",
    )
    .bind(skill)
    .bind(content_sha256)
    .fetch_one(conn)
    .await?)
}

pub async fn decide_update(
    conn: &mut PgConnection,
    id: Uuid,
    status: &str,
    user: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE skill_library_updates SET status = $2, decided_at = now(), decided_by_user_id = $3
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(user)
    .execute(conn)
    .await?;
    Ok(())
}

/// A pending-update row with the skill's scope, for the review badges.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingRow {
    pub update_id: Uuid,
    pub skill_id: Uuid,
    pub library_id: Option<Uuid>,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub name: String,
    pub agent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub library_label: Option<String>,
}

const PENDING_SELECT: &str = "SELECT u.id AS update_id, u.skill_id, u.library_id, u.kind, \
    u.created_at, s.name, s.agent_id, s.workspace_id, s.library_label \
    FROM skill_library_updates u JOIN skills s ON s.id = u.skill_id \
    WHERE u.status = 'pending'";

pub async fn pending_for_org(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Vec<PendingRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PENDING_SELECT} AND s.org_id = $1 ORDER BY u.created_at, u.id"
    )))
    .bind(org)
    .fetch_all(conn)
    .await?)
}

pub async fn pending_for_workspace(
    conn: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<PendingRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PENDING_SELECT} AND s.workspace_id = $1 ORDER BY u.created_at, u.id"
    )))
    .bind(workspace)
    .fetch_all(conn)
    .await?)
}

pub async fn pending_for_agent(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<PendingRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PENDING_SELECT} AND s.agent_id = $1 ORDER BY u.created_at, u.id"
    )))
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

// ---- refresh job -----------------------------------------------------------------------------

/// Claims up to `limit` due libraries that have at least one added skill under a per-library
/// lease (`FOR UPDATE SKIP LOCKED`): two replicas never refresh one library.
pub async fn claim_due(
    conn: &mut PgConnection,
    owner: Uuid,
    limit: i64,
    lease_secs: f64,
) -> Result<Vec<LibraryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE skill_libraries SET refresh_lease_until = now() + make_interval(secs => $3),
            lease_owner = $1
         WHERE id IN (
            SELECT l.id FROM skill_libraries l
            WHERE l.status IN ('active', 'error', 'over_budget') AND l.next_refresh_at <= now()
              AND (l.refresh_lease_until IS NULL OR l.refresh_lease_until < now())
              AND EXISTS (SELECT 1 FROM skills s WHERE s.library_id = l.id)
            ORDER BY l.next_refresh_at LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING {LIB_COLS}"
    )))
    .bind(owner)
    .bind(limit)
    .bind(lease_secs)
    .fetch_all(conn)
    .await?)
}

/// Outcome of one refresh, written only by the lease owner.
pub struct RefreshDone<'a> {
    pub id: Uuid,
    pub owner: Uuid,
    pub head_sha: Option<&'a str>,
    /// The library's `head_epoch` when the pass was claimed: the head is written only if no add
    /// cleared it since (compare-and-set).
    pub head_epoch: i32,
    /// `active` or `error` (the budget is per organization, `skill_library_org_state`; the library's own
    /// status never becomes `over_budget` since P5-SKLIB-2).
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub failures: i32,
    pub next_in_secs: f64,
}

pub async fn finish_refresh(conn: &mut PgConnection, d: &RefreshDone<'_>) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE skill_libraries SET status = $3, last_error = $4, refresh_failures = $5,
            last_head_sha = CASE WHEN head_epoch <> $8 THEN last_head_sha
                                 ELSE COALESCE($6, last_head_sha) END,
            last_refresh_at = now(),
            next_refresh_at = now() + make_interval(secs => $7),
            refresh_lease_until = NULL, lease_owner = NULL, updated_at = now()
         WHERE id = $1 AND lease_owner = $2 AND status <> 'paused'",
    )
    .bind(d.id)
    .bind(d.owner)
    .bind(d.status)
    .bind(d.error)
    .bind(d.failures)
    .bind(d.head_sha)
    .bind(d.next_in_secs)
    .bind(d.head_epoch)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Releases a lease without recording an outcome (shutdown).
pub async fn release_lease(conn: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE skill_libraries SET refresh_lease_until = NULL, lease_owner = NULL
         WHERE id = $1 AND lease_owner = $2",
    )
    .bind(id)
    .bind(owner)
    .execute(conn)
    .await?;
    Ok(())
}

/// Daily retention: prune decided proposals older than 90 days and expire stale leases.
/// Returns `(pruned, 0)` (P5-SKLIB-1: libraries no longer have a connection to lose).
pub async fn retention(conn: &mut PgConnection) -> Result<(u64, u64), DbError> {
    let pruned = sqlx::query(
        "DELETE FROM skill_library_updates
         WHERE status <> 'pending' AND decided_at < now() - interval '90 days'",
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE skill_libraries SET refresh_lease_until = NULL, lease_owner = NULL
         WHERE refresh_lease_until < now()",
    )
    .execute(conn)
    .await?;
    Ok((pruned, 0))
}

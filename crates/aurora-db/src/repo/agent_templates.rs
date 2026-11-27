//! Agent template rows (03 §agent templates, P6-TPL-2, C-1106): the sources, the templates of a
//! source, the ingested versions and the organizations' switches.

use crate::DbError;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

const SEL: &str = "id, name, url, ref, path, token_ref, token_updated_at, homepage, contact,
    packages_url, manifest, last_head_sha, update_interval, status, check_failed_at,
    check_failed_reason, last_checked_at, next_check_at, check_lease_until, lease_owner,
    created_by, created_at, updated_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SourceRow {
    pub id: Uuid,
    /// From the manifest's `repo.name`, cleaned.
    pub name: String,
    /// The stored, normalized https git address (C-1123 form).
    pub url: String,
    #[sqlx(rename = "ref")]
    pub reference: Option<String>,
    /// The folder in the repository (may be the root).
    pub path: String,
    pub token_ref: Option<String>,
    pub token_updated_at: Option<DateTime<Utc>>,
    pub homepage: Option<String>,
    pub contact: Option<String>,
    /// The manifest's `packages.url` template (`{id}`, `{version}`).
    pub packages_url: String,
    /// The last accepted manifest as parsed (the dashboard reads this, never the live source).
    pub manifest: Value,
    pub last_head_sha: Option<String>,
    pub update_interval: String,
    pub status: String,
    pub check_failed_at: Option<DateTime<Utc>>,
    pub check_failed_reason: Option<String>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub next_check_at: DateTime<Utc>,
    pub check_lease_until: Option<DateTime<Utc>>,
    pub lease_owner: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One template of a source, as `template.yaml` at the head said (03 §`agent_templates`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TemplateRow {
    pub id: Uuid,
    pub template_id: String,
    pub source_id: Uuid,
    pub dir: String,
    pub name: String,
    pub summary: String,
    pub head_version: Option<String>,
    /// `ok` | `needs_newer_aurora` | `invalid`.
    pub head_state: String,
    pub head_problems: Option<Value>,
    pub offered: bool,
    pub removed_upstream: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewSource<'a> {
    pub id: Uuid,
    pub name: &'a str,
    pub url: &'a str,
    pub reference: Option<&'a str>,
    pub path: &'a str,
    /// The sealed token's column form (`None`: no token).
    pub token_ref: Option<&'a str>,
    pub homepage: Option<&'a str>,
    pub contact: Option<&'a str>,
    pub packages_url: &'a str,
    /// The manifest as parsed.
    pub manifest: &'a Value,
    pub update_interval: &'a str,
    /// Seconds until the first scheduled check (`None`: never scheduled, manual only).
    pub first_check_in_secs: Option<i64>,
    pub created_by: Uuid,
}

/// Insert a source (C-1107). `last_head_sha` stays null: it is the head the last successful
/// *check* compared (docs/03), so the first check after the add runs the full C-1108 pass and
/// records the templates and candidates the add read.
pub async fn insert_source(
    conn: &mut PgConnection,
    s: &NewSource<'_>,
) -> Result<SourceRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO agent_template_sources (id, name, url, ref, path, token_ref,
            token_updated_at, homepage, contact, packages_url, manifest,
            update_interval, next_check_at, last_checked_at, created_by)
         VALUES ($1,$2,$3,$4,$5,$6, CASE WHEN $6 IS NULL THEN NULL ELSE now() END,
            $7,$8,$9,$10,$11,
            now() + make_interval(secs => COALESCE($12, 31536000)), NULL, $13)
         RETURNING {SEL}"
    )))
    .bind(s.id)
    .bind(s.name)
    .bind(s.url)
    .bind(s.reference)
    .bind(s.path)
    .bind(s.token_ref)
    .bind(s.homepage)
    .bind(s.contact)
    .bind(s.packages_url)
    .bind(s.manifest)
    .bind(s.update_interval)
    .bind(s.first_check_in_secs)
    .bind(s.created_by)
    .fetch_one(conn)
    .await?)
}

pub async fn get_source<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<SourceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SEL} FROM agent_template_sources WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Whether this exact address (url + ref + path) is already a source (C-1107's 409).
pub async fn addr_taken<'e, E: PgExecutor<'e>>(
    exec: E,
    url: &str,
    reference: Option<&str>,
    path: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agent_template_sources
            WHERE url = $1 AND coalesce(ref, '') = coalesce($2, '') AND path = $3)",
    )
    .bind(url)
    .bind(reference)
    .bind(path)
    .fetch_one(exec)
    .await?)
}

/// The source holding each of `template_ids` (the `id_taken` rule: one source holds a template
/// id): `(template_id, source_id)` for the ids that exist.
pub async fn template_holders<'e, E: PgExecutor<'e>>(
    exec: E,
    template_ids: &[String],
) -> Result<Vec<(String, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT template_id, source_id FROM agent_templates WHERE template_id = ANY($1)",
    )
    .bind(template_ids)
    .fetch_all(exec)
    .await?)
}

pub async fn list_sources<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<SourceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SEL} FROM agent_template_sources ORDER BY created_at, id"
    )))
    .fetch_all(exec)
    .await?)
}

pub async fn count_sources<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM agent_template_sources")
            .fetch_one(exec)
            .await?,
    )
}

/// The templates of one source, oldest first (the source detail's list).
pub async fn templates_of<'e, E: PgExecutor<'e>>(
    exec: E,
    source_id: Uuid,
) -> Result<Vec<TemplateRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, template_id, source_id, dir, name, summary, head_version, head_state,
            head_problems, offered, removed_upstream, created_at, updated_at
         FROM agent_templates WHERE source_id = $1 ORDER BY created_at, id",
    )
    .bind(source_id)
    .fetch_all(exec)
    .await?)
}

/// Whether each of a source's templates has at least one version row (the `ingested` state):
/// `(agent_templates.id, version count)`.
pub async fn version_counts<'e, E: PgExecutor<'e>>(
    exec: E,
    source_id: Uuid,
) -> Result<Vec<(Uuid, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.id, count(*) FROM agent_templates t
         JOIN agent_template_versions v ON v.agent_template_id = t.id
         WHERE t.source_id = $1 GROUP BY t.id",
    )
    .bind(source_id)
    .fetch_all(exec)
    .await?)
}

/// Every template of every source (the admin list), in creation order; the source is
/// `template.source_id`.
pub async fn all_templates<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<TemplateRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, template_id, source_id, dir, name, summary, head_version, head_state,
            head_problems, offered, removed_upstream, created_at, updated_at
         FROM agent_templates ORDER BY created_at, id",
    )
    .fetch_all(exec)
    .await?)
}

/// Update the source's schedule; the next scheduled check moves with it (`next_in_secs`,
/// `None` = never).
pub async fn update_interval(
    conn: &mut PgConnection,
    id: Uuid,
    interval: &str,
    next_in_secs: Option<i64>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_template_sources SET update_interval = $2, updated_at = now(),
            next_check_at = now() + make_interval(secs => COALESCE($3, 31536000))
         WHERE id = $1",
    )
    .bind(id)
    .bind(interval)
    .bind(next_in_secs)
    .execute(conn)
    .await?;
    Ok(())
}

/// The new address parts of a patch that re-read the repository (C-1107).
pub struct NewAddress<'a> {
    pub reference: Option<&'a str>,
    pub path: &'a str,
    /// The sealed token's column form (`None`: no token).
    pub token_ref: Option<&'a str>,
    pub packages_url: &'a str,
    /// The manifest as parsed.
    pub manifest: &'a Value,
}

/// Replace the source's address parts (a patch that re-read the repository first). The head
/// is not written: `last_head_sha` is the head the last successful *check* compared
/// (docs/03), so the next check compares freshly and records what the new address holds.
pub async fn update_address(
    conn: &mut PgConnection,
    id: Uuid,
    a: &NewAddress<'_>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_template_sources SET ref = $2, path = $3,
            token_ref = $4,
            token_updated_at = CASE WHEN $4 IS NULL THEN NULL ELSE now() END,
            packages_url = $5, manifest = $6, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(a.reference)
    .bind(a.path)
    .bind(a.token_ref)
    .bind(a.packages_url)
    .bind(a.manifest)
    .execute(conn)
    .await?;
    Ok(())
}

/// Replace only the token (a patch that changed the token, nothing else).
pub async fn update_token(
    conn: &mut PgConnection,
    id: Uuid,
    token_ref: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_template_sources SET token_ref = $2,
            token_updated_at = CASE WHEN $2 IS NULL THEN NULL ELSE now() END, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(token_ref)
    .execute(conn)
    .await?;
    Ok(())
}

/// Delete a source row; its templates and versions go with it (`ON DELETE CASCADE`), the
/// organizations' switches stay (they are keyed by the template id).
pub async fn delete_source(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM agent_template_sources WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

// ---- the lease (C-1107, C-598's shape: shared with the addon sources) ------------------------

/// Every source that is due (`next_check_at <= now()`, lease free), claiming the lease.
pub async fn claim_due(
    conn: &mut PgConnection,
    owner: Uuid,
    limit: i64,
    lease_secs: f64,
) -> Result<Vec<SourceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE agent_template_sources s SET check_lease_until = now() + make_interval(secs => $3),
            lease_owner = $1, updated_at = now()
         WHERE s.id IN (
            SELECT id FROM agent_template_sources
            WHERE next_check_at <= now()
              AND (check_lease_until IS NULL OR check_lease_until < now())
            ORDER BY next_check_at LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING {SEL}"
    )))
    .bind(owner)
    .bind(limit)
    .bind(lease_secs)
    .fetch_all(conn)
    .await?)
}

/// Claim one source for a manual check (the same lease); `None` when busy or missing.
pub async fn claim_one(
    conn: &mut PgConnection,
    id: Uuid,
    owner: Uuid,
    lease_secs: f64,
) -> Result<Option<SourceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE agent_template_sources s SET check_lease_until = now() + make_interval(secs => $3),
            lease_owner = $1, updated_at = now()
         WHERE s.id = $2 AND (check_lease_until IS NULL OR check_lease_until < now())
         RETURNING {SEL}"
    )))
    .bind(owner)
    .bind(id)
    .bind(lease_secs)
    .fetch_optional(conn)
    .await?)
}

/// Only the lease owner releases without an outcome (shutdown, a panicking check).
pub async fn release_lease(conn: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_template_sources SET check_lease_until = NULL, lease_owner = NULL,
            updated_at = now()
         WHERE id = $1 AND lease_owner = $2",
    )
    .bind(id)
    .bind(owner)
    .execute(conn)
    .await?;
    Ok(())
}

/// A still-running check extends its own lease (the 5-minute lease does not cover a source
/// with many templates downloading packages). Only the current owner succeeds: `false`
/// means the lease was lost — another replica claimed the source — and the caller aborts.
pub async fn extend_lease(
    conn: &mut PgConnection,
    id: Uuid,
    owner: Uuid,
    lease_secs: f64,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE agent_template_sources SET check_lease_until = now() + make_interval(secs => $3),
            updated_at = now()
         WHERE id = $1 AND lease_owner = $2",
    )
    .bind(id)
    .bind(owner)
    .bind(lease_secs)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Outcome of one check (the lease owner writes it). The templates and versions are written
/// by the check itself; this only records the source's own state.
pub struct CheckDone {
    pub id: Uuid,
    pub owner: Uuid,
    /// `ok` | `check_failed`.
    pub status: &'static str,
    pub reason: Option<String>,
    /// Seconds until the next scheduled check (the interval, the package retry or the
    /// failure backoff).
    pub next_in_secs: i64,
}

pub async fn finish_check(conn: &mut PgConnection, d: &CheckDone) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_template_sources SET
            status = $3,
            check_failed_at = CASE WHEN $3 = 'ok' THEN NULL ELSE now() END,
            check_failed_reason = $4,
            last_checked_at = now(),
            next_check_at = now() + make_interval(secs => $5),
            check_lease_until = NULL,
            lease_owner = NULL,
            updated_at = now()
         WHERE id = $1 AND lease_owner = $2",
    )
    .bind(d.id)
    .bind(d.owner)
    .bind(d.status)
    .bind(d.reason.clone())
    .bind(d.next_in_secs)
    .execute(conn)
    .await?;
    Ok(())
}

/// Set the head commit the last completed check read (C-1108 step 7, written with the check
/// outcome).
pub async fn set_head_sha(conn: &mut PgConnection, id: Uuid, sha: &str) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_template_sources SET last_head_sha = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(sha)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether a version has waited past `wait_secs` since its `created_at`. The comparison runs
/// against the database's `now()`: the row's `created_at` is the database's clock, and a
/// replica's clock skew must not move the package-wait expiry.
pub async fn package_wait_expired<'e, E: PgExecutor<'e>>(
    exec: E,
    version_id: Uuid,
    wait_secs: i64,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT created_at + make_interval(secs => $2) <= now()
         FROM agent_template_versions WHERE id = $1",
    )
    .bind(version_id)
    .bind(wait_secs)
    .fetch_one(exec)
    .await?)
}

// ---- the templates of a source ---------------------------------------------------------------

/// Whether a template id is held by a template of *another* source (the `id_taken` rule).
pub async fn id_taken_elsewhere<'e, E: PgExecutor<'e>>(
    exec: E,
    template_id: &str,
    source_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agent_templates
            WHERE template_id = $1 AND source_id <> $2)",
    )
    .bind(template_id)
    .bind(source_id)
    .fetch_one(exec)
    .await?)
}

/// A stored template of one source by its template id.
pub async fn template_by_id<'e, E: PgExecutor<'e>>(
    exec: E,
    source_id: Uuid,
    template_id: &str,
) -> Result<Option<TemplateRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, template_id, source_id, dir, name, summary, head_version, head_state,
            head_problems, offered, removed_upstream, created_at, updated_at
         FROM agent_templates WHERE source_id = $1 AND template_id = $2",
    )
    .bind(source_id)
    .bind(template_id)
    .fetch_optional(exec)
    .await?)
}

pub struct NewTemplate<'a> {
    pub id: Uuid,
    pub template_id: &'a str,
    pub source_id: Uuid,
    pub dir: &'a str,
    pub name: &'a str,
    pub summary: &'a str,
    pub head_version: Option<&'a str>,
    pub head_state: &'a str,
}

/// Store a template the head read found (a new one, or a stored one whose head is `ok` or
/// `needs_newer_aurora`): the head's description replaces the stored one and the directory
/// is back (`removed_upstream` cleared).
///
/// The upsert never changes the row's owner: another source may have taken the template id
/// between the caller's unlocked `id_taken_elsewhere` read and this write (the ownership
/// race), and the `DO UPDATE ... WHERE` refuses to hand the template — with all its stored
/// versions — over to the newcomer. `None` means the caller must report `id_taken`, exactly
/// like the pre-check branch.
pub async fn upsert_template(
    conn: &mut PgConnection,
    t: &NewTemplate<'_>,
) -> Result<Option<TemplateRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(
        "INSERT INTO agent_templates (id, template_id, source_id, dir, name, summary,
            head_version, head_state, removed_upstream)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,false)
         ON CONFLICT (template_id) DO UPDATE SET dir = EXCLUDED.dir,
            name = EXCLUDED.name, summary = EXCLUDED.summary,
            head_version = EXCLUDED.head_version, head_state = EXCLUDED.head_state,
            head_problems = NULL, removed_upstream = false, updated_at = now()
         WHERE agent_templates.source_id = EXCLUDED.source_id
         RETURNING id, template_id, source_id, dir, name, summary, head_version, head_state,
            head_problems, offered, removed_upstream, created_at, updated_at"
            .to_owned(),
    ))
    .bind(t.id)
    .bind(t.template_id)
    .bind(t.source_id)
    .bind(t.dir)
    .bind(t.name)
    .bind(t.summary)
    .bind(t.head_version)
    .bind(t.head_state)
    .fetch_optional(conn)
    .await?)
}

/// A stored template's head turned `invalid` (or `needs_newer_aurora`): its state and the
/// head problems change; the stored versions and the description stay as they are.
pub async fn set_head_state(
    conn: &mut PgConnection,
    id: Uuid,
    head_state: &str,
    problems: Option<&Value>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_templates SET head_state = $2, head_problems = $3, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(head_state)
    .bind(problems)
    .execute(conn)
    .await?;
    Ok(())
}

/// The directory of a stored template disappeared (or came back): the flag changes, the
/// template stays as it is and the admin decides.
pub async fn set_removed_upstream(
    conn: &mut PgConnection,
    id: Uuid,
    removed: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_templates SET removed_upstream = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(removed)
    .execute(conn)
    .await?;
    Ok(())
}

// ---- the versions ------------------------------------------------------------------------------

const VSEL: &str = "id, agent_template_id, version, status, status_reason, commit_sha,
    package_url, archive_sha256, size_bytes, content_sha256, content, last_attempt_at,
    decided_by, decided_at, created_at";

/// [`VSEL`] qualified for the joined query ([`versions_of_source`], which joins
/// `agent_templates` and needs every column attributed).
const VSELQ: &str = "v.id, v.agent_template_id, v.version, v.status, v.status_reason,
    v.commit_sha, v.package_url, v.archive_sha256, v.size_bytes, v.content_sha256,
    v.content, v.last_attempt_at, v.decided_by, v.decided_at, v.created_at";

/// One ingested version of a template (03 §`agent_template_versions`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VersionRow {
    pub id: Uuid,
    pub agent_template_id: Uuid,
    pub version: String,
    /// `awaiting_package` | `pending_approval` | `approved` | `rejected` | `invalid`.
    pub status: String,
    pub status_reason: Option<String>,
    /// The head commit where `template.yaml` had this version.
    pub commit_sha: String,
    pub package_url: String,
    pub archive_sha256: Option<String>,
    pub size_bytes: Option<i32>,
    pub content_sha256: Option<String>,
    pub content: Option<Value>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Every version row of one source (any status), with the template id and folder: the check
/// reads this for the candidate rule and the `awaiting_package` retries.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SourceVersionRow {
    #[sqlx(flatten)]
    pub version: VersionRow,
    pub template_id: String,
    pub dir: String,
}

pub async fn versions_of_source<'e, E: PgExecutor<'e>>(
    exec: E,
    source_id: Uuid,
) -> Result<Vec<SourceVersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VSELQ}, t.template_id, t.dir
         FROM agent_template_versions v
         JOIN agent_templates t ON t.id = v.agent_template_id
         WHERE t.source_id = $1
         ORDER BY v.created_at, v.id"
    )))
    .bind(source_id)
    .fetch_all(exec)
    .await?)
}

pub struct NewVersion<'a> {
    pub id: Uuid,
    pub agent_template_id: Uuid,
    pub version: &'a str,
    /// The head commit where `template.yaml` had this version.
    pub commit_sha: &'a str,
    pub package_url: &'a str,
}

/// Record a candidate version as `awaiting_package` (C-1108 step 4). A version string is
/// unique per template; the lease and the candidate rule keep races away, and the unique
/// violation maps to the caller's `version_not_newer` handling.
pub async fn insert_version<'e, E: PgExecutor<'e>>(
    exec: E,
    v: &NewVersion<'_>,
) -> Result<VersionRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO agent_template_versions (id, agent_template_id, version, status,
            commit_sha, package_url)
         VALUES ($1,$2,$3,'awaiting_package',$4,$5)
         RETURNING {VSEL}"
    )))
    .bind(v.id)
    .bind(v.agent_template_id)
    .bind(v.version)
    .bind(v.commit_sha)
    .bind(v.package_url)
    .fetch_one(exec)
    .await?)
}

/// What a version became after the check processed it (C-1108 steps 5–6). `content` is set
/// exactly when the row leaves `awaiting_package` as `pending_approval` (the CHECK holds).
#[allow(clippy::too_many_arguments)]
/// A check's write of one `awaiting_package` row's outcome (C-1108 step 7). Guarded by the
/// status: a check that lost its lease while it worked (another check re-claimed the source)
/// must not overwrite what the new owner or an admin has since written — e.g. a version the
/// new check ingested and an admin already approved. Zero rows matched means exactly that:
/// the caller must abort its transaction (`check.rs` does).
pub async fn finish_version(
    conn: &mut PgConnection,
    id: Uuid,
    status: &str,
    status_reason: Option<&str>,
    content: Option<&Value>,
    content_sha256: Option<&str>,
    archive_sha256: Option<&str>,
    size_bytes: Option<i32>,
    attempted: bool,
) -> Result<u64, DbError> {
    let res = sqlx::query(sqlx::AssertSqlSafe(
        "UPDATE agent_template_versions SET status = $2, status_reason = $3,
            content = $4, content_sha256 = $5, archive_sha256 = $6, size_bytes = $7,
            last_attempt_at = CASE WHEN $8 THEN now() ELSE last_attempt_at END
         WHERE id = $1 AND status = 'awaiting_package'"
            .to_owned(),
    ))
    .bind(id)
    .bind(status)
    .bind(status_reason)
    .bind(content)
    .bind(content_sha256)
    .bind(archive_sha256)
    .bind(size_bytes)
    .bind(attempted)
    .execute(conn)
    .await?;
    Ok(res.rows_affected())
}

/// The versions of a template, newest first (the admin list, C-1108).
pub async fn versions_of<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_template_id: Uuid,
) -> Result<Vec<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VSEL} FROM agent_template_versions
         WHERE agent_template_id = $1 ORDER BY created_at DESC, id DESC"
    )))
    .bind(agent_template_id)
    .fetch_all(exec)
    .await?)
}

/// One version of one template (the review, approve and reject routes).
pub async fn get_version<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_template_id: Uuid,
    version: &str,
) -> Result<Option<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VSEL} FROM agent_template_versions
         WHERE agent_template_id = $1 AND version = $2"
    )))
    .bind(agent_template_id)
    .bind(version)
    .fetch_optional(exec)
    .await?)
}

/// One version row by its id (the avatar route, the review of the check's writes).
pub async fn version_by_id<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VSEL} FROM agent_template_versions WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

// ---- the admin list and the review (C-1108) ----------------------------------------------------

/// A template with its source's display name (the admin list's `source: { id, name }`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TemplateWithSourceRow {
    #[sqlx(flatten)]
    pub template: TemplateRow,
    pub source_name: String,
}

/// Every template of every source with the source's name, in creation order (C-1108).
pub async fn all_templates_with_source<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<TemplateWithSourceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.template_id, t.source_id, t.dir, t.name, t.summary, t.head_version,
            t.head_state, t.head_problems, t.offered, t.removed_upstream, t.created_at,
            t.updated_at, s.name AS source_name
         FROM agent_templates t JOIN agent_template_sources s ON s.id = t.source_id
         ORDER BY t.created_at, t.id",
    )
    .fetch_all(exec)
    .await?)
}

/// A stored template by its template id (whichever source holds it).
pub async fn template_by_template_id<'e, E: PgExecutor<'e>>(
    exec: E,
    template_id: &str,
) -> Result<Option<TemplateRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, template_id, source_id, dir, name, summary, head_version, head_state,
            head_problems, offered, removed_upstream, created_at, updated_at
         FROM agent_templates WHERE template_id = $1",
    )
    .bind(template_id)
    .fetch_optional(exec)
    .await?)
}

/// [`template_by_template_id`] with FOR SHARE on the row (SEC-146 B): the create's transaction
/// pins the template row (its `offered` flag included) before re-reading the offer, so a
/// concurrent withdrawal or approval is either seen or waits behind the lock.
pub async fn template_by_template_id_share<'e, E: PgExecutor<'e>>(
    exec: E,
    template_id: &str,
) -> Result<Option<TemplateRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, template_id, source_id, dir, name, summary, head_version, head_state,
            head_problems, offered, removed_upstream, created_at, updated_at
         FROM agent_templates WHERE template_id = $1 FOR SHARE",
    )
    .bind(template_id)
    .fetch_optional(exec)
    .await?)
}

/// A version with its decider's display name (the admin list's `decided_by`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VersionWithDeciderRow {
    #[sqlx(flatten)]
    pub version: VersionRow,
    pub decided_by_name: Option<String>,
}

/// The versions of a template, newest first, with the decider's display name (C-1108).
pub async fn versions_with_decider<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_template_id: Uuid,
) -> Result<Vec<VersionWithDeciderRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VSELQ}, u.display_name AS decided_by_name
         FROM agent_template_versions v
         LEFT JOIN users u ON u.id = v.decided_by
         WHERE v.agent_template_id = $1
         ORDER BY v.created_at DESC, v.id DESC"
    )))
    .bind(agent_template_id)
    .fetch_all(exec)
    .await?)
}

/// The approved versions of an *offered* template, newest first (the offered version is the
/// semver-newest of these; the caller picks it, one rule for every reader, C-1108).
pub async fn approved_versions<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_template_id: Uuid,
) -> Result<Vec<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VSELQ}
         FROM agent_template_versions v
         JOIN agent_templates t ON t.id = v.agent_template_id
         WHERE v.agent_template_id = $1 AND t.offered AND v.status = 'approved'
         ORDER BY v.created_at DESC, v.id DESC"
    )))
    .bind(agent_template_id)
    .fetch_all(exec)
    .await?)
}

/// [`approved_versions`] with FOR SHARE on the version rows (SEC-146 B): the create's
/// transaction re-reads the offer under the row lock, so a concurrent approval of a newer
/// version is either seen by the re-read or waits behind it.
pub async fn approved_versions_share<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_template_id: Uuid,
) -> Result<Vec<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VSELQ}
         FROM agent_template_versions v
         JOIN agent_templates t ON t.id = v.agent_template_id
         WHERE v.agent_template_id = $1 AND t.offered AND v.status = 'approved'
         ORDER BY v.created_at DESC, v.id DESC
         FOR SHARE OF v"
    )))
    .bind(agent_template_id)
    .fetch_all(exec)
    .await?)
}

/// (FIX-251, D-488(a)) The approved version strings of the given templates (the templates'
/// business id as `agents.template_id` stores it, and the version), newest stored first: one
/// small read the agents list marks every agent whose template offers a semver-newer version
/// than the agent's stored one with (the offered version is the semver-newest approved one,
/// C-1108 — the caller picks it, not the row order).
pub async fn approved_version_strings<'e, E: PgExecutor<'e>>(
    exec: E,
    template_ids: &[String],
) -> Result<Vec<(String, String)>, DbError> {
    if template_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT t.template_id, v.version \
         FROM agent_template_versions v \
         JOIN agent_templates t ON t.id = v.agent_template_id \
         WHERE t.offered AND v.status = 'approved' AND t.template_id = ANY($1) \
         ORDER BY v.created_at DESC, v.id DESC",
    )
    .bind(template_ids)
    .fetch_all(exec)
    .await?)
}

/// Approve a version (C-1108): only from `pending_approval` and only when `content_sha256`
/// is still the stored one. `Ok(false)` means the row did not match (the caller has already
/// read it and answers 409 `template_content_changed`).
pub async fn approve(
    conn: &mut PgConnection,
    id: Uuid,
    content_sha256: &str,
    decided_by: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE agent_template_versions SET status = 'approved', status_reason = NULL,
            decided_by = $3, decided_at = now()
         WHERE id = $1 AND status = 'pending_approval' AND content_sha256 = $2",
    )
    .bind(id)
    .bind(content_sha256)
    .bind(decided_by)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Reject a version (C-1108): only from `pending_approval`; that version string is never
/// ingested again. `Ok(false)` means the row was not in `pending_approval`.
pub async fn reject(conn: &mut PgConnection, id: Uuid, decided_by: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE agent_template_versions SET status = 'rejected', status_reason = NULL,
            decided_by = $2, decided_at = now()
         WHERE id = $1 AND status = 'pending_approval'",
    )
    .bind(id)
    .bind(decided_by)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Withdraw or offer again a template (C-1108's PATCH). The stored versions are untouched:
/// the offered version is a rule over them.
pub async fn set_offered(conn: &mut PgConnection, id: Uuid, offered: bool) -> Result<(), DbError> {
    sqlx::query("UPDATE agent_templates SET offered = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(offered)
        .execute(conn)
        .await?;
    Ok(())
}

// ---- the organizations' switches (C-1109) ------------------------------------------------------

/// The template ids the organization turned off (C-1109); every other offered template is on.
pub async fn org_disabled<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Vec<String>, DbError> {
    Ok(
        sqlx::query_scalar(
            "SELECT template_id FROM org_disabled_agent_templates WHERE org_id = $1",
        )
        .bind(org)
        .fetch_all(exec)
        .await?,
    )
}

/// Turn a template off for one organization (C-1109): write the switch row, keeping who did it
/// current when one already exists. Keyed by the template id (not a FK), so a source removed
/// and added again keeps the organization's choice.
pub async fn set_org_disabled<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    template_id: &str,
    disabled_by: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO org_disabled_agent_templates (org_id, template_id, disabled_by)
         VALUES ($1, $2, $3)
         ON CONFLICT (org_id, template_id) DO UPDATE SET disabled_by = $3",
    )
    .bind(org)
    .bind(template_id)
    .bind(disabled_by)
    .execute(exec)
    .await?;
    Ok(())
}

/// Turn a template back on for one organization (C-1109): delete the switch row. Every
/// template is on by default, so this is a no-op when there is none.
pub async fn set_org_enabled<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    template_id: &str,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM org_disabled_agent_templates WHERE org_id = $1 AND template_id = $2")
        .bind(org)
        .bind(template_id)
        .execute(exec)
        .await?;
    Ok(())
}

// ---- the per-agent parts (P6-TPL-3, C-1112) ----------------------------------------------------

/// One part an agent last took from a template (`03 §agent_template_parts`): `kind` is
/// `instructions` | `skill` | `helpers` | `library_skill`, `key` is `''` for the singleton
/// kinds and the skill key / helper path / ref key otherwise, and `content_sha256` is the
/// C-1104 part hash of what was last written (for `library_skill`, the hash of the ref key —
/// a library skill's content is never compared).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PartRow {
    pub agent_id: Uuid,
    pub kind: String,
    pub key: String,
    pub content_sha256: String,
}

pub async fn insert_part(conn: &mut PgConnection, p: &PartRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO agent_template_parts (agent_id, kind, key, content_sha256)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT (agent_id, kind, key) DO UPDATE SET content_sha256 = $4",
    )
    .bind(p.agent_id)
    .bind(&p.kind)
    .bind(&p.key)
    .bind(&p.content_sha256)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The parts of one agent (the apply route's diff base and the detach's rows).
pub async fn parts_of<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<PartRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT agent_id, kind, key, content_sha256 FROM agent_template_parts
         WHERE agent_id = $1 ORDER BY kind, key",
    )
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// Drop the parts of one agent (the detach; its provenance columns go with them).
pub async fn delete_parts(conn: &mut PgConnection, agent: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM agent_template_parts WHERE agent_id = $1")
        .bind(agent)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// (P6-TPL-3, C-1112) Drop one recorded part (an apply that takes a `removed_upstream` part
/// deletes the agent's copy and the recorded hash with it).
pub async fn delete_part(
    conn: &mut PgConnection,
    agent: Uuid,
    kind: &str,
    key: &str,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM agent_template_parts WHERE agent_id = $1 AND kind = $2 AND key = $3")
        .bind(agent)
        .bind(kind)
        .bind(key)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// (FIX-251, D-488(a)) The update notices: one per workspace and version, the row is the dedup.
// ---------------------------------------------------------------------------------------------

/// One update notice a workspace's people were told (FIX-251, D-488(a)).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpdateNoticeRow {
    pub workspace_id: Uuid,
    /// The `<id>` of `template.yaml` (no FK, like the provenance).
    pub template_id: String,
    /// The template's display name (joined at read time).
    pub template_name: String,
    pub version: String,
    pub created_at: DateTime<Utc>,
    /// The workspace's display name (joined at read time).
    pub workspace_name: String,
    /// How many of the workspace's agents made from the template are behind this version
    /// (computed at read time, never stored).
    pub behind: i64,
}

/// Record that `version` of `template_id` was announced to `workspace_id`'s people. `true` when
/// this run inserted the row (the caller tells the people); `false` when it was already told —
/// the one-notice-per-workspace-and-version dedup (D-488(a)).
pub async fn record_update_notice<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
    template_id: &str,
    version: &str,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "INSERT INTO agent_template_update_notices (workspace_id, template_id, version) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(workspace_id)
    .bind(template_id)
    .bind(version)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// The templates one workspace was ever told an update of: what the caller works out the
/// offered versions for before [`update_notices_for_workspace`].
pub async fn update_notice_template_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
) -> Result<Vec<String>, DbError> {
    sqlx::query_scalar(
        "SELECT DISTINCT template_id FROM agent_template_update_notices WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_all(exec)
    .await
    .map_err(DbError::from)
}

/// The update notices of one workspace whose offer still stands and that still concern at least
/// one of its agents (D-488(a)): the template is offered, the notice is for its offered
/// version, the organization has not turned the template off, and at least one agent made from
/// the template is behind. Which version is offered is a semver rule SQL does not have, so the
/// caller passes it: `offered` is `(template_id, version)`, one pair per offered template.
/// Oldest first (the notice's `created_at` is when it was first told).
pub async fn update_notices_for_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
    offered: &[(String, String)],
    limit: i64,
) -> Result<Vec<UpdateNoticeRow>, DbError> {
    let (ids, versions): (Vec<&str>, Vec<&str>) = offered
        .iter()
        .map(|(id, v)| (id.as_str(), v.as_str()))
        .unzip();
    sqlx::query_as(
        "SELECT n.workspace_id, n.template_id, t.name AS template_name, n.version, n.created_at, \
                w.name AS workspace_name, \
                (SELECT count(*) FROM agents a \
                  WHERE a.workspace_id = n.workspace_id AND a.template_id = n.template_id \
                    AND (a.template_version IS NULL OR a.template_version <> n.version)) AS behind \
         FROM agent_template_update_notices n \
         JOIN unnest($2::text[], $3::text[]) AS o(template_id, version) \
              ON o.template_id = n.template_id AND o.version = n.version \
         JOIN agent_templates t ON t.template_id = n.template_id AND t.offered \
         JOIN workspaces w ON w.id = n.workspace_id \
         WHERE n.workspace_id = $1 \
           AND NOT EXISTS (SELECT 1 FROM org_disabled_agent_templates d \
                            WHERE d.org_id = (SELECT org_id FROM workspaces WHERE id = n.workspace_id) \
                              AND d.template_id = n.template_id) \
           AND EXISTS (SELECT 1 FROM agents a \
                        WHERE a.workspace_id = n.workspace_id AND a.template_id = n.template_id \
                          AND (a.template_version IS NULL OR a.template_version <> n.version)) \
         ORDER BY n.created_at, n.template_id, n.version LIMIT $4",
    )
    .bind(workspace_id)
    .bind(ids)
    .bind(versions)
    .bind(limit)
    .fetch_all(exec)
    .await
    .map_err(DbError::from)
}

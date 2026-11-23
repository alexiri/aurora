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
    /// The commit the add read (also the first `last_head_sha`).
    pub head_sha: &'a str,
    pub update_interval: &'a str,
    /// Seconds until the first scheduled check (`None`: never scheduled, manual only).
    pub first_check_in_secs: Option<i64>,
    pub created_by: Uuid,
}

pub async fn insert_source(
    conn: &mut PgConnection,
    s: &NewSource<'_>,
) -> Result<SourceRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO agent_template_sources (id, name, url, ref, path, token_ref,
            token_updated_at, homepage, contact, packages_url, manifest, last_head_sha,
            update_interval, next_check_at, last_checked_at, created_by)
         VALUES ($1,$2,$3,$4,$5,$6, CASE WHEN $6 IS NULL THEN NULL ELSE now() END,
            $7,$8,$9,$10,$11,$12,
            now() + make_interval(secs => COALESCE($13, 31536000)), now(), $14)
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
    .bind(s.head_sha)
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

pub async fn list_sources<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<SourceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SEL} FROM agent_template_sources ORDER BY created_at, id"
    )))
    .fetch_all(exec)
    .await?)
}

pub async fn count_sources<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM agent_template_sources")
        .fetch_one(exec)
        .await?)
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
    /// The commit the patch read.
    pub head_sha: &'a str,
}

/// Replace the source's address parts and head (a patch that re-read the repository first).
pub async fn update_address(
    conn: &mut PgConnection,
    id: Uuid,
    a: &NewAddress<'_>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agent_template_sources SET ref = $2, path = $3,
            token_ref = $4,
            token_updated_at = CASE WHEN $4 IS NULL THEN NULL ELSE now() END,
            packages_url = $5, manifest = $6, last_head_sha = $7,
            last_checked_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(a.reference)
    .bind(a.path)
    .bind(a.token_ref)
    .bind(a.packages_url)
    .bind(a.manifest)
    .bind(a.head_sha)
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

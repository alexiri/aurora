//! What needs an instance admin's attention (FIX-167 Part B, D-370, C-1022..C-1024): the cheap
//! reads behind `GET /v1/admin/attention`, and the `admin_attention_notices` table that makes the
//! e-mail and the Web Push of a new decision item happen exactly once per administrator.
//!
//! Reads return names, versions and times only: never a manifest body beyond what the caller
//! needs to name the widening, never a setting value.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

// ---------------------------------------------------------------------------------------------
// Notices
// ---------------------------------------------------------------------------------------------

/// Record one notice per active instance admin for a decision item that just reached its pending
/// state. Call it in the transaction of that state change; `(kind, subject_id, subject_version)`
/// is the identity of the item, so a replay inserts nothing. Returns the rows inserted.
pub async fn record_notices<'e, E: PgExecutor<'e>>(
    exec: E,
    kind: &str,
    subject_id: &str,
    subject_version: &str,
    title: &str,
    detail: &str,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "INSERT INTO admin_attention_notices (kind, subject_id, subject_version, user_id, title, detail)
         SELECT $1, $2, $3, u.id, $4, $5 FROM users u
         WHERE u.is_instance_admin AND u.disabled_at IS NULL
         ON CONFLICT DO NOTHING",
    )
    .bind(kind)
    .bind(subject_id)
    .bind(subject_version)
    .bind(title)
    .bind(detail)
    .execute(exec)
    .await?
    .rows_affected())
}

/// A claimed notice with its recipient.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Claimed {
    pub kind: String,
    pub subject_id: String,
    pub subject_version: String,
    pub user_id: Uuid,
    pub title: String,
    pub detail: String,
    pub email: String,
    /// The user is still an enabled instance admin and has not turned the e-mail off.
    pub wants_email: bool,
    /// The user is still an enabled instance admin.
    pub is_admin: bool,
}

/// Claim up to `limit` notices whose e-mail half is open (`mailed_at = now()`), skipping rows
/// another replica holds. Claim-then-send: a crash between the two loses one e-mail, never
/// doubles it.
pub async fn claim_mail<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Claimed>, DbError> {
    Ok(sqlx::query_as(
        "WITH picked AS (
            SELECT kind, subject_id, subject_version, user_id FROM admin_attention_notices
            WHERE mailed_at IS NULL ORDER BY created_at LIMIT $1 FOR UPDATE SKIP LOCKED)
         UPDATE admin_attention_notices n SET mailed_at = now()
         FROM picked p JOIN users u ON u.id = p.user_id
         WHERE n.kind = p.kind AND n.subject_id = p.subject_id
           AND n.subject_version = p.subject_version AND n.user_id = p.user_id
         RETURNING n.kind, n.subject_id, n.subject_version, n.user_id, n.title, n.detail, u.email,
            (u.is_instance_admin AND u.disabled_at IS NULL AND u.admin_attention_email_enabled)
                AS wants_email,
            (u.is_instance_admin AND u.disabled_at IS NULL) AS is_admin",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// The same for the Web Push half. Only notices younger than `max_age_secs` are claimed: a push
/// for a long-resolved item is noise (a replica that was down, push switched on much later).
pub async fn claim_push<'e, E: PgExecutor<'e>>(
    exec: E,
    max_age_secs: f64,
    limit: i64,
) -> Result<Vec<Claimed>, DbError> {
    Ok(sqlx::query_as(
        "WITH picked AS (
            SELECT kind, subject_id, subject_version, user_id FROM admin_attention_notices
            WHERE pushed_at IS NULL AND created_at > now() - make_interval(secs => $1)
            ORDER BY created_at LIMIT $2 FOR UPDATE SKIP LOCKED)
         UPDATE admin_attention_notices n SET pushed_at = now()
         FROM picked p JOIN users u ON u.id = p.user_id
         WHERE n.kind = p.kind AND n.subject_id = p.subject_id
           AND n.subject_version = p.subject_version AND n.user_id = p.user_id
         RETURNING n.kind, n.subject_id, n.subject_version, n.user_id, n.title, n.detail, u.email,
            (u.is_instance_admin AND u.disabled_at IS NULL AND u.admin_attention_email_enabled)
                AS wants_email,
            (u.is_instance_admin AND u.disabled_at IS NULL) AS is_admin",
    )
    .bind(max_age_secs)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Delete notices older than `days` (a sent or abandoned claim is history after that).
pub async fn prune<'e, E: PgExecutor<'e>>(exec: E, days: i32) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM admin_attention_notices WHERE created_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(exec)
    .await?
    .rows_affected())
}

/// `users.admin_attention_email_enabled` of one user (default `true`).
pub async fn email_enabled<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT admin_attention_email_enabled FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(exec)
            .await?
            .unwrap_or(true),
    )
}

pub async fn set_email_enabled<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    enabled: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE users SET admin_attention_email_enabled = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(enabled)
    .execute(exec)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------------------------

/// A connector that may wait for a decision. The caller decides with `manifest::update_pending`
/// (a connector table is a dozen rows).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectorCandidate {
    pub id: String,
    pub display_name: String,
    pub version: Option<String>,
    pub status: String,
    pub manifest: Option<serde_json::Value>,
    pub since: DateTime<Utc>,
}

/// Connectors in `pending_approval`, and `active` ones whose announced manifest differs from the
/// approved one. `since` is when its notice was written, else the row's last change.
pub async fn connector_candidates<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<ConnectorCandidate>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.display_name, c.version, c.status, c.manifest,
            COALESCE((SELECT min(n.created_at) FROM admin_attention_notices n
                      WHERE n.kind = 'connector_approval' AND n.subject_id = c.id
                        AND n.subject_version = c.manifest_digest), c.updated_at) AS since
         FROM connectors c
         WHERE c.status = 'pending_approval'
            OR (c.status = 'active' AND c.manifest_digest IS DISTINCT FROM c.approved_manifest_digest)
         ORDER BY since, c.id",
    )
    .fetch_all(exec)
    .await?)
}

/// A package version held for a decision.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HeldPackage {
    pub id: Uuid,
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub status_reason: Option<String>,
    pub since: DateTime<Utc>,
}

/// Every package version in `pending_approval`, oldest first.
pub async fn held_packages<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<HeldPackage>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, package_id, COALESCE(manifest #>> '{package,name}', package_id) AS name,
            version, status_reason, created_at AS since
         FROM plugin_packages WHERE status = 'pending_approval'
         ORDER BY created_at, id",
    )
    .fetch_all(exec)
    .await?)
}

/// An addon source whose signing key changed and waits for a re-confirmation.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct KeyChangedSource {
    pub id: Uuid,
    pub name: String,
    pub since: DateTime<Utc>,
}

pub async fn key_changed_sources<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<KeyChangedSource>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, name, COALESCE(check_failed_at, updated_at) AS since
         FROM addon_sources WHERE status = 'signing_key_changed' ORDER BY since, id",
    )
    .fetch_all(exec)
    .await?)
}

/// An agent template version waiting for a decision (C-1108's attention item).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TemplateVersionAttention {
    /// The version row's id (the notice subject, the item's subject id).
    pub id: Uuid,
    pub agent_template_id: Uuid,
    /// The template's id (`template.yaml` `id`, the admin routes' path part).
    pub template_id: String,
    /// The template's display name (the sentence).
    pub name: String,
    pub version: String,
    pub since: DateTime<Utc>,
}

/// Every version in `pending_approval`, oldest first (C-1108: one decision item each).
pub async fn template_versions_pending<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<TemplateVersionAttention>, DbError> {
    Ok(sqlx::query_as(
        "SELECT v.id, v.agent_template_id, t.template_id, t.name, v.version, v.created_at AS since
         FROM agent_template_versions v
         JOIN agent_templates t ON t.id = v.agent_template_id
         WHERE v.status = 'pending_approval'
         ORDER BY v.created_at, v.id",
    )
    .fetch_all(exec)
    .await?)
}

/// Versions of offered sources that have been `awaiting_package` for more than `age_secs`
/// (C-1108: an attention item without decision), oldest first.
pub async fn template_versions_awaiting<'e, E: PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
) -> Result<Vec<TemplateVersionAttention>, DbError> {
    Ok(sqlx::query_as(
        "SELECT v.id, v.agent_template_id, t.template_id, t.name, v.version, v.created_at AS since
         FROM agent_template_versions v
         JOIN agent_templates t ON t.id = v.agent_template_id
         WHERE v.status = 'awaiting_package' AND v.created_at < now() - make_interval(secs => $1)
         ORDER BY v.created_at, v.id",
    )
    .bind(age_secs)
    .fetch_all(exec)
    .await?)
}

/// A managed connector that is not being served.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DownConnector {
    pub id: String,
    pub display_name: String,
    /// A live host reports `error` (otherwise: no live host reports it at all).
    pub errored: bool,
    pub since: DateTime<Utc>,
}

/// Active managed connectors whose hosts report `error`, or that no host seen within
/// `stale_secs` reports.
pub async fn down_connectors<'e, E: PgExecutor<'e>>(
    exec: E,
    stale_secs: f64,
) -> Result<Vec<DownConnector>, DbError> {
    Ok(sqlx::query_as(
        "WITH live AS (
            SELECT s.connector_id, s.state, s.updated_at
            FROM connector_host_status s JOIN connector_hosts h ON h.instance_id = s.host_instance
            WHERE h.last_seen_at > now() - make_interval(secs => $1))
         SELECT c.id, c.display_name,
            EXISTS (SELECT 1 FROM live l WHERE l.connector_id = c.id AND l.state = 'error') AS errored,
            COALESCE((SELECT max(l.updated_at) FROM live l WHERE l.connector_id = c.id), c.updated_at)
                AS since
         FROM connectors c
         WHERE c.managed AND c.status = 'active'
           AND (EXISTS (SELECT 1 FROM live l WHERE l.connector_id = c.id AND l.state = 'error')
                OR NOT EXISTS (SELECT 1 FROM live l WHERE l.connector_id = c.id))
         ORDER BY since, c.id",
    )
    .bind(stale_secs)
    .fetch_all(exec)
    .await?)
}

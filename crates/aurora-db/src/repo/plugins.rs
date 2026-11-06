//! Plugin registry rows: packages, blobs, trusted keys (03 §3, 09 §10).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

const SEL: &str = "p.id, p.package_id, p.version, p.contract, p.publisher, p.digest,
    p.signer_key_id, tk.name AS signer_name, p.manifest, p.permissions, p.violations,
    p.source, p.status, p.status_reason, p.approved_by, u.display_name AS approved_by_name,
    p.approved_at, p.created_at";
const FROM: &str = "plugin_packages p
    LEFT JOIN trusted_keys tk ON tk.public_key = p.signer_key_id
    LEFT JOIN users u ON u.id = p.approved_by";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PackageRow {
    pub id: Uuid,
    pub package_id: String,
    pub version: String,
    pub contract: i32,
    pub publisher: String,
    pub digest: String,
    /// `builtin` | `unsigned` | base64 public key.
    pub signer_key_id: String,
    pub signer_name: Option<String>,
    pub manifest: Value,
    pub permissions: Value,
    pub violations: Value,
    pub source: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub approved_by: Option<Uuid>,
    pub approved_by_name: Option<String>,
    pub approved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub struct NewPackage<'a> {
    pub id: Uuid,
    pub package_id: &'a str,
    pub version: &'a str,
    pub contract: i32,
    pub publisher: &'a str,
    pub digest: &'a str,
    pub signer_key_id: &'a str,
    pub manifest: &'a Value,
    pub permissions: &'a Value,
    pub violations: &'a Value,
    pub source: &'a str,
    pub status: &'a str,
    pub status_reason: Option<&'a str>,
    /// Set `approved_at = now()` (automatic activation).
    pub approved_now: bool,
}

/// Insert; `None` when `(package_id, version, digest)` already exists.
pub async fn insert_package(
    conn: &mut PgConnection,
    p: &NewPackage<'_>,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO plugin_packages (id, package_id, version, contract, publisher, digest,
            signer_key_id, manifest, permissions, violations, source, status, status_reason,
            approved_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13, CASE WHEN $14 THEN now() END)
         ON CONFLICT (package_id, version, digest) DO NOTHING RETURNING id",
    )
    .bind(p.id)
    .bind(p.package_id)
    .bind(p.version)
    .bind(p.contract)
    .bind(p.publisher)
    .bind(p.digest)
    .bind(p.signer_key_id)
    .bind(p.manifest)
    .bind(p.permissions)
    .bind(p.violations)
    .bind(p.source)
    .bind(p.status)
    .bind(p.status_reason)
    .bind(p.approved_now)
    .fetch_optional(conn)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<PackageRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {SEL} FROM {FROM} WHERE p.id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn find_identity<'e, E: PgExecutor<'e>>(
    exec: E,
    package_id: &str,
    version: &str,
    digest: &str,
) -> Result<Option<PackageRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM {FROM}
         WHERE p.package_id = $1 AND p.version = $2 AND p.digest = $3"
    ))
    .bind(package_id)
    .bind(version)
    .bind(digest)
    .fetch_optional(exec)
    .await?)
}

pub async fn list<'e, E: PgExecutor<'e>>(
    exec: E,
    status: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<PackageRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM {FROM}
         WHERE ($1::text IS NULL OR p.status = $1)
           AND ($2::timestamptz IS NULL OR (p.created_at, p.id) < ($2, $3))
         ORDER BY p.created_at DESC, p.id DESC LIMIT $4"
    ))
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// The version a new upload of `package_id` is compared against: the active one,
/// else the most recently approved superseded one.
pub async fn previous_approved<'e, E: PgExecutor<'e>>(
    exec: E,
    package_id: &str,
) -> Result<Option<PackageRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM {FROM}
         WHERE p.package_id = $1 AND p.status IN ('active','superseded') AND p.approved_at IS NOT NULL
         ORDER BY (p.status = 'active') DESC, p.approved_at DESC, p.id DESC LIMIT 1"
    ))
    .bind(package_id)
    .fetch_optional(exec)
    .await?)
}

/// `(package_id, manifest)` of every active package.
pub async fn active_manifests<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<(String, Value)>, DbError> {
    Ok(
        sqlx::query_as("SELECT package_id, manifest FROM plugin_packages WHERE status = 'active'")
            .fetch_all(exec)
            .await?,
    )
}

/// Active package -> `superseded` (frees the one-active-per-package index).
pub async fn supersede_active(conn: &mut PgConnection, package_id: &str) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE plugin_packages SET status = 'superseded', status_reason = NULL, updated_at = now()
         WHERE package_id = $1 AND status = 'active'",
    )
    .bind(package_id)
    .execute(conn)
    .await?
    .rows_affected())
}

/// Content-addressed blob; existing digests are left untouched.
pub async fn put_blob<'e, E: PgExecutor<'e>>(
    exec: E,
    digest: &str,
    kind: &str,
    content: &[u8],
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO plugin_blobs (digest, kind, content) VALUES ($1,$2,$3)
         ON CONFLICT (digest) DO NOTHING",
    )
    .bind(digest)
    .bind(kind)
    .bind(content)
    .execute(exec)
    .await?;
    Ok(())
}

// ---- trusted keys ------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TrustedKeyRow {
    pub id: Uuid,
    pub publisher: String,
    pub name: String,
    pub public_key: String,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const KEY_SEL: &str = "id, publisher, name, public_key, revoked_at, created_at";

pub async fn all_trusted_keys<'e, E: PgExecutor<'e>>(
    exec: E,
) -> Result<Vec<TrustedKeyRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {KEY_SEL} FROM trusted_keys"))
            .fetch_all(exec)
            .await?,
    )
}

pub async fn list_trusted_keys<'e, E: PgExecutor<'e>>(
    exec: E,
    after: After,
    fetch: i64,
) -> Result<Vec<TrustedKeyRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {KEY_SEL} FROM trusted_keys
         WHERE ($1::timestamptz IS NULL OR (created_at, id) < ($1, $2))
         ORDER BY created_at DESC, id DESC LIMIT $3"
    ))
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn insert_trusted_key<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    publisher: &str,
    name: &str,
    public_key: &str,
    added_by: Uuid,
) -> Result<TrustedKeyRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO trusted_keys (id, publisher, name, public_key, added_by)
         VALUES ($1,$2,$3,$4,$5) RETURNING {KEY_SEL}"
    ))
    .bind(id)
    .bind(publisher)
    .bind(name)
    .bind(public_key)
    .bind(added_by)
    .fetch_one(exec)
    .await?)
}

/// Mark revoked (first revocation wins). `None` when the key does not exist.
pub async fn revoke_trusted_key(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<TrustedKeyRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE trusted_keys SET revoked_at = COALESCE(revoked_at, now()), updated_at = now()
         WHERE id = $1 RETURNING {KEY_SEL}"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Disable every live package signed by `public_key`; returns `(id, was_active)`.
pub async fn disable_signed_by(
    conn: &mut PgConnection,
    public_key: &str,
    reason: &str,
) -> Result<Vec<(Uuid, bool)>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE plugin_packages p SET status = 'disabled', status_reason = $2, updated_at = now()
         FROM (SELECT id, status FROM plugin_packages
               WHERE signer_key_id = $1
                 AND status IN ('pending_approval','active','superseded') FOR UPDATE) o
         WHERE p.id = o.id RETURNING p.id, (o.status = 'active')",
    )
    .bind(public_key)
    .bind(reason)
    .fetch_all(conn)
    .await?)
}

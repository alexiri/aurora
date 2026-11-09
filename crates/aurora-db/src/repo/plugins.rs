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

// ---- lifecycle, decisions, snapshots (BE-041) ----------------------------------

/// Transaction-scoped advisory lock serialising snapshot compile/activate and
/// lifecycle transitions across API replicas.
pub async fn lock_compile(conn: &mut PgConnection) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('aurora:registry:compile', 0))")
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<PackageRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM {FROM} WHERE p.id = $1 FOR UPDATE OF p"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Packages in any of `statuses`, ordered by package id then approval time.
pub async fn list_by_status<'e, E: PgExecutor<'e>>(
    exec: E,
    statuses: &[&str],
) -> Result<Vec<PackageRow>, DbError> {
    let statuses: Vec<String> = statuses.iter().map(|s| (*s).to_owned()).collect();
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM {FROM} WHERE p.status = ANY($1)
         ORDER BY p.package_id, p.created_at DESC, p.id DESC"
    ))
    .bind(statuses)
    .fetch_all(exec)
    .await?)
}

/// Move a package to `status`. `approver` (with `approve = true`) stamps
/// `approved_by`/`approved_at` (kept when already set).
pub async fn transition(
    conn: &mut PgConnection,
    id: Uuid,
    status: &str,
    reason: Option<&str>,
    approver: Option<Uuid>,
    approve: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE plugin_packages SET status = $2, status_reason = $3, updated_at = now(),
            approved_by = CASE WHEN $5 THEN COALESCE(approved_by, $4) ELSE approved_by END,
            approved_at = CASE WHEN $5 THEN COALESCE(approved_at, now()) ELSE approved_at END
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(reason)
    .bind(approver)
    .bind(approve)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_package(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM plugin_packages WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Raw blob bytes of `kind`.
pub async fn get_blob<'e, E: PgExecutor<'e>>(
    exec: E,
    digest: &str,
    kind: &str,
) -> Result<Option<Vec<u8>>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT content FROM plugin_blobs WHERE digest = $1 AND kind = $2")
            .bind(digest)
            .bind(kind)
            .fetch_optional(exec)
            .await?,
    )
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DecisionRow {
    pub claim_key: String,
    pub claim_fingerprint: String,
    pub winner_package_id: String,
    pub winner_app_id: String,
    pub decided_by: Option<Uuid>,
    pub decided_by_name: Option<String>,
}

pub async fn decisions<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<DecisionRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT d.claim_key, d.claim_fingerprint, d.winner_package_id, d.winner_app_id,
                d.decided_by, u.display_name AS decided_by_name
         FROM host_claim_decisions d LEFT JOIN users u ON u.id = d.decided_by
         ORDER BY d.claim_key",
    )
    .fetch_all(exec)
    .await?)
}

pub async fn upsert_decision(
    conn: &mut PgConnection,
    id: Uuid,
    claim_key: &str,
    fingerprint: &str,
    winner_package_id: &str,
    winner_app_id: &str,
    decided_by: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO host_claim_decisions
            (id, claim_key, claim_fingerprint, winner_package_id, winner_app_id, decided_by)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (claim_key) DO UPDATE SET claim_fingerprint = EXCLUDED.claim_fingerprint,
            winner_package_id = EXCLUDED.winner_package_id, winner_app_id = EXCLUDED.winner_app_id,
            decided_by = EXCLUDED.decided_by, updated_at = now()",
    )
    .bind(id)
    .bind(claim_key)
    .bind(fingerprint)
    .bind(winner_package_id)
    .bind(winner_app_id)
    .bind(decided_by)
    .execute(conn)
    .await?;
    Ok(())
}

/// Store a snapshot by content hash and (re)activate it: `activated_at` is
/// bumped when the hash already exists, so the newest activation wins.
pub async fn put_snapshot(
    conn: &mut PgConnection,
    id: Uuid,
    contract: i32,
    hash: &str,
    snapshot: &Value,
    compiled_by: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at)
         VALUES ($1,$2,$3,$4,$5, clock_timestamp())
         ON CONFLICT (content_hash) DO UPDATE SET activated_at = clock_timestamp()",
    )
    .bind(id)
    .bind(contract)
    .bind(hash)
    .bind(snapshot)
    .bind(compiled_by)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn snapshot_exists<'e, E: PgExecutor<'e>>(exec: E) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM registry_snapshots)")
            .fetch_one(exec)
            .await?,
    )
}

/// Connections of `package_id` on any of `app_ids` that are not yet orphaned.
pub async fn live_connection_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    package_id: &str,
    app_ids: &[String],
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM app_connections
         WHERE package_id = $1 AND app_id = ANY($2) AND status <> 'orphaned'",
    )
    .bind(package_id)
    .bind(app_ids)
    .fetch_all(exec)
    .await?)
}

/// The active version of `package_id`, if any (RFE-022: the version a new one upgrades).
pub async fn active_of_package<'e, E: PgExecutor<'e>>(
    exec: E,
    package_id: &str,
) -> Result<Option<PackageRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM {FROM} WHERE p.package_id = $1 AND p.status = 'active' LIMIT 1"
    ))
    .bind(package_id)
    .fetch_optional(exec)
    .await?)
}

/// The stored manifest of one package version (any status), newest row first.
pub async fn manifest_of_version<'e, E: PgExecutor<'e>>(
    exec: E,
    package_id: &str,
    version: &str,
) -> Result<Option<Value>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT manifest FROM plugin_packages WHERE package_id = $1 AND version = $2
         AND status <> 'invalid' ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(package_id)
    .bind(version)
    .fetch_optional(exec)
    .await?)
}

/// A live (`active` or `reauth_required`) connection, for the major-upgrade check. Names only.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpgradeConnection {
    pub id: Uuid,
    pub app_id: String,
    pub status: String,
    pub app_version: String,
    /// `None` = unknown (legacy row).
    pub credential_keys: Option<Vec<String>>,
}

/// Live connections of `package_id` on `app_ids`, any organization and scope, by `app_id, id`.
pub async fn upgrade_connections<'e, E: PgExecutor<'e>>(
    exec: E,
    package_id: &str,
    app_ids: &[String],
) -> Result<Vec<UpgradeConnection>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, app_id, status, app_version, credential_keys FROM app_connections
         WHERE package_id = $1 AND app_id = ANY($2) AND status IN ('active', 'reauth_required')
         ORDER BY app_id, id",
    )
    .bind(package_id)
    .bind(app_ids)
    .fetch_all(exec)
    .await?)
}

/// Flip one `active` connection to `reauth_required` (C-342); false when it was no longer active.
pub async fn flip_reauth_required(
    conn: &mut PgConnection,
    id: Uuid,
    reason: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_connections SET status = 'reauth_required', status_reason = $2,
            reauth_required_at = now(), updated_at = now() WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .bind(reason)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

pub async fn orphan_connections(conn: &mut PgConnection, ids: &[Uuid]) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE app_connections SET status = 'orphaned', status_reason = 'app_removed',
            updated_at = now() WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(conn)
    .await?;
    Ok(())
}

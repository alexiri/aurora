//! Addon source rows (03 §`addon_sources`, P5-ASRC, D-164).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

const SEL: &str = "id, name, url, homepage, contact, publisher, signing_key, key_fingerprint,
    new_key_fingerprint, manifest_digest, update_interval, status, check_failed_at,
    check_failed_reason, last_checked_at, next_check_at, check_lease_until, lease_owner,
    created_by, created_at, updated_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AddonSourceRow {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub homepage: Option<String>,
    pub contact: Option<String>,
    pub publisher: String,
    pub signing_key: String,
    pub key_fingerprint: String,
    pub new_key_fingerprint: Option<String>,
    pub manifest_digest: String,
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

pub struct NewSource<'a> {
    pub id: Uuid,
    pub name: &'a str,
    pub url: &'a str,
    pub homepage: Option<&'a str>,
    pub contact: Option<&'a str>,
    pub publisher: &'a str,
    pub signing_key: &'a str,
    pub key_fingerprint: &'a str,
    pub manifest_digest: &'a str,
    pub update_interval: &'a str,
    pub created_by: Uuid,
}

pub async fn insert(conn: &mut PgConnection, s: &NewSource<'_>) -> Result<AddonSourceRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO addon_sources (id, name, url, homepage, contact, publisher, signing_key,
            key_fingerprint, manifest_digest, update_interval, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING {SEL}"
    ))
    .bind(s.id)
    .bind(s.name)
    .bind(s.url)
    .bind(s.homepage)
    .bind(s.contact)
    .bind(s.publisher)
    .bind(s.signing_key)
    .bind(s.key_fingerprint)
    .bind(s.manifest_digest)
    .bind(s.update_interval)
    .bind(s.created_by)
    .fetch_one(conn)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<AddonSourceRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {SEL} FROM addon_sources WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn get_by_url<'e, E: PgExecutor<'e>>(
    exec: E,
    url: &str,
) -> Result<Option<AddonSourceRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {SEL} FROM addon_sources WHERE url = $1"))
            .bind(url)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn list<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<AddonSourceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM addon_sources ORDER BY created_at, id"
    ))
    .fetch_all(exec)
    .await?)
}

/// Every source that is due (`next_check_at <= now()`, lease free), claiming the lease.
/// `signing_key_changed` sources are not claimed (they wait for the admin's re-confirm).
pub async fn claim_due(
    conn: &mut PgConnection,
    owner: Uuid,
    limit: i64,
    lease_secs: f64,
) -> Result<Vec<AddonSourceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE addon_sources s SET check_lease_until = now() + make_interval(secs => $3),
            lease_owner = $1, updated_at = now()
         WHERE s.id IN (
            SELECT id FROM addon_sources
            WHERE next_check_at <= now()
              AND (check_lease_until IS NULL OR check_lease_until < now())
              AND status <> 'signing_key_changed'
            ORDER BY next_check_at LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING {SEL}"
    ))
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
) -> Result<Option<AddonSourceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE addon_sources s SET check_lease_until = now() + make_interval(secs => $3),
            lease_owner = $1, updated_at = now()
         WHERE s.id = $2 AND (check_lease_until IS NULL OR check_lease_until < now())
         RETURNING {SEL}"
    ))
    .bind(owner)
    .bind(id)
    .bind(lease_secs)
    .fetch_optional(conn)
    .await?)
}

/// Only the lease owner releases without an outcome (shutdown).
pub async fn release_lease(conn: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE addon_sources SET check_lease_until = NULL, lease_owner = NULL, updated_at = now()
         WHERE id = $1 AND lease_owner = $2",
    )
    .bind(id)
    .bind(owner)
    .execute(conn)
    .await?;
    Ok(())
}

/// Outcome of one check (the lease owner writes it).
pub struct CheckDone {
    pub id: Uuid,
    pub owner: Uuid,
    /// `ok` | `check_failed` | `signing_key_changed`.
    pub status: &'static str,
    pub reason: Option<String>,
    pub new_key_fingerprint: Option<String>,
    pub manifest_digest: Option<String>,
    /// Seconds until the next scheduled check (the interval or the failure backoff).
    pub next_in_secs: i64,
}

pub async fn finish_check(conn: &mut PgConnection, d: &CheckDone) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE addon_sources SET
            status = $3,
            check_failed_at = CASE WHEN $3 = 'ok' THEN NULL ELSE now() END,
            check_failed_reason = $4,
            new_key_fingerprint = $5,
            manifest_digest = COALESCE($6, manifest_digest),
            last_checked_at = now(),
            next_check_at = now() + make_interval(secs => $7),
            check_lease_until = NULL,
            lease_owner = NULL,
            updated_at = now()
         WHERE id = $1 AND lease_owner = $2",
    )
    .bind(d.id)
    .bind(d.owner)
    .bind(d.status)
    .bind(d.reason.clone())
    .bind(d.new_key_fingerprint.clone())
    .bind(d.manifest_digest.clone())
    .bind(d.next_in_secs)
    .execute(conn)
    .await?;
    Ok(())
}

/// Update the stored key after an admin re-confirm (C-595): the source's fingerprint and
/// the `trusted_keys` row move together (the caller updates the key row in the same
/// transaction).
pub async fn confirm_key(
    conn: &mut PgConnection,
    id: Uuid,
    signing_key: &str,
    key_fingerprint: &str,
    manifest_digest: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE addon_sources SET signing_key = $2, key_fingerprint = $3,
            new_key_fingerprint = NULL, status = 'ok', check_failed_at = NULL,
            check_failed_reason = NULL, manifest_digest = $4, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(signing_key)
    .bind(key_fingerprint)
    .bind(manifest_digest)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn update_interval(
    conn: &mut PgConnection,
    id: Uuid,
    interval: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE addon_sources SET update_interval = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(interval)
        .execute(conn)
        .await?;
    Ok(())
}

/// Package counts of one source: `(ingested, active)` — live rows (any status but
/// `invalid`/`rejected`) and currently `active` ones.
pub async fn package_counts<'e, E: PgExecutor<'e>>(
    exec: E,
    source_id: Uuid,
) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status NOT IN ('invalid','rejected')),
                count(*) FILTER (WHERE status = 'active')
         FROM plugin_packages WHERE source_id = $1",
    )
    .bind(source_id)
    .fetch_one(exec)
    .await?)
}

/// True while any package of the source is `active` (removal is refused).
pub async fn has_active<'e, E: PgExecutor<'e>>(exec: E, source_id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM plugin_packages WHERE source_id = $1 AND status = 'active')",
    )
    .bind(source_id)
    .fetch_one(exec)
    .await?)
}

/// Newest ingested version (any status but `invalid`) of each package of the source,
/// as `(package_id, version)` — the comparison base for "newer upstream versions".
pub async fn newest_ingested<'e, E: PgExecutor<'e>>(
    exec: E,
    source_id: Uuid,
) -> Result<Vec<(String, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT ON (package_id) package_id, version
         FROM plugin_packages
         WHERE source_id = $1 AND status <> 'invalid'
         ORDER BY package_id, string_to_array(version, '.')::int[] DESC NULLS LAST, created_at DESC",
    )
    .bind(source_id)
    .fetch_all(exec)
    .await?)
}

/// Packages of a source with their newest ingested version per `package_id` (for the
/// source detail view): `(package_id, version, status)`.
pub async fn packages_of<'e, E: PgExecutor<'e>>(
    exec: E,
    source_id: Uuid,
) -> Result<Vec<(String, String, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT package_id, version, status FROM plugin_packages
         WHERE source_id = $1 AND status <> 'invalid'
         ORDER BY package_id, created_at DESC",
    )
    .bind(source_id)
    .fetch_all(exec)
    .await?)
}

/// Delete a source row (its packages stay: `source_id` is set null by the FK).
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM addon_sources WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

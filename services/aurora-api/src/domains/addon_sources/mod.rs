//! Addon sources (P5-ASRC, D-164): registration (TOFU), the shared check path, and the
//! poll job. Trust model: the manifest's `signing_key` is the anchor — registration stores
//! it in a `trusted_keys` row, checks verify packages against it, a changed key flags the
//! source for an admin's re-confirm and never auto-trusts (SEC-86).

pub mod fetch;
pub mod manifest;
pub mod poll;
pub mod routes;
pub use routes::router;

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::plugins::{ingest, service};
use aurora_core::ids;
use aurora_core::{ApiError, sanitize};
use aurora_db::repo::addon_sources as repo;
use aurora_db::repo::addon_sources::AddonSourceRow;
use aurora_db::repo::plugins::PackageRow;
use aurora_registry::manifest::parse_semver;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// Interval seconds for the owner's enum (`update_interval`).
#[must_use]
pub fn interval_secs(interval: &str) -> Option<i64> {
    match interval {
        "6h" => Some(6 * 3600),
        "12h" => Some(12 * 3600),
        "1d" => Some(24 * 3600),
        "1w" => Some(7 * 24 * 3600),
        "2w" => Some(14 * 24 * 3600),
        "1mo" => Some(30 * 24 * 3600),
        "never" => None,
        _ => None,
    }
}

/// ±10 % jitter keyed by the source id (stable per row, no clock read).
fn jitter(id: Uuid) -> f64 {
    let n = (id.as_u128() % 2001) as f64 / 10_000.0 - 0.1;
    1.0 + n
}

fn next_in_secs(interval: &str, id: Uuid) -> i64 {
    match interval_secs(interval) {
        Some(s) => ((s as f64) * jitter(id)) as i64,
        None => 365 * 24 * 3600, // `never`: far in the future; manual checks still work
    }
}

/// The per-replica lease owner id (random per process, like the library refresh).
static OWNER: AtomicU64 = AtomicU64::new(0);

fn lease_owner() -> Uuid {
    let mut b = *ids::new_id().as_bytes();
    b[0] = OWNER.fetch_add(1, Ordering::Relaxed) as u8;
    Uuid::from_bytes(b)
}

fn refused(e: manifest::ManifestError) -> ApiError {
    match e {
        manifest::ManifestError::UrlRefused => {
            ApiError::unprocessable("addon_url_refused", "The manifest URL was refused.")
        }
        manifest::ManifestError::Unreachable(r) => {
            ApiError::unprocessable("addon_unreachable", "The addon source is unreachable.")
                .with_details(json!({ "reason": r }))
        }
        manifest::ManifestError::TooLarge => {
            ApiError::too_large("The manifest is larger than 1 MiB.")
        }
        manifest::ManifestError::Invalid(r) => {
            ApiError::unprocessable("addon_manifest_invalid", "The addon manifest is invalid.")
                .with_details(json!({ "reason": r }))
        }
    }
}

/// `refused` for the routes module.
pub fn refused_public(e: manifest::ManifestError) -> ApiError {
    refused(e)
}

/// The sanitized reason of a manifest error (used by failure paths that only store
/// `check_failed_reason`).
#[allow(dead_code)]
fn manifest_error_reason(e: &manifest::ManifestError) -> String {
    e.reason()
}

fn fetcher(state: &AppState) -> fetch::Fetcher<'_> {
    fetch::Fetcher::with_env(&state.connect)
}

/// Fetch, size-check and parse the manifest (the shared first step of every path).
/// `Ok` carries the parsed manifest and the raw bytes; `Err` is already the mapped
/// `ApiError` (413 / 422s of C-599).
async fn fetch_manifest(
    state: &AppState,
    url: &str,
) -> Result<(manifest::Manifest, Vec<u8>), ApiError> {
    let f = fetcher(state);
    let bytes = f.manifest(url).await.map_err(refused)?;
    let m = manifest::parse(&bytes).map_err(refused)?;
    Ok((m, bytes))
}

/// `fetch_manifest` for the routes module.
pub async fn fetch_manifest_public(
    state: &AppState,
    url: &str,
) -> Result<(manifest::Manifest, Vec<u8>), ApiError> {
    fetch_manifest(state, url).await
}

/// Fetch a package archive (routes module: registration).
pub async fn fetch_package(
    state: &AppState,
    url: &str,
) -> Result<Vec<u8>, manifest::ManifestError> {
    fetcher(state).package(url).await
}

/// `core_supports` for the routes module.
pub fn core_supports_public(min_core: Option<&str>) -> Result<(), &'static str> {
    core_supports(min_core)
}

/// sha256 hex of `bytes` compared case-insensitively against `expected`.
pub fn digest_matches(bytes: &[u8], expected: &str) -> bool {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    use std::fmt::Write;
    for b in d {
        let _ = write!(s, "{b:02x}");
    }
    s.eq_ignore_ascii_case(expected)
}

/// `is_newer` for the routes module.
pub async fn is_newer_public(
    state: &AppState,
    package_id: &str,
    candidate: (u64, u64, u64, bool),
) -> Result<bool, ApiError> {
    is_newer(state, package_id, candidate).await
}

/// The DTO-facing view of one addon entry with install state.
pub struct AddonView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub versions: Vec<VersionView>,
}

pub struct VersionView {
    pub version: String,
    pub min_core: Option<String>,
    pub ingestable: bool,
    pub not_ingestable_reason: Option<String>,
    pub installed_version: Option<String>,
    pub update_available: bool,
}

fn core_supports(min_core: Option<&str>) -> Result<(), &'static str> {
    let Some(mc) = min_core else {
        return Ok(());
    };
    let Some((maj, min, _, _)) = parse_semver(mc) else {
        return Ok(());
    };
    let running = parse_semver(env!("CARGO_PKG_VERSION")).map_or((0, 0, 0, false), |v| v);
    if (running.0, running.1) >= (maj, min) {
        Ok(())
    } else {
        Err("core_too_new")
    }
}

/// Per-addon view state for a source (or a preview: `installed` empty).
pub fn addon_views(m: &manifest::Manifest, installed: &BTreeMap<String, String>) -> Vec<AddonView> {
    m.addons
        .iter()
        .map(|a| AddonView {
            id: a.id.clone(),
            name: a.name.clone(),
            description: a.description.clone(),
            versions: a
                .versions
                .iter()
                .map(|(v, ver)| {
                    let ingestable = core_supports(ver.min_core.as_deref()).is_ok();
                    let not_ingestable_reason = core_supports(ver.min_core.as_deref())
                        .err()
                        .map(str::to_owned);
                    let newest_installed = installed.get(&a.id);
                    let newer = newest_installed
                        .and_then(|iv| Some((parse_semver(iv)?, parse_semver(v)?)))
                        .is_some_and(|(i, u)| semver_newer(u, i));
                    VersionView {
                        version: v.clone(),
                        min_core: ver.min_core.clone(),
                        ingestable,
                        not_ingestable_reason,
                        installed_version: newest_installed.cloned(),
                        update_available: newer,
                    }
                })
                .collect(),
        })
        .collect()
}

/// `(major, minor, patch, pre)` newer-than (pre-releases sort first, as `parse_semver` does).
fn semver_newer(a: (u64, u64, u64, bool), b: (u64, u64, u64, bool)) -> bool {
    (a.3, a.0, a.1, a.2) > (b.3, b.0, b.1, b.2)
}

/// Newer than the newest ingested version of `package_id` (all sources, not just this
/// one: a package id is globally unique).
async fn is_newer(
    state: &AppState,
    package_id: &str,
    candidate: (u64, u64, u64, bool),
) -> Result<bool, ApiError> {
    let rows = aurora_db::repo::plugins::list_by_status(
        &state.pool,
        &[
            "pending_approval",
            "active",
            "superseded",
            "rejected",
            "disabled",
        ],
    )
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| r.package_id == package_id)
        .filter_map(|r| parse_semver(&r.version))
        .all(|v| semver_newer(candidate, v)))
}

/// Ingest one archive through the ordinary pipeline with `Source::Addon`: fetch (≤ 1
/// redirect), digest-check **before** parsing, then the ordinary ingest (the package must
/// verify against the source's `trusted_keys` row inside the pipeline). `Ok(Err)` carries
/// a sanitized per-version failure reason that never blocks the other versions.
async fn ingest_version(
    state: &AppState,
    source: &AddonSourceRow,
    actor: &AuditActor,
    version: &manifest::AddonVersion,
) -> Result<Result<PackageRow, String>, ApiError> {
    let bytes = match fetcher(state).package(&version.package).await {
        Ok(b) => b,
        Err(e) => return Ok(Err(e.reason())),
    };
    if !digest_matches(&bytes, &version.sha256) {
        return Ok(Err(
            "the package digest does not match the manifest".to_owned()
        ));
    }
    // A version that fails validation (signature, linter, limits) is reported per version
    // and never fails the whole check (C-607).
    match ingest::ingest_for_source(state, &bytes, source, actor).await {
        Ok(row) => Ok(Ok(row)),
        Err(e) => Ok(Err(sanitize::one_line(&e.message(), 200))),
    }
}

/// The full check of one source: fetch manifest, compare digest, ingest newer versions.
/// Shared by the scheduled poll and the manual buttons (`trigger` lands in the audit row).
/// The caller holds the source's lease.
pub async fn check_source(
    state: &AppState,
    source: &AddonSourceRow,
    trigger: &str,
    actor: &AuditActor,
) -> Result<(String, Vec<PackageRow>, Vec<(String, String, String)>), ApiError> {
    let fetched = fetch_manifest(state, &source.url).await;
    let (m, _bytes) = match fetched {
        Ok(v) => v,
        Err(e) => {
            let reason = e
                .details()
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("the manifest could not be fetched")
                .to_owned();
            finish_failure(state, source, &reason, None).await?;
            audit::record(
                state,
                AuditEvent::new(actor.clone(), "addon_source.check", Outcome::Failure)
                    .target("addon_source", source.id)
                    .metadata(json!({"trigger": trigger, "reason": reason})),
            );
            return Err(e);
        }
    };
    // Key change: flag, refuse everything, wait for the admin (C-595).
    let fp = manifest::key_fingerprint(&m.signing_key);
    if fp != source.key_fingerprint {
        finish_failure(state, source, "signing_key_changed", Some(&fp)).await?;
        audit::record(
            state,
            AuditEvent::new(actor.clone(), "addon_source.check", Outcome::Failure)
                .target("addon_source", source.id)
                .metadata(json!({"trigger": trigger, "reason": "signing_key_changed",
                    "old": source.key_fingerprint, "new": fp})),
        );
        return Ok(("failed".to_owned(), Vec::new(), Vec::new()));
    }
    let actor = actor.clone();
    let mut ingested = Vec::new();
    let mut failed: Vec<(String, String, String)> = Vec::new();
    let mut any_new = false;
    let views = m.addons.clone();
    for a in &views {
        for (v, ver) in &a.versions {
            let Some(sem) = parse_semver(v) else { continue };
            if core_supports(ver.min_core.as_deref()).is_err() {
                continue;
            }
            if !is_newer(state, &a.id, sem).await? {
                continue;
            }
            any_new = true;
            match ingest_version(state, source, &actor, ver).await? {
                Ok(row) => ingested.push(row),
                Err(reason) => failed.push((a.id.clone(), v.clone(), reason)),
            }
        }
    }
    let status = if any_new { "updated" } else { "up_to_date" };
    // The check result and its audit row land together (addon_source.check is
    // security-critical: `04 §Audit`).
    let ev = AuditEvent::new(actor.clone(), "addon_source.check", Outcome::Success)
        .target("addon_source", source.id)
        .metadata(json!({"trigger": trigger, "status": status,
            "ingested": ingested.len(), "failed_versions": failed.len()}));
    let mut tx = audit::begin(state).await?;
    repo::finish_check(
        &mut *tx,
        &repo::CheckDone {
            id: source.id,
            owner: source.lease_owner.unwrap_or_default(),
            status: "ok",
            reason: None,
            new_key_fingerprint: None,
            manifest_digest: Some(m.digest.clone()),
            next_in_secs: next_in_secs(&source.update_interval, source.id),
        },
    )
    .await?;
    audit::commit_critical(tx, &ev).await?;
    Ok((status.to_owned(), ingested, failed))
}

async fn finish_failure(
    state: &AppState,
    source: &AddonSourceRow,
    reason: &str,
    new_fp: Option<&str>,
) -> Result<(), ApiError> {
    let status = if new_fp.is_some() {
        "signing_key_changed"
    } else {
        "check_failed"
    };
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    repo::finish_check(
        &mut c,
        &repo::CheckDone {
            id: source.id,
            owner: source.lease_owner.unwrap_or_default(),
            status,
            reason: Some(sanitize::one_line(reason, 200)),
            new_key_fingerprint: new_fp.map(str::to_owned),
            manifest_digest: None,
            next_in_secs: aurora_core::limits::ADDON_FAILURE_BACKOFF.as_secs() as i64,
        },
    )
    .await?;
    Ok(())
}

/// The scheduled poll: claim due sources under their lease and check them.
pub async fn poll_once(state: &AppState) -> Result<usize, ApiError> {
    let interval = state.config.plugins.addon_sources.update_interval.clone();
    let _ = interval;
    let owner = lease_owner();
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let claimed = repo::claim_due(
        &mut c,
        owner,
        aurora_core::limits::ADDON_CHECK_CONCURRENCY as i64,
        aurora_core::limits::ADDON_LEASE.as_secs_f64(),
    )
    .await?;
    drop(c);
    let n = claimed.len();
    for source in claimed {
        let s = state.clone();
        let actor = AuditActor::system("addon-source-poll");
        let res = check_source(&s, &source, "scheduled", &actor).await;
        if let Err(e) = res {
            tracing::warn!(source = %source.id, error = %e.message(), "addon source check failed");
        }
    }
    Ok(n)
}

/// Registration payload validated against the config enum.
pub fn valid_interval(v: &str) -> bool {
    matches!(v, "6h" | "12h" | "1d" | "1w" | "2w" | "1mo" | "never")
}

/// One audited transaction: source row + `trusted_keys` row (C-593).
pub struct Registration {
    pub source: AddonSourceRow,
    pub key: aurora_db::repo::plugins::TrustedKeyRow,
}

pub async fn register(
    state: &AppState,
    actor: &AuditActor,
    admin: Uuid,
    url: &str,
    update_interval: &str,
) -> Result<Registration, ApiError> {
    let url = manifest::normalize_url(url).map_err(refused)?;
    if !valid_interval(update_interval) {
        return Err(ApiError::invalid_request(
            "Invalid update interval.",
            vec![aurora_core::FieldError::new(
                "/update_interval",
                "must be one of 6h, 12h, 1d, 1w, 2w, 1mo, never",
            )],
        ));
    }
    if repo::get_by_url(&state.pool, &url).await?.is_some() {
        return Err(ApiError::conflict(
            "addon_source_exists",
            "This source is already registered.",
        ));
    }
    let (m, bytes) = fetch_manifest(state, &url).await?;
    let fp = manifest::key_fingerprint(&m.signing_key);
    let publisher = m.publisher.clone();
    let key_name = format!("{} signing key", m.name);
    let signing_key = m.signing_key.clone();

    let mut tx = audit::begin(state).await?;
    let key = aurora_db::repo::plugins::insert_trusted_key(
        &mut *tx,
        ids::new_id(),
        &publisher,
        &key_name,
        &signing_key,
        admin,
    )
    .await
    .map_err(|e| {
        aurora_db::conflict_code(e, &[("trusted_keys_public_key_key", "addon_key_exists")])
    })?;
    let source = repo::insert(
        &mut *tx,
        &repo::NewSource {
            id: ids::new_id(),
            name: &m.name,
            url: &url,
            homepage: m.homepage.as_deref(),
            contact: m.contact.as_deref(),
            publisher: &publisher,
            signing_key: &signing_key,
            key_fingerprint: &fp,
            manifest_digest: &m.digest,
            update_interval,
            created_by: admin,
        },
    )
    .await
    .map_err(|e| {
        aurora_db::conflict_code(e, &[("addon_sources_url_key", "addon_source_exists")])
    })?;
    let _ = bytes;
    let ev = AuditEvent::new(actor.clone(), "addon_source.add", Outcome::Success)
        .target("addon_source", source.id)
        .metadata(json!({"url": url, "publisher": publisher,
            "key_fingerprint": fp, "addons": m.addons.len()}));
    audit::commit_critical(tx, &ev).await?;
    Ok(Registration { source, key })
}

/// Source DTO builder (C-590): row + manifest-derived views + install state.
pub async fn to_dto(state: &AppState, r: &AddonSourceRow) -> Result<Value, ApiError> {
    let (m, _bytes) = match fetch_manifest(state, &r.url).await {
        Ok(v) => v,
        Err(_) => {
            // Last known state keeps serving even when the source is unreachable right now:
            // render from the stored columns without live manifest data.
            return Ok(json!({
                "id": r.id, "name": r.name, "url": r.url,
                "homepage": r.homepage, "contact": r.contact,
                "publisher": r.publisher, "key_fingerprint": r.key_fingerprint,
                "new_key_fingerprint": r.new_key_fingerprint,
                "manifest_digest": r.manifest_digest,
                "update_interval": r.update_interval,
                "status": r.status,
                "check_failed_at": r.check_failed_at,
                "check_failed_reason": r.check_failed_reason,
                "last_checked_at": r.last_checked_at,
                "addons": [], "addon_count": 0, "removed_upstream": [],
                "created_at": r.created_at, "updated_at": r.updated_at,
            }));
        }
    };
    let installed_map: BTreeMap<String, String> = repo::newest_ingested(&state.pool, r.id)
        .await?
        .into_iter()
        .collect();
    // "removed upstream": packages of this source whose package_id is no longer in the manifest.
    let stored = repo::packages_of(&state.pool, r.id).await?;
    let mut removed: Vec<String> = stored
        .iter()
        .map(|(p, _, _)| p.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|p| !m.addons.iter().any(|a| &a.id == p))
        .collect();
    removed.sort();
    let views = addon_views(&m, &installed_map);
    let addons: Vec<Value> = views
        .iter()
        .map(|a| {
            json!({
                "id": a.id, "name": a.name, "description": a.description,
                "versions": a.versions.iter().map(|v| json!({
                    "version": v.version, "min_core": v.min_core,
                    "ingestable": v.ingestable,
                    "not_ingestable_reason": v.not_ingestable_reason,
                    "installed_version": v.installed_version,
                    "update_available": v.update_available,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let addon_count: usize = addons
        .iter()
        .map(|a| a["versions"].as_array().map_or(0, Vec::len))
        .sum();
    Ok(json!({
        "id": r.id, "name": r.name, "url": r.url,
        "homepage": r.homepage, "contact": r.contact,
        "publisher": r.publisher, "key_fingerprint": r.key_fingerprint,
        "new_key_fingerprint": r.new_key_fingerprint,
        "manifest_digest": r.manifest_digest,
        "update_interval": r.update_interval,
        "status": r.status,
        "check_failed_at": r.check_failed_at,
        "check_failed_reason": r.check_failed_reason,
        "last_checked_at": r.last_checked_at,
        "addons": addons, "addon_count": addon_count,
        "removed_upstream": removed,
        "created_at": r.created_at, "updated_at": r.updated_at,
    }))
}

/// Removal (C-597): refused while packages are active; packages stay.
pub async fn remove(state: &AppState, actor: &AuditActor, id: Uuid) -> Result<(), ApiError> {
    let r = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Addon source not found."))?;
    let mut tx = audit::begin(state).await?;
    if repo::has_active(&mut *tx, id).await? {
        return Err(ApiError::conflict(
            "addon_source_active",
            "Packages of this source are active; remove or disable them first.",
        ));
    }
    let (_ingested, active) = repo::package_counts(&mut *tx, id).await?;
    repo::delete(&mut *tx, id).await?;
    let ev = AuditEvent::new(actor.clone(), "addon_source.remove", Outcome::Success)
        .target("addon_source", id)
        .metadata(json!({"url": r.url, "packages_kept": _ingested, "active": active}));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}

/// Interval update (C-594).
pub async fn update_interval_for(
    state: &AppState,
    actor: &AuditActor,
    id: Uuid,
    interval: &str,
) -> Result<AddonSourceRow, ApiError> {
    if !valid_interval(interval) {
        return Err(ApiError::invalid_request(
            "Invalid update interval.",
            vec![aurora_core::FieldError::new(
                "/update_interval",
                "must be one of 6h, 12h, 1d, 1w, 2w, 1mo, never",
            )],
        ));
    }
    let r = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Addon source not found."))?;
    let mut tx = audit::begin(state).await?;
    repo::update_interval(&mut *tx, id, interval).await?;
    let ev = AuditEvent::new(actor.clone(), "addon_source.update", Outcome::Success)
        .target("addon_source", id)
        .metadata(json!({"change": "interval", "interval": format!("{} → {}", r.update_interval, interval)}));
    audit::commit_critical(tx, &ev).await?;
    Ok(repo::get(&state.pool, id).await?.unwrap_or(r))
}

/// Key re-confirm (C-595): update the source and its `trusted_keys` row together.
pub async fn confirm_key(
    state: &AppState,
    actor: &AuditActor,
    id: Uuid,
    new_key_fingerprint: &str,
) -> Result<AddonSourceRow, ApiError> {
    let r = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Addon source not found."))?;
    if r.status != "signing_key_changed" {
        return Err(ApiError::conflict(
            "addon_key_not_changed",
            "This source's signing key has not changed.",
        ));
    }
    let (m, _bytes) = fetch_manifest(state, &r.url).await?;
    let fp = manifest::key_fingerprint(&m.signing_key);
    if fp != new_key_fingerprint || fp != manifest::key_fingerprint(&m.signing_key) {
        return Err(ApiError::conflict(
            "addon_key_changed_again",
            "The manifest's signing key changed again; re-review it.",
        ));
    }
    let old_fp = r.key_fingerprint.clone();
    let mut tx = audit::begin(state).await?;
    // The trusted key row of this source's publisher (identified by the old key bytes).
    let keys = aurora_db::repo::plugins::all_trusted_keys(&mut *tx).await?;
    let key = keys
        .iter()
        .find(|k| k.public_key == r.signing_key)
        .ok_or_else(|| ApiError::internal("The source's trusted key row is missing."))?;
    aurora_db::repo::plugins::update_trusted_key_public_key(&mut *tx, key.id, &m.signing_key)
        .await?;
    repo::confirm_key(&mut *tx, id, &m.signing_key, &fp, &m.digest).await?;
    let ev = AuditEvent::new(actor.clone(), "addon_source.update", Outcome::Success)
        .target("addon_source", id)
        .metadata(json!({"change": "signing_key_confirmed", "old": old_fp, "new": fp}));
    audit::commit_critical(tx, &ev).await?;
    Ok(repo::get(&state.pool, id).await?.unwrap_or(r))
}

/// The plugins list joins source names for provenance (C-600).
pub async fn source_names(state: &AppState) -> Result<BTreeMap<Uuid, String>, ApiError> {
    Ok(repo::list(&state.pool)
        .await?
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect())
}

/// Manual check of one source (C-596): lease-protected, synchronous.
pub async fn check_now(
    state: &AppState,
    actor: &AuditActor,
    id: Uuid,
) -> Result<(String, Vec<PackageRow>, Vec<(String, String, String)>), ApiError> {
    let owner = lease_owner();
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let source = repo::claim_one(
        &mut c,
        id,
        owner,
        aurora_core::limits::ADDON_LEASE.as_secs_f64(),
    )
    .await?
    .ok_or_else(|| {
        ApiError::conflict(
            "addon_check_in_progress",
            "A check of this source is already running.",
        )
    })?;
    drop(c);
    let mut source = source;
    source.lease_owner = Some(owner);
    let out = check_source(state, &source, "manual", actor).await;
    if out.is_err() {
        // The check recorded its failure (or could not run): make sure the lease is free.
        if let Ok(mut c) = state.pool.acquire().await {
            let _ = repo::release_lease(&mut c, id, owner).await;
        }
    }
    out
}

/// Manual check of every source (check-all, C-596): sequential, a failure never blocks.
pub async fn check_all(state: &AppState, actor: &AuditActor) -> Result<Vec<Value>, ApiError> {
    let mut out = Vec::new();
    for r in repo::list(&state.pool).await? {
        let owner = lease_owner();
        let mut c = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        let claimed = repo::claim_one(
            &mut c,
            r.id,
            owner,
            aurora_core::limits::ADDON_LEASE.as_secs_f64(),
        )
        .await?;
        drop(c);
        let Some(mut source) = claimed else {
            out.push(json!({"source_id": r.id, "status": "failed",
                "check_failed_reason": "busy", "ingested": [], "failed_versions": []}));
            continue;
        };
        source.lease_owner = Some(owner);
        match check_source(state, &source, "manual", actor).await {
            Ok((status, ingested, failed)) => {
                out.push(json!({"source_id": source.id, "status": status,
                    "ingested": ingested.iter().map(|p| json!({"id": p.id,
                        "package_id": p.package_id, "version": p.version, "status": p.status}))
                        .collect::<Vec<_>>(),
                    "failed_versions": failed.iter().map(|(p, v, r)| json!({
                        "package_id": p, "version": v, "reason": r})).collect::<Vec<_>>(),
                    "check_failed_reason": Value::Null}));
            }
            Err(e) => {
                if let Ok(mut c) = state.pool.acquire().await {
                    let _ = repo::release_lease(&mut c, source.id, owner).await;
                }
                out.push(json!({"source_id": source.id, "status": "failed",
                    "check_failed_reason": sanitize::one_line(e.message(), 200),
                    "ingested": [], "failed_versions": []}));
            }
        }
    }
    Ok(out)
}

/// The `PluginPackage` DTO for a freshly ingested row (used by check results).
pub async fn package_dto(state: &AppState, row: PackageRow) -> Result<Value, ApiError> {
    let dto = service::to_dto(state, row).await?;
    serde_json::to_value(dto)
        .map_err(|_| ApiError::internal("The package DTO could not be serialized."))
}

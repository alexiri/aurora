//! Addon sources (P5-ASRC, D-250): registration (TOFU), the shared check path, and the poll job.
//!
//! Trust model: the manifest's `signing_key` is the anchor — registration stores it in a
//! `trusted_keys` row, every package must be signed by exactly that key, a changed key flags the
//! source for an admin's re-confirm and never auto-trusts (SEC-86). Packages from a source are
//! ordinary packages in every other respect and never activate on first sight.

pub mod dto;
pub mod fetch;
pub mod manifest;
pub mod poll;
pub mod routes;
pub use routes::router;

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::plugins::dto::PluginPackage;
use crate::domains::plugins::ingest::{self, AddonExpect};
use crate::domains::plugins::service;
use aurora_core::{ApiError, ids, sanitize};
use aurora_db::repo::addon_sources::{self as repo, AddonSourceRow};
use aurora_db::repo::plugins::{self as plugins_repo, NewPackage, PackageRow};
use aurora_registry::manifest::parse_semver;
use dto::{
    AddonEntry, AddonRepoInfo, AddonSource, AddonSourceCheckAll, AddonSourceCheckResult,
    AddonSourcePreview, AddonVersion, FailedVersion, RemovedUpstream,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// `(major, minor, patch, is_release)`: the derived tuple order is the semver order.
type Semver = (u64, u64, u64, bool);

/// The interval values of the owner's enum.
pub const INTERVALS: [&str; 7] = ["6h", "12h", "1d", "1w", "2w", "1mo", "never"];

/// Whether `v` is one of [`INTERVALS`].
#[must_use]
pub fn valid_interval(v: &str) -> bool {
    INTERVALS.contains(&v)
}

/// Interval seconds for the owner's enum (`None` for `never`).
#[must_use]
pub fn interval_secs(interval: &str) -> Option<i64> {
    match interval {
        "6h" => Some(6 * 3600),
        "12h" => Some(12 * 3600),
        "1d" => Some(24 * 3600),
        "1w" => Some(7 * 24 * 3600),
        "2w" => Some(14 * 24 * 3600),
        "1mo" => Some(30 * 24 * 3600),
        _ => None,
    }
}

/// ±10 % jitter keyed by the source id (stable per row, no clock read).
fn jitter(id: Uuid) -> f64 {
    let n = (id.as_u128() % 2001) as f64 / 10_000.0 - 0.1;
    1.0 + n
}

/// Seconds until the next scheduled check, `None` for `never` (no schedule: manual only).
fn next_in_secs(interval: &str, id: Uuid) -> Option<i64> {
    interval_secs(interval).map(|s| ((s as f64) * jitter(id)) as i64)
}

/// The per-process lease owner id (random per call, like the library refresh).
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

/// The sanitized one-sentence reason of a failed fetch or parse, for `check_failed_reason`.
fn reason_of(e: &ApiError) -> String {
    e.details()
        .and_then(|d| d.get("reason"))
        .and_then(Value::as_str)
        .map_or_else(
            || sanitize::one_line(e.message(), 200),
            |r| sanitize::one_line(r, 200),
        )
}

fn fetcher(state: &AppState) -> fetch::Fetcher<'_> {
    fetch::Fetcher::with_env(&state.connect)
}

/// Fetch, size-check and parse the manifest (the shared first step of every path).
async fn fetch_manifest(state: &AppState, url: &str) -> Result<manifest::Manifest, ApiError> {
    let bytes = fetcher(state).manifest(url).await.map_err(refused)?;
    manifest::parse(&bytes).map_err(refused)
}

fn semver_of(v: &str) -> Option<Semver> {
    parse_semver(v)
}

/// The newest of `(version, semver)` pairs.
fn newest(rows: impl Iterator<Item = (String, Semver)>) -> Option<(String, Semver)> {
    rows.max_by_key(|(_, s)| *s)
}

/// Whether this core can run a version that needs `min_core`: `Err("core_too_new")` when not.
fn core_supports(min_core: Option<&str>) -> Result<(), &'static str> {
    let Some(mc) = min_core.and_then(parse_semver) else {
        return Ok(());
    };
    let running = parse_semver(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0, true));
    if (running.0, running.1, running.2) >= (mc.0, mc.1, mc.2) {
        Ok(())
    } else {
        Err("core_too_new")
    }
}

/// What the instance holds of the packages of a manifest, from any source (package ids are
/// unique across the instance): the active version and every non-invalid version.
#[derive(Default, Debug)]
struct Installed {
    active: Option<String>,
    all: Vec<(String, Semver)>,
}

impl Installed {
    fn newest(&self) -> Option<(String, Semver)> {
        newest(self.all.iter().cloned())
    }
}

async fn installed_state(
    state: &AppState,
    package_ids: &[String],
) -> Result<BTreeMap<String, Installed>, ApiError> {
    let mut out: BTreeMap<String, Installed> = BTreeMap::new();
    for (pid, version, status) in repo::package_versions(&state.pool, package_ids).await? {
        let e = out.entry(pid).or_default();
        if status == "active" {
            e.active = Some(version.clone());
        }
        if let Some(s) = semver_of(&version) {
            e.all.push((version, s));
        }
    }
    Ok(out)
}

fn versions_desc(a: &manifest::Addon) -> Vec<(&String, &manifest::AddonVersion, Semver)> {
    let mut v: Vec<_> = a
        .versions
        .iter()
        .filter_map(|(k, ver)| semver_of(k).map(|s| (k, ver, s)))
        .collect();
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v
}

fn entry_views(
    addons: &[manifest::Addon],
    installed: &BTreeMap<String, Installed>,
) -> Vec<AddonEntry> {
    addons
        .iter()
        .map(|a| {
            let inst = installed.get(&a.id);
            let newest_ingested = inst.and_then(Installed::newest);
            let installed_version = inst
                .and_then(|i| i.active.clone())
                .or_else(|| newest_ingested.as_ref().map(|(v, _)| v.clone()));
            AddonEntry {
                id: a.id.clone(),
                name: a.name.clone(),
                description: a.description.clone(),
                versions: versions_desc(a)
                    .into_iter()
                    .map(|(v, ver, s)| {
                        let gate = core_supports(ver.min_core.as_deref());
                        AddonVersion {
                            version: v.clone(),
                            min_core: ver.min_core.clone(),
                            ingestable: gate.is_ok(),
                            not_ingestable_reason: gate.err().map(str::to_owned),
                            installed_version: installed_version.clone(),
                            update_available: newest_ingested.as_ref().is_some_and(|(_, n)| s > *n),
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The versions of `a` a check ingests now: every ingestable version newer than the newest one
/// the instance holds (ascending); the newest ingestable one when the instance holds none (a
/// first registration does not pull the whole history). Versions this source stored `invalid`
/// are not retried.
fn candidates<'a>(
    a: &'a manifest::Addon,
    inst: Option<&Installed>,
    invalid: &BTreeSet<(String, String)>,
) -> Vec<(&'a String, &'a manifest::AddonVersion)> {
    let held = inst.and_then(Installed::newest);
    let mut ok: Vec<_> = versions_desc(a)
        .into_iter()
        .filter(|(_, ver, _)| core_supports(ver.min_core.as_deref()).is_ok())
        .filter(|(v, _, _)| !invalid.contains(&(a.id.clone(), (*v).clone())))
        .collect();
    ok.reverse(); // ascending
    match held {
        Some((_, n)) => ok
            .into_iter()
            .filter(|(_, _, s)| *s > n)
            .map(|(v, ver, _)| (v, ver))
            .collect(),
        None => ok
            .into_iter()
            .next_back()
            .map(|(v, ver, _)| vec![(v, ver)])
            .unwrap_or_default(),
    }
}

/// sha256 hex of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The short reason a failed ingest is reported and stored with (`code: message` of the first
/// violation, or the error's own message).
fn ingest_reason(e: &ApiError) -> String {
    let first = e
        .details()
        .and_then(|d| d.get("violations"))
        .and_then(Value::as_array)
        .and_then(|v| v.first());
    match first {
        Some(v) => {
            let code = v.get("code").and_then(Value::as_str).unwrap_or("invalid");
            let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
            sanitize::one_line(&format!("{code}: {msg}"), 200)
        }
        None => sanitize::one_line(e.message(), 200),
    }
}

/// Store a version that failed signature, digest, identity or lint as an `invalid` package row
/// with the reason (nothing of the archive is kept): the dashboard shows it, the next check does
/// not retry it, and nothing partial survives.
async fn store_invalid(
    state: &AppState,
    source: &AddonSourceRow,
    package_id: &str,
    version: &str,
    digest: &str,
    reason: &str,
) -> Result<(), ApiError> {
    // `status_reason` is a short code (≤ 64 characters); the sentence lives in `violations`.
    let code: String = reason
        .split(':')
        .next()
        .unwrap_or("invalid")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(64)
        .collect();
    let violations = json!([{ "code": code, "message": reason }]);
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    plugins_repo::insert_package(
        &mut c,
        &NewPackage {
            id: ids::new_id(),
            package_id,
            version,
            contract: 0,
            publisher: &source.publisher,
            digest,
            signer_key_id: "unsigned",
            manifest: &json!({}),
            permissions: &json!({}),
            violations: &violations,
            source: "addon",
            source_id: Some(source.id),
            status: "invalid",
            status_reason: Some(&code),
            approved_now: false,
        },
    )
    .await?;
    Ok(())
}

/// Fetch and ingest one listed version. `Ok(Err(reason))` is a per-version failure that never
/// blocks the other versions (stored `invalid` when the archive itself is at fault; a network
/// failure is transient and stores nothing).
async fn ingest_version(
    state: &AppState,
    source: &AddonSourceRow,
    actor: &AuditActor,
    package_id: &str,
    version: &str,
    ver: &manifest::AddonVersion,
) -> Result<Result<PackageRow, String>, ApiError> {
    let bytes = match fetcher(state).package(&ver.package).await {
        Ok(b) => b,
        Err(e) => return Ok(Err(e.reason())),
    };
    let actual = sha256_hex(&bytes);
    // The digest is the integrity anchor: nothing is parsed before it matches the manifest.
    if !actual.eq_ignore_ascii_case(&ver.sha256) {
        let reason = "digest_mismatch: the archive does not match the manifest's sha256";
        store_invalid(
            state,
            source,
            package_id,
            version,
            &ver.sha256.to_ascii_lowercase(),
            reason,
        )
        .await?;
        return Ok(Err(reason.to_owned()));
    }
    let expect = AddonExpect {
        source,
        package_id,
        version,
    };
    match ingest::ingest_for_source(state, &bytes, &expect, actor).await {
        Ok(row) => Ok(Ok(row)),
        Err(e) if e.status().is_server_error() => Err(e),
        Err(e) => {
            let reason = ingest_reason(&e);
            store_invalid(state, source, package_id, version, &actual, &reason).await?;
            Ok(Err(reason))
        }
    }
}

/// What a check ingested and what it could not.
#[derive(Default)]
pub struct Ingestion {
    pub ingested: Vec<PackageRow>,
    pub failed: Vec<(String, String, String)>,
}

/// Ingest what `m` lists and the instance lacks (see [`candidates`]); the caller holds the lease
/// (or is registering).
async fn ingest_listed(
    state: &AppState,
    source: &AddonSourceRow,
    actor: &AuditActor,
    m: &manifest::Manifest,
) -> Result<Ingestion, ApiError> {
    let ids: Vec<String> = m.addons.iter().map(|a| a.id.clone()).collect();
    let installed = installed_state(state, &ids).await?;
    let invalid: BTreeSet<(String, String)> = repo::invalid_versions(&state.pool, source.id)
        .await?
        .into_iter()
        .collect();
    let mut out = Ingestion::default();
    for a in &m.addons {
        for (v, ver) in candidates(a, installed.get(&a.id), &invalid) {
            match ingest_version(state, source, actor, &a.id, v, ver).await? {
                Ok(row) => out.ingested.push(row),
                Err(reason) => out.failed.push((a.id.clone(), v.clone(), reason)),
            }
        }
    }
    Ok(out)
}

/// The accepted manifest as stored on the source row.
fn stored_manifest(m: &manifest::Manifest) -> Value {
    json!({ "key_description": m.signing_key_description, "addons": m.addons })
}

fn stored_addons(row: &AddonSourceRow) -> Vec<manifest::Addon> {
    row.manifest
        .get("addons")
        .and_then(|a| serde_json::from_value(a.clone()).ok())
        .unwrap_or_default()
}

/// The result of a check of one source.
pub struct CheckOutcome {
    /// `updated` | `up_to_date` | `failed`.
    pub status: &'static str,
    pub ingestion: Ingestion,
    pub check_failed_reason: Option<String>,
}

impl CheckOutcome {
    fn failed(reason: String) -> Self {
        Self {
            status: "failed",
            ingestion: Ingestion::default(),
            check_failed_reason: Some(reason),
        }
    }
}

/// Record a failed check: the source keeps serving its last known state, the reason is stored
/// (sanitized) and the failure is a security-critical `addon_source.check` row.
async fn fail_check(
    state: &AppState,
    source: &AddonSourceRow,
    trigger: &str,
    actor: &AuditActor,
    reason: &str,
    new_fingerprint: Option<&str>,
) -> Result<(), ApiError> {
    let status = if new_fingerprint.is_some() {
        "signing_key_changed"
    } else {
        "check_failed"
    };
    let reason = sanitize::one_line(reason, 200);
    let mut tx = audit::begin(state).await?;
    repo::finish_check(
        &mut *tx,
        &repo::CheckDone {
            id: source.id,
            owner: source.lease_owner.unwrap_or_default(),
            status,
            reason: Some(reason.clone()),
            new_key_fingerprint: new_fingerprint.map(str::to_owned),
            manifest_digest: None,
            manifest: None,
            next_in_secs: aurora_core::limits::ADDON_FAILURE_BACKOFF.as_secs() as i64,
        },
    )
    .await?;
    let mut meta = json!({ "trigger": trigger, "reason": reason });
    if let Some(n) = new_fingerprint {
        meta["old"] = json!(source.key_fingerprint);
        meta["new"] = json!(n);
    }
    let ev = AuditEvent::new(actor.clone(), "addon_source.check", Outcome::Failure)
        .target("addon_source", source.id)
        .metadata(meta);
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}

/// The full check of one source: fetch the manifest, refuse a changed key, ingest what is new.
/// Shared by the scheduled poll and the manual buttons (`trigger` lands in the audit row); the
/// caller holds the source's lease and this releases it with the outcome.
pub async fn check_source(
    state: &AppState,
    source: &AddonSourceRow,
    trigger: &str,
    actor: &AuditActor,
) -> Result<CheckOutcome, ApiError> {
    let m = match fetch_manifest(state, &source.url).await {
        Ok(m) => m,
        Err(e) => {
            let reason = reason_of(&e);
            fail_check(state, source, trigger, actor, &reason, None).await?;
            return Ok(CheckOutcome::failed(reason));
        }
    };
    // Key change: flag, ingest nothing, wait for the admin (C-595).
    let fp = manifest::key_fingerprint(&m.signing_key);
    if fp != source.key_fingerprint {
        fail_check(
            state,
            source,
            trigger,
            actor,
            "signing_key_changed",
            Some(&fp),
        )
        .await?;
        return Ok(CheckOutcome::failed("signing_key_changed".to_owned()));
    }
    if m.publisher != source.publisher {
        let reason = "the manifest's publisher changed";
        fail_check(state, source, trigger, actor, reason, None).await?;
        return Ok(CheckOutcome::failed(reason.to_owned()));
    }
    let ingestion = ingest_listed(state, source, actor, &m).await?;
    let status = if ingestion.ingested.is_empty() {
        "up_to_date"
    } else {
        "updated"
    };
    // The check outcome and its audit row land together (`addon_source.check` is
    // security-critical, `04 §Audit`).
    let ev = AuditEvent::new(actor.clone(), "addon_source.check", Outcome::Success)
        .target("addon_source", source.id)
        .metadata(json!({"trigger": trigger, "status": status,
            "ingested": ingestion.ingested.len(), "failed_versions": ingestion.failed.len()}));
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
            manifest: Some(stored_manifest(&m)),
            next_in_secs: next_in_secs(&source.update_interval, source.id)
                .unwrap_or(365 * 24 * 3600),
        },
    )
    .await?;
    audit::commit_critical(tx, &ev).await?;
    Ok(CheckOutcome {
        status,
        ingestion,
        check_failed_reason: None,
    })
}

/// The scheduled poll: claim due sources under their lease and check them. Returns how many
/// sources this tick checked.
pub async fn poll_once(state: &AppState) -> Result<usize, ApiError> {
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
        let actor = AuditActor::system("addon-source-poll");
        if let Err(e) = check_source(state, &source, "scheduled", &actor).await {
            tracing::warn!(source = %source.id, error = %e.message(), "addon source check failed");
            release(state, &source).await;
        }
    }
    Ok(n)
}

/// Free the lease of a check that ended without writing its outcome.
async fn release(state: &AppState, source: &AddonSourceRow) {
    if let (Some(owner), Ok(mut c)) = (source.lease_owner, state.pool.acquire().await) {
        let _ = repo::release_lease(&mut c, source.id, owner).await;
    }
}

/// Manual check of one source (C-596): the same path under the same lease, synchronous.
pub async fn check_now(
    state: &AppState,
    actor: &AuditActor,
    id: Uuid,
) -> Result<AddonSourceCheckResult, ApiError> {
    if repo::get(&state.pool, id).await?.is_none() {
        return Err(ApiError::not_found("Addon source not found."));
    }
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
    let out = check_source(state, &source, "manual", actor).await;
    if out.is_err() {
        release(state, &source).await;
    }
    result_dto(state, &source, out?).await
}

/// Manual check of every source (C-596): sequential, a failing or busy source never blocks the
/// others.
pub async fn check_all(
    state: &AppState,
    actor: &AuditActor,
) -> Result<AddonSourceCheckAll, ApiError> {
    let mut items = Vec::new();
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
        let Some(source) = claimed else {
            items.push(AddonSourceCheckResult {
                source_id: r.id,
                status: "failed".to_owned(),
                ingested: Vec::new(),
                failed_versions: Vec::new(),
                check_failed_reason: Some("busy".to_owned()),
            });
            continue;
        };
        match check_source(state, &source, "manual", actor).await {
            Ok(out) => items.push(result_dto(state, &source, out).await?),
            Err(e) => {
                release(state, &source).await;
                items.push(AddonSourceCheckResult {
                    source_id: source.id,
                    status: "failed".to_owned(),
                    ingested: Vec::new(),
                    failed_versions: Vec::new(),
                    check_failed_reason: Some(sanitize::one_line(e.message(), 200)),
                });
            }
        }
    }
    Ok(AddonSourceCheckAll { items })
}

async fn result_dto(
    state: &AppState,
    source: &AddonSourceRow,
    out: CheckOutcome,
) -> Result<AddonSourceCheckResult, ApiError> {
    let mut ingested: Vec<PluginPackage> = Vec::with_capacity(out.ingestion.ingested.len());
    for row in out.ingestion.ingested {
        ingested.push(service::to_dto(state, row).await?);
    }
    Ok(AddonSourceCheckResult {
        source_id: source.id,
        status: out.status.to_owned(),
        ingested,
        failed_versions: out
            .ingestion
            .failed
            .into_iter()
            .map(|(package_id, version, reason)| FailedVersion {
                package_id,
                version,
                reason,
            })
            .collect(),
        check_failed_reason: out.check_failed_reason,
    })
}

/// What registering a manifest does, without doing it (C-592).
pub async fn preview(state: &AppState, pasted: &str) -> Result<AddonSourcePreview, ApiError> {
    let url = manifest::normalize_url(pasted).map_err(refused)?;
    if repo::get_by_url(&state.pool, &url).await?.is_some() {
        return Err(ApiError::conflict(
            "addon_source_exists",
            "This source is already registered.",
        ));
    }
    let m = fetch_manifest(state, &url).await?;
    let ids: Vec<String> = m.addons.iter().map(|a| a.id.clone()).collect();
    let installed = installed_state(state, &ids).await?;
    let none = BTreeSet::new();
    let packages_new = m
        .addons
        .iter()
        .map(|a| candidates(a, installed.get(&a.id), &none).len())
        .sum();
    Ok(AddonSourcePreview {
        url,
        repo: AddonRepoInfo {
            name: m.name.clone(),
            description: m.description.clone(),
            homepage: m.homepage.clone(),
            contact: m.contact.clone(),
        },
        publisher: m.publisher.clone(),
        key_fingerprint: manifest::key_fingerprint(&m.signing_key),
        key_description: m.signing_key_description.clone(),
        addons: entry_views(&m.addons, &installed),
        packages_new,
    })
}

/// The registered source and what its first ingestion produced (C-593).
pub struct Registration {
    pub source: AddonSourceRow,
    pub ingestion: Ingestion,
}

/// Register a source: one audited transaction creates the `addon_sources` row and the
/// `trusted_keys` row for the manifest's key (TOFU: the admin saw the fingerprint in the
/// preview), then the listed packages are ingested — each in its own transaction, a failing
/// version never rolls the registration back.
pub async fn register(
    state: &AppState,
    actor: &AuditActor,
    admin: Uuid,
    pasted: &str,
    update_interval: &str,
) -> Result<Registration, ApiError> {
    let url = manifest::normalize_url(pasted).map_err(refused)?;
    if !valid_interval(update_interval) {
        return Err(bad_interval());
    }
    if repo::get_by_url(&state.pool, &url).await?.is_some() {
        return Err(ApiError::conflict(
            "addon_source_exists",
            "This source is already registered.",
        ));
    }
    let m = fetch_manifest(state, &url).await?;
    let fp = manifest::key_fingerprint(&m.signing_key);
    let id = ids::new_id();
    let manifest_json = stored_manifest(&m);

    let mut tx = audit::begin(state).await?;
    plugins_repo::insert_trusted_key(
        &mut *tx,
        ids::new_id(),
        &m.publisher,
        &format!("{} signing key", m.name),
        &m.signing_key,
        admin,
    )
    .await
    .map_err(|e| {
        aurora_db::conflict_code(e, &[("trusted_keys_public_key_key", "addon_key_exists")])
    })?;
    let source = repo::insert(
        &mut *tx,
        &repo::NewSource {
            id,
            name: &m.name,
            url: &url,
            homepage: m.homepage.as_deref(),
            contact: m.contact.as_deref(),
            publisher: &m.publisher,
            signing_key: &m.signing_key,
            key_fingerprint: &fp,
            manifest_digest: &m.digest,
            manifest: &manifest_json,
            update_interval,
            first_check_in_secs: next_in_secs(update_interval, id),
            created_by: admin,
        },
    )
    .await
    .map_err(|e| {
        aurora_db::conflict_code(e, &[("addon_sources_url_key", "addon_source_exists")])
    })?;
    let ev = AuditEvent::new(actor.clone(), "addon_source.add", Outcome::Success)
        .target("addon_source", source.id)
        .metadata(
            json!({"url": url, "publisher": m.publisher, "key_fingerprint": fp,
            "update_interval": update_interval, "addons": m.addons.len()}),
        );
    audit::commit_critical(tx, &ev).await?;

    let ingestion = ingest_listed(state, &source, actor, &m).await?;
    Ok(Registration { source, ingestion })
}

fn bad_interval() -> ApiError {
    ApiError::invalid_request(
        "Invalid update interval.",
        vec![aurora_core::FieldError::new(
            "/update_interval",
            "must be one of 6h, 12h, 1d, 1w, 2w, 1mo, never",
        )],
    )
}

/// The `AddonSource` of a row, from its stored manifest (never a live fetch: the last known
/// state keeps serving when the source is unreachable).
pub async fn source_dto(state: &AppState, r: &AddonSourceRow) -> Result<AddonSource, ApiError> {
    let addons = stored_addons(r);
    let ids: Vec<String> = addons.iter().map(|a| a.id.clone()).collect();
    let installed = installed_state(state, &ids).await?;
    let entries = entry_views(&addons, &installed);
    let addon_count = entries.len();
    let listed: BTreeSet<(String, String)> = addons
        .iter()
        .flat_map(|a| a.versions.keys().map(|v| (a.id.clone(), v.clone())))
        .collect();
    let removed_upstream = repo::packages_of(&state.pool, r.id)
        .await?
        .into_iter()
        .filter(|(p, v, _)| !listed.contains(&(p.clone(), v.clone())))
        .map(|(package_id, version, _)| RemovedUpstream {
            package_id,
            version,
        })
        .collect();
    Ok(AddonSource {
        id: r.id,
        name: r.name.clone(),
        url: r.url.clone(),
        homepage: r.homepage.clone(),
        contact: r.contact.clone(),
        publisher: r.publisher.clone(),
        key_fingerprint: r.key_fingerprint.clone(),
        key_description: r
            .manifest
            .get("key_description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        update_interval: r.update_interval.clone(),
        status: r.status.clone(),
        check_failed_at: r.check_failed_at,
        check_failed_reason: r.check_failed_reason.clone(),
        new_key_fingerprint: r.new_key_fingerprint.clone(),
        last_checked_at: r.last_checked_at,
        manifest_digest: r.manifest_digest.clone(),
        addons: entries,
        addon_count,
        removed_upstream,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

/// Removal (C-597): refused while packages are active; the packages stay, polling stops.
pub async fn remove(state: &AppState, actor: &AuditActor, id: Uuid) -> Result<(), ApiError> {
    let r = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Addon source not found."))?;
    let mut tx = audit::begin(state).await?;
    if repo::has_active(&mut *tx, id).await? {
        return Err(ApiError::conflict(
            "addon_source_active",
            "Packages of this source are active; disable or reject them first.",
        ));
    }
    let (kept, _active) = repo::package_counts(&mut *tx, id).await?;
    repo::delete(&mut *tx, id).await?;
    let ev = AuditEvent::new(actor.clone(), "addon_source.remove", Outcome::Success)
        .target("addon_source", id)
        .metadata(json!({"url": r.url, "packages_kept": kept}));
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
        return Err(bad_interval());
    }
    let r = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Addon source not found."))?;
    let mut tx = audit::begin(state).await?;
    repo::update_interval(&mut *tx, id, interval, next_in_secs(interval, id)).await?;
    let ev = AuditEvent::new(actor.clone(), "addon_source.update", Outcome::Success)
        .target("addon_source", id)
        .metadata(json!({"change": "interval",
            "interval": format!("{} → {}", r.update_interval, interval)}));
    audit::commit_critical(tx, &ev).await?;
    Ok(repo::get(&state.pool, id).await?.unwrap_or(r))
}

/// Key re-confirm (C-595): the source and its `trusted_keys` row move to the new key together.
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
    let m = fetch_manifest(state, &r.url).await?;
    let fp = manifest::key_fingerprint(&m.signing_key);
    // The admin confirms the fingerprint they saw; if the manifest moved on, they re-review.
    if fp != new_key_fingerprint {
        return Err(ApiError::conflict(
            "addon_key_changed_again",
            "The manifest's signing key changed again; re-review it.",
        ));
    }
    if m.publisher != r.publisher {
        return Err(ApiError::unprocessable(
            "addon_manifest_invalid",
            "The manifest's publisher changed; register the source again.",
        ));
    }
    let old_fp = r.key_fingerprint.clone();
    let mut tx = audit::begin(state).await?;
    let keys = plugins_repo::all_trusted_keys(&mut *tx).await?;
    let key = keys
        .iter()
        .find(|k| k.public_key == r.signing_key && k.revoked_at.is_none())
        .ok_or_else(|| {
            ApiError::conflict(
                "addon_key_revoked",
                "The source's trusted key was revoked; register the source again.",
            )
        })?;
    plugins_repo::update_trusted_key_public_key(&mut *tx, key.id, &m.signing_key)
        .await
        .map_err(|e| {
            aurora_db::conflict_code(e, &[("trusted_keys_public_key_key", "addon_key_exists")])
        })?;
    repo::confirm_key(
        &mut *tx,
        id,
        &m.signing_key,
        &fp,
        &m.digest,
        &stored_manifest(&m),
    )
    .await?;
    let ev = AuditEvent::new(actor.clone(), "addon_source.update", Outcome::Success)
        .target("addon_source", id)
        .metadata(json!({"change": "signing_key_confirmed", "old": old_fp, "new": fp}));
    audit::commit_critical(tx, &ev).await?;
    Ok(repo::get(&state.pool, id).await?.unwrap_or(r))
}

/// Provenance and update badges for a page of the plugins list (C-600): the addon source's name
/// at read time, and `update_available` — a manifest version of the package is newer than the row,
/// and the row is the newest version the instance holds (or the active one).
pub async fn decorate(state: &AppState, items: &mut [PluginPackage]) -> Result<(), ApiError> {
    if !items.iter().any(|i| i.source_id.is_some()) {
        return Ok(());
    }
    let sources = repo::list(&state.pool).await?;
    let names: BTreeMap<Uuid, String> = sources.iter().map(|s| (s.id, s.name.clone())).collect();
    // The newest version each source's manifest lists, per package id.
    let mut listed: BTreeMap<String, Semver> = BTreeMap::new();
    for s in &sources {
        for a in stored_addons(s) {
            for (v, ver) in &a.versions {
                if core_supports(ver.min_core.as_deref()).is_err() {
                    continue;
                }
                if let Some(sv) = semver_of(v) {
                    let e = listed.entry(a.id.clone()).or_insert(sv);
                    *e = (*e).max(sv);
                }
            }
        }
    }
    let pids: Vec<String> = items
        .iter()
        .filter(|i| i.source_id.is_some())
        .map(|i| i.package_id.clone())
        .collect();
    let installed = installed_state(state, &pids).await?;
    for i in items.iter_mut() {
        let Some(sid) = i.source_id else { continue };
        i.source_name = names.get(&sid).cloned();
        let here = semver_of(&i.version);
        let newest_held = installed.get(&i.package_id).and_then(Installed::newest);
        i.update_available = match (here, listed.get(&i.package_id), newest_held) {
            // A newer version is listed, and this row is the one in use (active) or the newest
            // the instance holds: the badge points at the version waiting for the admin.
            (Some(h), Some(l), Some((_, n))) => {
                *l > h && (i.status == "active" || (h >= n && i.status == "pending_approval"))
            }
            _ => false,
        };
    }
    Ok(())
}

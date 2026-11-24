//! Package ingestion (09 §2, §6, §10): read, verify, validate, store, decide status.

use super::registry;
use super::{keys, service, upgrade};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::{ApiError, ids};
use aurora_db::DbError;
use aurora_db::repo::plugins::{self, NewPackage, PackageRow};
use aurora_policy::HostPattern;
use aurora_registry::claims::{self, Claim};
use aurora_registry::diff::widens;
use aurora_registry::manifest::valid_id;
use aurora_registry::signature::{self, Signer};
use aurora_registry::validate::{self, ValidationContext};
use aurora_registry::{Limits, Manifest, Package, Violation};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Directory,
    Upload,
    Addon,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Directory => "directory",
            Self::Upload => "upload",
            Self::Addon => "addon",
        }
    }
}

#[derive(Debug)]
pub struct Ingested {
    pub row: PackageRow,
    /// False when the same `(package_id, version, digest)` already existed.
    pub created: bool,
}

use ::hex::encode as hex;

/// Each violation with its sentence (C-1071) next to the raw `code`, `path` and `message`.
fn violations_json(v: &[Violation]) -> (Value, Vec<String>) {
    let sentences: Vec<String> = v
        .iter()
        .map(|v| super::sentences::violation_sentence(v.code, &v.path, &v.message))
        .collect();
    let items = v
        .iter()
        .zip(&sentences)
        .map(|(v, s)| json!({"code": v.code, "path": v.path, "message": v.message, "sentence": s}))
        .collect();
    (Value::Array(items), sentences)
}

/// 422 `package_invalid` (413 for oversize) without creating a row. (C-1071) `message` is a
/// sentence; every violation carries its own.
fn reject(v: &[Violation]) -> ApiError {
    if v.iter().any(|v| v.code == "package_too_large") {
        return ApiError::too_large(super::sentences::too_large_sentence());
    }
    let (items, sentences) = violations_json(v);
    ApiError::unprocessable("package_invalid", super::sentences::summary(&sentences))
        .with_details(json!({"violations": items}))
}

/// `reject` for sibling domains (addon sources surface the same violations).
pub fn reject_public(v: &Violation) -> ApiError {
    reject(std::slice::from_ref(v))
}

/// Reserved hosts (09 §6): LLM endpoints, the external and service hosts,
/// `*.aurora.internal`.
pub fn reserved_names(state: &AppState) -> Vec<String> {
    let cfg = &state.config;
    let mut names = state.llm().endpoint_hosts();
    for url in [
        &cfg.external_url,
        &cfg.hookhost_url,
        &cfg.advertised_proxy_url,
    ] {
        let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r);
        let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
        let auth = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
        let host = auth.rsplit_once(':').map_or(auth, |(h, _)| h);
        if !host.is_empty() {
            names.push(host.to_owned());
        }
    }
    for n in [
        "aurora-api",
        "aurora-gateway",
        "aurora-runner",
        "aurora-hookhost",
    ] {
        names.push(n.to_owned());
    }
    names.push("*.aurora.internal".to_owned());
    names
}

fn reserved_hosts(state: &AppState) -> Vec<HostPattern> {
    reserved_names(state)
        .iter()
        .filter_map(|n| HostPattern::parse(n).ok())
        .collect()
}

async fn context(
    state: &AppState,
    active: &[(String, Value)],
) -> Result<ValidationContext, ApiError> {
    let mut active_app_ids = HashMap::new();
    let mut active_families = HashMap::new();
    for (pkg, m) in active {
        if let Ok(m) = serde_json::from_value::<Manifest>(m.clone()) {
            for a in &m.apps {
                active_app_ids.insert(a.about.id.clone(), pkg.clone());
                if let Some(f) = &a.about.family {
                    active_families.insert(f.id.clone(), pkg.clone());
                }
            }
        }
    }
    Ok(ValidationContext {
        running_version: env!("CARGO_PKG_VERSION").to_owned(),
        reserved_hosts: reserved_hosts(state),
        active_app_ids,
        active_families,
    })
}

/// `(package_id, version, publisher)` from a manifest that failed strict parsing,
/// so the admin still sees an `invalid` row. `None` when not even those exist.
fn loose_identity(pkg: &Package) -> Option<(String, String, String, Value)> {
    let yaml = pkg.manifest_yaml().filter(|y| y.len() <= 256 * 1024)?;
    let v: Value = serde_yaml::from_str::<Value>(yaml).ok()?;
    let p = v.get("package")?;
    let id = p.get("id")?.as_str().filter(|s| valid_id(s))?;
    let version = p.get("version")?.as_str().filter(|s| s.len() <= 64)?;
    let publisher = p
        .get("publisher")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 64)
        .unwrap_or("");
    Some((id.to_owned(), version.to_owned(), publisher.to_owned(), v))
}

struct Decision {
    status: &'static str,
    reason: Option<&'static str>,
}

async fn decide(
    state: &AppState,
    m: &Manifest,
    active: &[(String, Value)],
    source: Source,
    signer: &Signer,
) -> Result<Decision, ApiError> {
    let pending = |reason| Decision {
        status: "pending_approval",
        reason: Some(reason),
    };
    let activate = Decision {
        status: "active",
        reason: None,
    };
    // Claims conflicting with another active package need an admin decision.
    let mut all: Vec<(String, Claim)> = claims::claims(m)
        .into_iter()
        .map(|c| (m.package.id.clone(), c))
        .collect();
    // (FIX-123, D-215) A package this one supersedes never conflicts with it: its apps move here.
    let retired = aurora_registry::upgrade::retired_by(&m.package.id);
    let takes_over = !aurora_registry::upgrade::supersedes(m).is_empty();
    for (pkg, manifest) in active
        .iter()
        .filter(|(p, _)| *p != m.package.id)
        .filter(|(p, _)| !(takes_over && retired.contains(&p.as_str())))
    {
        if let Ok(o) = serde_json::from_value::<Manifest>(manifest.clone()) {
            all.extend(claims::claims(&o).into_iter().map(|c| (pkg.clone(), c)));
        }
    }
    let conflict = claims::conflicts(&all)
        .iter()
        .any(|c| c.package_a == m.package.id || c.package_b == m.package.id);
    if conflict {
        return Ok(pending("claim_conflict"));
    }
    let prev = plugins::previous_approved(&state.pool, &m.package.id).await?;
    // (P5-POLISH-3, D-381, C-1032, SEC-136) A package shipped with Aurora — from `plugins.builtin_dir`,
    // signed by the project key, or unsigned where `plugins.allow_unsigned` is on (dev values only;
    // `ingest_inner` rejects it otherwise) — arrives approved, at first install and at every later version,
    // widening permissions included. The holds below that protect data and identity still apply.
    let shipped = builtin_reason(source, signer).is_some();
    let Some(prev) = prev else {
        // (P5-ASRC, D-250(b)) addon packages are never auto-activated on first sight:
        // third-party permissions always need the ordinary instance-admin approval.
        return Ok(if shipped {
            activate
        } else {
            pending("first_install")
        });
    };
    let Ok(old) = serde_json::from_value::<Manifest>(prev.manifest) else {
        return Ok(pending("permissions_widened"));
    };
    // (FIX-107, SEC-15, 09 §10) A version is a first install when another publisher ships it,
    // when it is older than the highest approved version, or when the id is a built-in's and the
    // version does not come from the built-in source.
    if prev.publisher != m.package.publisher {
        return Ok(pending("publisher_changed"));
    }
    if source != Source::Builtin && plugins::has_builtin_source(&state.pool, &m.package.id).await? {
        return Ok(pending("builtin_id_reserved"));
    }
    let highest = plugins::approved_versions(&state.pool, &m.package.id)
        .await?
        .into_iter()
        .filter_map(|v| semver_key(&v))
        .max();
    match (semver_key(&m.package.version), highest) {
        (Some(new), Some(top)) if new < top => return Ok(pending("version_downgrade")),
        (None, _) => return Ok(pending("version_downgrade")),
        _ => {}
    }
    Ok(if shipped || widens(&old, m).is_empty() {
        activate
    } else {
        pending("permissions_widened")
    })
}

/// (SEC-136) Why a package from the builtin source is approved without review: `builtin_release` (signed by
/// the project key) or `builtin_dev_unsigned` (unsigned, which only `plugins.allow_unsigned` lets through).
/// `None` for every other source, and for a builtin whose signer is anyone else.
fn builtin_reason(source: Source, signer: &Signer) -> Option<&'static str> {
    if source != Source::Builtin {
        return None;
    }
    match signer {
        Signer::Project => Some("builtin_release"),
        Signer::Unsigned => Some("builtin_dev_unsigned"),
        Signer::Trusted { .. } => None,
    }
}

/// Ordering key of a semantic version: real semver precedence (`SemVer`) — a release sorts
/// above its pre-releases, `1.0.0-rc.1 < 1.0.0-rc.2 < 1.0.0`. `None` when it does not parse.
fn semver_key(v: &str) -> Option<aurora_registry::manifest::SemVer> {
    aurora_registry::manifest::parse_semver(v)
}

/// Ingest `bytes`. Archive/signature failures return 422 (413 when oversize)
/// without a row; other validation failures store an `invalid` row.
pub async fn ingest(
    state: &AppState,
    bytes: &[u8],
    source: Source,
    actor: &AuditActor,
) -> Result<Ingested, ApiError> {
    ingest_inner(state, bytes, source, None, actor).await
}

/// What an addon source expects of a package it lists (P5-ASRC, SEC-86): the package must be
/// signed by **the source's own key**, carry the repo's publisher and the listed id and
/// version, and use a package id nobody else owns.
pub struct AddonExpect<'a> {
    pub source: &'a aurora_db::repo::addon_sources::AddonSourceRow,
    pub package_id: &'a str,
    pub version: &'a str,
}

/// (P5-ASRC, C-607) Ingest an addon package: same pipeline, the row records `source = "addon"`
/// and `source_id`; a first version is always `pending_approval`; the audit action is the
/// security-critical `addon.ingest`, committed with the package row.
pub async fn ingest_for_source(
    state: &AppState,
    bytes: &[u8],
    expect: &AddonExpect<'_>,
    actor: &AuditActor,
) -> Result<PackageRow, ApiError> {
    let out = ingest_inner(state, bytes, Source::Addon, Some(expect), actor).await?;
    Ok(out.row)
}

async fn ingest_inner(
    state: &AppState,
    bytes: &[u8],
    source: Source,
    addon: Option<&AddonExpect<'_>>,
    actor: &AuditActor,
) -> Result<Ingested, ApiError> {
    let source_id = addon.map(|a| a.source.id);
    let pkg = Package::read(bytes, &Limits::default()).map_err(|v| reject(&v))?;
    let trust = keys::trust_store(&state.pool).await?;
    // The builtin source is fail-closed (09 §10): only the project key counts, and an unsigned package
    // only where `plugins.allow_unsigned` is on (dev values; SEC-136 audits it `builtin_dev_unsigned`).
    let allow_unsigned = state.config.plugins.allow_unsigned;
    let signer = signature::verify(&pkg, &trust, allow_unsigned).map_err(|v| reject(&[v]))?;
    if source == Source::Builtin && builtin_reason(source, &signer).is_none() {
        return Err(reject(&[Violation::new(
            "builtin_not_project_signed",
            "SIGNATURE",
            "built-in packages must be signed by the project key",
        )]));
    }
    if let Some(a) = addon {
        // The signature must verify against the key the source published (and the admin
        // confirmed): not the project key, not another publisher's trusted key, not unsigned.
        let ok =
            matches!(&signer, Signer::Trusted { key_id, .. } if *key_id == a.source.signing_key);
        if !ok {
            return Err(reject(&[Violation::new(
                "signer_not_source_key",
                "SIGNATURE",
                "the package is not signed by the addon source's key",
            )]));
        }
    }
    let signer_key_id = match &signer {
        Signer::Project => signature::BUILTIN.to_owned(),
        Signer::Trusted { key_id, .. } => key_id.clone(),
        Signer::Unsigned => "unsigned".to_owned(),
    };
    let digest = hex(aurora_registry::digest::digest(&pkg));

    let running = env!("CARGO_PKG_VERSION");
    let parsed = pkg.manifest(running);
    let (package_id, version, publisher, contract, manifest_json, permissions) = match &parsed {
        Ok(m) => (
            m.package.id.clone(),
            m.package.version.clone(),
            m.package.publisher.clone(),
            i32::try_from(m.package.contract).unwrap_or(i32::MAX),
            serde_json::to_value(m).map_err(|e| ApiError::internal(e.to_string()))?,
            serde_json::to_value(&m.permissions).unwrap_or_else(|_| json!({})),
        ),
        Err(vs) => {
            let (id, v, p, raw) = loose_identity(&pkg).ok_or_else(|| reject(vs))?;
            (id, v, p, 0, raw, json!({}))
        }
    };

    if let Some(a) = addon {
        let bad = |code: &'static str, msg: &'static str| {
            reject(&[Violation::new(code, "IDENTITY", msg)])
        };
        if publisher != a.source.publisher {
            return Err(bad(
                "publisher_mismatch",
                "the package's publisher is not the addon source's publisher",
            ));
        }
        if package_id != a.package_id || version != a.version {
            return Err(bad(
                "identity_mismatch",
                "the package is not the id and version the manifest lists",
            ));
        }
        if aurora_db::repo::addon_sources::package_id_owned_elsewhere(
            &state.pool,
            &package_id,
            a.source.id,
            &publisher,
        )
        .await?
        {
            return Err(bad(
                "package_id_taken",
                "the package id belongs to another source or publisher",
            ));
        }
    }

    if let Some(row) = plugins::find_identity(&state.pool, &package_id, &version, &digest).await? {
        return Ok(Ingested {
            row,
            created: false,
        });
    }

    let active = plugins::active_manifests(&state.pool).await?;
    let ctx = context(state, &active).await?;
    let violations = match &parsed {
        Ok(_) => validate::package(&pkg, &ctx, &trust, allow_unsigned),
        Err(v) => v.clone(),
    };
    let mut decision = match (&parsed, violations.is_empty()) {
        (Ok(m), true) => decide(state, m, &active, source, &signer).await?,
        _ => Decision {
            status: "invalid",
            reason: Some("validation_failed"),
        },
    };
    // (P5-ASRC, D-250(b)) Third-party code never activates on its own: not on first sight and
    // not as an update, whatever the permissions diff says. The admin approves the version (the
    // click is the ordinary activation: connection impact, grant conversion and claim holds
    // are computed then); the more specific holds above keep their reasons.
    if source == Source::Addon && decision.status == "active" {
        decision = Decision {
            status: "pending_approval",
            reason: Some("addon_update"),
        };
    }
    let violations_v = violations_json(&violations).0;
    let new_id = ids::new_id();

    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    // (FIX-064, SEC-67) The same exclusive compile lock every other activation path holds, taken
    // before the impact is computed: a connection-grant writer (shared lock) either committed its
    // `full` grant first, so it counts in `impact.converts` and holds this version for approval,
    // or waits until this transaction — package row *and* snapshot — has committed and then
    // sees the version's `custom_only` restriction.
    plugins::lock_compile(&mut tx).await?;
    // C-343 (RFE-022): a major upgrade that would flip a connection to `reauth_required` is
    // never activated automatically, widening or not; it waits for an instance admin.
    if let (Ok(m), true) = (&parsed, decision.status != "invalid") {
        let impact = upgrade::compute_for(&mut tx, None, &package_id, &version, m).await?;
        if !impact.flips.is_empty() && decision.reason != Some("claim_conflict") {
            decision = Decision {
                status: "pending_approval",
                reason: Some("breaks_connections"),
            };
        } else if !impact.converts.is_empty() && decision.reason != Some("claim_conflict") {
            // (P4-GRANTS, D-123) An upgrade that converts full grants to custom grants is never
            // activated automatically either: the admin sees "N agent grants will be converted".
            decision = Decision {
                status: "pending_approval",
                reason: Some("converts_grants"),
            };
        }
    }
    let auto = decision.status == "active";
    // (SEC-136) The reason a shipped package was approved without review, and the version it replaces.
    let auto_approved = if auto {
        builtin_reason(source, &signer)
    } else {
        None
    };
    let previous_version = if auto_approved.is_some() {
        plugins::previous_approved(&state.pool, &package_id)
            .await?
            .map(|p| p.version)
    } else {
        None
    };
    // (C-1137) Apps whose summary, essentials, operations file or skills differ from the version this one replaces.
    let mut agent_changed: Vec<String> = Vec::new();
    if auto {
        if let (Ok(m), Some(prev)) = (
            &parsed,
            plugins::active_of_package(&mut *tx, &package_id).await?,
        ) {
            let old = service::agent_view(&mut tx, &prev).await?;
            let new = service::AgentView {
                manifest: m.clone(),
                files: aurora_registry::snapshot::agent_files(m, &pkg),
            };
            agent_changed = service::agent_changed(Some(&old), &new);
        }
        plugins::supersede_active(&mut tx, &package_id).await?;
    }
    let inserted = plugins::insert_package(
        &mut tx,
        &NewPackage {
            id: new_id,
            package_id: &package_id,
            version: &version,
            contract,
            publisher: &publisher,
            digest: &digest,
            signer_key_id: &signer_key_id,
            manifest: &manifest_json,
            permissions: &permissions,
            violations: &violations_v,
            source: source.as_str(),
            source_id,
            status: decision.status,
            status_reason: decision.reason,
            approved_now: auto,
        },
    )
    .await?;
    if inserted.is_none() {
        // Lost a race with a concurrent identical ingest.
        drop(tx);
        let row = plugins::find_identity(&state.pool, &package_id, &version, &digest)
            .await?
            .ok_or_else(|| ApiError::internal("Package vanished during ingest."))?;
        return Ok(Ingested {
            row,
            created: false,
        });
    }
    if decision.status == "pending_approval" {
        // (FIX-167 Part B, D-370) A version that waits for a decision: tell the instance admins,
        // in this transaction (a concurrent identical ingest inserted nothing above and returned).
        let pkg_name = parsed
            .as_ref()
            .map_or(package_id.as_str(), |m| m.package.name.as_str());
        crate::domains::admin::attention::notify_package(
            &mut tx,
            new_id,
            pkg_name,
            &version,
            decision.reason,
        )
        .await?;
    }
    if decision.status != "invalid" {
        if let Some(wasm) = pkg.file(aurora_registry::package::HOOKS_WASM) {
            plugins::put_blob(&mut *tx, &hex(Sha256::digest(wasm)), "hooks_wasm", wasm).await?;
        }
        plugins::put_blob(&mut *tx, &digest, "archive", bytes).await?;
        // (P5-APPSKILL-1, C-1132) One `agent_file` blob per skill file, `SKILL.md` and operations
        // file, keyed by the digest of its bytes. Guide files of an old package are ignored
        // (D-453): nothing of them is stored for agents.
        let ops: std::collections::BTreeSet<String> = parsed
            .as_ref()
            .map(|m| {
                m.apps
                    .iter()
                    .filter_map(|a| {
                        a.agent
                            .as_ref()
                            .and_then(|g| g.operations.as_ref())
                            .map(|f| {
                                format!("{}{}/{f}", aurora_registry::package::AGENT_DIR, a.about.id)
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (path, content) in &pkg.files {
            if path.starts_with(aurora_registry::package::SKILLS_DIR) || ops.contains(path) {
                plugins::put_blob(
                    &mut *tx,
                    &hex(Sha256::digest(content)),
                    "agent_file",
                    content,
                )
                .await?;
            }
        }
    }
    let mut superseded: Option<AuditEvent> = None;
    if auto && let Ok(m) = &parsed {
        // (FIX-123, D-215) Apps this version took over as connect methods move with it, before the
        // snapshot is compiled (the retired package's host claims must be gone by then). The
        // security-critical audit row commits with the move.
        let report = super::supersede::apply(&mut tx, m, &version).await?;
        if !report.is_empty() {
            superseded = Some(
                AuditEvent::new(actor.clone(), "plugin.supersede", Outcome::Success)
                    .target("plugin_package", new_id)
                    .metadata(json!({"package_id": package_id, "version": version,
                        "result": report.metadata()})),
            );
        }
    }
    if auto {
        // Activate the snapshot in the same transaction as the package row: a window between
        // "package active" and "snapshot active" would let a `full` grant be written against the
        // old snapshot after the conversion check above (nothing would ever convert it).
        registry::activate_snapshot(state, &mut tx, true)
            .await
            .map_err(registry::flatten)?;
    }
    let ingest_event = |action: &'static str| {
        AuditEvent::new(actor.clone(), action, Outcome::Success)
            .target("plugin_package", new_id)
            .metadata(json!({
                "package_id": package_id, "version": version, "digest": digest,
                "source": source.as_str(), "source_id": source_id,
                "status": decision.status,
                "reason": decision.reason,
            }))
    };
    match &superseded {
        Some(ev) => audit::commit_critical(tx, ev).await?,
        // An addon package never activates on first sight, so there is no supersede row to
        // share the transaction with: `addon.ingest` is security-critical and commits with it.
        None if source == Source::Addon => {
            audit::commit_critical(tx, &ingest_event("addon.ingest")).await?;
        }
        None => tx.commit().await.map_err(DbError::from)?,
    }
    if source != Source::Addon {
        audit::record(state, ingest_event("plugin.ingest"));
    }
    if auto {
        let mut meta = json!({"package_id": package_id, "version": version, "auto": true,
            "connections_reauth": 0});
        if !agent_changed.is_empty() {
            meta["agent_changed"] = json!(agent_changed);
        }
        audit::record(
            state,
            AuditEvent::new(actor.clone(), "plugin.activate", Outcome::Success)
                .target("plugin_package", new_id)
                .metadata(meta),
        );
        registry::request_compile(state).await?;
    }
    if let Some(reason) = auto_approved {
        audit::record(
            state,
            AuditEvent::new(actor.clone(), "plugin.auto_approved", Outcome::Success)
                .target("plugin_package", new_id)
                .metadata(json!({"package_id": package_id, "version": version,
                    "previous_version": previous_version, "reason": reason})),
        );
    }
    let row = service::get_row(state, new_id).await?;
    Ok(Ingested { row, created: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SEC-136: only the builtin source skips review, and only for the project key (or, dev only, unsigned).
    #[test]
    fn only_the_builtin_source_is_approved_without_review() {
        let trusted = Signer::Trusted {
            key_id: "k".to_owned(),
            publisher: "acme".to_owned(),
        };
        assert_eq!(
            builtin_reason(Source::Builtin, &Signer::Project),
            Some("builtin_release")
        );
        assert_eq!(
            builtin_reason(Source::Builtin, &Signer::Unsigned),
            Some("builtin_dev_unsigned")
        );
        assert_eq!(builtin_reason(Source::Builtin, &trusted), None);
        for source in [Source::Directory, Source::Upload, Source::Addon] {
            for signer in [&Signer::Project, &Signer::Unsigned, &trusted] {
                assert_eq!(builtin_reason(source, signer), None, "{source:?}");
            }
        }
    }
}

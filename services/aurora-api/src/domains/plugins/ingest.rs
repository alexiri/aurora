//! Package ingestion (09 §2, §6, §10): read, verify, validate, store, decide status.

use super::registry;
use super::{keys, service};
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
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Directory => "directory",
            Self::Upload => "upload",
        }
    }
}

#[derive(Debug)]
pub struct Ingested {
    pub row: PackageRow,
    /// False when the same `(package_id, version, digest)` already existed.
    pub created: bool,
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn violations_json(v: &[Violation]) -> Value {
    Value::Array(
        v.iter()
            .map(|v| json!({"code": v.code, "path": v.path, "message": v.message}))
            .collect(),
    )
}

/// 422 `package_invalid` (413 for oversize) without creating a row.
fn reject(v: &[Violation]) -> ApiError {
    if v.iter().any(|v| v.code == "package_too_large") {
        return ApiError::too_large("Package is too large.");
    }
    ApiError::unprocessable("package_invalid", "The package is invalid.")
        .with_details(json!({"violations": violations_json(v)}))
}

/// Reserved hosts (09 §6): LLM endpoints, the external and service hosts,
/// `*.aurora.internal`.
fn reserved_hosts(state: &AppState) -> Vec<HostPattern> {
    let cfg = &state.config;
    let mut names = state.llm.endpoint_hosts();
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
        .iter()
        .filter_map(|n| HostPattern::parse(n).ok())
        .collect()
}

async fn context(
    state: &AppState,
    active: &[(String, Value)],
) -> Result<ValidationContext, ApiError> {
    let mut active_app_ids = HashMap::new();
    for (pkg, m) in active {
        if let Ok(m) = serde_json::from_value::<Manifest>(m.clone()) {
            for a in &m.apps {
                active_app_ids.insert(a.about.id.clone(), pkg.clone());
            }
        }
    }
    Ok(ValidationContext {
        running_version: env!("CARGO_PKG_VERSION").to_owned(),
        reserved_hosts: reserved_hosts(state),
        active_app_ids,
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
    for (pkg, manifest) in active.iter().filter(|(p, _)| *p != m.package.id) {
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
    let Some(prev) = prev else {
        return Ok(if source == Source::Builtin && *signer == Signer::Project {
            activate
        } else {
            pending("first_install")
        });
    };
    let Ok(old) = serde_json::from_value::<Manifest>(prev.manifest) else {
        return Ok(pending("permissions_widened"));
    };
    Ok(if widens(&old, m).is_empty() {
        activate
    } else {
        pending("permissions_widened")
    })
}

/// Ingest `bytes`. Archive/signature failures return 422 (413 when oversize)
/// without a row; other validation failures store an `invalid` row.
pub async fn ingest(
    state: &AppState,
    bytes: &[u8],
    source: Source,
    actor: &AuditActor,
) -> Result<Ingested, ApiError> {
    let pkg = Package::read(bytes, &Limits::default()).map_err(|v| reject(&v))?;
    let trust = keys::trust_store(&state.pool).await?;
    let allow_unsigned = state.config.plugins.allow_unsigned;
    let signer = signature::verify(&pkg, &trust, allow_unsigned).map_err(|v| reject(&[v]))?;
    let signer_key_id = match &signer {
        Signer::Project => signature::BUILTIN.to_owned(),
        Signer::Trusted { key_id, .. } => key_id.clone(),
        Signer::Unsigned => "unsigned".to_owned(),
    };
    let digest = hex(&aurora_registry::digest::digest(&pkg));

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
    let decision = match (&parsed, violations.is_empty()) {
        (Ok(m), true) => decide(state, m, &active, source, &signer).await?,
        _ => Decision {
            status: "invalid",
            reason: Some("validation_failed"),
        },
    };
    let auto = decision.status == "active";
    let violations_v = violations_json(&violations);
    let new_id = ids::new_id();

    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if auto {
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
    if decision.status != "invalid" {
        if let Some(wasm) = pkg.file(aurora_registry::package::HOOKS_WASM) {
            plugins::put_blob(&mut *tx, &hex(&Sha256::digest(wasm)), "hooks_wasm", wasm).await?;
        }
        plugins::put_blob(&mut *tx, &digest, "archive", bytes).await?;
    }
    tx.commit().await.map_err(DbError::from)?;

    audit::record(
        state,
        AuditEvent::new(actor.clone(), "plugin.ingest", Outcome::Success)
            .target("plugin_package", new_id)
            .metadata(json!({
                "package_id": package_id, "version": version, "digest": digest,
                "source": source.as_str(), "status": decision.status,
                "reason": decision.reason,
            })),
    );
    if auto {
        audit::record(
            state,
            AuditEvent::new(actor.clone(), "plugin.activate", Outcome::Success)
                .target("plugin_package", new_id)
                .metadata(json!({"package_id": package_id, "version": version, "auto": true})),
        );
        registry::request_compile(state).await?;
    }
    let row = service::get_row(state, new_id).await?;
    Ok(Ingested { row, created: true })
}

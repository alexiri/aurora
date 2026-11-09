//! Plugin major upgrades and live connections (09 §10, RFE-022, D-65, C-340..C-343, SEC-57).
//!
//! `compute` is the single implementation behind the advisory `impact` of
//! `GET /v1/admin/plugins/{id}`, the activation transaction (which re-runs it) and the automatic
//! activation hold. Only credential field **names** are read, never a value, and the result
//! carries counts (never an organization, workspace, connection label).

use super::dto::{PluginImpact, PluginImpactApp};
use aurora_core::ApiError;
use aurora_db::repo::{credentials, plugins};
use aurora_registry::Manifest;
use aurora_registry::upgrade as rules;
use serde_json::json;
use sqlx::PgConnection;
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

/// One `active` connection that fails the required-field check and would be flipped.
pub struct Flip {
    pub id: Uuid,
    pub app_id: String,
    pub reason: String,
}

pub struct Computed {
    pub major: bool,
    pub from_version: Option<String>,
    pub total: i64,
    /// Failing connections, `active` and `reauth_required` (the advisory count of C-340).
    pub needing: i64,
    pub by_app: Vec<PluginImpactApp>,
    /// The failing `active` ones: what activation flips.
    pub flips: Vec<Flip>,
}

impl Computed {
    pub fn into_dto(self) -> PluginImpact {
        PluginImpact {
            major_upgrade: self.major,
            from_version: self.from_version,
            connections_total: self.total,
            connections_needing_reauth: self.needing,
            by_app: self.by_app,
        }
    }
}

/// The result of flipping connections inside an activation transaction.
#[derive(Default)]
pub struct Applied {
    pub total: i64,
    pub by_app: BTreeMap<String, i64>,
}

impl Applied {
    pub fn metadata(&self) -> serde_json::Value {
        json!({ "connections_reauth": self.total, "by_app": self.by_app })
    }
}

/// Impact of activating `row` over the currently active version of its package.
pub async fn compute(
    conn: &mut PgConnection,
    row: &plugins::PackageRow,
) -> Result<Computed, ApiError> {
    let manifest: Manifest = serde_json::from_value(row.manifest.clone())
        .map_err(|e| ApiError::internal(format!("stored manifest unreadable: {e}")))?;
    compute_for(conn, Some(row.id), &row.package_id, &row.version, &manifest).await
}

/// [`compute`] for a version that may not be stored yet (`id = None`, automatic activation).
pub async fn compute_for(
    conn: &mut PgConnection,
    id: Option<Uuid>,
    package_id: &str,
    version: &str,
    manifest: &Manifest,
) -> Result<Computed, ApiError> {
    let mut app_ids: Vec<String> = manifest.apps.iter().map(|a| a.about.id.clone()).collect();
    app_ids.sort_unstable();
    app_ids.dedup();
    let from = plugins::active_of_package(&mut *conn, package_id)
        .await?
        .filter(|a| Some(a.id) != id)
        .map(|a| a.version);
    let major = from
        .as_deref()
        .is_some_and(|f| rules::is_major_upgrade(f, version));
    let conns = plugins::upgrade_connections(&mut *conn, package_id, &app_ids).await?;

    // Manifests of the versions legacy connections recorded (fallback for unknown keys).
    let mut recorded: HashMap<String, Option<Manifest>> = HashMap::new();
    let mut by_app: BTreeMap<String, (i64, i64)> =
        app_ids.iter().map(|a| (a.clone(), (0, 0))).collect();
    let mut flips = Vec::new();
    let (mut total, mut needing) = (0_i64, 0_i64);
    for c in &conns {
        total += 1;
        let entry = by_app.entry(c.app_id.clone()).or_default();
        entry.0 += 1;
        if !major {
            continue;
        }
        let Some(app) = manifest.apps.iter().find(|a| a.about.id == c.app_id) else {
            continue;
        };
        if c.credential_keys.is_none() && !recorded.contains_key(&c.app_version) {
            let m = plugins::manifest_of_version(&mut *conn, package_id, &c.app_version)
                .await?
                .and_then(|v| serde_json::from_value::<Manifest>(v).ok());
            recorded.insert(c.app_version.clone(), m);
        }
        let missing = rules::missing_fields(
            app,
            c.credential_keys.as_deref(),
            recorded.get(&c.app_version).and_then(Option::as_ref),
        );
        if missing.is_empty() {
            continue;
        }
        needing += 1;
        entry.1 += 1;
        if c.status == "active" {
            flips.push(Flip {
                id: c.id,
                app_id: c.app_id.clone(),
                reason: rules::reason(
                    package_id,
                    from.as_deref().unwrap_or_default(),
                    version,
                    &missing,
                ),
            });
        }
    }
    Ok(Computed {
        major,
        from_version: from,
        total,
        needing,
        by_app: by_app
            .into_iter()
            .map(|(app_id, (connections, needing_reauth))| PluginImpactApp {
                app_id,
                connections,
                needing_reauth,
            })
            .collect(),
        flips,
    })
}

/// Flip every failing `active` connection (C-342) inside the caller's transaction and tell the
/// gateways (`connection` scope; the NOTIFY is delivered on commit only).
pub async fn apply(conn: &mut PgConnection, c: &Computed) -> Result<Applied, ApiError> {
    let mut out = Applied::default();
    for f in &c.flips {
        if plugins::flip_reauth_required(&mut *conn, f.id, &f.reason).await? {
            credentials::notify_gateway_invalidate(&mut *conn, "connection", f.id).await?;
            out.total += 1;
            *out.by_app.entry(f.app_id.clone()).or_default() += 1;
        }
    }
    Ok(out)
}

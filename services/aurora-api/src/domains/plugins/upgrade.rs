//! Plugin major upgrades and live connections (09 §10, RFE-022, D-65, C-340..C-343, SEC-57).
//!
//! `compute` is the single implementation behind the advisory `impact` of
//! `GET /v1/admin/plugins/{id}`, the activation transaction (which re-runs it) and the automatic
//! activation hold. Only credential field **names** are read, never a value, and the result
//! carries counts (never an organization, workspace, connection label).

use super::dto::{PluginImpact, PluginImpactApp};
use crate::domains::grants::compile::compile_connection_grant;
use crate::domains::grants::dto::GrantMode;
use aurora_core::ApiError;
use aurora_db::repo::grants::{self, FullGrantRow};
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

/// One `full` grant that activation converts to a custom grant (D-123, `09 §10`): its
/// `allow_tools` are exactly the new version's whole catalogue.
pub struct Convert {
    pub grant: FullGrantRow,
    pub allow_tools: Vec<String>,
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
    /// (P4-GRANTS, D-123) The full grants activation converts to custom grants: those on
    /// connections of an app the new version marks `grants.custom_only`.
    pub converts: Vec<Convert>,
}

impl Computed {
    pub fn into_dto(self) -> PluginImpact {
        PluginImpact {
            major_upgrade: self.major,
            from_version: self.from_version,
            connections_total: self.total,
            connections_needing_reauth: self.needing,
            grants_to_convert: i64::try_from(self.converts.len()).unwrap_or(i64::MAX),
            by_app: self.by_app,
        }
    }
}

/// Every tool id of an app's catalogue in the manifest (items and group wildcards): what a
/// converted grant allows, so that what an agent can do with catalogued tools stays the same.
fn catalogue_ids(app: &aurora_registry::manifest::App) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(t) = &app.tools {
        for g in [&t.read, &t.write].into_iter().flatten() {
            out.extend(g.wildcard.iter().map(|w| w.id.clone()));
            out.extend(g.items.iter().map(|i| i.id.clone()));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A grant rewritten inside the activation transaction (for the audit row, one per grant).
pub struct Converted {
    pub grant_id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub connection_id: Uuid,
    pub app_id: String,
    pub tools: usize,
}

/// The result of flipping connections and converting grants inside an activation transaction.
#[derive(Default)]
pub struct Applied {
    pub total: i64,
    pub by_app: BTreeMap<String, i64>,
    pub converted: Vec<Converted>,
}

impl Applied {
    pub fn metadata(&self) -> serde_json::Value {
        let mut by_app_grants: BTreeMap<&str, i64> = BTreeMap::new();
        for c in &self.converted {
            *by_app_grants.entry(c.app_id.as_str()).or_default() += 1;
        }
        json!({ "connections_reauth": self.total, "by_app": self.by_app,
                "grants_converted": self.converted.len(), "grants_converted_by_app": by_app_grants })
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
        // (FIX-107, SEC-57) Every version is checked, not only majors: a minor upgrade that
        // renames or repurposes a required credential field breaks the connection the same way.
        // (A first activation has nothing to upgrade from.)
        if from.is_none() {
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
    // (P4-GRANTS, D-123) Full grants on connections of an app this version marks `custom_only`
    // become custom grants with exactly the catalogue; nothing is ever converted back.
    let custom_only: Vec<&aurora_registry::manifest::App> = manifest
        .apps
        .iter()
        .filter(|a| a.grants.custom_only)
        .collect();
    let mut converts = Vec::new();
    if !custom_only.is_empty() {
        let ids: Vec<String> = custom_only.iter().map(|a| a.about.id.clone()).collect();
        for g in grants::full_for_apps(&mut *conn, &ids).await? {
            if let Some(app) = custom_only.iter().find(|a| a.about.id == g.app_id) {
                let allow_tools = catalogue_ids(app);
                converts.push(Convert {
                    grant: g,
                    allow_tools,
                });
            }
        }
    }
    let mut per_app: BTreeMap<&str, (i64, std::collections::BTreeSet<Uuid>)> = BTreeMap::new();
    for c in &converts {
        let e = per_app.entry(c.grant.app_id.as_str()).or_default();
        e.0 += 1;
        e.1.insert(c.grant.workspace_id);
    }
    Ok(Computed {
        major,
        from_version: from,
        total,
        needing,
        by_app: by_app
            .into_iter()
            .map(|(app_id, (connections, needing_reauth))| {
                let (grants_to_convert, ws) =
                    per_app.get(app_id.as_str()).map_or((0, 0), |(n, w)| {
                        (*n, i64::try_from(w.len()).unwrap_or(i64::MAX))
                    });
                PluginImpactApp {
                    app_id,
                    connections,
                    needing_reauth,
                    grants_to_convert,
                    grant_workspaces: ws,
                }
            })
            .collect(),
        flips,
        converts,
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
    // (P4-GRANTS, D-123) Convert in the same transaction: the grant row, its derived rules
    // (recompiled as a custom grant) and the gateway invalidation commit together with the
    // activation and the snapshot (SEC-67).
    for c in &c.converts {
        let g = &c.grant;
        if !grants::convert_full_to_custom(&mut *conn, g.id, &c.allow_tools).await? {
            continue;
        }
        compile_connection_grant(
            &mut *conn,
            g.org_id,
            g.workspace_id,
            g.id,
            g.agent_id,
            g.connection_id,
            &g.label,
            GrantMode::Custom,
            &c.allow_tools,
            &[],
            None,
        )
        .await?;
        credentials::notify_gateway_invalidate(&mut *conn, "agent", g.agent_id).await?;
        out.converted.push(Converted {
            grant_id: g.id,
            org_id: g.org_id,
            workspace_id: g.workspace_id,
            agent_id: g.agent_id,
            connection_id: g.connection_id,
            app_id: g.app_id.clone(),
            tools: c.allow_tools.len(),
        });
    }
    Ok(out)
}

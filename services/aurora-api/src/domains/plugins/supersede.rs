//! Superseded apps (FIX-123, D-195(a), D-215, 09 §10 *Superseded apps*).
//!
//! `github-app` became the `github_app` connect method of `github` (and `github-app-ghes` of
//! `github-ghes`), `aws-role` the `assume_role` method of `aws-keys`. When a version that has the
//! target app with that method is activated, [`apply`] runs **inside the activation transaction**
//! (before the snapshot is compiled, so the retired package's host claims are gone by then):
//! the old app's connections, instances, skill libraries and availability move to the new app,
//! the host-claim decisions between the old and the new package are deleted, and the retired
//! package is disabled. Idempotent; only counts and ids are logged, never a credential.

use aurora_core::ApiError;
use aurora_db::repo::{credentials, plugins};
use aurora_registry::Manifest;
use aurora_registry::claims;
use aurora_registry::upgrade::{self as rules, Superseded};
use serde_json::json;
use sqlx::PgConnection;
use std::collections::BTreeMap;

/// What one [`apply`] changed (counts per old app, for the audit row and the upgrade log).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Connections moved, by old app id.
    pub connections: BTreeMap<String, i64>,
    /// Moved connections that fail the new version's required-field check and were flipped to
    /// `reauth_required` (normally none: the stored credentials are the same names).
    pub flipped: i64,
    /// Instance registrations moved or merged.
    pub instances: u64,
    pub libraries: u64,
    pub availability: u64,
    pub decisions_deleted: u64,
    /// Retired package versions disabled, by package id.
    pub disabled: BTreeMap<String, u64>,
}

impl Report {
    /// Did anything change at all?
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
            && self.instances == 0
            && self.libraries == 0
            && self.availability == 0
            && self.decisions_deleted == 0
            && self.disabled.is_empty()
    }

    pub fn metadata(&self) -> serde_json::Value {
        json!({
            "connections_moved": self.connections.values().sum::<i64>(),
            "by_app": self.connections, "connections_reauth": self.flipped,
            "instances": self.instances, "skill_libraries": self.libraries,
            "availability": self.availability, "claim_decisions_deleted": self.decisions_deleted,
            "retired_packages": self.disabled,
        })
    }
}

/// Run every superseded-app entry `manifest` (version `version` of its package) takes over.
pub async fn apply(
    conn: &mut PgConnection,
    manifest: &Manifest,
    version: &str,
) -> Result<Report, ApiError> {
    let mut report = Report::default();
    let entries: Vec<&'static Superseded> = rules::supersedes(manifest);
    if entries.is_empty() {
        return Ok(report);
    }
    // Dry-run counts first: the upgrade log says what is about to move before it moves.
    for s in &entries {
        let n = plugins::count_connections_of_app(&mut *conn, s.old_app).await?;
        tracing::info!(
            old_app = s.old_app,
            new_app = s.new_app,
            method = s.method,
            connections = n,
            "plugin supersede: dry run, connections that will move"
        );
    }
    let mut moved: Vec<(&'static Superseded, plugins::MovedConnection)> = Vec::new();
    for s in &entries {
        for c in plugins::move_connections(
            &mut *conn,
            s.old_app,
            s.new_app,
            &manifest.package.id,
            version,
            s.method,
        )
        .await?
        {
            *report.connections.entry(s.old_app.to_owned()).or_default() += 1;
            moved.push((s, c));
        }
        report.instances +=
            plugins::move_instances(&mut *conn, s.old_app, s.new_app, &manifest.package.id).await?;
        report.libraries += plugins::move_skill_libraries(&mut *conn, s.old_app, s.new_app).await?;
        report.availability += plugins::move_availability(&mut *conn, s.old_app, s.new_app).await?;
    }
    // A moved connection must satisfy the new app's required credential fields (it normally does:
    // the merged app declares the old app's fields; a connection with unknown keys fails closed).
    for (s, c) in &moved {
        if c.status != "active" {
            continue;
        }
        let Some(app) = manifest.apps.iter().find(|a| a.about.id == s.new_app) else {
            continue;
        };
        let missing = rules::missing_fields(app, c.credential_keys.as_deref(), None);
        if !missing.is_empty() {
            let reason = rules::reason(s.old_package, "superseded", &manifest.package.id, &missing);
            if plugins::flip_reauth_required(&mut *conn, c.id, &reason).await? {
                report.flipped += 1;
            }
        }
    }
    for (_, c) in &moved {
        credentials::notify_gateway_invalidate(&mut *conn, "connection", c.id).await?;
    }
    // The retired packages: their host-claim decisions go, then the packages are disabled.
    let mut retired: Vec<&'static str> = entries.iter().map(|s| s.old_package).collect();
    retired.sort_unstable();
    retired.dedup();
    for old in retired {
        let mut keys: Vec<String> = Vec::new();
        for m in plugins::manifests_of_package(&mut *conn, old).await? {
            if let Ok(m) = serde_json::from_value::<Manifest>(m) {
                keys.extend(claims::claims(&m).into_iter().map(|c| c.claim_key));
            }
        }
        keys.sort_unstable();
        keys.dedup();
        report.decisions_deleted += plugins::delete_decisions_on(&mut *conn, &keys).await?;
        let n = plugins::disable_retired(
            &mut *conn,
            old,
            &rules::superseded_reason(&manifest.package.id),
        )
        .await?;
        if n > 0 {
            report.disabled.insert(old.to_owned(), n);
        }
    }
    tracing::info!(
        package = %manifest.package.id, version,
        connections = report.connections.values().sum::<i64>(), flipped = report.flipped,
        instances = report.instances, skill_libraries = report.libraries,
        availability = report.availability, decisions_deleted = report.decisions_deleted,
        "plugin supersede: applied"
    );
    Ok(report)
}

//! Package lifecycle (09 §10): approve / reject / disable / enable / uninstall.

use super::registry::{self, CompileFail};
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::plugins::{self, PackageRow};
use aurora_registry::Manifest;
use aurora_registry::claims::ConflictGroup;
use serde_json::json;
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Approve,
    Reject,
    Disable,
    Enable,
}

impl Action {
    const fn name(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Disable => "disable",
            Self::Enable => "enable",
        }
    }
}

fn clean_reason(r: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(r) = r else { return Ok(None) };
    let r = r.trim().to_owned();
    if r.chars().count() > 64 || r.chars().any(char::is_control) {
        return Err(ApiError::invalid_request(
            "Invalid reason.",
            vec![FieldError::new(
                "/reason",
                "must be at most 64 characters, no control characters",
            )],
        ));
    }
    Ok((!r.is_empty()).then_some(r))
}

fn unresolved(groups: &[ConflictGroup]) -> ApiError {
    ApiError::conflict(
        "claim_conflict_unresolved",
        "Host claims conflict with another package; decide a winner first.",
    )
    .with_details(json!({"groups": groups.iter()
        .map(|g| json!({"group_key": g.claim_key, "fingerprint": g.fingerprint}))
        .collect::<Vec<_>>()}))
}

pub async fn act(
    state: &AppState,
    actor: &AuditActor,
    approver: Uuid,
    id: Uuid,
    action: Action,
    reason: Option<String>,
) -> Result<PackageRow, ApiError> {
    let reason = clean_reason(reason)?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    plugins::lock_compile(&mut tx).await?;
    let row = plugins::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Plugin package not found."))?;
    let bad_state = || {
        ApiError::conflict(
            "invalid_state",
            format!("Cannot {} a package that is {}.", action.name(), row.status),
        )
    };
    let mut activated = false;
    match action {
        Action::Reject => {
            if row.status != "pending_approval" {
                return Err(bad_state());
            }
            plugins::transition(&mut tx, id, "rejected", reason.as_deref(), None, false).await?;
        }
        Action::Disable => {
            if !matches!(row.status.as_str(), "active" | "pending_approval") {
                return Err(bad_state());
            }
            let was_active = row.status == "active";
            plugins::transition(
                &mut tx,
                id,
                "disabled",
                reason.as_deref().or(Some("disabled_by_admin")),
                None,
                false,
            )
            .await?;
            if was_active {
                registry::activate_snapshot(state, &mut tx, true)
                    .await
                    .map_err(|f| match f {
                        CompileFail::Api(e) => e,
                        CompileFail::Conflict(g) => unresolved(&g),
                    })?;
            }
        }
        Action::Approve | Action::Enable => {
            let from = if action == Action::Approve {
                "pending_approval"
            } else {
                "disabled"
            };
            if row.status != from {
                return Err(bad_state());
            }
            if action == Action::Enable && row.approved_at.is_none() {
                // A package disabled before its first approval must go through the
                // approval review (09 §10, 14 plugin approval flow), never `enable`.
                return Err(ApiError::conflict(
                    "invalid_state",
                    "This package was never approved; approve it instead.",
                ));
            }
            if action == Action::Enable {
                let revoked = plugins::all_trusted_keys(&mut *tx)
                    .await?
                    .iter()
                    .any(|k| k.public_key == row.signer_key_id && k.revoked_at.is_some());
                if revoked {
                    return Err(ApiError::conflict(
                        "invalid_state",
                        "The signing key of this package is revoked.",
                    ));
                }
            }
            plugins::supersede_active(&mut tx, &row.package_id).await?;
            plugins::transition(&mut tx, id, "active", None, Some(approver), true).await?;
            match registry::activate_snapshot(state, &mut tx, false).await {
                Ok(_) => activated = true,
                Err(CompileFail::Conflict(groups)) => {
                    drop(tx);
                    if action == Action::Approve
                        && let Ok(mut c) = state.pool.acquire().await
                    {
                        let _ = plugins::transition(
                            &mut c,
                            id,
                            "pending_approval",
                            Some("claim_conflict"),
                            None,
                            false,
                        )
                        .await;
                    }
                    return Err(unresolved(&groups));
                }
                Err(CompileFail::Api(e)) => return Err(e),
            }
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        state,
        AuditEvent::new(
            actor.clone(),
            format!("plugin.{}", action.name()),
            Outcome::Success,
        )
        .target("plugin_package", id)
        .metadata(json!({"package_id": row.package_id, "version": row.version,
                "reason": reason})),
    );
    if activated {
        audit::record(
            state,
            AuditEvent::new(actor.clone(), "plugin.activate", Outcome::Success)
                .target("plugin_package", id)
                .metadata(json!({"package_id": row.package_id, "version": row.version})),
        );
    }
    service::get_row(state, id).await
}

/// Delete a package. Apps that disappear with it and still have live
/// connections need `force`, which orphans those connections.
pub async fn delete(
    state: &AppState,
    actor: &AuditActor,
    id: Uuid,
    force: bool,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    plugins::lock_compile(&mut tx).await?;
    let row = plugins::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Plugin package not found."))?;
    let mut vanishing: Vec<String> = Vec::new();
    if matches!(row.status.as_str(), "active" | "disabled")
        && let Ok(m) = serde_json::from_value::<Manifest>(row.manifest.clone())
    {
        let others: HashSet<String> = plugins::list_by_status(&mut *tx, &["active"])
            .await?
            .into_iter()
            .filter(|r| r.id != id)
            .filter_map(|r| serde_json::from_value::<Manifest>(r.manifest).ok())
            .flat_map(|m| m.apps.into_iter().map(|a| a.about.id))
            .collect();
        vanishing = m
            .apps
            .into_iter()
            .map(|a| a.about.id)
            .filter(|a| !others.contains(a))
            .collect();
    }
    let live = plugins::live_connection_ids(&mut *tx, &row.package_id, &vanishing).await?;
    if !live.is_empty() {
        if !force {
            return Err(ApiError::conflict(
                "live_connections",
                "The package's apps still have live connections; use force to orphan them.",
            )
            .with_details(json!({"count": live.len()})));
        }
        plugins::orphan_connections(&mut tx, &live).await?;
        for c in &live {
            aurora_db::repo::credentials::notify_gateway_invalidate(&mut *tx, "connection", *c)
                .await?;
        }
    }
    plugins::delete_package(&mut tx, id).await?;
    if row.status == "active" {
        registry::activate_snapshot(state, &mut tx, true)
            .await
            .map_err(|f| match f {
                CompileFail::Api(e) => e,
                CompileFail::Conflict(g) => unresolved(&g),
            })?;
    }
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        state,
        AuditEvent::new(actor.clone(), "plugin.delete", Outcome::Success)
            .target("plugin_package", id)
            .metadata(json!({"package_id": row.package_id, "version": row.version,
                "force": force, "orphaned_connections": live.len()})),
    );
    Ok(())
}

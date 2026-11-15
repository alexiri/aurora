//! Per-section hooks of the settings framework (C-772 `extra`, C-774 `affected_orgs`): SET-1
//! gives every section the empty default; SET-2..SET-5 and FIX-124 add their arms here.

use super::dto::{AffectedOrg, DiffItem, SettingWarning};
use crate::app::AppState;
use crate::domains::governance::standing;
use crate::domains::secrets::gov::instance_from_governance;
use aurora_config::Effective;
use aurora_core::{ApiError, limits};
use serde_json::{Value, json};

/// Section-specific read-only data shown beside the settings (`extra`).
pub async fn extra(_state: &AppState, section: &str) -> Result<Option<Value>, ApiError> {
    Ok(match section {
        // (C-778) Where organizations stand against the instance ceiling.
        "governance" => Some(json!({ "orgs_url": "/v1/admin/settings/governance/orgs" })),
        _ => None,
    })
}

/// The organizations whose effective values change, their total, and section warnings.
/// `before` is the merged view in force, `after` the one the pending change would produce.
pub async fn affected(
    state: &AppState,
    section: &str,
    _diff: &[DiffItem],
    before: &Effective,
    after: &Effective,
) -> Result<(Vec<AffectedOrg>, i64, Vec<SettingWarning>), ApiError> {
    if section != "governance" {
        return Ok((Vec::new(), 0, Vec::new()));
    }
    // The endpoints are the registry's (SET-3 owns them); only the ceiling differs.
    let endpoints = state.gov_instance().endpoints.clone();
    let resolve = |e: &Effective| {
        e.raw
            .governance_config()
            .map(|g| instance_from_governance(&g, endpoints.clone()))
            .map_err(|_| ApiError::internal("Internal error."))
    };
    let (b, a) = (resolve(before)?, resolve(after)?);
    let (orgs, total) =
        standing::affected(&state.pool, &b, &a, limits::SETTINGS_PREVIEW_ORGS_MAX).await?;
    Ok((orgs, total, Vec::new()))
}

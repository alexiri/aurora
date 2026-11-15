//! Per-section hooks of the settings framework (C-772 `extra`, C-774 `affected_orgs`): SET-1
//! gives every section the empty default; SET-2..SET-5 and FIX-124 add their arms here.

use super::dto::{AffectedOrg, DiffItem, SettingWarning};
use crate::app::AppState;
use aurora_core::ApiError;
use serde_json::Value;

/// Section-specific read-only data shown beside the settings (`extra`).
pub async fn extra(_state: &AppState, _section: &str) -> Result<Option<Value>, ApiError> {
    Ok(None)
}

/// The organizations whose effective values change, their total, and section warnings.
pub async fn affected(
    _state: &AppState,
    _section: &str,
    _diff: &[DiffItem],
) -> Result<(Vec<AffectedOrg>, i64, Vec<SettingWarning>), ApiError> {
    Ok((Vec::new(), 0, Vec::new()))
}

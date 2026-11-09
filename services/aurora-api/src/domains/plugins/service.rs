use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::repo::plugins::{self, PackageRow};
use aurora_registry::Manifest;
use aurora_registry::diff::{Widening, widens};
use uuid::Uuid;

use super::dto::PluginPackage;

const STATUSES: [&str; 6] = [
    "pending_approval",
    "active",
    "superseded",
    "rejected",
    "disabled",
    "invalid",
];

pub fn check_status(s: &str) -> Result<(), ApiError> {
    if STATUSES.contains(&s) {
        Ok(())
    } else {
        Err(ApiError::invalid_request(
            "Invalid status filter.",
            vec![aurora_core::FieldError::new("/status", "unknown status")],
        ))
    }
}

pub async fn get_row(state: &AppState, id: Uuid) -> Result<PackageRow, ApiError> {
    plugins::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Plugin package not found."))
}

/// Widenings versus the previously approved version (pending packages only).
async fn widenings(state: &AppState, r: &PackageRow) -> Result<Vec<Widening>, ApiError> {
    if r.status != "pending_approval" {
        return Ok(Vec::new());
    }
    let Some(prev) = plugins::previous_approved(&state.pool, &r.package_id).await? else {
        return Ok(Vec::new());
    };
    let (Ok(old), Ok(new)) = (
        serde_json::from_value::<Manifest>(prev.manifest),
        serde_json::from_value::<Manifest>(r.manifest.clone()),
    ) else {
        return Ok(Vec::new());
    };
    Ok(widens(&old, &new))
}

/// The detail view: like [`to_dto`], plus `impact` for a `pending_approval` version (C-340).
pub async fn to_detail_dto(state: &AppState, r: PackageRow) -> Result<PluginPackage, ApiError> {
    let pending = r.status == "pending_approval";
    let impact = if pending {
        let mut conn = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        Some(super::upgrade::compute(&mut conn, &r).await?.into_dto())
    } else {
        None
    };
    let mut dto = to_dto(state, r).await?;
    dto.impact = impact;
    Ok(dto)
}

pub async fn to_dto(state: &AppState, r: PackageRow) -> Result<PluginPackage, ApiError> {
    let w = widenings(state, &r).await?;
    Ok(PluginPackage::from_row(r, &w))
}

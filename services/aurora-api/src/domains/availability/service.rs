use super::dto::*;
use crate::app::AppState;
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::{availability as repo, credentials};
use uuid::Uuid;

const MAX_ROWS: usize = 5000;

pub async fn get(state: &AppState, org: Uuid) -> Result<AppAvailability, ApiError> {
    let posture = repo::posture(&state.pool, org)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    let allow = repo::list(&state.pool, org)
        .await?
        .into_iter()
        .map(|(app_id, workspace_id)| AllowEntry {
            app_id,
            workspace_id,
        })
        .collect();
    Ok(AppAvailability { posture, allow })
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request(
        "Invalid app availability.",
        vec![FieldError::new(path, msg)],
    )
}

pub async fn put(
    state: &AppState,
    org: Uuid,
    req: AppAvailability,
) -> Result<AppAvailability, ApiError> {
    if !matches!(req.posture.as_str(), "open" | "restricted") {
        return Err(bad("/posture", "must be open or restricted"));
    }
    if req.allow.len() > MAX_ROWS {
        return Err(bad("/allow", "too many entries"));
    }
    if req
        .allow
        .iter()
        .any(|a| a.app_id.is_empty() || a.app_id.len() > 128)
    {
        return Err(bad("/allow", "app_id must be 1-128 characters"));
    }
    let mut ws: Vec<Uuid> = req.allow.iter().filter_map(|a| a.workspace_id).collect();
    ws.sort_unstable();
    ws.dedup();
    let known = repo::workspaces_in_org(&state.pool, org, &ws).await?;
    if known.len() != ws.len() {
        return Err(bad("/allow", "unknown workspace"));
    }
    let rows: Vec<(String, Option<Uuid>)> = req
        .allow
        .into_iter()
        .map(|a| (a.app_id, a.workspace_id))
        .collect();
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::replace(&mut tx, org, &req.posture, &rows).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "org", org).await?;
    tx.commit().await.map_err(DbError::from)?;
    get(state, org).await
}

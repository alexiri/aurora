//! Instance administration and the public catalogue of connectors (C-31, C-32).

use super::dto::*;
use super::manifest;
use crate::app::AppState;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, limits, sanitize};
use aurora_db::repo::channel_integrations as integrations;
use aurora_db::repo::connectors::{self, ConnectorRow};
use aurora_db::{DbError, conflict_code};
use serde_json::{Value, json};
use uuid::Uuid;

async fn view(s: &AppState, r: ConnectorRow, count: i64) -> Result<ConnectorAdmin, ApiError> {
    let online = limits::REPLICA_ONLINE_THRESHOLD.as_secs_f64();
    let replicas = connectors::list_live_replicas(&s.pool, &r.id, online)
        .await?
        .into_iter()
        .map(|p| ReplicaInfo {
            id: p.id,
            version: p.version,
            last_seen_at: p.last_seen_at,
        })
        .collect();
    let status = ConnectorStatus::parse(&r.status);
    let widening = match (&r.manifest, status) {
        (Some(m), ConnectorStatus::PendingApproval) => manifest::stored_widening(m),
        _ => Vec::new(),
    };
    Ok(ConnectorAdmin {
        id: r.id,
        display_name: r.display_name,
        status,
        version: r.version,
        contract: r.contract.and_then(|c| u32::try_from(c).ok()),
        manifest: r.manifest.as_ref().map(manifest::public),
        manifest_digest: r.manifest_digest,
        approved_manifest_digest: r.approved_manifest_digest,
        widening,
        replicas,
        integration_count: count,
        approved_at: r.approved_at,
        created_at: r.created_at,
    })
}

async fn view_one(s: &AppState, r: ConnectorRow) -> Result<ConnectorAdmin, ApiError> {
    let n = integrations::count_for_connector(&s.pool, &r.id).await?;
    view(s, r, n).await
}

pub async fn list(s: &AppState) -> Result<ConnectorAdminList, ApiError> {
    let counts = integrations::counts_by_connector(&s.pool).await?;
    let mut items = Vec::new();
    for r in connectors::list(&s.pool).await? {
        let n = counts
            .iter()
            .find(|(id, _)| *id == r.id)
            .map_or(0, |(_, n)| *n);
        items.push(view(s, r, n).await?);
    }
    Ok(ConnectorAdminList { items })
}

fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    (2..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub async fn create(
    s: &AppState,
    req: CreateConnectorRequest,
) -> Result<ConnectorWithToken, ApiError> {
    let mut errs = Vec::new();
    if !valid_id(&req.id) {
        errs.push(FieldError::new("/id", "must match [a-z][a-z0-9-]{1,31}"));
    }
    let name = sanitize::label(req.display_name.trim());
    if name.is_empty() {
        errs.push(FieldError::new("/display_name", "must not be empty"));
    }
    if !errs.is_empty() {
        return Err(ApiError::invalid_request("Invalid connector.", errs));
    }
    let tok = token::generate(TokenPrefix::Connector);
    let row = connectors::insert(&s.pool, &req.id, &token::hash(tok.expose()), &name)
        .await
        .map_err(|e| conflict_code(e, &[("connectors_pkey", "conflict")]))?;
    Ok(ConnectorWithToken {
        connector: view_one(s, row).await?,
        token: tok.expose().clone(),
    })
}

/// Applies `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` (docs/15): each `id:aun_…` entry is upserted
/// with the hashed token, `awaiting_registration` when new. Manifest approval is unchanged.
pub async fn bootstrap_tokens(s: &AppState) -> Result<(), DbError> {
    for (id, tok) in &s.config.bootstrap_connector_tokens {
        connectors::upsert_bootstrap(&s.pool, id, &token::hash(tok.expose()), id).await?;
        tracing::info!(connector = %id, "bootstrap connector token applied");
    }
    Ok(())
}

pub async fn rotate(s: &AppState, id: &str) -> Result<ConnectorToken, ApiError> {
    let tok = token::generate(TokenPrefix::Connector);
    if !connectors::set_token(&s.pool, id, &token::hash(tok.expose())).await? {
        return Err(ApiError::not_found("Connector not found."));
    }
    Ok(ConnectorToken {
        token: tok.expose().clone(),
    })
}

/// Approves exactly the announced manifest (C-31).
pub async fn approve(
    s: &AppState,
    id: &str,
    digest: &str,
    by: Uuid,
) -> Result<ConnectorAdmin, ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let row = connectors::get_for_update(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connector not found."))?;
    let stale = || {
        ApiError::conflict(
            "stale_manifest",
            "The announced manifest changed; review it again.",
        )
    };
    let (Some(stored), Some(current)) = (&row.manifest, &row.manifest_digest) else {
        return Err(stale());
    };
    if current != digest {
        return Err(stale());
    }
    match row.status.as_str() {
        "pending_approval" => {
            connectors::set_approved(&mut *tx, id, current, &manifest::to_approved(stored), by)
                .await?;
        }
        // Already approved: idempotent.
        "active" => {}
        _ => {
            return Err(ApiError::conflict(
                "conflict",
                "The connector cannot be approved in its current state.",
            ));
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    let row = connectors::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connector not found."))?;
    view_one(s, row).await
}

pub async fn delete(s: &AppState, id: &str) -> Result<(), ApiError> {
    let in_use = || ApiError::conflict("connector_in_use", "The connector still has integrations.");
    if integrations::count_for_connector(&s.pool, id).await? > 0 {
        return Err(in_use());
    }
    match connectors::delete(&s.pool, id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ApiError::not_found("Connector not found.")),
        Err(DbError::ForeignKeyViolation { .. }) => Err(in_use()),
        Err(e) => Err(e.into()),
    }
}

// ------------------------------------------------------------------ catalogue

fn info(r: &ConnectorRow) -> Option<ConnectorInfo> {
    let m = r.manifest.as_ref()?;
    let fields_of = |v: Option<&Value>| json!({"fields": v.and_then(|v| v.get("fields")).cloned().unwrap_or_else(|| json!([]))});
    let presence = m.get("presence_setup");
    let presence_setup = match presence.and_then(|p| p.get("dedicated")) {
        Some(d) => json!({"dedicated": fields_of(Some(d))}),
        None => json!({}),
    };
    Some(ConnectorInfo {
        id: r.id.clone(),
        display_name: r.display_name.clone(),
        icon_url: format!("/v1/connectors/{}/icon", r.id),
        setup_guide_md: m
            .get("about")
            .and_then(|a| a.get("setup_guide_md"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        capabilities: m.get("capabilities").cloned().unwrap_or_else(|| json!({})),
        integration_setup: fields_of(m.get("integration_setup")),
        presence_setup,
    })
}

pub async fn catalogue(s: &AppState) -> Result<ConnectorInfoList, ApiError> {
    Ok(ConnectorInfoList {
        items: connectors::list_active(&s.pool)
            .await?
            .iter()
            .filter_map(info)
            .collect(),
    })
}

/// Sanitized PNG icon of an `active` connector (validated at registration).
pub async fn icon(s: &AppState, id: &str) -> Result<Vec<u8>, ApiError> {
    let not_found = || ApiError::not_found("Connector icon not found.");
    let row = connectors::get(&s.pool, id).await?.ok_or_else(not_found)?;
    if row.status != "active" {
        return Err(not_found());
    }
    row.manifest
        .as_ref()
        .and_then(manifest::icon_bytes)
        .ok_or_else(not_found)
}

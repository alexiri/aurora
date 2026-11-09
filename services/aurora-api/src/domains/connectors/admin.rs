//! Instance administration and the public catalogue of connectors (C-31, C-32).

use super::dto::*;
use super::{managed, manifest, setup};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
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
    let approved = r.manifest.as_ref().and_then(manifest::approved_value);
    let mv = managed::managed_view(s, &r).await?;
    Ok(ConnectorAdmin {
        managed: r.managed,
        type_id: r.type_id.clone(),
        settings: mv.settings,
        secrets: mv.secrets,
        host_status: mv.host_status,
        id: r.id,
        display_name: r.display_name,
        status,
        version: r.version,
        contract: r.contract.and_then(|c| u32::try_from(c).ok()),
        manifest: r.manifest.as_ref().map(manifest::public),
        manifest_digest: r.manifest_digest,
        approved_manifest_digest: r.approved_manifest_digest,
        approved_manifest: approved,
        widening,
        replicas,
        integration_count: count,
        approved_at: r.approved_at,
        created_at: r.created_at,
    })
}

pub(super) async fn view_one(s: &AppState, r: ConnectorRow) -> Result<ConnectorAdmin, ApiError> {
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

/// Security-critical audit event of a connector change (RFE-008): committed with the change.
pub(super) fn event(actor: &AuditActor, action: &str, id: &str) -> AuditEvent {
    AuditEvent::new(actor.clone(), action, Outcome::Success).target("connector", id)
}

pub async fn create(
    s: &AppState,
    req: CreateConnectorRequest,
    actor: &AuditActor,
) -> Result<ConnectorWithToken, ApiError> {
    if req.managed {
        return managed::create(s, req, actor).await;
    }
    let mut errs = Vec::new();
    if !valid_id(&req.id) {
        errs.push(FieldError::new("/id", "must match [a-z][a-z0-9-]{1,31}"));
    }
    let name = sanitize::label(req.display_name.as_deref().unwrap_or("").trim());
    if name.is_empty() {
        errs.push(FieldError::new("/display_name", "must not be empty"));
    }
    if !errs.is_empty() {
        return Err(ApiError::invalid_request("Invalid connector.", errs));
    }
    let tok = token::generate(TokenPrefix::Connector);
    let mut tx = audit::begin(s).await?;
    let row = connectors::insert(&mut *tx, &req.id, &token::hash(tok.expose()), &name)
        .await
        .map_err(|e| conflict_code(e, &[("connectors_pkey", "conflict")]))?;
    audit::commit_critical(tx, &event(actor, "admin.connector_create", &req.id)).await?;
    Ok(ConnectorWithToken {
        connector: view_one(s, row).await?,
        token: Some(tok.expose().clone()),
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

pub async fn rotate(
    s: &AppState,
    id: &str,
    actor: &AuditActor,
) -> Result<ConnectorToken, ApiError> {
    // C-454: a managed connector has no `aun_` token.
    let row = connectors::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connector not found."))?;
    if row.managed {
        return Err(ApiError::conflict(
            "connector_managed",
            "A managed connector has no token.",
        ));
    }
    let tok = token::generate(TokenPrefix::Connector);
    let mut tx = audit::begin(s).await?;
    if !connectors::set_token(&mut *tx, id, &token::hash(tok.expose())).await? {
        return Err(ApiError::not_found("Connector not found."));
    }
    audit::commit_critical(tx, &event(actor, "admin.connector_rotate", id)).await?;
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
    actor: &AuditActor,
) -> Result<ConnectorAdmin, ApiError> {
    let mut tx = audit::begin(s).await?;
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
            if row.managed {
                // The status is part of the host's assignments (C-456).
                connectors::bump_revision(&mut *tx).await?;
            }
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
    let ev =
        event(actor, "admin.connector_approve", id).metadata(json!({"manifest_digest": digest}));
    audit::commit_critical(tx, &ev).await?;
    let row = connectors::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connector not found."))?;
    view_one(s, row).await
}

pub async fn delete(s: &AppState, id: &str, actor: &AuditActor) -> Result<(), ApiError> {
    let in_use = || ApiError::conflict("connector_in_use", "The connector still has integrations.");
    if integrations::count_for_connector(&s.pool, id).await? > 0 {
        return Err(in_use());
    }
    let existing = connectors::get(&s.pool, id).await?;
    let mut tx = audit::begin(s).await?;
    match connectors::delete(&mut *tx, id).await {
        Ok(true) => {
            if existing.as_ref().is_some_and(|r| r.managed) {
                connectors::bump_revision(&mut *tx).await?;
            }
            audit::commit_critical(tx, &event(actor, "admin.connector_delete", id)).await?;
            // C-454: a managed connector's sealed settings are discarded after the commit
            // (best effort: a failed discard is logged and never blocks).
            if let Some(r) = existing.filter(|r| r.managed) {
                managed::discard_refs(
                    s,
                    r.secret_owner,
                    setup::refs_of(&r.secret_refs).into_values(),
                )
                .await;
            }
            Ok(())
        }
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

//! Integration setup and lifecycle (docs/13 §5.8; C-33).

use super::dto::*;
use super::presences;
use super::{capability_enabled, map_correlated, object_field};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::connectors::integration_seal_ctx;
use crate::domains::connectors::ops::{self, CORRELATED_WAIT, NewOp};
use crate::http::auth::Caller;
use crate::http::pagination::{Page, Pagination};
use aurora_core::{ApiError, FieldError, Secret};
use aurora_db::conflict_code;
use aurora_db::repo::channel_integrations::{self as repo, IntegrationRow, NewIntegration};
use aurora_db::repo::channel_ops;
use aurora_db::repo::connectors;
use aurora_protocol::connector::CredentialsValidateResult;
use aurora_secrets::{SecretRef, ValueSource};
use serde_json::{Value, json};
use uuid::Uuid;

const CONFIG_MAX: usize = 16 * 1024;
const CREDENTIALS_MAX: usize = 64 * 1024;
const TENANT_CONSTRAINT: &str = "channel_integrations_connector_id_tenant_ref_key";

pub fn view(r: &IntegrationRow) -> ChannelIntegration {
    ChannelIntegration {
        id: r.id,
        org_id: r.org_id,
        connector_id: r.connector_id.clone(),
        tenant_ref: r.tenant_ref.clone(),
        tenant_label: r.tenant_label.clone(),
        config: r.config.clone(),
        shared_bot: r.shared_bot.as_ref().and_then(|b| {
            Some(SharedBotInfo {
                handle: b.get("handle")?.as_str()?.to_owned(),
                display_name: b.get("display_name")?.as_str()?.to_owned(),
            })
        }),
        status: IntegrationStatus::parse(&r.status),
        status_reason: r.status_reason.clone(),
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

/// Integration visible to an org owner/admin (others: 404 non-member, 403 plain member).
pub async fn load_admin(s: &AppState, c: &Caller, id: Uuid) -> Result<IntegrationRow, ApiError> {
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Integration not found."))?;
    c.org(s, row.org_id).await?.require_admin()?;
    Ok(row)
}

pub async fn list(
    s: &AppState,
    c: &Caller,
    org: Uuid,
    p: Pagination,
) -> Result<Page<ChannelIntegration>, ApiError> {
    c.org(s, org).await?.require_admin()?;
    let rows = repo::page_for_org(&s.pool, org, p.after.map(|a| (a.t, a.id)), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Page {
        items: page.items.iter().map(view).collect(),
        next_cursor: page.next_cursor,
    })
}

pub async fn get(s: &AppState, c: &Caller, id: Uuid) -> Result<ChannelIntegration, ApiError> {
    Ok(view(&load_admin(s, c, id).await?))
}

fn invalid_field(path: &str, m: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, m)])
}

fn check_config(config: &Value) -> Result<Value, ApiError> {
    let config = object_field(config, "/config", CONFIG_MAX, true)?;
    if config.get("_capabilities").is_some() {
        return Err(invalid_field("/config/_capabilities", "reserved key"));
    }
    if config
        .get("verified_email_autolink")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(invalid_field(
            "/config/verified_email_autolink",
            "must be a boolean",
        ));
    }
    Ok(config)
}

fn autolink_on(config: &Value) -> bool {
    super::inbox::speaker::autolink_enabled(config)
}

/// RFE-001: auto-linking can only be on for an integration whose effective capabilities
/// include `email_verified` (manifest narrowed by validation, already merged into `config`).
fn check_autolink(config: &Value, manifest: Option<&Value>) -> Result<(), ApiError> {
    if autolink_on(config) && !capability_enabled(manifest, config, "email_verified") {
        return Err(ApiError::unprocessable(
            "autolink_unsupported",
            "This integration cannot report verified emails, so members cannot be linked automatically.",
        ));
    }
    Ok(())
}

/// The `autolink_enable` / `autolink_disable` event when the flag changed (security-critical,
/// RFE-008: committed with the change that flipped it).
fn autolink_event(c: &Caller, org: Uuid, id: Uuid, was: bool, now: bool) -> Option<AuditEvent> {
    if was == now {
        return None;
    }
    let action = if now {
        "channel_integration.autolink_enable"
    } else {
        "channel_integration.autolink_disable"
    };
    Some(
        AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
            .org(org)
            .target("channel_integration", id),
    )
}

/// `config` + the narrowing the connector reported (only capabilities the manifest enables
/// may appear; a capability can be switched off, never on).
fn config_with_capabilities(
    mut config: Value,
    manifest: Option<&Value>,
    res: &CredentialsValidateResult,
) -> Value {
    if let (Some(caps), Some(obj)) = (&res.capabilities, config.as_object_mut()) {
        let mut narrowed = serde_json::Map::new();
        for (name, v) in caps {
            let offered = manifest.and_then(|m| m.pointer(&format!("/capabilities/{name}")));
            let manifest_on = match offered {
                Some(Value::Bool(b)) => *b,
                Some(Value::Object(_)) => true,
                _ => false,
            };
            let requested_on = match v {
                Value::Bool(b) => *b,
                Value::Object(_) => true,
                _ => false,
            };
            if manifest_on || !requested_on {
                narrowed.insert(name.clone(), v.clone());
            }
        }
        obj.insert("_capabilities".to_owned(), Value::Object(narrowed));
    }
    config
}

fn shared_bot_json(res: &CredentialsValidateResult) -> Option<Value> {
    res.shared_bot
        .as_ref()
        .and_then(|b| serde_json::to_value(b).ok())
}

/// Runs the correlated `credentials.validate`. The op carries no integration id: it must be
/// claimable by any live replica of the connector (13 §4.2), because the integration is not
/// served (and so not owned) until it is `active`.
async fn validate(
    s: &AppState,
    connector_id: &str,
    config: &Value,
    credentials: &Value,
) -> Result<CredentialsValidateResult, ApiError> {
    let out = ops::call_correlated(
        s,
        NewOp {
            kind: "credentials.validate".to_owned(),
            connector_id: Some(connector_id.to_owned()),
            integration_id: None,
            presence_id: None,
            payload: json!({"config": config, "credentials": credentials}),
            dedupe_key: None,
        },
        CORRELATED_WAIT,
    )
    .await;
    // The payload carried plaintext credentials: blank it whatever happened.
    if let Err(e) = channel_ops::scrub_validate_payloads(&s.pool, connector_id).await {
        tracing::warn!(error = %e, "validate payload not scrubbed");
    }
    let value = out.map_err(map_correlated)?;
    serde_json::from_value::<CredentialsValidateResult>(value).map_err(|_| {
        ApiError::bad_gateway(
            "connector_error",
            "The connector returned an invalid validation result.",
        )
    })
}

async fn seal_json(s: &AppState, id: Uuid, org: Uuid, v: &Value) -> Result<SecretRef, ApiError> {
    let bytes = serde_json::to_vec(v).map_err(|e| ApiError::internal(e.to_string()))?;
    s.vault
        .seal(&integration_seal_ctx(id, org), Secret::from(bytes))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

async fn open_json(s: &AppState, r: &IntegrationRow) -> Result<Value, ApiError> {
    let sealed: SecretRef = r
        .credentials_ref
        .parse()
        .map_err(|_| ApiError::internal("Stored credentials are unreadable."))?;
    let plain = s
        .vault
        .open(&integration_seal_ctx(r.id, r.org_id), &sealed)
        .await
        .map_err(|_| ApiError::internal("Stored credentials are unreadable."))?;
    serde_json::from_slice(plain.expose())
        .map_err(|_| ApiError::internal("Stored credentials are unreadable."))
}

pub async fn discard(s: &AppState, org: Uuid, owner: Uuid, presence: bool, sealed: &str) {
    let Ok(r) = sealed.parse::<SecretRef>() else {
        return;
    };
    let ctx = if presence {
        crate::domains::connectors::presence_seal_ctx(owner, org)
    } else {
        integration_seal_ctx(owner, org)
    };
    if let Err(e) = s.vault.discard(&ctx, &r, ValueSource::Managed).await {
        tracing::warn!(error = %e, "discarding sealed credentials failed");
    }
}

/// Drop a `validating` row after a failed validation.
async fn remove_failed(s: &AppState, row: &IntegrationRow) {
    if let Err(e) = repo::delete(&s.pool, row.id).await {
        tracing::error!(error = %e, integration_id = %row.id, "failed integration not removed");
    }
    discard(s, row.org_id, row.id, false, &row.credentials_ref).await;
}

pub async fn create(
    s: &AppState,
    c: &Caller,
    org: Uuid,
    req: CreateIntegrationRequest,
) -> Result<ChannelIntegration, ApiError> {
    c.org(s, org).await?.require_admin()?;
    let config = check_config(&req.config)?;
    let credentials = object_field(&req.credentials, "/credentials", CREDENTIALS_MAX, false)?;
    let connector = connectors::get(&s.pool, &req.connector_id)
        .await?
        .filter(|r| r.status == "active")
        .ok_or_else(|| invalid_field("/connector_id", "unknown or inactive connector"))?;

    let id = Uuid::now_v7();
    let sealed = seal_json(s, id, org, &credentials).await?;
    let placeholder = format!("pending:{id}");
    if let Err(e) = repo::insert(
        &s.pool,
        &NewIntegration {
            id,
            org_id: org,
            connector_id: &connector.id,
            tenant_ref: &placeholder,
            config: &config,
            credentials_ref: &sealed.to_column(),
            created_by: Some(c.user_id),
        },
    )
    .await
    {
        discard(s, org, id, false, &sealed.to_column()).await;
        return Err(e.into());
    }
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("Integration vanished."))?;

    let res = match validate(s, &connector.id, &config, &credentials).await {
        Ok(r) => r,
        Err(e) => {
            remove_failed(s, &row).await;
            return Err(e);
        }
    };
    let new_config = config_with_capabilities(config, connector.manifest.as_ref(), &res);
    if let Err(e) = check_autolink(&new_config, connector.manifest.as_ref()) {
        remove_failed(s, &row).await;
        return Err(e);
    }
    let shared = shared_bot_json(&res);
    // RFE-008: activating the integration and its audit rows are one transaction; if they
    // cannot be written the half-created integration is removed again.
    let mut tx = match audit::begin(s).await {
        Ok(tx) => tx,
        Err(e) => {
            remove_failed(s, &row).await;
            return Err(e);
        }
    };
    if let Err(e) = repo::complete_validation(
        &mut *tx,
        id,
        &res.tenant_ref,
        &res.tenant_label,
        &new_config,
        shared.as_ref(),
    )
    .await
    {
        remove_failed(s, &row).await;
        return Err(conflict_code(
            e,
            &[(TENANT_CONSTRAINT, "tenant_already_bound")],
        ));
    }
    let mut events = vec![
        AuditEvent::new(
            AuditActor::from_caller(c),
            "channel_integration.create",
            Outcome::Success,
        )
        .org(org)
        .target("channel_integration", id)
        .metadata(json!({"connector_id": row.connector_id, "tenant_ref": res.tenant_ref})),
    ];
    events.extend(autolink_event(c, org, id, false, autolink_on(&new_config)));
    if let Err(e) = audit::commit_critical_many(tx, &events).await {
        remove_failed(s, &row).await;
        return Err(e);
    }
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("Integration vanished."))?;
    Ok(view(&row))
}

pub async fn patch(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    req: PatchIntegrationRequest,
) -> Result<ChannelIntegration, ApiError> {
    let row = load_admin(s, c, id).await?;
    if req.config.is_none() && req.credentials.is_none() && req.status.is_none() {
        return Err(ApiError::invalid_request(
            "Nothing to change.",
            vec![FieldError::new("", "give config, credentials or status")],
        ));
    }
    let new_config = req.config.as_ref().map(check_config).transpose()?;
    let new_creds = req
        .credentials
        .as_ref()
        .map(|v| object_field(v, "/credentials", CREDENTIALS_MAX, false))
        .transpose()?;
    if row.status == "validating" {
        return Err(ApiError::conflict(
            "conflict",
            "The integration is still being validated.",
        ));
    }

    // The transaction opens after the (network) re-validation, never across it.
    let mut open_tx: Option<audit::Tx> = None;
    let mut autolink_now = autolink_on(&row.config);
    let mut replaced: Option<(Option<String>, Option<String>)> = None;
    if new_config.is_some() || new_creds.is_some() {
        let connector = connectors::get(&s.pool, &row.connector_id)
            .await?
            .filter(|r| r.status == "active")
            .ok_or_else(|| {
                ApiError::unavailable("connector_unavailable", "The connector is not active.")
            })?;
        let config = new_config.unwrap_or_else(|| {
            let mut c = row.config.clone();
            if let Some(o) = c.as_object_mut() {
                o.remove("_capabilities");
            }
            c
        });
        let creds = match &new_creds {
            Some(v) => v.clone(),
            None => open_json(s, &row).await?,
        };
        let res = validate(s, &connector.id, &config, &creds).await?;
        if res.tenant_ref != row.tenant_ref {
            return Err(ApiError::unprocessable(
                "credentials_invalid",
                "These credentials belong to a different workspace.",
            ));
        }
        let config = config_with_capabilities(config, connector.manifest.as_ref(), &res);
        check_autolink(&config, connector.manifest.as_ref())?;
        let shared = shared_bot_json(&res);
        let sealed = match &new_creds {
            Some(v) => Some(seal_json(s, row.id, row.org_id, v).await?),
            None => None,
        };
        let col = sealed.as_ref().map(SecretRef::to_column);
        autolink_now = autolink_on(&config);
        let mut t = match audit::begin(s).await {
            Ok(t) => t,
            Err(e) => {
                if let Some(new) = &col {
                    discard(s, row.org_id, row.id, false, new).await;
                }
                return Err(e);
            }
        };
        let old = repo::replace_settings(
            &mut *t,
            row.id,
            &res.tenant_label,
            &config,
            shared.as_ref(),
            col.as_deref(),
        )
        .await;
        match old {
            Ok(old) => {
                replaced = Some((old, col));
                open_tx = Some(t);
            }
            Err(e) => {
                if let Some(new) = &col {
                    discard(s, row.org_id, row.id, false, new).await;
                }
                return Err(e.into());
            }
        }
    }
    let new_ref = replaced.as_ref().and_then(|(_, n)| n.clone());
    let mut tx = match open_tx {
        Some(t) => t,
        None => audit::begin(s).await?,
    };
    let finish = async {
        if let Some(st) = req.status {
            repo::set_status(&mut *tx, row.id, st.as_str(), None).await?;
        }
        presences::bump_integration_agents(&mut tx, row.id).await?;
        // RFE-008: the update and its audit rows commit together.
        let mut events = vec![
            AuditEvent::new(
                AuditActor::from_caller(c),
                "channel_integration.update",
                Outcome::Success,
            )
            .org(row.org_id)
            .target("channel_integration", row.id)
            .metadata(json!({
                "config": req.config.is_some(),
                "credentials": req.credentials.is_some(),
                "status": req.status.map(SettableStatus::as_str),
            })),
        ];
        events.extend(autolink_event(
            c,
            row.org_id,
            row.id,
            autolink_on(&row.config),
            autolink_now,
        ));
        audit::commit_critical_many(tx, &events).await
    }
    .await;
    if let Err(e) = finish {
        if let Some(new) = &new_ref {
            discard(s, row.org_id, row.id, false, new).await;
        }
        return Err(e);
    }
    if let Some((Some(old), Some(_))) = &replaced {
        discard(s, row.org_id, row.id, false, old).await;
    }
    get_row(s, row.id).await
}

async fn get_row(s: &AppState, id: Uuid) -> Result<ChannelIntegration, ApiError> {
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Integration not found."))?;
    Ok(view(&row))
}

/// `DELETE /channel-integrations/{id}`: deprovisions dedicated presences (best effort), then
/// removes the integration with its presences, bindings, links and defaults.
pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let row = load_admin(s, c, id).await?;
    let refs = repo::sealed_refs(&s.pool, id).await?;
    let mut tx = audit::begin(s).await?;
    // Serialize with concurrent presence changes.
    repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Integration not found."))?;
    presences::enqueue_deprovision_for_integration(&mut tx, id).await?;
    presences::bump_integration_agents(&mut tx, id).await?;
    repo::delete(&mut *tx, id).await?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(c),
        "channel_integration.delete",
        Outcome::Success,
    )
    .org(row.org_id)
    .target("channel_integration", id);
    audit::commit_critical(tx, &ev).await?;
    for (presence, sealed) in refs {
        match presence {
            Some(p) => discard(s, row.org_id, p, true, &sealed).await,
            None => discard(s, row.org_id, id, false, &sealed).await,
        }
    }
    Ok(())
}

//! Presence lifecycle (docs/13 §5.7; C-34): attach, detach, re-attach, provisioning results,
//! deprovision on agent/workspace/org/integration deletion, rename.

use super::dto::*;
use super::integrations::discard;
use super::{capability_enabled, map_correlated};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::connect::fields;
use crate::domains::connectors::ops::{self, CORRELATED_WAIT, NewOp};
use crate::domains::connectors::presence_seal_ctx;
use crate::domains::conversations::service::load_agent;
use crate::http::auth::Caller;
use aurora_core::{ApiError, FieldError, Secret};
use aurora_db::repo::channel_integrations as integ_repo;
use aurora_db::repo::channel_ops;
use aurora_db::repo::channel_presences::{
    self as repo, DeprovisionTarget, NewPresence, PresenceRow,
};
use aurora_db::repo::{agents, connectors, memory};
use aurora_db::{DbError, conflict_code};
use aurora_protocol::connector::{CredentialsValidateResult, PresenceProvisionResult};
use aurora_registry::manifest::FormField;
use serde_json::{Value, json};
use sqlx::PgConnection;
use std::collections::BTreeMap;
use uuid::Uuid;

const AGENT_INTEGRATION_CONSTRAINT: &str = "channel_presences_agent_id_integration_id_key";
const IDENTITY_CONSTRAINT: &str = "channel_presences_integration_id_platform_user_ref_key";

pub fn view(r: &PresenceRow) -> Presence {
    Presence {
        id: r.id,
        agent_id: r.agent_id,
        integration_id: r.integration_id,
        connector_id: r.connector_id.clone(),
        tenant_label: r.tenant_label.clone(),
        mode: PresenceMode::parse(&r.mode),
        handle: r.handle.clone(),
        status: PresenceStatus::parse(&r.status),
        status_reason: r.status_reason.clone(),
        created_at: r.created_at,
    }
}

pub async fn bump_integration_agents(
    conn: &mut PgConnection,
    integration: Uuid,
) -> Result<(), ApiError> {
    Ok(repo::bump_agents_of_integration(conn, integration).await?)
}

// ------------------------------------------------------------------ deprovision

async fn enqueue_deprovision(
    tx: &mut PgConnection,
    t: &DeprovisionTarget,
    keep_integration: bool,
) -> Result<(), ApiError> {
    ops::enqueue(
        tx,
        NewOp {
            kind: "presence.deprovision".to_owned(),
            connector_id: Some(t.connector_id.clone()),
            integration_id: keep_integration.then_some(t.integration_id),
            presence_id: Some(t.presence_id),
            payload: json!({"presence_id": t.presence_id, "handle": t.handle}),
            dedupe_key: Some(format!("presence.deprovision:{}", t.presence_id)),
        },
    )
    .await?;
    Ok(())
}

/// Agent deletion (04 §Deletion semantics): before the cascade removes the rows.
pub async fn enqueue_deprovision_for_agent(
    tx: &mut PgConnection,
    agent: Uuid,
) -> Result<(), ApiError> {
    for t in repo::dedicated_for_agent(&mut *tx, agent).await? {
        enqueue_deprovision(tx, &t, true).await?;
    }
    Ok(())
}

pub async fn enqueue_deprovision_for_workspace(
    tx: &mut PgConnection,
    workspace: Uuid,
) -> Result<(), ApiError> {
    for t in repo::dedicated_for_workspace(&mut *tx, workspace).await? {
        enqueue_deprovision(tx, &t, true).await?;
    }
    Ok(())
}

/// Org deletion removes the integrations too, so the ops carry no integration id.
pub async fn enqueue_deprovision_for_org(tx: &mut PgConnection, org: Uuid) -> Result<(), ApiError> {
    for t in repo::dedicated_for_org(&mut *tx, org).await? {
        enqueue_deprovision(tx, &t, false).await?;
    }
    Ok(())
}

pub async fn enqueue_deprovision_for_integration(
    tx: &mut PgConnection,
    integration: Uuid,
) -> Result<(), ApiError> {
    for t in repo::dedicated_for_integration(&mut *tx, integration).await? {
        enqueue_deprovision(tx, &t, false).await?;
    }
    Ok(())
}

/// An agent was renamed: dedicated bots follow (`presence.rename`).
pub async fn on_agent_renamed(s: &AppState, agent: Uuid, name: &str) -> Result<(), ApiError> {
    let targets = repo::dedicated_for_agent(&s.pool, agent).await?;
    if targets.is_empty() {
        return Ok(());
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    for t in targets {
        ops::enqueue(
            &mut tx,
            NewOp {
                kind: "presence.rename".to_owned(),
                connector_id: Some(t.connector_id),
                integration_id: Some(t.integration_id),
                presence_id: Some(t.presence_id),
                payload: json!({"presence_id": t.presence_id, "name": name}),
                dedupe_key: None,
            },
        )
        .await?;
    }
    memory::bump_agent_sync(&mut tx, agent).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The agent's avatar changed or was removed: dedicated bots follow (`presence.set_icon`).
pub async fn on_avatar_changed(
    s: &AppState,
    agent: Uuid,
    avatar_url: Option<&str>,
) -> Result<(), ApiError> {
    let targets = repo::active_dedicated_for_agent(&s.pool, agent).await?;
    if targets.is_empty() {
        return Ok(());
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    for t in targets {
        ops::enqueue(
            &mut tx,
            NewOp {
                kind: "presence.set_icon".to_owned(),
                connector_id: Some(t.connector_id),
                integration_id: Some(t.integration_id),
                presence_id: Some(t.presence_id),
                payload: json!({"presence_id": t.presence_id, "avatar_url": avatar_url}),
                dedupe_key: None,
            },
        )
        .await?;
    }
    memory::bump_agent_sync(&mut tx, agent).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ------------------------------------------------------------------------ reads

pub async fn list(s: &AppState, c: &Caller, agent: Uuid) -> Result<Vec<Presence>, ApiError> {
    load_agent(s, c, agent).await?;
    Ok(repo::list_for_agent(&s.pool, agent)
        .await?
        .iter()
        .map(view)
        .collect())
}

/// Modes an integration offers, shared by C-34 attach validation and C-39 options.
fn offered_modes(
    manifest: Option<&Value>,
    integ: &integ_repo::IntegrationRow,
    connector_active: bool,
) -> Vec<PresenceMode> {
    let mut out = Vec::new();
    if !connector_active {
        return out;
    }
    if capability_enabled(manifest, &integ.config, "presence_dedicated") {
        out.push(PresenceMode::Dedicated);
    }
    if capability_enabled(manifest, &integ.config, "presence_shared") && integ.shared_bot.is_some()
    {
        out.push(PresenceMode::Shared);
    }
    out
}

/// Integrations an agent can attach to (C-39, `can_use`): metadata only, never config or
/// credentials, so members need no access to the owner/admin integration list (I-27).
pub async fn options(
    s: &AppState,
    c: &Caller,
    agent_id: Uuid,
) -> Result<Vec<PresenceOption>, ApiError> {
    let agent = load_agent(s, c, agent_id).await?;
    let attached: std::collections::BTreeSet<Uuid> = repo::list_for_agent(&s.pool, agent_id)
        .await?
        .iter()
        .map(|p| p.integration_id)
        .collect();
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let rows = integ_repo::page_for_org(&s.pool, agent.org_id, after, 200).await?;
        let n = rows.len();
        after = rows.last().map(|r| (r.created_at, r.id));
        for integ in rows.iter().filter(|i| i.status == "active") {
            let connector = connectors::get(&s.pool, &integ.connector_id).await?;
            let active = connector.as_ref().is_some_and(|r| r.status == "active");
            let manifest = connector.as_ref().and_then(|r| r.manifest.as_ref());
            let modes = offered_modes(manifest, integ, active);
            let fields = manifest
                .and_then(|m| m.pointer("/presence_setup/dedicated/fields"))
                .cloned();
            let presence_setup = match fields {
                Some(f) if modes.contains(&PresenceMode::Dedicated) => {
                    json!({"dedicated": {"fields": f}})
                }
                _ => json!({}),
            };
            out.push(PresenceOption {
                integration_id: integ.id,
                connector_id: integ.connector_id.clone(),
                connector_name: connector
                    .as_ref()
                    .map_or_else(|| integ.connector_id.clone(), |r| r.display_name.clone()),
                tenant_label: integ.tenant_label.clone(),
                modes,
                presence_setup,
                attached: attached.contains(&integ.id),
            });
        }
        if n < 200 {
            break;
        }
    }
    Ok(out)
}

async fn load_presence(
    s: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<(PresenceRow, agents::AgentRow), ApiError> {
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Presence not found."))?;
    let agent = load_agent(s, c, row.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            ApiError::not_found("Presence not found.")
        } else {
            e
        }
    })?;
    Ok((row, agent))
}

// ----------------------------------------------------------------------- attach

fn unprocessable_field(path: &str, m: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, m)])
}

/// `presence_setup.dedicated.fields` of the connector's manifest.
fn manual_fields(manifest: Option<&Value>) -> Vec<FormField> {
    manifest
        .and_then(|m| m.pointer("/presence_setup/dedicated/fields"))
        .and_then(|v| serde_json::from_value::<Vec<FormField>>(v.clone()).ok())
        .unwrap_or_default()
}

pub async fn attach(
    s: &AppState,
    c: &Caller,
    agent_id: Uuid,
    req: AttachPresenceRequest,
) -> Result<Presence, ApiError> {
    let agent = load_agent(s, c, agent_id).await?;
    let integ = integ_repo::get(&s.pool, req.integration_id)
        .await?
        .filter(|i| i.org_id == agent.org_id)
        .ok_or_else(|| unprocessable_field("/integration_id", "unknown integration"))?;
    if integ.status != "active" {
        return Err(ApiError::conflict(
            "conflict",
            "The integration is not active.",
        ));
    }
    let connector = connectors::get(&s.pool, &integ.connector_id)
        .await?
        .filter(|r| r.status == "active");
    let manifest = connector.as_ref().and_then(|r| r.manifest.as_ref());
    let cap = match req.mode {
        PresenceMode::Dedicated => "presence_dedicated",
        PresenceMode::Shared => "presence_shared",
    };
    if connector.is_none() || !capability_enabled(manifest, &integ.config, cap) {
        return Err(ApiError::unprocessable(
            "mode_unsupported",
            "This connector does not offer that presence mode.",
        ));
    }
    let id = Uuid::now_v7();
    match req.mode {
        PresenceMode::Shared => {
            if integ.shared_bot.is_none() {
                return Err(ApiError::unprocessable(
                    "no_shared_bot",
                    "The integration has no shared bot.",
                ));
            }
            let bot = integ.shared_bot.as_ref();
            let handle = bot.and_then(|b| b.get("handle")).and_then(Value::as_str);
            let mut tx = s.pool.begin().await.map_err(DbError::from)?;
            repo::insert(
                &mut *tx,
                &NewPresence {
                    id,
                    agent_id,
                    integration_id: integ.id,
                    mode: "shared",
                    status: "active",
                    platform_user_ref: None,
                    handle,
                },
            )
            .await
            .map_err(map_insert)?;
            memory::bump_agent_sync(&mut tx, agent_id).await?;
            tx.commit().await.map_err(DbError::from)?;
        }
        PresenceMode::Dedicated => {
            let fields_def = manual_fields(manifest);
            let parsed = if fields_def.is_empty() {
                if req.fields.as_ref().is_some_and(|f| !f.is_empty()) {
                    return Err(unprocessable_field(
                        "/fields",
                        "this connector takes no setup fields",
                    ));
                }
                None
            } else {
                Some(fields::parse(
                    &fields_def,
                    &req.fields.clone().unwrap_or_default(),
                )?)
            };
            let mut tx = s.pool.begin().await.map_err(DbError::from)?;
            repo::insert(
                &mut *tx,
                &NewPresence {
                    id,
                    agent_id,
                    integration_id: integ.id,
                    mode: "dedicated",
                    status: "pending_setup",
                    platform_user_ref: None,
                    handle: None,
                },
            )
            .await
            .map_err(map_insert)?;
            if parsed.is_none() {
                enqueue_provision(
                    &mut tx,
                    &s.config.external_url,
                    &integ.connector_id,
                    integ.id,
                    id,
                    &agent,
                )
                .await?;
            }
            memory::bump_agent_sync(&mut tx, agent_id).await?;
            tx.commit().await.map_err(DbError::from)?;
            if let Some(p) = parsed
                && let Err(e) = validate_manual(s, &integ, id, &p.credentials).await
            {
                // Like C-33: the half-created presence does not stay behind.
                if let Err(d) = delete_row(s, id, agent_id).await {
                    tracing::error!(error = %d, "failed presence not removed");
                }
                return Err(e);
            }
        }
    }
    audit_presence(
        s,
        c,
        &agent,
        "channel_presence.attach",
        id,
        json!({"integration_id": integ.id, "mode": req.mode.as_str()}),
    );
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("Presence vanished."))?;
    Ok(view(&row))
}

fn map_insert(e: DbError) -> ApiError {
    conflict_code(
        e,
        &[
            (AGENT_INTEGRATION_CONSTRAINT, "conflict"),
            (IDENTITY_CONSTRAINT, "conflict"),
        ],
    )
}

/// `presence.provision` carries the agent's avatar as the absolute public URL, the same shape as
/// `presence.set_icon` and the config feed (`13 §4.2`); `null` when the agent has none.
async fn enqueue_provision(
    tx: &mut PgConnection,
    external_url: &str,
    connector_id: &str,
    integration: Uuid,
    presence: Uuid,
    agent: &agents::AgentRow,
) -> Result<(), ApiError> {
    let avatar_url = agent
        .avatar_public_key
        .as_deref()
        .map(|k| crate::domains::agents::avatar::public_url(external_url, k));
    ops::enqueue(
        tx,
        NewOp {
            kind: "presence.provision".to_owned(),
            connector_id: Some(connector_id.to_owned()),
            integration_id: Some(integration),
            presence_id: Some(presence),
            payload: json!({"agent": {
                "handle": agent.handle, "name": agent.name, "avatar_url": avatar_url,
            }}),
            dedupe_key: None,
        },
    )
    .await?;
    Ok(())
}

/// Manual presence setup: the pasted fields go through a correlated `credentials.validate`
/// (op carries `presence_id`); success stores the sealed credentials and activates.
async fn validate_manual(
    s: &AppState,
    integ: &integ_repo::IntegrationRow,
    presence: Uuid,
    credentials: &serde_json::Map<String, Value>,
) -> Result<(), ApiError> {
    let credentials = Value::Object(credentials.clone());
    let out = ops::call_correlated(
        s,
        NewOp {
            kind: "credentials.validate".to_owned(),
            connector_id: Some(integ.connector_id.clone()),
            integration_id: None,
            presence_id: Some(presence),
            payload: json!({"config": integ.config, "credentials": credentials}),
            dedupe_key: None,
        },
        CORRELATED_WAIT,
    )
    .await;
    if let Err(e) = channel_ops::scrub_validate_payloads(&s.pool, &integ.connector_id).await {
        tracing::warn!(error = %e, "validate payload not scrubbed");
    }
    let res: CredentialsValidateResult = serde_json::from_value(out.map_err(map_correlated)?)
        .map_err(|_| {
            ApiError::bad_gateway(
                "connector_error",
                "The connector returned an invalid validation result.",
            )
        })?;
    if res.tenant_ref != integ.tenant_ref {
        return Err(ApiError::unprocessable(
            "credentials_invalid",
            "These credentials belong to a different workspace.",
        ));
    }
    let bytes = serde_json::to_vec(&credentials).map_err(|e| ApiError::internal(e.to_string()))?;
    let sealed = s
        .vault
        .seal(
            &presence_seal_ctx(presence, integ.org_id),
            Secret::from(bytes),
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let bot = res.shared_bot.as_ref();
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let done = repo::activate(
        &mut *tx,
        presence,
        bot.map(|b| b.platform_user_ref.as_str()),
        bot.map(|b| b.handle.as_str()),
        &sealed.to_column(),
    )
    .await;
    match done {
        Ok(true) => {
            if let Some(p) = repo::get(&mut *tx, presence).await? {
                memory::bump_agent_sync(&mut tx, p.agent_id).await?;
            }
            tx.commit().await.map_err(DbError::from)?;
            Ok(())
        }
        Ok(false) => {
            discard(s, integ.org_id, presence, true, &sealed.to_column()).await;
            Err(ApiError::not_found("Presence not found."))
        }
        Err(e) => {
            drop(tx);
            discard(s, integ.org_id, presence, true, &sealed.to_column()).await;
            Err(map_insert(e))
        }
    }
}

async fn delete_row(s: &AppState, id: Uuid, agent: Uuid) -> Result<(), ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::delete(&mut *tx, id).await?;
    memory::bump_agent_sync(&mut tx, agent).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn audit_presence(
    s: &AppState,
    c: &Caller,
    agent: &agents::AgentRow,
    action: &str,
    id: Uuid,
    meta: Value,
) {
    audit::record(
        s,
        AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
            .org(agent.org_id)
            .workspace(agent.workspace_id)
            .target("channel_presence", id)
            .metadata(meta),
    );
}

// ----------------------------------------------------------------------- detach

pub async fn detach(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let (row, agent) = load_presence(s, c, id).await?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let Some(cur) = repo::get_for_update(&mut tx, id).await? else {
        return Err(ApiError::not_found("Presence not found."));
    };
    if cur.mode == "dedicated" {
        enqueue_deprovision(
            &mut tx,
            &DeprovisionTarget {
                presence_id: cur.id,
                integration_id: cur.integration_id,
                connector_id: cur.connector_id.clone(),
                agent_id: cur.agent_id,
                handle: cur.handle.clone(),
            },
            true,
        )
        .await?;
    }
    repo::forget_agent_routing(&mut tx, cur.agent_id, cur.integration_id).await?;
    repo::delete(&mut *tx, id).await?;
    memory::bump_agent_sync(&mut tx, cur.agent_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Some(sealed) = &row.credentials_ref {
        discard(s, row.org_id, row.id, true, sealed).await;
    }
    audit_presence(
        s,
        c,
        &agent,
        "channel_presence.detach",
        id,
        json!({"integration_id": row.integration_id}),
    );
    Ok(())
}

// --------------------------------------------------------------------- reattach

pub async fn reattach(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    fields_in: Option<BTreeMap<String, String>>,
) -> Result<Presence, ApiError> {
    let (row, agent) = load_presence(s, c, id).await?;
    let integ = integ_repo::get(&s.pool, row.integration_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Presence not found."))?;
    if integ.status != "active" {
        return Err(ApiError::conflict(
            "conflict",
            "The integration is not active.",
        ));
    }
    if row.mode == "shared" {
        if integ.shared_bot.is_none() {
            return Err(ApiError::unprocessable(
                "no_shared_bot",
                "The integration has no shared bot.",
            ));
        }
        repo::set_status(&s.pool, id, "active", None).await?;
    } else {
        let connector = connectors::get(&s.pool, &integ.connector_id)
            .await?
            .filter(|r| r.status == "active")
            .ok_or_else(|| {
                ApiError::unavailable("connector_unavailable", "The connector is not active.")
            })?;
        let defs = manual_fields(connector.manifest.as_ref());
        if defs.is_empty() {
            let mut tx = s.pool.begin().await.map_err(DbError::from)?;
            repo::set_status(&mut *tx, id, "pending_setup", None).await?;
            enqueue_provision(
                &mut tx,
                &s.config.external_url,
                &integ.connector_id,
                integ.id,
                id,
                &agent,
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
        } else {
            let parsed = fields::parse(&defs, &fields_in.unwrap_or_default())?;
            validate_manual(s, &integ, id, &parsed.credentials).await?;
        }
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    memory::bump_agent_sync(&mut conn, row.agent_id).await?;
    drop(conn);
    audit_presence(s, c, &agent, "channel_presence.reattach", id, json!({}));
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Presence not found."))?;
    Ok(view(&row))
}

// ------------------------------------------------- connector-plane result hooks

/// `presence.provision` finished: store the bot identity and sealed credentials and activate
/// (13 §5.7). A malformed result or an identity already in use leaves the presence in
/// `needs_attention`.
pub async fn on_provision_done(
    s: &AppState,
    tx: &mut PgConnection,
    presence_id: Uuid,
    data: &Value,
) -> Result<(), ApiError> {
    let Some(p) = repo::get_for_update(tx, presence_id).await? else {
        return Ok(()); // detached meanwhile
    };
    let Ok(res) = serde_json::from_value::<PresenceProvisionResult>(data.clone()) else {
        repo::set_status(
            &mut *tx,
            p.id,
            "needs_attention",
            Some("invalid_provision_result"),
        )
        .await?;
        return Ok(());
    };
    let bytes =
        serde_json::to_vec(&res.credentials).map_err(|e| ApiError::internal(e.to_string()))?;
    let sealed = s
        .vault
        .seal(&presence_seal_ctx(p.id, p.org_id), Secret::from(bytes))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let col = sealed.to_column();
    // A savepoint keeps the outer transaction (op result) usable after a unique violation.
    let mut sp = sqlx::Connection::begin(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let done = repo::activate(
        &mut *sp,
        p.id,
        Some(&res.platform_user_ref),
        Some(&res.handle),
        &col,
    )
    .await;
    match done {
        Ok(_) => {
            sp.commit().await.map_err(DbError::from)?;
            memory::bump_agent_sync(tx, p.agent_id).await?;
            if let Some(old) = p.credentials_ref {
                discard(s, p.org_id, p.id, true, &old).await;
            }
        }
        Err(_) => {
            sp.rollback().await.map_err(DbError::from)?;
            discard(s, p.org_id, p.id, true, &col).await;
            repo::set_status(&mut *tx, p.id, "needs_attention", Some("identity_in_use")).await?;
        }
    }
    Ok(())
}

/// A provisioning op failed for good.
pub async fn on_provision_failed(
    tx: &mut PgConnection,
    presence_id: Uuid,
    code: &str,
) -> Result<(), ApiError> {
    if let Some(p) = repo::get(&mut *tx, presence_id).await?
        && p.status == "pending_setup"
    {
        repo::set_status(&mut *tx, presence_id, "needs_attention", Some(code)).await?;
    }
    Ok(())
}

/// `credential_dead` (13 §5.7): the connector layer already marked the presence/integration
/// `disabled`. Operators see it on the resource, in the audit log, and the agent's home sync
/// regenerates (tool availability).
pub async fn on_credential_dead(
    tx: &mut PgConnection,
    s: &AppState,
    integration: Option<Uuid>,
    presence: Option<Uuid>,
    connector_id: &str,
) -> Result<(), ApiError> {
    let mut org = None;
    if let Some(p) = presence {
        if let Some(row) = repo::get(&mut *tx, p).await? {
            org = Some(row.org_id);
            memory::bump_agent_sync(tx, row.agent_id).await?;
        }
    } else if let Some(i) = integration {
        if let Some(row) = integ_repo::get(&mut *tx, i).await? {
            org = Some(row.org_id);
        }
        repo::bump_agents_of_integration(tx, i).await?;
    }
    let mut ev = AuditEvent::new(
        AuditActor::system(connector_id.to_owned()).with_source("channel"),
        "channel.credential_dead",
        Outcome::Failure,
    )
    .metadata(json!({"integration_id": integration, "presence_id": presence}));
    if let Some(o) = org {
        ev = ev.org(o);
    }
    if let Some(p) = presence {
        ev = ev.target("channel_presence", p);
    } else if let Some(i) = integration {
        ev = ev.target("channel_integration", i);
    }
    audit::record(s, ev);
    Ok(())
}

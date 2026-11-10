//! Managed connectors (P4-MCON, D-128; docs/13 §3.1): the type catalogue announced by connector
//! hosts (C-450), managed create (C-451) and PATCH (C-453), and the sealed, write-only secret
//! settings (SEC-70).
//!
//! Secret values are sealed under purpose `connector.settings` (owner = `connectors.secret_owner`,
//! nil org). They never appear in a response, an audit row, a log line or an error message; the
//! only place they are decrypted is the host `assignments` response (`host.rs`).

use super::admin::{event, view_one};
use super::dto::*;
use super::manifest;
use super::setup::{self, Field, Kind};
use crate::app::AppState;
use crate::audit::{self, AuditActor};
use aurora_core::token;
use aurora_core::{ApiError, FieldError, Secret, limits, sanitize};
use aurora_db::conflict_code;
use aurora_db::repo::connector_hosts;
use aurora_db::repo::connectors::{self, ConnectorRow, NewManaged};
use aurora_protocol::connector::ConnectorManifest;
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use chrono::Utc;
use http::StatusCode;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Seal context of a managed connector's secret settings.
pub fn seal_ctx(owner: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::ConnectorSettings,
        owner_id: owner,
        org_id: Uuid::nil(),
        workspace_id: None,
    }
}

pub fn store_unavailable() -> ApiError {
    ApiError::unavailable(
        "secret_store_unavailable",
        "The secret store is unavailable; nothing was changed. Try again shortly.",
    )
}

/// 422 `invalid_request` with `details.fields` (C-451: managed validation is 422).
fn invalid(message: &str, fields: Vec<FieldError>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", message)
        .with_details(json!({ "fields": fields }))
}

fn stale_manifest() -> ApiError {
    ApiError::conflict(
        "stale_manifest",
        "The type's manifest changed; review it again.",
    )
}

// ------------------------------------------------------------------ catalogue

/// A type as announced by the winning live host (C-450).
pub struct CatalogType {
    pub type_id: String,
    pub manifest: ConnectorManifest,
    pub digest: String,
    pub hosts: i64,
    /// `false` only for a type accepted unsigned under `plugins.allow_unsigned` (D-142).
    pub signed: bool,
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// `(host_running, catalogue)` from the hosts seen within `CONNECTOR_HOST_STALE`. Two live hosts
/// announcing different digests for one type: the newest host version wins (ties: the most
/// recently seen host) and `hosts` counts only hosts with that digest.
pub async fn live_catalogue(s: &AppState) -> Result<(bool, Vec<CatalogType>), ApiError> {
    let hosts =
        connector_hosts::list_seen(&s.pool, limits::CONNECTOR_HOST_STALE.as_secs_f64()).await?;
    let running = !hosts.is_empty();
    // type id -> (version key, last seen, digest, manifest, hosts per digest)
    struct Entry {
        key: (Vec<u64>, chrono::DateTime<Utc>),
        digest: String,
        manifest: ConnectorManifest,
        signed: bool,
    }
    let mut by_type: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for h in &hosts {
        let Some(types) = h.types.as_array() else {
            continue;
        };
        for t in types {
            let (Some(id), Some(digest), Some(m)) = (
                t.get("type_id").and_then(Value::as_str),
                t.get("manifest_digest").and_then(Value::as_str),
                t.get("manifest")
                    .and_then(|m| serde_json::from_value::<ConnectorManifest>(m.clone()).ok()),
            ) else {
                continue;
            };
            by_type.entry(id.to_owned()).or_default().push(Entry {
                key: (version_key(&h.version), h.last_seen_at),
                digest: digest.to_owned(),
                manifest: m,
                // Rows stored before D-142 were all verified.
                signed: t.get("signed").and_then(Value::as_bool).unwrap_or(true),
            });
        }
    }
    let mut out = Vec::new();
    for (type_id, entries) in by_type {
        let Some(best) = entries.iter().max_by(|a, b| a.key.cmp(&b.key)) else {
            continue;
        };
        let digest = best.digest.clone();
        let hosts = i64::try_from(entries.iter().filter(|e| e.digest == digest).count())
            .unwrap_or(i64::MAX);
        out.push(CatalogType {
            type_id,
            manifest: best.manifest.clone(),
            digest,
            hosts,
            signed: best.signed,
        });
    }
    Ok((running, out))
}

pub async fn catalogue(s: &AppState) -> Result<ConnectorTypeList, ApiError> {
    let (host_running, types) = live_catalogue(s).await?;
    let existing: Vec<String> = connectors::list(&s.pool)
        .await?
        .into_iter()
        .map(|r| r.id)
        .collect();
    let mut items: Vec<ConnectorType> = Vec::new();
    for t in types {
        let m = &t.manifest;
        let mut mv = serde_json::to_value(m).map_err(|e| ApiError::internal(e.to_string()))?;
        if let Some(about) = mv.get_mut("about").and_then(Value::as_object_mut) {
            about.remove("icon_png_b64");
        }
        items.push(ConnectorType {
            name: m.about.name.clone(),
            version: m.package.version.clone(),
            publisher: m.package.publisher.clone(),
            icon_png_b64: m.about.icon_png_b64.clone(),
            setup_guide_md: m.about.setup_guide_md.clone(),
            manifest: mv,
            manifest_digest: t.digest,
            instance_setup: json!({
                "fields": m.instance_setup.as_ref().map(|s| s.fields.clone()).unwrap_or_default()
            }),
            hosts: t.hosts,
            signed: t.signed,
            added: existing.contains(&t.type_id),
            type_id: t.type_id,
        });
    }
    items.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.type_id.cmp(&b.type_id)));
    Ok(ConnectorTypeList {
        host_running,
        items,
    })
}

/// `GET /v1/admin/overview` (C-459): hosts seen within 24 hours.
pub async fn overview(s: &AppState) -> Result<AdminOverview, ApiError> {
    let hosts =
        connector_hosts::list_seen(&s.pool, limits::CONNECTOR_HOST_RETAIN.as_secs_f64()).await?;
    Ok(AdminOverview {
        connector_hosts: hosts
            .into_iter()
            .map(|h| {
                let mut types: Vec<String> = h
                    .types
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.get("type_id").and_then(Value::as_str).map(str::to_owned))
                    .collect();
                types.sort();
                ConnectorHostInfo {
                    instance_id: h.instance_id,
                    version: h.version,
                    last_seen_at: h.last_seen_at,
                    types,
                }
            })
            .collect(),
    })
}

// -------------------------------------------------------------------- helpers

/// The `instance_setup` fields a managed connector was approved with.
pub fn fields_of(row: &ConnectorRow) -> Vec<Field> {
    let Some(stored) = &row.manifest else {
        return Vec::new();
    };
    let m = manifest::baseline(stored)
        .or_else(|| serde_json::from_value(manifest::public(stored)).ok());
    m.and_then(|m| setup::parse_fields(&m).ok())
        .unwrap_or_default()
}

async fn seal_one(
    s: &AppState,
    owner: Uuid,
    value: &Secret<String>,
) -> Result<SecretRef, ApiError> {
    s.vault
        .seal(
            &seal_ctx(owner),
            Secret::new(value.expose().clone().into_bytes()),
        )
        .await
        .map_err(|e| {
            // The error text of a store may name paths; log the kind only.
            tracing::error!(error = %e, "connector settings could not be sealed");
            store_unavailable()
        })
}

/// Best-effort removal of sealed values (after a commit, or after a failed attempt).
pub async fn discard_refs(s: &AppState, owner: Uuid, refs: impl IntoIterator<Item = String>) {
    for col in refs {
        let Ok(r) = col.parse::<SecretRef>() else {
            continue;
        };
        if let Err(e) = s
            .vault
            .discard(&seal_ctx(owner), &r, ValueSource::Managed)
            .await
        {
            tracing::warn!(error = %e, "sealed connector setting could not be discarded");
        }
    }
}

fn display_name(raw: &str, errs: &mut Vec<FieldError>) -> String {
    let name = sanitize::label(raw.trim());
    if name.is_empty() || name.chars().count() > 80 {
        errs.push(FieldError::new("/display_name", "must be 1-80 characters"));
    }
    name
}

fn names<'a>(it: impl IntoIterator<Item = &'a String>) -> Vec<&'a str> {
    it.into_iter().map(String::as_str).collect()
}

// --------------------------------------------------------------------- create

pub async fn create(
    s: &AppState,
    req: CreateConnectorRequest,
    actor: &AuditActor,
) -> Result<ConnectorWithToken, ApiError> {
    let (_, types) = live_catalogue(s).await?;
    let ty = types
        .into_iter()
        .find(|t| t.type_id == req.id)
        .ok_or_else(|| {
            ApiError::unprocessable(
                "unknown_connector_type",
                "No connector host announces this connector type.",
            )
        })?;
    let Some(approve) = req.approve_manifest_digest.as_deref() else {
        return Err(invalid(
            "Invalid connector.",
            vec![FieldError::new("/approve_manifest_digest", "required")],
        ));
    };
    if approve != ty.digest {
        return Err(stale_manifest());
    }
    let fields = setup::parse_fields(&ty.manifest).map_err(|_| {
        ApiError::unprocessable("invalid_manifest", "The type's manifest is invalid.")
    })?;

    let mut errs = Vec::new();
    let name = match req.display_name.as_deref() {
        Some(n) => display_name(n, &mut errs),
        None => display_name(&ty.manifest.about.name, &mut errs),
    };
    let mut settings: Map<String, Value> = req.settings.unwrap_or_default();
    settings.retain(|_, v| !v.is_null());
    setup::validate_settings(&fields, &settings, &mut errs);
    let supplied = req.secrets.unwrap_or_default();
    setup::validate_secrets(
        &fields,
        supplied.iter().map(|(k, v)| (k, v.as_str())),
        &mut errs,
    );
    for f in fields
        .iter()
        .filter(|f| f.kind == Kind::Secret && f.required)
    {
        if !supplied.contains_key(&f.name) {
            errs.push(FieldError::new(format!("/secrets/{}", f.name), "required"));
        }
    }
    if !errs.is_empty() {
        return Err(invalid("Invalid connector.", errs));
    }
    if connectors::get(&s.pool, &req.id).await?.is_some() {
        return Err(ApiError::conflict(
            "connector_exists",
            "A connector with this id already exists.",
        ));
    }

    // Seal every secret and generated value; on any failure nothing is created.
    let owner = Uuid::now_v7();
    let mut plain: Vec<(String, Secret<String>)> = supplied
        .into_iter()
        .map(|(k, v)| (k, Secret::new(v)))
        .collect();
    for f in fields.iter().filter(|f| f.kind == Kind::GeneratedSecret) {
        plain.push((f.name.clone(), token::generate_setting()));
    }
    let now = Utc::now().to_rfc3339();
    let mut refs = Map::new();
    let mut stamps = Map::new();
    for (name, value) in &plain {
        match seal_one(s, owner, value).await {
            Ok(r) => {
                refs.insert(name.clone(), json!(r.to_column()));
                stamps.insert(name.clone(), json!(now));
            }
            Err(e) => {
                discard_refs(s, owner, setup::refs_of(&Value::Object(refs)).into_values()).await;
                return Err(e);
            }
        }
    }
    let refs_v = Value::Object(refs);
    let stamps_v = Value::Object(stamps);
    let settings_names: Vec<String> = settings.keys().cloned().collect();
    let settings_v = Value::Object(settings);
    let stored = manifest::to_stored(&ty.manifest, Some(&ty.manifest), &[])
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let result = async {
        let mut tx = audit::begin(s).await?;
        let row = connectors::insert_managed(
            &mut *tx,
            &NewManaged {
                id: &req.id,
                display_name: &name,
                settings: &settings_v,
                secret_owner: owner,
                secret_refs: &refs_v,
                secret_updated_at: &stamps_v,
                manifest: &stored,
                digest: &ty.digest,
            },
        )
        .await
        .map_err(|e| conflict_code(e, &[("connectors_pkey", "connector_exists")]))?;
        connectors::bump_revision(&mut *tx).await?;
        let meta = json!({
            "managed": true,
            "type_id": req.id,
            "settings": settings_names,
            "secrets": names(plain.iter().map(|(k, _)| k)),
        });
        audit::commit_critical(
            tx,
            &event(actor, "connector.created", &req.id).metadata(meta),
        )
        .await?;
        Ok::<_, ApiError>(row)
    }
    .await;
    match result {
        Ok(row) => Ok(ConnectorWithToken {
            connector: view_one(s, row).await?,
            token: None,
        }),
        Err(e) => {
            discard_refs(s, owner, setup::refs_of(&refs_v).into_values()).await;
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------- patch

pub async fn patch(
    s: &AppState,
    id: &str,
    req: PatchConnectorRequest,
    actor: &AuditActor,
) -> Result<ConnectorAdmin, ApiError> {
    let mut tx = audit::begin(s).await?;
    let row = connectors::get_for_update(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connector not found."))?;
    if !row.managed && (req.settings.is_some() || req.secrets.is_some() || req.regenerate.is_some())
    {
        return Err(ApiError::unprocessable(
            "connector_not_managed",
            "Settings, secrets and regenerate apply to managed connectors only.",
        ));
    }
    let fields = fields_of(&row);
    let mut errs = Vec::new();
    let mut changed: Vec<String> = Vec::new();

    let name = match req.display_name.as_deref() {
        Some(n) => {
            let n = display_name(n, &mut errs);
            if n != row.display_name {
                changed.push("display_name".to_owned());
            }
            n
        }
        None => row.display_name.clone(),
    };

    // Settings: merge per key; `null` restores the default (removes the stored key).
    let mut settings: Map<String, Value> = row
        .settings
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(patch) = &req.settings {
        for (k, v) in patch {
            changed.push(format!("settings.{k}"));
            if v.is_null() {
                settings.remove(k);
            } else {
                settings.insert(k.clone(), v.clone());
            }
        }
        setup::validate_settings(&fields, &settings, &mut errs);
    }

    // Secrets: replace (string), clear (null), regenerate.
    let replace: Vec<(&String, &str)> = req
        .secrets
        .iter()
        .flatten()
        .filter_map(|(k, v)| v.as_deref().map(|v| (k, v)))
        .collect();
    setup::validate_secrets(&fields, replace.iter().copied(), &mut errs);
    let mut clear: Vec<&String> = Vec::new();
    for (k, v) in req.secrets.iter().flatten() {
        if v.is_some() {
            continue;
        }
        match fields.iter().find(|f| &f.name == k) {
            Some(f) if f.kind == Kind::Secret && !f.required => clear.push(k),
            Some(f) if f.kind == Kind::Secret => {
                errs.push(FieldError::new(
                    format!("/secrets/{k}"),
                    "is required and cannot be cleared",
                ));
            }
            Some(_) => errs.push(FieldError::new(
                format!("/secrets/{k}"),
                "cannot be cleared; regenerate it instead",
            )),
            None => errs.push(FieldError::new(format!("/secrets/{k}"), "unknown secret")),
        }
    }
    let regen: Vec<&String> = req.regenerate.iter().flatten().collect();
    for k in &regen {
        let ok = fields
            .iter()
            .any(|f| &f.name == *k && f.kind == Kind::GeneratedSecret);
        if !ok {
            errs.push(FieldError::new(
                format!("/regenerate/{k}"),
                "is not a generated secret",
            ));
        }
        if req.secrets.as_ref().is_some_and(|m| m.contains_key(*k)) {
            errs.push(FieldError::new(
                format!("/regenerate/{k}"),
                "also given in `secrets`",
            ));
        }
    }
    if !errs.is_empty() {
        return Err(invalid("Invalid connector.", errs));
    }
    for (k, _) in &replace {
        changed.push(format!("secrets.{k}"));
    }
    for k in &clear {
        changed.push(format!("secrets.{k}"));
    }
    for k in &regen {
        changed.push(format!("regenerate.{k}"));
    }

    // Status.
    let mut status = row.status.clone();
    match req.enabled {
        Some(false) if status != "disabled" => {
            status = "disabled".to_owned();
            changed.push("enabled".to_owned());
        }
        Some(true) if status == "disabled" => {
            status = if row.version.is_none() {
                "awaiting_registration"
            } else if row.manifest_digest.is_some()
                && row.manifest_digest == row.approved_manifest_digest
            {
                "active"
            } else {
                "pending_approval"
            }
            .to_owned();
            changed.push("enabled".to_owned());
        }
        _ => {}
    }
    if changed.is_empty() {
        drop(tx);
        return view_one(s, row).await;
    }

    // Seal the new values first: a store failure changes nothing.
    let mut refs = setup::refs_of(&row.secret_refs);
    let mut stamps: Map<String, Value> = row
        .secret_updated_at
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut old: Vec<String> = Vec::new();
    let mut fresh: Vec<String> = Vec::new();
    let now = Utc::now().to_rfc3339();
    let mut to_seal: Vec<(String, Secret<String>)> = replace
        .iter()
        .map(|(k, v)| ((*k).clone(), Secret::new((*v).to_owned())))
        .collect();
    for k in &regen {
        to_seal.push(((*k).clone(), token::generate_setting()));
    }
    for (k, v) in &to_seal {
        match seal_one(s, row.secret_owner, v).await {
            Ok(r) => {
                let col = r.to_column();
                fresh.push(col.clone());
                if let Some(prev) = refs.insert(k.clone(), col) {
                    old.push(prev);
                }
                stamps.insert(k.clone(), json!(now));
            }
            Err(e) => {
                discard_refs(s, row.secret_owner, fresh).await;
                return Err(e);
            }
        }
    }
    for k in &clear {
        if let Some(prev) = refs.remove(*k) {
            old.push(prev);
        }
        stamps.remove(*k);
    }
    let refs_v = json!(refs);
    let stamps_v = Value::Object(stamps);
    let settings_v = row.managed.then_some(Value::Object(settings));

    let result = async {
        connectors::update_managed(
            &mut *tx,
            id,
            &name,
            &status,
            settings_v.as_ref(),
            &refs_v,
            &stamps_v,
        )
        .await?;
        if row.managed {
            connectors::bump_revision(&mut *tx).await?;
        }
        let meta = json!({ "changed": changed, "enabled": status != "disabled" });
        audit::commit_critical(tx, &event(actor, "connector.updated", id).metadata(meta)).await
    }
    .await;
    if let Err(e) = result {
        discard_refs(s, row.secret_owner, fresh).await;
        return Err(e);
    }
    discard_refs(s, row.secret_owner, old).await;
    let row = connectors::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connector not found."))?;
    view_one(s, row).await
}

/// View helpers for [`super::admin`]: managed fields of the `Connector` DTO (C-452).
pub struct ManagedView {
    pub settings: Option<Value>,
    pub secrets: BTreeMap<String, ConnectorSecretInfo>,
    pub host_status: Vec<HostStatusInfo>,
}

pub async fn managed_view(s: &AppState, r: &ConnectorRow) -> Result<ManagedView, ApiError> {
    if !r.managed {
        return Ok(ManagedView {
            settings: None,
            secrets: BTreeMap::new(),
            host_status: Vec::new(),
        });
    }
    let fields = fields_of(r);
    let refs = setup::refs_of(&r.secret_refs);
    let secrets = fields
        .iter()
        .filter(|f| f.is_secret())
        .map(|f| {
            let at = r
                .secret_updated_at
                .get(&f.name)
                .and_then(Value::as_str)
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Utc));
            (
                f.name.clone(),
                ConnectorSecretInfo {
                    set: refs.contains_key(&f.name),
                    generated: f.kind == Kind::GeneratedSecret,
                    updated_at: at,
                },
            )
        })
        .collect();
    let host_status =
        connector_hosts::list_status(&s.pool, &r.id, limits::CONNECTOR_HOST_STALE.as_secs_f64())
            .await?
            .into_iter()
            .map(|h| HostStatusInfo {
                host_instance: h.host_instance,
                state: if h.state == "running" {
                    HostState::Running
                } else {
                    HostState::Error
                },
                message: h.message,
                at: h.updated_at,
            })
            .collect();
    Ok(ManagedView {
        settings: Some(Value::Object(setup::with_defaults(
            &fields,
            r.settings.as_ref(),
        ))),
        secrets,
        host_status,
    })
}

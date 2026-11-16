//! The one settings service (phase 5, SET-1, D-236, SEC-89, SEC-90): every write path — the
//! dashboard/API routes, import and the CLI — goes through [`apply`], so a locked key is refused,
//! the merged view is validated, the write is compare-and-set, audited in its transaction and
//! announced in exactly one place.

use super::dto::*;
use super::handle::{Problem, RowMeta, SettingsHandle, stored_from_rows};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::auth::mail::Mail;
use crate::http::auth::{Caller, Principal};
use aurora_config::{
    Effective, ListKind, SectionDef, SettingDef, SettingType, Source, Stored, registry, sections,
};
use aurora_core::{ApiError, Secret, limits};
use aurora_db::repo::instance_settings as repo;
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use chrono::Utc;
use http::StatusCode;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Who is writing.
#[derive(Clone)]
pub enum Via {
    /// An instance admin's dashboard session (session only; no re-authentication, D-317).
    Admin(Caller),
    /// The operator's shell (`aurora-api settings …`): source `cli`.
    Cli,
}

impl Via {
    fn actor(&self) -> AuditActor {
        match self {
            Self::Admin(c) => AuditActor::instance_admin(c),
            Self::Cli => AuditActor {
                kind: "system",
                id: None,
                label: "cli".into(),
                source: "cli",
                ip: None,
            },
        }
    }
    fn user_id(&self) -> Option<Uuid> {
        match self {
            Self::Admin(c) => Some(c.user_id),
            Self::Cli => None,
        }
    }
    fn label(&self) -> String {
        match self {
            Self::Admin(c) => c.email.clone(),
            Self::Cli => "cli".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Update,
    Import,
    Reset,
}

impl Kind {
    const fn action(self) -> &'static str {
        match self {
            Self::Update => "instance_settings.update",
            Self::Import => "instance_settings.import",
            Self::Reset => "instance_settings.reset",
        }
    }
}

// ----------------------------------------------------------------------------------- errors

fn dberr(e: aurora_db::DbError) -> ApiError {
    e.into()
}

pub fn unknown_section(section: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "setting_section_unknown",
        format!("There is no settings section `{section}`."),
    )
}

fn section_def(section: &str) -> Result<&'static SectionDef, ApiError> {
    sections()
        .iter()
        .find(|s| s.id == section)
        .ok_or_else(|| unknown_section(section))
}

pub(super) fn field(path: String, code: &str, message: impl Into<String>) -> Value {
    json!({ "path": path, "code": code, "message": message.into() })
}

pub(super) fn invalid(fields: Vec<Value>) -> ApiError {
    ApiError::unprocessable(
        "settings_invalid",
        "Some settings are not valid. Fix the listed fields and save again.",
    )
    .with_details(json!({ "fields": fields }))
}

fn problems_error(problems: Vec<Problem>) -> ApiError {
    invalid(
        problems
            .into_iter()
            .map(|p| field(format!("/{}", p.key), &p.code, p.message))
            .collect(),
    )
}

fn stale(section: &str, current: i64) -> ApiError {
    ApiError::conflict(
        "stale_revision",
        "These settings changed since you opened them. Reload and apply your change again.",
    )
    .with_details(json!({ "section": section, "current_generation": current }))
}

fn locked(key: &str, helm_key: &str) -> ApiError {
    ApiError::conflict(
        "setting_locked",
        format!("`{key}` is configured in Helm (`{helm_key}`) and cannot be changed here."),
    )
    .with_details(json!({ "key": key, "helm_key": helm_key }))
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "settings_too_large",
        "These settings are too large.",
    )
}

/// Settings writes are session only: an API key or agent token is refused (FIX-157, D-317: no
/// OIDC re-authentication is required any more). The CLI is the operator's shell.
pub fn require_session(via: &Via) -> Result<(), ApiError> {
    let Via::Admin(c) = via else { return Ok(()) };
    let Principal::Session { .. } = c.principal else {
        return Err(ApiError::forbidden(
            "Sign in with your account to change instance settings.",
        ));
    };
    Ok(())
}

// ------------------------------------------------------------------------------ secret values

fn is_stored_secret(v: &Value) -> bool {
    v.as_object()
        .filter(|m| m.len() == 1)
        .and_then(|m| m.get("$secret"))
        .and_then(Value::as_object)
        .is_some_and(|s| s.contains_key("ref"))
}

fn is_secret_marker(v: &Value) -> bool {
    v.as_object()
        .is_some_and(|m| m.len() == 1 && m.contains_key("$secret"))
}

/// Entry fields that do not say where or as whom a stored secret is used: they may change while a
/// secret is kept.
const KEEP_EXEMPT_FIELDS: &[&str] = &[
    "reference_scopes",
    "references",
    "kv_mount",
    "managed_prefix",
];

fn without_secrets(v: &Value) -> Value {
    if is_secret_marker(v) {
        return Value::Null;
    }
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, c)| (k.clone(), without_secrets(c)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(without_secrets).collect()),
        other => other.clone(),
    }
}

fn has_keep(v: &Value) -> bool {
    if is_secret_marker(v) {
        return v["$secret"].as_str() == Some("keep");
    }
    match v {
        Value::Object(m) => m.values().any(has_keep),
        Value::Array(a) => a.iter().any(has_keep),
        _ => false,
    }
}

/// SEC-90: `{ "$secret": "keep" }` may not carry a stored secret to a different destination. True
/// when `new` keeps a stored secret but any other field of the entry (address, auth mount, role,
/// CA …) differs from the stored entry `old`: the secret must be entered again.
pub fn keep_retargets(new: &Value, old: &Value) -> bool {
    if !has_keep(new) {
        return false;
    }
    let strip = |v: &Value| {
        let mut v = without_secrets(v);
        if let Value::Object(m) = &mut v {
            m.retain(|k, c| !KEEP_EXEMPT_FIELDS.contains(&k.as_str()) && !c.is_null());
        }
        v
    };
    strip(new) != strip(old)
}

pub fn reentry_required(path: &str) -> ApiError {
    ApiError::unprocessable(
        "secret_reentry_required",
        "The connection settings changed, so the stored secret cannot be kept. Enter it again.",
    )
    .with_details(json!({ "fields": [field(
        path.to_string(),
        "secret_reentry_required",
        "enter the secret again: its destination changed",
    )] }))
}

/// Every stored secret `(owner, ref)` anywhere in a value.
fn collect_secrets(v: &Value, out: &mut Vec<(Uuid, String)>) {
    if is_stored_secret(v) {
        if let Some(s) = v.get("$secret")
            && let (Some(r), Some(o)) = (
                s.get("ref").and_then(Value::as_str),
                s.get("owner")
                    .and_then(Value::as_str)
                    .and_then(|o| o.parse().ok()),
            )
        {
            out.push((o, r.to_string()));
        }
        return;
    }
    match v {
        Value::Object(m) => m.values().for_each(|c| collect_secrets(c, out)),
        Value::Array(a) => a.iter().for_each(|c| collect_secrets(c, out)),
        _ => {}
    }
}

/// Replace every stored secret in `v` by `label` (what responses, exports and audit rows show).
fn redact(v: &Value, label: &str) -> Value {
    if is_stored_secret(v) || is_secret_marker(v) {
        return Value::String(label.to_string());
    }
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, c)| (k.clone(), redact(c, label)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|c| redact(c, label)).collect()),
        other => other.clone(),
    }
}

/// A stored secret as the C-772 shape `{ "$secret": { set, updated_at } }`.
fn public_secrets(v: &Value) -> Value {
    if is_stored_secret(v) {
        let at = v["$secret"]
            .get("updated_at")
            .cloned()
            .unwrap_or(Value::Null);
        return json!({ "$secret": { "set": true, "updated_at": at } });
    }
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, c)| (k.clone(), public_secrets(c)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(public_secrets).collect()),
        other => other.clone(),
    }
}

/// Stored secrets as `{ "$secret": <word> }` (`keep` for round trips, `set` for exports).
fn secrets_as(v: &Value, word: &str) -> Value {
    if is_stored_secret(v) {
        return json!({ "$secret": word });
    }
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, c)| (k.clone(), secrets_as(c, word)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|c| secrets_as(c, word)).collect()),
        other => other.clone(),
    }
}

fn store_unavailable() -> ApiError {
    ApiError::unavailable(
        "secret_store_unavailable",
        "The secret store is unavailable; nothing was changed. Try again shortly.",
    )
}

/// Resolve `{ "$secret": … }` markers in `v` against the stored value `old`: a string seals a new
/// value, `"keep"` keeps the stored one (400 when there is none), anything else is refused.
/// `dry` seals nothing (previews).
#[allow(clippy::too_many_arguments)]
async fn resolve_secrets(
    state: &AppState,
    dry: bool,
    import: bool,
    fresh: &mut Vec<(Uuid, String)>,
    v: &mut Value,
    old: Option<&Value>,
    id_field: Option<&str>,
    path: &str,
    fields: &mut Vec<Value>,
) -> Result<(), ApiError> {
    if is_secret_marker(v) {
        let word = v["$secret"].clone();
        match word.as_str() {
            Some("keep") => match old.filter(|o| is_stored_secret(o)) {
                Some(o) => *v = o.clone(),
                None if import => fields.push(field(
                    path.to_string(),
                    "secret_missing",
                    "there is no stored secret to keep; enter it on the page",
                )),
                None => {
                    return Err(ApiError::invalid_request(
                        "There is no stored secret to keep.",
                        vec![aurora_core::FieldError::new(
                            path,
                            "no stored secret to keep",
                        )],
                    ));
                }
            },
            Some(plain) if !plain.is_empty() => {
                if dry {
                    *v = json!({ "$secret": { "ref": "preview", "owner": Uuid::nil(), "updated_at": Utc::now() } });
                } else {
                    let owner = Uuid::now_v7();
                    let ctx = SealContext {
                        purpose: Purpose::InstanceSetting,
                        owner_id: owner,
                        org_id: Uuid::nil(),
                        workspace_id: None,
                    };
                    let r = state
                        .vault
                        .seal_inline(&ctx, Secret::from(plain.as_bytes().to_vec()))
                        .await
                        .map_err(|_| store_unavailable())?;
                    fresh.push((owner, r.to_column()));
                    *v = json!({ "$secret": { "ref": r.to_column(), "owner": owner, "updated_at": Utc::now() } });
                }
            }
            _ => fields.push(field(
                path.to_string(),
                "invalid_value",
                "a secret must be a non-empty string",
            )),
        }
        return Ok(());
    }
    match v {
        Value::Object(m) => {
            for (k, child) in m.iter_mut() {
                let o = old.and_then(|o| o.get(k));
                let p = format!("{path}/{k}");
                Box::pin(resolve_secrets(
                    state, dry, import, fresh, child, o, None, &p, fields,
                ))
                .await?;
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                let id = id_field
                    .and_then(|f| item.get(f))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let o = match (id_field, &id) {
                    (Some(f), Some(id)) => old.and_then(Value::as_array).and_then(|a| {
                        a.iter()
                            .find(|e| e.get(f).and_then(Value::as_str) == Some(id.as_str()))
                    }),
                    _ => None,
                };
                let p = format!("{path}/{}", id.as_deref().unwrap_or("?"));
                // A provider's secret is bound to its issuer and client id only (the other fields
                // are edited freely); any other keyed entry to its whole connection (SEC-90).
                let retargets = |o: &Value| {
                    if path == "/auth.oidc_providers" {
                        super::sign_in::keep_retargets(item, o)
                    } else {
                        keep_retargets(item, o)
                    }
                };
                if let Some(o) = o
                    && retargets(o)
                {
                    return Err(reentry_required(&p));
                }
                Box::pin(resolve_secrets(
                    state, dry, import, fresh, item, o, None, &p, fields,
                ))
                .await?;
            }
        }
        _ => {}
    }
    Ok(())
}

// ----------------------------------------------------------------------------- normalisation

fn id_field_of(d: &SettingDef) -> Option<&'static str> {
    match d.list {
        Some(ListKind::Keyed { id_field }) => Some(id_field),
        _ => None,
    }
}

fn helm_entry_ids(h: &SettingsHandle, d: &SettingDef) -> Vec<String> {
    let Some(f) = id_field_of(d) else {
        return Vec::new();
    };
    h.source
        .helm_value(d.key)
        .map(|v| aurora_config::list_entries(d, v))
        .into_iter()
        .flatten()
        .filter_map(|e| e.get(f).and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Check a request's `values` against the registry, the locks and the types. Returns the values
/// to store (locked keys that equal their effective value dropped) and warnings.
fn normalize(
    h: &SettingsHandle,
    section: &str,
    values: &Map<String, Value>,
    import: bool,
) -> Result<(Map<String, Value>, Vec<SettingWarning>), ApiError> {
    let reg = registry();
    let mut out = Map::new();
    let mut warnings = Vec::new();
    let mut fields = Vec::new();
    for (key, val) in values {
        let Some(d) = reg.get(key).filter(|d| d.section == section) else {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "setting_unknown",
                format!("`{key}` is not a setting of section `{section}`."),
            )
            .with_details(json!({ "key": key })));
        };
        if d.secret {
            if val.is_null() {
                continue;
            }
            if !is_secret_marker(val) {
                fields.push(field(
                    format!("/{key}"),
                    "invalid_value",
                    "a secret is written as { \"$secret\": \"…\" }",
                ));
                continue;
            }
            // A secret Helm sets is Helm's: the dashboard neither sets, keeps nor clears it.
            if h.source.sets(key) || h.source.secret_env(key).is_some() {
                if import && matches!(val["$secret"].as_str(), Some("set" | "keep")) {
                    warnings.push(SettingWarning {
                        entry_id: None,
                        code: "locked_ignored".into(),
                        key: Some(key.clone()),
                        message: format!("`{key}` is configured in Helm and is ignored."),
                    });
                    continue;
                }
                return Err(locked(key, &d.helm_key));
            }
            if import
                && val["$secret"]
                    .as_str()
                    .is_some_and(|s| s != "set" && s != "keep")
            {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "secret_in_import",
                    "Secrets are entered on the page, not imported.",
                )
                .with_details(json!({ "key": key })));
            }
            out.insert(key.clone(), val.clone());
            continue;
        }
        match d.list {
            Some(ListKind::Keyed { id_field }) => {
                let Some(items) = val.as_array() else {
                    fields.push(field(format!("/{key}"), "invalid_value", "must be a list"));
                    continue;
                };
                let helm_ids = helm_entry_ids(h, d);
                let mut seen: Vec<&str> = Vec::new();
                for e in items {
                    let id = e.get(id_field).and_then(Value::as_str).unwrap_or_default();
                    if !e.is_object() || id.is_empty() {
                        fields.push(field(
                            format!("/{key}/{id}/{id_field}"),
                            "invalid_value",
                            format!("every entry needs a non-empty `{id_field}`"),
                        ));
                    } else if helm_ids.iter().any(|x| x == id) {
                        fields.push(field(
                            format!("/{key}/{id}/{id_field}"),
                            "entry_id_in_helm",
                            format!("`{id}` is defined in Helm; choose another id"),
                        ));
                    } else if seen.contains(&id) {
                        fields.push(field(
                            format!("/{key}/{id}/{id_field}"),
                            "duplicate_entry_id",
                            format!("`{id}` appears twice"),
                        ));
                    }
                    seen.push(id);
                    for p in super::rules::entry_problems(key, e) {
                        fields.push(field(format!("/{key}/{id}/{}", p.field), p.code, p.message));
                    }
                }
                if let Err(m) = d.check(val) {
                    fields.push(field(format!("/{key}"), "invalid_value", m));
                }
                out.insert(key.clone(), val.clone());
            }
            _ => {
                if h.source.sets(key) {
                    let helm = h.source.helm_value(key).cloned().unwrap_or(Value::Null);
                    if &helm != val {
                        return Err(locked(key, &d.helm_key));
                    }
                    if import {
                        warnings.push(SettingWarning {
                            entry_id: None,
                            code: "locked_ignored".into(),
                            key: Some(key.clone()),
                            message: format!("`{key}` is configured in Helm; the imported value equals it and is ignored."),
                        });
                    }
                    continue;
                }
                if let Err(m) = d.check(val) {
                    fields.push(field(format!("/{key}"), "invalid_value", m));
                }
                out.insert(key.clone(), val.clone());
            }
        }
    }
    if fields.is_empty() {
        Ok((out, warnings))
    } else {
        Err(invalid(fields))
    }
}

/// The new stored object of a section: locked keys keep their mirrored copy, a keyed list keeps
/// the entries Helm defines plus the submitted dashboard entries, everything else is the
/// submitted value (absent = default).
fn build_new(
    h: &SettingsHandle,
    section: &str,
    old: &Map<String, Value>,
    norm: &Map<String, Value>,
) -> Map<String, Value> {
    let mut out = Map::new();
    for d in registry().section(section) {
        let key = d.key;
        match d.list {
            Some(ListKind::Keyed { id_field }) => {
                let helm_ids = helm_entry_ids(h, d);
                let mut list: Vec<Value> = old
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|e| {
                        e.get(id_field)
                            .and_then(Value::as_str)
                            .is_some_and(|i| helm_ids.iter().any(|x| x == i))
                    })
                    .cloned()
                    .collect();
                if let Some(v) = norm.get(key).and_then(Value::as_array) {
                    list.extend(v.iter().cloned());
                }
                if !list.is_empty() {
                    out.insert(key.to_string(), Value::Array(list));
                }
            }
            _ if h.source.sets(key) => {
                // The mirrored copy stays as it is (mirroring owns it); nothing is invented here.
                if let Some(v) = old.get(key) {
                    out.insert(key.to_string(), v.clone());
                }
            }
            _ => {
                if let Some(v) = norm.get(key) {
                    out.insert(key.to_string(), v.clone());
                }
            }
        }
    }
    out
}

// -------------------------------------------------------------------------------------- diffs

fn bound(v: &Value) -> Value {
    let s = v.to_string();
    if s.chars().count() <= limits::SETTINGS_AUDIT_VALUE_MAX {
        return v.clone();
    }
    let cut: String = s.chars().take(limits::SETTINGS_AUDIT_VALUE_MAX).collect();
    Value::String(format!("{cut}…"))
}

/// The stored change of a section as audit items `{ key, entry_id?, before, after }`.
fn stored_changes(section: &str, old: &Map<String, Value>, new: &Map<String, Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for d in registry().section(section) {
        let (o, n) = (old.get(d.key), new.get(d.key));
        if o == n {
            continue;
        }
        let item = |entry: Option<&str>, b: Option<&Value>, a: Option<&Value>| {
            let had_b = b.is_some();
            let before = b.map_or(Value::Null, |v| bound(&redact(v, "(secret)")));
            let after = match (a, b) {
                (None, _) if d.secret => Value::String("(secret removed)".into()),
                (Some(v), _) if d.secret || has_secret(v) => {
                    if had_b {
                        Value::String("(secret changed)".into())
                    } else {
                        bound(&redact(v, "(secret changed)"))
                    }
                }
                (Some(v), _) => bound(v),
                (None, _) => Value::Null,
            };
            let mut m = Map::new();
            m.insert("key".into(), json!(d.key));
            if let Some(e) = entry {
                m.insert("entry_id".into(), json!(e));
            }
            m.insert("before".into(), before);
            m.insert("after".into(), after);
            Value::Object(m)
        };
        match id_field_of(d) {
            Some(f) => {
                let ids = |v: Option<&Value>| -> Vec<(String, Value)> {
                    v.and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|e| {
                            (
                                e.get(f)
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                e.clone(),
                            )
                        })
                        .collect()
                };
                let (ob, nb) = (ids(o), ids(n));
                let mut all: Vec<&String> = ob.iter().chain(nb.iter()).map(|(i, _)| i).collect();
                all.sort();
                all.dedup();
                for id in all {
                    let b = ob.iter().find(|(i, _)| i == id).map(|(_, v)| v);
                    let a = nb.iter().find(|(i, _)| i == id).map(|(_, v)| v);
                    if a != b {
                        out.push(item(Some(id), b, a));
                    }
                }
            }
            None => out.push(item(None, o, n)),
        }
    }
    out
}

fn has_secret(v: &Value) -> bool {
    let mut s = Vec::new();
    collect_secrets(v, &mut s);
    !s.is_empty() || is_secret_marker(v)
}

fn src(s: Source) -> String {
    match s {
        Source::Helm => "helm",
        Source::Dashboard => "dashboard",
        Source::Default => "default",
    }
    .into()
}

/// The effective diff of one section between two merged views, with the keys that loosen.
fn effective_diff(
    section: &str,
    before: &Effective,
    after: &Effective,
    old: &Map<String, Value>,
    new: &Map<String, Value>,
) -> (Vec<DiffItem>, Vec<LooseningItem>) {
    let mut diff = Vec::new();
    let mut loose = Vec::new();
    for d in registry().section(section) {
        if d.secret {
            let (o, n) = (old.get(d.key), new.get(d.key));
            if o != n {
                diff.push(DiffItem {
                    key: d.key.into(),
                    entry_id: None,
                    before: o.map_or(Value::Null, |_| json!("(secret)")),
                    after: match (o, n) {
                        (Some(_), Some(_)) => json!("(secret changed)"),
                        (None, Some(_)) => json!("(secret)"),
                        _ => json!("(secret removed)"),
                    },
                    before_source: if o.is_some() { "dashboard" } else { "default" }.into(),
                    after_source: if n.is_some() { "dashboard" } else { "default" }.into(),
                });
            }
            continue;
        }
        let (Some(b), Some(a)) = (before.keys.get(d.key), after.keys.get(d.key)) else {
            continue;
        };
        if let (Some(be), Some(ae)) = (&b.entries, &a.entries) {
            let mut ids: Vec<&String> = be.iter().chain(ae.iter()).map(|e| &e.id).collect();
            ids.sort();
            ids.dedup();
            for id in ids {
                let x = be.iter().find(|e| &e.id == id);
                let y = ae.iter().find(|e| &e.id == id);
                if x.map(|e| (&e.value, e.source)) == y.map(|e| (&e.value, e.source)) {
                    continue;
                }
                diff.push(DiffItem {
                    key: d.key.into(),
                    entry_id: Some(id.clone()),
                    before: x.map_or(Value::Null, |e| redact(&e.value, "(secret)")),
                    after: y.map_or(Value::Null, |e| redact(&e.value, "(secret)")),
                    before_source: x.map_or("default".into(), |e| src(e.source)),
                    after_source: y.map_or("default".into(), |e| src(e.source)),
                });
            }
            continue;
        }
        if b.value == a.value && b.source == a.source {
            continue;
        }
        diff.push(DiffItem {
            key: d.key.into(),
            entry_id: None,
            before: b.value.clone(),
            after: a.value.clone(),
            before_source: src(b.source),
            after_source: src(a.source),
        });
        if d.is_loosening(&b.value, &a.value) {
            loose.push(LooseningItem {
                key: d.key.into(),
                entry_id: None,
                reason: format!("`{}` becomes less strict.", d.key),
            });
        }
    }
    (diff, loose)
}

// --------------------------------------------------------------------------------- the plan

struct Plan {
    section: String,
    current: i64,
    old: Map<String, Value>,
    new: Map<String, Value>,
    warnings: Vec<SettingWarning>,
}

fn row_object(row: Option<&repo::SettingsRow>) -> (i64, Map<String, Value>) {
    match row {
        Some(r) => (
            r.generation,
            match &r.value {
                Value::Object(m) => m.clone(),
                _ => Map::new(),
            },
        ),
        None => (0, Map::new()),
    }
}

async fn plan_section(
    state: &AppState,
    section: &str,
    row: Option<&repo::SettingsRow>,
    values: &Map<String, Value>,
    import: bool,
    dry: bool,
    fresh: &mut Vec<(Uuid, String)>,
) -> Result<Plan, ApiError> {
    let h = &state.settings;
    let (current, old) = row_object(row);
    let (mut norm, warnings) = normalize(h, section, values, import)?;
    if section == "sign_in" {
        let fields = super::sign_in::prepare_entries(&mut norm, &old);
        if !fields.is_empty() {
            return Err(invalid(fields));
        }
    }
    if section == "email" {
        email_password_binding(h, &old, &norm)?;
    }
    let mut new = build_new(h, section, &old, &norm);
    let mut fields = Vec::new();
    for d in registry().section(section) {
        if let Some(v) = new.get_mut(d.key) {
            // Only submitted (non-locked) values can carry markers; stored ones are already sealed.
            if !has_marker(v) {
                continue;
            }
            let path = format!("/{}", d.key);
            resolve_secrets(
                state,
                dry,
                import,
                fresh,
                v,
                old.get(d.key),
                id_field_of(d),
                &path,
                &mut fields,
            )
            .await?;
        }
    }
    if !fields.is_empty() {
        return Err(invalid(fields));
    }
    if serde_json::to_vec(&Value::Object(new.clone())).map_or(0, |b| b.len())
        > limits::SETTINGS_SECTION_BYTES_MAX
    {
        return Err(too_large());
    }
    Ok(Plan {
        section: section.to_string(),
        current,
        old,
        new,
        warnings,
    })
}

/// The keys that decide where the SMTP password goes.
const SMTP_CONNECTION_KEYS: [&str; 2] = ["email.smtp_url", "email.smtp_username"];

/// SEC-90 / D-310: the SMTP password may not be carried to a destination the dashboard chose.
/// (1) While Helm supplies the password (`AURORA_SMTP_PASSWORD`), `email.smtp_url` and
/// `email.smtp_username` cannot be changed from the dashboard (409 `setting_locked`).
/// (2) `{ "$secret": "keep" }` on the password is refused (422 `secret_reentry_required`) when
/// the URL or the user name differs from the stored one.
fn email_password_binding(
    h: &SettingsHandle,
    old: &Map<String, Value>,
    norm: &Map<String, Value>,
) -> Result<(), ApiError> {
    let changed = |key: &str| {
        !h.source.sets(key)
            && norm.get(key).unwrap_or(&Value::Null) != old.get(key).unwrap_or(&Value::Null)
    };
    if h.source.secret_env("email.smtp_password").is_some() {
        for key in SMTP_CONNECTION_KEYS {
            if changed(key) {
                return Err(locked(key, "api.extraEnv: AURORA_SMTP_PASSWORD"));
            }
        }
    }
    let keeps = norm
        .get("email.smtp_password")
        .and_then(|v| v.get("$secret"))
        .and_then(Value::as_str)
        == Some("keep");
    if keeps && SMTP_CONNECTION_KEYS.iter().any(|k| changed(k)) {
        return Err(reentry_required("/email.smtp_password"));
    }
    Ok(())
}

fn has_marker(v: &Value) -> bool {
    if is_secret_marker(v) {
        return true;
    }
    match v {
        Value::Object(m) => m.values().any(has_marker),
        Value::Array(a) => a.iter().any(has_marker),
        _ => false,
    }
}

fn overlay(stored: &mut Stored, plans: &[&Plan]) {
    for p in plans {
        stored.sections.insert(
            p.section.clone(),
            p.new.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        );
    }
}

// ------------------------------------------------------------------------------------ apply

/// SEC-89/SEC-90 (S1): a Vault address chosen in the dashboard may not be a metadata, private,
/// loopback or link-local destination unless `gateway.allowed_private` opens it. Every submitted
/// dashboard entry is judged (Helm entries are never submitted). Runs before the write
/// transaction: DNS must not be awaited while the settings lock is held.
async fn guard_dashboard_addresses(
    state: &AppState,
    reqs: &[SectionRequest],
) -> Result<(), ApiError> {
    let guard = aurora_secrets::DialGuard::new(&state.ops().gateway_allowed_private);
    let mut fields = Vec::new();
    for r in reqs.iter().filter(|r| r.section == "secrets") {
        let Some(list) = r.values.get("secrets.stores").and_then(Value::as_array) else {
            continue;
        };
        for e in list {
            let id = e.get("id").and_then(Value::as_str).unwrap_or_default();
            let Some(addr) = e.get("address").and_then(Value::as_str) else {
                continue;
            };
            if !guard.permits_address(addr).await {
                fields.push(field(
                    format!("/secrets.stores/{id}/address"),
                    "destination_refused",
                    "this address is not reachable from here (private, loopback, link-local or metadata); an operator can open it with gateway.allowed_private",
                ));
            }
        }
    }
    // The SMTP server a dashboard names gets the same guard (an operator's `email.smtp_url` is
    // never submitted: a locked key is not judged).
    for r in reqs.iter().filter(|r| r.section == "email") {
        if state.settings.source.sets("email.smtp_url") {
            continue;
        }
        if let Some(url) = r.values.get("email.smtp_url").and_then(Value::as_str)
            && !guard.permits_address(url).await
        {
            fields.push(field(
                "/email.smtp_url".to_string(),
                "destination_refused",
                "this address is not reachable from here (private, loopback, link-local or metadata); an operator can open it with gateway.allowed_private",
            ));
        }
    }
    if fields.is_empty() {
        Ok(())
    } else {
        Err(invalid(fields))
    }
}

/// The harness ids the registered runners report (union of their `capabilities.harnesses`).
pub async fn runner_harnesses(state: &AppState) -> Result<Vec<String>, ApiError> {
    let rows = aurora_db::repo::runners::list(&state.pool)
        .await
        .map_err(dberr)?;
    let mut out: Vec<String> = rows
        .iter()
        .filter_map(|r| r.capabilities.as_ref()?.get("harnesses")?.as_array())
        .flatten()
        .filter_map(|h| h.as_str().map(str::to_owned))
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// SET-5 (C-782): `hosted.enabled_harnesses` may only name harnesses a registered runner reports
/// (422 `harness_not_available`). With no runner registered yet nothing can be judged, so the
/// list is accepted; `fake` is the development harness and is judged by startup validation.
async fn check_harnesses(state: &AppState, reqs: &[SectionRequest]) -> Result<(), ApiError> {
    let mut wanted: Vec<&str> = Vec::new();
    for r in reqs.iter().filter(|r| r.section == "agents") {
        if let Some(list) = r
            .values
            .get("hosted.enabled_harnesses")
            .and_then(Value::as_array)
        {
            wanted.extend(list.iter().filter_map(Value::as_str));
        }
    }
    if wanted.is_empty() || state.settings.source.sets("hosted.enabled_harnesses") {
        return Ok(());
    }
    let known = runner_harnesses(state).await?;
    if known.is_empty() {
        return Ok(());
    }
    let fields: Vec<Value> = wanted
        .into_iter()
        .filter(|h| *h != "fake" && !known.iter().any(|k| k == h))
        .map(|h| {
            field(
                "/hosted.enabled_harnesses".to_string(),
                "harness_not_available",
                format!("no registered runner reports the harness `{h}`"),
            )
        })
        .collect();
    if fields.is_empty() {
        Ok(())
    } else {
        Err(invalid(fields))
    }
}

/// SET-5 (C-782, D-237(d)): turning `terminal.enabled` / `scim.enabled` on while the deployment
/// does not provide the wiring (`terminal.available` / `scim.available` is false) is refused with
/// 422 `setting_requires_deployment`. Only a change to `true` is judged: a switch that is already
/// on (the chart was rolled back) does not block saving the section's other keys.
fn check_deployment(state: &AppState, reqs: &[SectionRequest]) -> Result<(), ApiError> {
    let ops = state.ops();
    for (section, key, available, requires, helm_key) in [
        (
            "terminal",
            "terminal.enabled",
            ops.terminal.available,
            "terminal.available",
            "terminal.enabled",
        ),
        (
            "scim",
            "scim.enabled",
            ops.scim.available,
            "scim.available",
            "scim.enabled",
        ),
    ] {
        let wants_on = reqs
            .iter()
            .filter(|r| r.section == section)
            .any(|r| r.values.get(key).and_then(Value::as_bool) == Some(true));
        let is_on = state
            .settings
            .snapshot()
            .effective
            .keys
            .get(key)
            .and_then(|k| k.value.as_bool())
            == Some(true);
        if wants_on && !is_on && !available {
            return Err(ApiError::unprocessable(
                "setting_requires_deployment",
                "The deployment does not provide this feature's wiring, so it cannot be turned on here.",
            )
            .with_details(json!({ "key": key, "requires": requires, "helm_key": helm_key })));
        }
    }
    Ok(())
}

pub struct SectionRequest {
    pub section: String,
    /// The generation the writer read; `None` for the CLI (the current one under the row lock).
    pub expected: Option<i64>,
    pub values: Map<String, Value>,
}

pub struct Applied {
    pub changed: Vec<(String, i64)>,
}

/// Validate and write one or more sections in **one transaction** (C-773, C-776, C-784).
pub async fn apply(
    state: &AppState,
    via: &Via,
    kind: Kind,
    reqs: Vec<SectionRequest>,
    import: bool,
) -> Result<Applied, ApiError> {
    // Secrets sealed for a write that then fails (stale, invalid, audit down) are discarded at
    // once: nothing references them.
    require_session(via)?;
    guard_dashboard_addresses(state, &reqs).await?;
    super::llm_dest::check_submitted(state, &reqs).await?;
    check_harnesses(state, &reqs).await?;
    check_deployment(state, &reqs)?;
    for r in reqs.iter().filter(|r| r.section == "sign_in") {
        super::sign_in::check_submitted(state, &r.values).await?;
    }
    let mut fresh = Vec::new();
    let r = apply_inner(state, via, kind, reqs, import, &mut fresh).await;
    if r.is_err() && !fresh.is_empty() {
        // The error may have come after COMMIT was sent (outcome unknown): discard only what the
        // stored rows do not reference; when they cannot be read, keep everything (a leaked
        // sealed value is harmless, a discarded live one is not).
        if let Ok(rows) = repo::list_all(&state.pool).await {
            let mut live = Vec::new();
            for row in &rows {
                collect_secrets(&row.value, &mut live);
            }
            fresh.retain(|f| !live.contains(f));
            discard_secrets(state, fresh, std::time::Duration::ZERO).await;
        }
    }
    r
}

/// Discard sealed instance-setting secrets (after `grace` when they were replaced by a committed
/// write: a replica that has not reloaded yet may still open the old one).
async fn discard_secrets(
    state: &AppState,
    secrets: Vec<(Uuid, String)>,
    grace: std::time::Duration,
) {
    if secrets.is_empty() {
        return;
    }
    if !grace.is_zero() {
        tokio::time::sleep(grace).await;
    }
    for (owner, r) in secrets {
        if let Ok(r) = r.parse::<SecretRef>() {
            let ctx = SealContext {
                purpose: Purpose::InstanceSetting,
                owner_id: owner,
                org_id: Uuid::nil(),
                workspace_id: None,
            };
            if let Err(e) = state.vault.discard(&ctx, &r, ValueSource::Managed).await {
                tracing::warn!(error = %e, "settings secret not discarded");
            }
        }
    }
}

async fn apply_inner(
    state: &AppState,
    via: &Via,
    kind: Kind,
    mut reqs: Vec<SectionRequest>,
    import: bool,
    fresh: &mut Vec<(Uuid, String)>,
) -> Result<Applied, ApiError> {
    for r in &reqs {
        section_def(&r.section)?;
    }
    require_session(via)?;
    reqs.sort_by(|a, b| a.section.cmp(&b.section));
    let h = &state.settings;
    let mut tx = audit::begin(state).await?;
    // One settings writer at a time (also against Helm mirroring): the cross-section validation
    // below reads every section, including rows that do not exist yet and cannot be row-locked.
    repo::lock_mirroring(&mut tx).await.map_err(dberr)?;
    let mut plans: Vec<Plan> = Vec::new();
    for r in &reqs {
        let row = repo::get_for_update(&mut tx, &r.section)
            .await
            .map_err(dberr)?;
        let current = row.as_ref().map_or(0, |x| x.generation);
        if r.expected.is_some_and(|e| e != current) {
            return Err(stale(&r.section, current));
        }
        plans.push(
            plan_section(
                state,
                &r.section,
                row.as_ref(),
                &r.values,
                import,
                false,
                fresh,
            )
            .await?,
        );
    }
    let changed: Vec<&Plan> = plans.iter().filter(|p| p.new != p.old).collect();
    if changed.is_empty() {
        return Ok(Applied {
            changed: Vec::new(),
        });
    }
    // The merged view of every section, with the new values in place, must be valid.
    let rows = repo::list_all(&mut *tx).await.map_err(dberr)?;
    let (mut stored, _) = stored_from_rows(&rows);
    overlay(&mut stored, &changed);
    let after = match h.evaluate(&stored) {
        Ok(e) => e,
        Err(problems) => return Err(problems_error(problems)),
    };
    // D-238(b): a save by an admin never strands that admin (the shell is exempt).
    if let Via::Admin(c) = via
        && changed.iter().any(|p| p.section == "sign_in")
    {
        super::sign_in::guard_own_provider(state, c.user_id, &h.snapshot().effective, &after)
            .await?;
    }
    let mut written = Vec::new();
    for p in &changed {
        let g = repo::cas_write(
            &mut tx,
            &p.section,
            p.current,
            &Value::Object(p.new.clone()),
            via.user_id(),
            &via.label(),
        )
        .await
        .map_err(dberr)?
        .ok_or_else(|| stale(&p.section, p.current + 1))?;
        written.push((p.section.clone(), g));
    }
    let actor = via.actor();
    let events: Vec<AuditEvent> = match kind {
        Kind::Import => {
            let mut changes = Vec::new();
            for p in &changed {
                for c in stored_changes(&p.section, &p.old, &p.new) {
                    let mut c = c;
                    c["section"] = json!(p.section);
                    changes.push(c);
                }
            }
            let mut ev = AuditEvent::new(actor, kind.action(), Outcome::Success).metadata(json!({
                "sections": written.iter().map(|(s, g)| json!({ "section": s, "generation": g })).collect::<Vec<_>>(),
                "change": changes,
            }));
            ev.target_kind = Some("instance_settings".into());
            vec![ev]
        }
        _ => changed
            .iter()
            .zip(&written)
            .map(|(p, (_, g))| {
                AuditEvent::new(actor.clone(), kind.action(), Outcome::Success)
                    .target("instance_settings", &p.section)
                    .metadata(json!({
                        "section": p.section,
                        "generation": g,
                        "change": stored_changes(&p.section, &p.old, &p.new),
                    }))
            })
            .collect(),
    };
    audit::commit_critical_many(tx, &events).await?;
    // After commit: this replica's snapshot, the other replicas and gateways, the secrets that
    // were replaced, and the administrators.
    if let Err(e) = h.reload(&state.pool, "api").await {
        tracing::error!(error = %e, "settings reload after a write failed");
    }
    for (section, g) in &written {
        if let Err(e) = repo::notify(&state.pool, section, *g).await {
            tracing::warn!(error = %e, section, "settings notify failed; replicas catch up by poll");
        }
    }
    let mut dropped = Vec::new();
    for p in &changed {
        let (mut before, mut after) = (Vec::new(), Vec::new());
        collect_secrets(&Value::Object(p.old.clone()), &mut before);
        collect_secrets(&Value::Object(p.new.clone()), &mut after);
        dropped.extend(before.into_iter().filter(|b| !after.contains(b)));
    }
    // Replaced secrets go after the migration job's grace: a replica that has not reloaded yet
    // may still open the old one.
    if !dropped.is_empty() {
        let st = state.clone();
        tokio::spawn(async move {
            discard_secrets(&st, dropped, limits::SECRET_MIGRATION_DISCARD_GRACE).await;
        });
    }
    notify_admins(state, via, &changed, &reqs);
    Ok(Applied { changed: written })
}

/// E-mail every instance admin who is not disabled: actor, time and key names only.
fn notify_admins(state: &AppState, via: &Via, changed: &[&Plan], _reqs: &[SectionRequest]) {
    let state = state.clone();
    let actor = via.label();
    let mut titles = Vec::new();
    let mut keys = Vec::new();
    for p in changed {
        if let Ok(s) = section_def(&p.section) {
            titles.push(s.title.to_string());
        }
        for c in stored_changes(&p.section, &p.old, &p.new) {
            if let Some(k) = c.get("key").and_then(Value::as_str) {
                keys.push(k.to_string());
            }
        }
    }
    keys.sort();
    keys.dedup();
    tokio::spawn(async move {
        let emails = match repo::admin_emails(&state.pool).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "settings admin e-mail: recipients not read");
                return;
            }
        };
        let subject = format!("Aurora: instance settings changed ({})", titles.join(", "));
        let intro = format!(
            "{actor} changed instance settings at {}.\n\nChanged: {}.\n\nThe before and after values are in the audit log:",
            Utc::now().format("%Y-%m-%d %H:%M UTC"),
            keys.join(", ")
        );
        let link = format!(
            "{}/admin/audit?action=instance_settings",
            state.config.external_url.trim_end_matches('/')
        );
        for to in emails {
            crate::domains::auth::mail::send_detached(
                &state.mailer,
                Mail::new(&to, &subject, &intro, &link),
            );
        }
    });
}

// ---------------------------------------------------------------------------------- preview

/// The merged view a section's proposed values would produce (nothing sealed, nothing written):
/// the test e-mail's input.
pub async fn proposed_effective(
    state: &AppState,
    section: &str,
    values: &Map<String, Value>,
) -> Result<Effective, ApiError> {
    let rows = repo::list_all(&state.pool).await.map_err(dberr)?;
    let row = rows.iter().find(|r| r.section == section);
    let plan = plan_section(state, section, row, values, false, true, &mut Vec::new()).await?;
    let (stored, _) = stored_from_rows(&rows);
    let mut after = stored.clone();
    overlay(&mut after, &[&plan]);
    state.settings.evaluate(&after).map_err(problems_error)
}

pub async fn preview(
    state: &AppState,
    section: &str,
    generation: i64,
    values: &Map<String, Value>,
    caller: Option<Uuid>,
) -> Result<SettingsPreview, ApiError> {
    section_def(section)?;
    let h = &state.settings;
    let rows = repo::list_all(&state.pool).await.map_err(dberr)?;
    let row = rows.iter().find(|r| r.section == section);
    let current = row.map_or(0, |r| r.generation);
    if generation != current {
        return Err(stale(section, current));
    }
    let plan = plan_section(state, section, row, values, false, true, &mut Vec::new()).await?;
    let (stored_before, _) = stored_from_rows(&rows);
    let mut stored_after = stored_before.clone();
    overlay(&mut stored_after, &[&plan]);
    let before = h.source.effective(&stored_before).map_err(|e| {
        problems_error(
            e.0.into_iter()
                .map(|e| Problem::new(e.key, "invalid", e.message))
                .collect(),
        )
    })?;
    let after = match h.evaluate(&stored_after) {
        Ok(e) => e,
        Err(p) => return Err(problems_error(p)),
    };
    let (diff, mut loosening) = effective_diff(section, &before, &after, &plan.old, &plan.new);
    if section == "sign_in" {
        loosening.extend(super::sign_in::entry_loosening(&before, &after));
    }
    let (mut affected_orgs, affected_orgs_total, mut warnings) =
        super::sections::affected(state, section, &diff, &before, &after, caller).await?;
    // At most SETTINGS_PREVIEW_ORGS_MAX are listed; `affected_orgs_total` counts the rest.
    affected_orgs.truncate(limits::SETTINGS_PREVIEW_ORGS_MAX);
    warnings.extend(plan.warnings);
    Ok(SettingsPreview {
        generation: current,
        diff,
        loosening,
        affected_orgs,
        affected_orgs_total,
        warnings,
    })
}

// ------------------------------------------------------------------------------------ views

fn schema_of(ty: &SettingType) -> SettingSchema {
    let mut s = SettingSchema {
        kind: String::new(),
        min: None,
        max: None,
        max_len: None,
        options: None,
        item_kind: None,
    };
    match ty {
        SettingType::Bool => s.kind = "bool".into(),
        SettingType::Int { min, max } => {
            s.kind = "int".into();
            s.min = Some(*min);
            s.max = Some(*max);
        }
        SettingType::Float { min, max } => {
            // Whole bounds only (the schema's bounds are integers); the exact ones are checked.
            s.kind = "float".into();
            s.min = Some(min.floor() as i64);
            s.max = Some(max.ceil() as i64);
        }
        SettingType::DurationSecs { min, max } => {
            s.kind = "duration_secs".into();
            s.min = Some(*min);
            s.max = Some(*max);
        }
        SettingType::Str { max_len } => {
            s.kind = "string".into();
            s.max_len = Some(i64::try_from(*max_len).unwrap_or(i64::MAX));
        }
        SettingType::Enum(o) => {
            s.kind = "enum".into();
            s.options = Some(o.iter().map(|x| (*x).to_string()).collect());
        }
        SettingType::List { item, max_items } => {
            s.kind = "list".into();
            s.max = Some(i64::try_from(*max_items).unwrap_or(i64::MAX));
            s.item_kind = Some(schema_of(item).kind);
        }
        SettingType::Object => s.kind = "object".into(),
    }
    s
}

/// Sections whose keys are not editable in the dashboard yet: they answer `editable: false` and
/// their running values (C-973). Empty since SET-4/5 registered every section; a section with no
/// registered key is read-only too.
const READ_ONLY_SECTIONS: &[&str] = &[];

fn section_read_only(section: &str) -> bool {
    READ_ONLY_SECTIONS.contains(&section) || registry().section(section).next().is_none()
}

/// A field in words as C-972.
pub fn field_info(m: &aurora_config::FieldMeta) -> FieldInfo {
    FieldInfo {
        key: m.key.into(),
        label: m.label.into(),
        help: m.help.into(),
        kind: m.kind.as_str().into(),
        choices: (!m.choices.is_empty()).then(|| {
            m.choices
                .iter()
                .map(|c| FieldChoice {
                    value: c.value.into(),
                    label: c.label.into(),
                    description: c.description.into(),
                })
                .collect()
        }),
        default_display: m.default_display.into(),
        unit: m.unit.map(str::to_string),
        doc: m.doc.map(|d| FieldDoc {
            title: d.title.into(),
            path: d.path.into(),
        }),
    }
}

/// The running values of a section in words, read-only (C-973). A secret key reads only "Set" or
/// "Not set": its value is never looked at.
fn current_values(eff: &Effective, stored: &Stored, section: &str) -> Vec<CurrentValue> {
    let mut out = Vec::new();
    for d in registry().section(section) {
        let (Some(m), Some(k)) = (aurora_config::field(d.key), eff.keys.get(d.key)) else {
            continue;
        };
        let set = d.secret && stored.get(section, d.key).is_some_and(is_stored_secret);
        let value = if d.secret { &Value::Null } else { &k.value };
        out.push(CurrentValue {
            key: d.key.into(),
            label: m.label.into(),
            value_display: aurora_config::value_words(m, value, set),
            source: if k.source == Source::Helm {
                "helm"
            } else {
                "default"
            }
            .into(),
        });
    }
    out
}

/// A section as C-772 from an effective view and the stored values.
pub fn build_view(
    eff: &Effective,
    stored: &Stored,
    meta: &BTreeMap<String, RowMeta>,
    section: &str,
    extra: Option<Value>,
) -> Result<SettingsSection, ApiError> {
    build_view_as(
        eff,
        stored,
        meta,
        section,
        extra,
        section_read_only(section),
    )
}

fn build_view_as(
    eff: &Effective,
    stored: &Stored,
    meta: &BTreeMap<String, RowMeta>,
    section: &str,
    extra: Option<Value>,
    read_only: bool,
) -> Result<SettingsSection, ApiError> {
    let def = section_def(section)?;
    let m = meta.get(section);
    let mut settings = Vec::new();
    for d in registry().section(section) {
        let Some(k) = eff.keys.get(d.key) else {
            continue;
        };
        let stored_v = stored.get(section, d.key);
        let secret = d.secret.then(|| SecretState {
            set: stored_v.is_some_and(is_stored_secret),
            updated_at: stored_v
                .and_then(|v| v["$secret"].get("updated_at"))
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok()),
            from_env: None,
        });
        let entries = k.entries.as_ref().map(|es| {
            es.iter()
                .map(|e| SettingEntry {
                    id: e.id.clone(),
                    value: public_secrets(&e.value),
                    source: src(e.source),
                    locked: e.source == Source::Helm,
                    warnings: None,
                })
                .collect()
        });
        settings.push(Setting {
            key: d.key.into(),
            value: if d.secret {
                Value::Null
            } else {
                public_secrets(&k.value)
            },
            source: src(k.source),
            locked: k.entries.is_none() && k.source == Source::Helm,
            helm_key: d.helm_key.clone(),
            default: d.default.clone(),
            schema: schema_of(&d.ty),
            list: d.list.map(|l| match l {
                ListKind::Whole => "whole".to_string(),
                ListKind::Keyed { .. } => "keyed".to_string(),
            }),
            id_field: id_field_of(d).map(str::to_string),
            entries,
            secret,
        });
    }
    Ok(SettingsSection {
        section: section.into(),
        title: def.title.into(),
        description: def.description.into(),
        generation: m.map_or(0, |m| m.generation),
        updated_at: m.map(|m| m.updated_at),
        updated_by_label: m.and_then(|m| m.updated_by_label.clone()),
        fields: aurora_config::section_fields(section)
            .into_iter()
            .map(field_info)
            .collect(),
        editable: !read_only,
        current: read_only.then(|| current_values(eff, stored, section)),
        settings,
        extra,
    })
}

/// (C-813, D-262) A dashboard endpoint whose host resolves into a refused range that
/// `gateway.allowed_private` does not cover is marked `needs_allowed_private`: the gateway will
/// refuse it. Helm endpoints are never marked (they pass the guard by themselves).
async fn mark_private_endpoints(view: &mut SettingsSection, allowed_private: &[String]) {
    let Some(entries) = view
        .settings
        .iter_mut()
        .find(|s| s.key == "llm.endpoints")
        .and_then(|s| s.entries.as_mut())
    else {
        return;
    };
    let dashboard: Vec<(String, Value)> = entries
        .iter()
        .filter(|e| e.source == "dashboard")
        .map(|e| (e.id.clone(), e.value.clone()))
        .collect();
    let needing = super::llm_dest::needing_allowed_private(&dashboard, allowed_private).await;
    for e in entries.iter_mut().filter(|e| needing.contains(&e.id)) {
        e.warnings = Some(vec!["needs_allowed_private".into()]);
    }
}

pub async fn section_view(
    state: &AppState,
    section: &str,
    caller: Option<Uuid>,
) -> Result<SettingsSection, ApiError> {
    section_def(section)?;
    let snap = state.settings.snapshot();
    let extra = super::sections::extra(state, section, caller).await?;
    let mut view = build_view(&snap.effective, &snap.stored, &snap.meta, section, extra)?;
    if section == "llm" {
        mark_private_endpoints(&mut view, &snap.effective.raw.gateway.allowed_private).await;
    }
    // A secret the environment supplies (`AURORA_SMTP_PASSWORD`) is locked and says so (C-772).
    for s in &mut view.settings {
        if let (Some(secret), Some(var)) =
            (s.secret.as_mut(), state.settings.source.secret_env(&s.key))
        {
            secret.from_env = Some(var.to_owned());
            secret.set = true;
            s.locked = true;
            s.source = "helm".into();
            if let Some(row) = view
                .current
                .as_mut()
                .and_then(|c| c.iter_mut().find(|r| r.key == s.key))
            {
                row.value_display = "Set".into();
                row.source = "helm".into();
            }
        }
    }
    Ok(view)
}

pub fn index(state: &AppState) -> SettingsIndex {
    let snap = state.settings.snapshot();
    let mut out = Vec::new();
    for s in sections() {
        let (mut total, mut lockedn, mut dash) = (0, 0, 0);
        for d in registry().section(s.id) {
            total += 1;
            if let Some(k) = snap.effective.keys.get(d.key) {
                let helm = match &k.entries {
                    Some(es) => es.iter().any(|e| e.source == Source::Helm),
                    None => k.source == Source::Helm,
                };
                if helm {
                    lockedn += 1;
                } else if k.source == Source::Dashboard {
                    dash += 1;
                }
            }
        }
        let m = snap.meta.get(s.id);
        out.push(SectionSummary {
            section: s.id.into(),
            title: s.title.into(),
            description: s.description.into(),
            generation: m.map_or(0, |m| m.generation),
            updated_at: m.map(|m| m.updated_at),
            updated_by_label: m.and_then(|m| m.updated_by_label.clone()),
            keys_total: total,
            keys_locked: lockedn,
            keys_dashboard: dash,
            fields: aurora_config::section_fields(s.id)
                .into_iter()
                .map(field_info)
                .collect(),
        });
    }
    let locked = super::handle::locked_keys(&state.settings)
        .into_iter()
        .map(|(section, key, entry_id, helm_key)| LockedKey {
            section: section.into(),
            key: key.into(),
            entry_id,
            helm_key,
        })
        .collect();
    SettingsIndex {
        sections: out,
        locked,
    }
}

// ------------------------------------------------------------------------- dashboard values

/// The dashboard-owned values of a section (what a round trip through PUT would send): locked
/// keys and Helm entries left out, secrets as `{ "$secret": <word> }`.
pub fn dashboard_values(
    h: &SettingsHandle,
    stored: &Map<String, Value>,
    section: &str,
    secret_word: &str,
) -> Map<String, Value> {
    let mut out = Map::new();
    for d in registry().section(section) {
        let Some(v) = stored.get(d.key) else { continue };
        if let Some(f) = id_field_of(d) {
            let helm_ids = helm_entry_ids(h, d);
            let list: Vec<Value> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| {
                    !e.get(f)
                        .and_then(Value::as_str)
                        .is_some_and(|i| helm_ids.iter().any(|x| x == i))
                })
                .map(|e| secrets_as(e, secret_word))
                .collect();
            if !list.is_empty() {
                out.insert(d.key.to_string(), Value::Array(list));
            }
        } else if !d.secret && h.source.sets(d.key) {
            continue;
        } else {
            out.insert(d.key.to_string(), secrets_as(v, secret_word));
        }
    }
    out
}

// ---------------------------------------------------------------------------------- export

pub fn export_document(state: &AppState, only: Option<&[String]>) -> Result<Value, ApiError> {
    let h = &state.settings;
    let snap = h.snapshot();
    let mut secs = Map::new();
    for s in sections() {
        if only.is_some_and(|o| !o.iter().any(|x| x == s.id)) {
            continue;
        }
        let stored: Map<String, Value> = snap
            .stored
            .sections
            .get(s.id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let vals = dashboard_values(h, &stored, s.id, "set");
        if !vals.is_empty() {
            secs.insert(s.id.to_string(), Value::Object(vals));
        }
    }
    if let Some(o) = only {
        for id in o {
            section_def(id)?;
        }
    }
    let locked: Vec<Value> = super::handle::locked_keys(h)
        .into_iter()
        .map(|(section, key, _, helm_key)| json!({ "section": section, "key": key, "helm_key": helm_key }))
        .collect();
    Ok(json!({
        "aurora_settings": 1,
        "exported_at": Utc::now(),
        "sections": secs,
        "locked": locked,
    }))
}

// ---------------------------------------------------------------------------------- import

pub struct ImportPlan {
    pub reqs: Vec<SectionRequest>,
}

fn parse_document(doc: &str, format: &str) -> Result<Value, ApiError> {
    let v: Value = match format {
        "json" => serde_json::from_str(doc).map_err(|e| bad_doc(&e.to_string()))?,
        "yaml" => serde_yaml::from_str(doc).map_err(|e| bad_doc(&e.to_string()))?,
        _ => return Err(bad_doc("format must be `yaml` or `json`")),
    };
    if v.get("aurora_settings").and_then(Value::as_i64) != Some(1) {
        return Err(bad_doc(
            "not an Aurora settings document (aurora_settings: 1 expected)",
        ));
    }
    Ok(v)
}

/// The path of the first `{ "$secret": "<plaintext>" }` (anything but `set` / `keep`), if any.
fn plaintext_secret(v: &Value, path: &str) -> Option<String> {
    if is_secret_marker(v) {
        return match v["$secret"].as_str() {
            Some("set" | "keep") => None,
            _ => Some(path.to_string()),
        };
    }
    match v {
        Value::Object(m) => m
            .iter()
            .find_map(|(k, c)| plaintext_secret(c, &format!("{path}/{k}"))),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, c)| plaintext_secret(c, &format!("{path}/{i}"))),
        _ => None,
    }
}

fn bad_doc(msg: &str) -> ApiError {
    ApiError::invalid_request(
        "The settings document could not be read.",
        vec![aurora_core::FieldError::new("/document", msg)],
    )
}

/// The per-section requests of an import document (C-776).
pub fn import_requests(
    state: &AppState,
    doc: &str,
    format: &str,
    merge: bool,
    generations: Option<&Map<String, Value>>,
) -> Result<Vec<SectionRequest>, ApiError> {
    let v = parse_document(doc, format)?;
    let h = &state.settings;
    let snap = h.snapshot();
    let mut reqs = Vec::new();
    let Some(secs) = v.get("sections").and_then(Value::as_object) else {
        return Err(bad_doc("`sections` is missing"));
    };
    // Secrets are entered on the page: a plaintext one anywhere in a document refuses it (C-776).
    if let Some(key) = plaintext_secret(&Value::Object(secs.clone()), "") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "secret_in_import",
            "Secrets are entered on the page, not imported.",
        )
        .with_details(json!({ "key": key })));
    }
    for (section, vals) in secs {
        section_def(section).map_err(|_| bad_doc(&format!("unknown section `{section}`")))?;
        let Some(vals) = vals.as_object() else {
            return Err(bad_doc(&format!("section `{section}` must be an object")));
        };
        let mut values = if merge {
            let stored: Map<String, Value> = snap
                .stored
                .sections
                .get(section.as_str())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            dashboard_values(h, &stored, section, "keep")
        } else {
            Map::new()
        };
        for (k, x) in vals {
            let x = if is_secret_marker(x) && x["$secret"].as_str() == Some("set") {
                json!({ "$secret": "keep" })
            } else {
                x.clone()
            };
            values.insert(k.clone(), x);
        }
        let expected = generations
            .and_then(|g| g.get(section.as_str()))
            .and_then(Value::as_i64);
        reqs.push(SectionRequest {
            section: section.clone(),
            expected,
            values,
        });
    }
    Ok(reqs)
}

/// Dry run of an import: each section's diff, loosening and warnings, and the errors per section.
pub async fn import_dry_run(state: &AppState, reqs: &[SectionRequest]) -> ImportResult {
    let rows = repo::list_all(&state.pool).await.unwrap_or_default();
    let mut sections_out = Vec::new();
    let mut errors = Vec::new();
    for r in reqs {
        let current = rows
            .iter()
            .find(|x| x.section == r.section)
            .map_or(0, |x| x.generation);
        let pv = match preview_import(state, &r.section, current, &r.values).await {
            Ok(p) => p,
            Err(e) => {
                errors.push(ImportError {
                    section: r.section.clone(),
                    code: e.code().into(),
                    message: e.message().into(),
                    details: e.details().cloned(),
                });
                continue;
            }
        };
        sections_out.push(ImportSection {
            section: r.section.clone(),
            generation: current,
            diff: pv.diff,
            loosening: pv.loosening,
            warnings: pv.warnings,
        });
    }
    ImportResult {
        applied: false,
        sections: sections_out,
        errors,
    }
}

async fn preview_import(
    state: &AppState,
    section: &str,
    generation: i64,
    values: &Map<String, Value>,
) -> Result<SettingsPreview, ApiError> {
    // The same computation as a preview, with the import's normalisation (locked keys ignored with a warning).
    let h = &state.settings;
    let rows = repo::list_all(&state.pool).await.map_err(dberr)?;
    let row = rows.iter().find(|r| r.section == section);
    let plan = plan_section(state, section, row, values, true, true, &mut Vec::new()).await?;
    let (stored_before, _) = stored_from_rows(&rows);
    let mut stored_after = stored_before.clone();
    overlay(&mut stored_after, &[&plan]);
    let before = h.source.effective(&stored_before).map_err(|e| {
        problems_error(
            e.0.into_iter()
                .map(|e| Problem::new(e.key, "invalid", e.message))
                .collect(),
        )
    })?;
    let after = h.evaluate(&stored_after).map_err(problems_error)?;
    let (diff, mut loosening) = effective_diff(section, &before, &after, &plan.old, &plan.new);
    if section == "sign_in" {
        loosening.extend(super::sign_in::entry_loosening(&before, &after));
    }
    Ok(SettingsPreview {
        generation,
        diff,
        loosening,
        affected_orgs: Vec::new(),
        affected_orgs_total: 0,
        warnings: plan.warnings,
    })
}

// ------------------------------------------------------------------------------------ reset

/// Remove dashboard values: the whole section, one key, or one keyed entry (C-784).
pub async fn reset(
    state: &AppState,
    via: &Via,
    section: &str,
    key: Option<&str>,
    entry: Option<&str>,
    dry_run: bool,
) -> Result<Vec<Value>, ApiError> {
    section_def(section)?;
    let h = &state.settings;
    let rows = repo::list_all(&state.pool).await.map_err(dberr)?;
    let stored_all = stored_from_rows(&rows).0;
    let stored: Map<String, Value> = stored_all
        .sections
        .get(section)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut values = dashboard_values(h, &stored, section, "keep");
    match (key, entry) {
        (None, None) => values.clear(),
        (Some(k), None) => {
            let d = registry()
                .get(k)
                .filter(|d| d.section == section)
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "setting_unknown",
                        format!("`{k}` is not a setting of section `{section}`."),
                    )
                    .with_details(json!({ "key": k }))
                })?;
            if !d.secret && id_field_of(d).is_none() && h.source.sets(k) {
                return Err(locked(k, &d.helm_key));
            }
            values.remove(k);
        }
        (Some(k), Some(id)) => {
            let d = registry()
                .get(k)
                .filter(|d| d.section == section)
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "setting_unknown",
                        format!("`{k}` is not a setting of section `{section}`."),
                    )
                    .with_details(json!({ "key": k }))
                })?;
            let f = id_field_of(d).ok_or_else(|| {
                ApiError::invalid_request(
                    "`--entry` needs a keyed list.",
                    vec![aurora_core::FieldError::new(
                        format!("/{k}"),
                        "not a keyed list",
                    )],
                )
            })?;
            if helm_entry_ids(h, d).iter().any(|x| x == id) {
                return Err(locked(k, &d.helm_key));
            }
            if let Some(Value::Array(a)) = values.get_mut(k) {
                a.retain(|e| e.get(f).and_then(Value::as_str) != Some(id));
            }
        }
        (None, Some(_)) => {
            return Err(ApiError::invalid_request(
                "`--entry` needs `--key`.",
                vec![aurora_core::FieldError::new("/entry", "needs --key")],
            ));
        }
    }
    let req = SectionRequest {
        section: section.to_string(),
        expected: None,
        values,
    };
    if dry_run {
        let row = rows.iter().find(|r| r.section == section);
        let plan = plan_section(
            state,
            section,
            row,
            &req.values,
            false,
            true,
            &mut Vec::new(),
        )
        .await?;
        return Ok(stored_changes(section, &plan.old, &plan.new));
    }
    let applied = apply(state, via, Kind::Reset, vec![req], false).await?;
    Ok(applied
        .changed
        .iter()
        .map(|(s, g)| json!({ "section": s, "generation": g }))
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use aurora_config::ConfigSource;

    /// C-972, C-973: a section that is not editable answers its running values in words, and a
    /// secret key reads only "Set" or "Not set".
    #[test]
    fn a_read_only_section_lists_current_values_in_words_and_never_echoes_a_secret() {
        let eff = ConfigSource::empty().effective(&Stored::default()).unwrap();
        let mut stored = Stored::default();
        stored.sections.entry("email".into()).or_default().insert(
            "email.smtp_password".into(),
            json!({ "$secret": { "ref": "inline:abc", "plaintext": "hunter2" } }),
        );
        let view = build_view_as(&eff, &stored, &BTreeMap::new(), "email", None, true).unwrap();
        assert!(!view.editable);
        let current = view
            .current
            .expect("read-only sections carry current values");
        let by = |k: &str| current.iter().find(|c| c.key == k).unwrap();
        assert_eq!(by("email.smtp_password").value_display, "Set");
        assert_eq!(by("email.smtp_url").value_display, "Not set");
        assert_eq!(by("email.from").source, "default");
        assert!(!serde_json::to_string(&current).unwrap().contains("hunter2"));
        // An editable section carries `fields` for every setting and no `current`.
        let editable =
            build_view_as(&eff, &stored, &BTreeMap::new(), "approvals", None, false).unwrap();
        assert!(editable.editable && editable.current.is_none());
        assert_eq!(editable.fields.len(), editable.settings.len());
        assert_eq!(
            editable.fields[0].label,
            "How long a question waits for an answer"
        );
    }
}

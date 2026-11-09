//! `instance_setup` fields of a managed connector (docs/13 §3, §3.1): parsing the manifest's
//! field declarations and validating, merging and defaulting settings against them.
//!
//! A field is the `09 §3.4` form-field schema (`name`, `label`, `type`, `required`, `placeholder`,
//! `help_url`, `options`) plus the types `url`, `number` (optional `min`/`max`) and
//! `generated_secret`, and an optional static `default`. `file`/`json_file` and `store` are not
//! valid here.

use aurora_core::{FieldError, sanitize};
use aurora_protocol::connector::ConnectorManifest;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Per-secret size cap (C-459).
pub const SECRET_MAX_BYTES: usize = 8 * 1024;
/// Whole settings object cap (C-459).
pub const SETTINGS_MAX_BYTES: usize = 16 * 1024;
const TEXT_MAX: usize = 4096;
const URL_MAX: usize = 2048;
const MAX_FIELDS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Url,
    Number,
    Select,
    Secret,
    GeneratedSecret,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    pub required: bool,
    pub default: Option<Value>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub options: Vec<String>,
}

impl Field {
    pub fn is_secret(&self) -> bool {
        matches!(self.kind, Kind::Secret | Kind::GeneratedSecret)
    }
}

fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn parse_field(v: &Value) -> Result<Field, String> {
    let o = v.as_object().ok_or("a field must be an object")?;
    let name = o
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| valid_name(n))
        .ok_or("a field needs a `name` matching [a-z][a-z0-9_]{0,63}")?
        .to_owned();
    let kind = match o.get("type").and_then(Value::as_str).unwrap_or("text") {
        "text" => Kind::Text,
        "url" => Kind::Url,
        "number" => Kind::Number,
        "select" => Kind::Select,
        "secret" => Kind::Secret,
        "generated_secret" => Kind::GeneratedSecret,
        other => {
            return Err(format!(
                "field `{name}`: type `{}` is not allowed in instance_setup",
                sanitize::one_line(other, 32)
            ));
        }
    };
    if o.contains_key("store") {
        return Err(format!(
            "field `{name}`: `store` is not allowed in instance_setup"
        ));
    }
    let options = o
        .get("options")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("value").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let f = Field {
        name,
        kind,
        required: o.get("required").and_then(Value::as_bool).unwrap_or(false),
        default: o.get("default").filter(|d| !d.is_null()).cloned(),
        min: o.get("min").and_then(Value::as_i64),
        max: o.get("max").and_then(Value::as_i64),
        options,
    };
    if f.is_secret() && f.default.is_some() {
        return Err(format!(
            "field `{}`: a secret field cannot have a default",
            f.name
        ));
    }
    if f.kind == Kind::Select && f.options.is_empty() {
        return Err(format!("field `{}`: a select field needs options", f.name));
    }
    if let Some(d) = &f.default {
        let mut errs = Vec::new();
        check_value(&f, d, "/default", &mut errs);
        if !errs.is_empty() {
            return Err(format!("field `{}`: its default is not valid", f.name));
        }
    }
    Ok(f)
}

/// The `instance_setup` fields of a manifest; an error names the first invalid field.
pub fn parse_fields(m: &ConnectorManifest) -> Result<Vec<Field>, String> {
    let Some(setup) = &m.instance_setup else {
        return Ok(Vec::new());
    };
    if setup.fields.len() > MAX_FIELDS {
        return Err("too many instance_setup fields".to_owned());
    }
    let mut out: Vec<Field> = Vec::new();
    for v in &setup.fields {
        let f = parse_field(v)?;
        if out.iter().any(|g| g.name == f.name) {
            return Err(format!("duplicate field `{}`", f.name));
        }
        out.push(f);
    }
    Ok(out)
}

fn clean_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
}

fn check_value(f: &Field, v: &Value, path: &str, errs: &mut Vec<FieldError>) {
    let mut bad = |msg: &str| errs.push(FieldError::new(path, msg));
    match f.kind {
        Kind::Text => match v.as_str() {
            Some(s) if clean_text(s, TEXT_MAX) => {}
            _ => bad("must be non-empty text without control characters"),
        },
        Kind::Url => match v.as_str() {
            Some(s) if clean_text(s, URL_MAX) => match reqwest::Url::parse(s) {
                Ok(u)
                    if matches!(u.scheme(), "http" | "https")
                        && u.host_str().is_some()
                        && u.username().is_empty()
                        && u.password().is_none() => {}
                _ => bad("must be an http or https URL without credentials"),
            },
            _ => bad("must be an http or https URL"),
        },
        Kind::Number => match v.as_i64() {
            Some(n) => {
                if f.min.is_some_and(|m| n < m) || f.max.is_some_and(|m| n > m) {
                    bad(&format!(
                        "must be between {} and {}",
                        f.min.map_or("-∞".to_owned(), |m| m.to_string()),
                        f.max.map_or("∞".to_owned(), |m| m.to_string())
                    ));
                }
            }
            None => bad("must be an integer"),
        },
        Kind::Select => match v.as_str() {
            Some(s) if f.options.iter().any(|o| o == s) => {}
            _ => bad("must be one of the options"),
        },
        Kind::Secret | Kind::GeneratedSecret => bad("is a secret field"),
    }
}

/// Validates the non-secret settings an admin supplied (create, or the merged result of a PATCH
/// where `null` has already removed keys). Keys must be non-secret fields; every required field
/// needs a value or a default. The stored object holds supplied values only (no defaults).
pub fn validate_settings(
    fields: &[Field],
    settings: &Map<String, Value>,
    errs: &mut Vec<FieldError>,
) {
    for (k, v) in settings {
        let path = format!("/settings/{k}");
        match fields.iter().find(|f| &f.name == k) {
            None => errs.push(FieldError::new(path, "unknown setting")),
            Some(f) if f.is_secret() => {
                errs.push(FieldError::new(
                    path,
                    "is a secret field; send it in `secrets`",
                ));
            }
            Some(f) => check_value(f, v, &path, errs),
        }
    }
    for f in fields.iter().filter(|f| !f.is_secret()) {
        if f.required && f.default.is_none() && !settings.contains_key(&f.name) {
            errs.push(FieldError::new(format!("/settings/{}", f.name), "required"));
        }
    }
    if serde_json::to_vec(settings).map_or(0, |b| b.len()) > SETTINGS_MAX_BYTES {
        errs.push(FieldError::new("/settings", "too large (max 16 KiB)"));
    }
}

/// Validates secret values an admin supplied: keys must be `secret` fields, values non-empty and
/// at most 8 KiB. A supplied `generated_secret` is always an error.
pub fn validate_secrets<'a>(
    fields: &[Field],
    secrets: impl IntoIterator<Item = (&'a String, &'a str)>,
    errs: &mut Vec<FieldError>,
) {
    for (k, v) in secrets {
        let path = format!("/secrets/{k}");
        match fields.iter().find(|f| &f.name == k) {
            None => errs.push(FieldError::new(path, "unknown secret")),
            Some(f) if f.kind == Kind::GeneratedSecret => errs.push(FieldError::new(
                path,
                "is generated by Aurora and cannot be supplied",
            )),
            Some(f) if f.kind != Kind::Secret => {
                errs.push(FieldError::new(path, "is not a secret field"));
            }
            Some(_) if v.is_empty() => errs.push(FieldError::new(path, "must not be empty")),
            Some(_) if v.len() > SECRET_MAX_BYTES => {
                errs.push(FieldError::new(path, "too large (max 8 KiB)"));
            }
            Some(_) if v.contains('\0') => {
                errs.push(FieldError::new(path, "must not contain NUL"));
            }
            Some(_) => {}
        }
    }
}

/// Settings with static defaults applied for unset fields.
pub fn with_defaults(fields: &[Field], stored: Option<&Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for f in fields.iter().filter(|f| !f.is_secret()) {
        if let Some(d) = &f.default {
            out.insert(f.name.clone(), d.clone());
        }
    }
    if let Some(o) = stored.and_then(Value::as_object) {
        for (k, v) in o {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// `secret_refs` as a map of field name → sealed reference column value.
pub fn refs_of(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

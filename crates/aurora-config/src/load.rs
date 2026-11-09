//! Layered loading: TOML file, `AURORA__SECTION__KEY` overrides, secret env vars.

use crate::error::{ConfigError, ConfigErrors, Result};
use crate::raw::{EnvSecrets, RawConfig};
use crate::redacted::Redacted;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/aurora/aurora.toml";
pub const CONFIG_PATH_ENV: &str = "AURORA_CONFIG";
const OVERRIDE_PREFIX: &str = "AURORA__";

/// Load configuration.
///
/// `path` overrides `AURORA_CONFIG` in `env`, which overrides the default
/// `/etc/aurora/aurora.toml`. A missing file is tolerated only for the default
/// path (env may then supply everything; `validate_for` checks completeness).
pub fn load(path: Option<&Path>, env: impl Iterator<Item = (String, String)>) -> Result<RawConfig> {
    let env: BTreeMap<String, String> = env.collect();
    let (file, explicit) = match path {
        Some(p) => (p.to_path_buf(), true),
        None => match env.get(CONFIG_PATH_ENV).filter(|v| !v.is_empty()) {
            Some(p) => (PathBuf::from(p), true),
            None => (PathBuf::from(DEFAULT_CONFIG_PATH), false),
        },
    };
    let mut errors = Vec::new();

    let mut table = match std::fs::read_to_string(&file) {
        Ok(text) => match text.parse::<toml::Table>() {
            Ok(t) => t,
            Err(e) => {
                return Err(one(file.display().to_string(), describe(&text, &e)));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => toml::Table::new(),
        Err(e) => {
            let key = if path.is_none() && explicit {
                CONFIG_PATH_ENV.to_string()
            } else {
                file.display().to_string()
            };
            return Err(one(key, format!("cannot read {}: {e}", file.display())));
        }
    };

    apply_overrides(&mut table, &env, &mut errors);

    // Round-trip through canonical text so deserialization errors can be
    // mapped back to a dotted key path.
    let text = match toml::to_string(&table) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigError::new("config", e.to_string()));
            return Err(ConfigErrors(errors));
        }
    };
    let mut raw = match toml::from_str::<RawConfig>(&text) {
        Ok(r) => r,
        Err(e) => {
            errors.push(ConfigError::new(locate(&text, &e), e.message().to_string()));
            return Err(ConfigErrors(errors));
        }
    };

    raw.env_secrets = read_env_secrets(&env, &raw, &mut errors);
    if errors.is_empty() {
        Ok(raw)
    } else {
        Err(ConfigErrors(errors))
    }
}

fn one(key: String, message: String) -> ConfigErrors {
    ConfigErrors(vec![ConfigError::new(key, message)])
}

fn describe(text: &str, e: &toml::de::Error) -> String {
    match e.span() {
        Some(s) => {
            let line = text[..s.start.min(text.len())].matches('\n').count() + 1;
            format!("line {line}: {}", e.message())
        }
        None => e.message().to_string(),
    }
}

/// Dotted key path for an error located inside canonical TOML `text`.
fn locate(text: &str, e: &toml::de::Error) -> String {
    let Some(span) = e.span() else {
        return "config".into();
    };
    let start = span.start.min(text.len());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = text[line_start..line_end].trim();
    let header = text[..line_start]
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('['))
        .map(|l| l.trim_matches(|c| c == '[' || c == ']').to_string());
    if line.starts_with('[') {
        return line.trim_matches(|c| c == '[' || c == ']').to_string();
    }
    let key = line
        .split('=')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    match (header, key.is_empty()) {
        (Some(h), false) => format!("{h}.{key}"),
        (Some(h), true) => h,
        (None, false) => key.to_string(),
        (None, true) => "config".into(),
    }
}

fn apply_overrides(
    table: &mut toml::Table,
    env: &BTreeMap<String, String>,
    errors: &mut Vec<ConfigError>,
) {
    for (name, value) in env {
        let Some(rest) = name.strip_prefix(OVERRIDE_PREFIX) else {
            continue;
        };
        let parts: Vec<String> = rest.split("__").map(str::to_ascii_lowercase).collect();
        if parts.iter().any(String::is_empty) {
            errors.push(ConfigError::new(name, "malformed override name"));
            continue;
        }
        let Some((last, parents)) = parts.split_last() else {
            continue;
        };
        if let Err(p) = set_path(table, parents, last, parse_value(value)) {
            errors.push(ConfigError::new(
                name,
                format!("`{p}` is not a table in the configuration file"),
            ));
        }
    }
}

fn set_path(
    table: &mut toml::Table,
    parents: &[String],
    last: &str,
    value: toml::Value,
) -> std::result::Result<(), String> {
    match parents.split_first() {
        None => {
            table.insert(last.to_string(), value);
            Ok(())
        }
        Some((p, rest)) => {
            let slot = table
                .entry(p.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            match slot {
                toml::Value::Table(t) => set_path(t, rest, last, value),
                _ => Err(p.clone()),
            }
        }
    }
}

/// Parse as a TOML value (number, bool, array, inline table, quoted string);
/// anything else is a plain string.
fn parse_value(s: &str) -> toml::Value {
    if let Ok(mut t) = format!("v = {s}").parse::<toml::Table>()
        && t.len() == 1
        && let Some(v) = t.remove("v")
        && !matches!(v, toml::Value::Datetime(_))
    {
        return v;
    }
    toml::Value::String(s.to_string())
}

/// Read `<name>` or `<name>_FILE`. Empty values count as unset.
fn read_secret(
    env: &BTreeMap<String, String>,
    name: &str,
    errors: &mut Vec<ConfigError>,
) -> Option<Redacted<String>> {
    let file_var = format!("{name}_FILE");
    let plain = env.get(name).filter(|v| !v.is_empty());
    let file = env.get(&file_var).filter(|v| !v.is_empty());
    match (plain, file) {
        (Some(_), Some(_)) => {
            errors.push(ConfigError::new(
                name,
                format!("set either {name} or {file_var}, not both"),
            ));
            None
        }
        (Some(v), None) => Some(Redacted::new(v.clone())),
        (None, Some(path)) => match std::fs::read_to_string(path) {
            Ok(content) => {
                let v = content.trim_end_matches(['\n', '\r']);
                (!v.is_empty()).then(|| Redacted::new(v.to_string()))
            }
            Err(e) => {
                errors.push(ConfigError::new(
                    file_var,
                    format!("cannot read {path}: {e}"),
                ));
                None
            }
        },
        (None, None) => None,
    }
}

fn read_env_secrets(
    env: &BTreeMap<String, String>,
    raw: &RawConfig,
    errors: &mut Vec<ConfigError>,
) -> EnvSecrets {
    let mut s = EnvSecrets {
        database_url: read_secret(env, "AURORA_DATABASE_URL", errors),
        encryption_keys: read_secret(env, "AURORA_ENCRYPTION_KEYS", errors),
        encryption_active_key: read_secret(env, "AURORA_ENCRYPTION_ACTIVE_KEY", errors),
        signing_key: read_secret(env, "AURORA_SIGNING_KEY", errors),
        signing_active_key: read_secret(env, "AURORA_SIGNING_ACTIVE_KEY", errors),
        internal_secret: read_secret(env, "AURORA_INTERNAL_SECRET", errors),
        internal_active_key: read_secret(env, "AURORA_INTERNAL_ACTIVE_KEY", errors),
        runner_session_key: read_secret(env, "AURORA_RUNNER_SESSION_KEY", errors),
        runner_token: read_secret(env, "AURORA_RUNNER_TOKEN", errors),
        connector_token: read_secret(env, "AURORA_CONNECTOR_TOKEN", errors),
        bootstrap_runner_token: read_secret(env, "AURORA_BOOTSTRAP_RUNNER_TOKEN", errors),
        bootstrap_connector_tokens: read_secret(env, "AURORA_BOOTSTRAP_CONNECTOR_TOKENS", errors),
        bootstrap_connector_host_token: read_secret(
            env,
            "AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN",
            errors,
        ),
        oidc_client_secrets: BTreeMap::new(),
    };
    for p in &raw.auth.oidc_providers {
        if p.id.is_empty() || p.client_secret_env.is_empty() {
            continue;
        }
        if let Some(v) = read_secret(env, &p.client_secret_env, errors) {
            s.oidc_client_secrets.insert(p.id.clone(), v);
        }
    }
    s
}

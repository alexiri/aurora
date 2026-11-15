//! `aurora-api settings show|reset|export|import` (C-784, D-238(b)): the break-glass path, through
//! the same service as the dashboard (validation, generation, critical audit with source `cli`,
//! notify, admin e-mail), without re-authentication, and working while the stored settings are
//! invalid (it reads the rows directly and never needs a valid merged view to start).

use super::handle::stored_from_rows;
use super::service::{self, Kind, Via};
use crate::app::AppState;
use aurora_config::sections;
use aurora_core::ApiError;
use aurora_db::repo::instance_settings as repo;
use serde_json::{Value, json};

/// Exit code of a refused command: 2 validation, 3 locked key, 4 database.
pub fn exit_code(e: &ApiError) -> u8 {
    match e.code() {
        "setting_locked" => 3,
        "database_unavailable" | "service_unavailable" => 4,
        _ => 2,
    }
}

fn describe(e: &ApiError) -> String {
    match e.details() {
        Some(d) => format!("{}: {} {d}", e.code(), e.message()),
        None => format!("{}: {}", e.code(), e.message()),
    }
}

/// A printable error for stderr.
pub fn render_error(e: &ApiError) -> String {
    describe(e)
}

/// Effective values and sources (secrets as set/unset). Works with invalid stored settings.
pub async fn show(
    state: &AppState,
    section: Option<&str>,
    as_json: bool,
) -> Result<String, ApiError> {
    let rows = repo::list_all(&state.pool).await.map_err(ApiError::from)?;
    let (stored, meta) = stored_from_rows(&rows);
    let eff = state
        .settings
        .source
        .effective(&stored)
        .map_err(|e| ApiError::unprocessable("settings_invalid", e.to_string()))?;
    let ids: Vec<&str> = match section {
        Some(s) => {
            if !sections().iter().any(|x| x.id == s) {
                return Err(service::unknown_section(s));
            }
            vec![s]
        }
        None => sections().iter().map(|s| s.id).collect(),
    };
    let mut views = Vec::new();
    for id in ids {
        views.push(service::build_view(&eff, &stored, &meta, id, None)?);
    }
    if as_json {
        return Ok(serde_json::to_string_pretty(
            &views
                .iter()
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default());
    }
    let mut out = String::new();
    for v in &views {
        out.push_str(&format!("[{}] generation {}\n", v.section, v.generation));
        for s in &v.settings {
            let shown = match &s.secret {
                Some(sec) => {
                    if sec.set {
                        "(secret: set)".to_string()
                    } else {
                        "(secret: unset)".to_string()
                    }
                }
                None => s.value.to_string(),
            };
            let lock = if s.locked {
                format!(", locked by Helm `{}`", s.helm_key)
            } else {
                String::new()
            };
            out.push_str(&format!("  {} = {shown}  ({}{lock})\n", s.key, s.source));
        }
    }
    Ok(out)
}

pub async fn reset(
    state: &AppState,
    section: &str,
    key: Option<&str>,
    entry: Option<&str>,
    dry_run: bool,
) -> Result<String, ApiError> {
    let changes = service::reset(state, &Via::Cli, section, key, entry, dry_run).await?;
    let verb = if dry_run { "would change" } else { "changed" };
    if changes.is_empty() {
        return Ok(format!("nothing to reset in `{section}`\n"));
    }
    Ok(format!(
        "{verb}: {}\n",
        serde_json::to_string(&json!(changes)).unwrap_or_default()
    ))
}

pub fn export(state: &AppState, format: &str, only: Option<&[String]>) -> Result<String, ApiError> {
    let doc = service::export_document(state, only)?;
    Ok(match format {
        "json" => serde_json::to_string_pretty(&doc).unwrap_or_default(),
        _ => serde_yaml::to_string(&doc).unwrap_or_default(),
    })
}

pub async fn import(
    state: &AppState,
    text: &str,
    format: &str,
    merge: bool,
    dry_run: bool,
) -> Result<String, ApiError> {
    let reqs = service::import_requests(state, text, format, merge, None)?;
    if dry_run {
        let r = service::import_dry_run(state, &reqs).await;
        let out = serde_json::to_string_pretty(&r).unwrap_or_default();
        if r.errors.is_empty() {
            return Ok(out);
        }
        let first = &r.errors[0];
        return Err(ApiError::unprocessable(
            "settings_invalid",
            format!(
                "section `{}`: {} ({})\n{out}",
                first.section, first.message, first.code
            ),
        ));
    }
    let applied = service::apply(state, &Via::Cli, Kind::Import, reqs, true).await?;
    Ok(format!(
        "applied: {}\n",
        serde_json::to_string(&json!(applied.changed)).unwrap_or_default()
    ))
}

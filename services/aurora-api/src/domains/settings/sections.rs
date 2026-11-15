//! Per-section hooks of the settings framework (C-772 `extra`, C-774 `affected_orgs`): SET-1
//! gives every section the empty default; SET-2..SET-5 and FIX-124 add their arms here.

use super::dto::{AffectedOrg, DiffItem, SettingWarning};
use crate::app::AppState;
use crate::domains::governance::standing;
use crate::domains::secrets::gov::instance_from_governance;
use aurora_config::Effective;
use aurora_core::{ApiError, limits};
use serde_json::{Value, json};

/// Section-specific read-only data shown beside the settings (`extra`).
pub async fn extra(
    state: &AppState,
    section: &str,
    caller: Option<uuid::Uuid>,
) -> Result<Option<Value>, ApiError> {
    Ok(match section {
        // (C-780) How many providers Helm defines and which one the caller signs in with.
        "sign_in" => Some(super::sign_in::extra(state, caller).await),
        // (C-779) The providers this release can enable: the page offers exactly these.
        "llm" => Some(json!({ "providers": aurora_llm::supported_providers() })),
        // (C-778) Where organizations stand against the instance ceiling.
        "governance" => Some(json!({ "orgs_url": "/v1/admin/settings/governance/orgs" })),
        // (C-782) The runners' sandbox sizes, `max_sandboxes` and harnesses, read-only (D-235(a)).
        "agents" => Some(json!({ "runners": runners(state).await? })),
        _ => None,
    })
}

async fn runners(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let rows = aurora_db::repo::runners::list(&state.pool)
        .await
        .map_err(|_| ApiError::internal("Internal error."))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let c = r.capabilities.unwrap_or(Value::Null);
            let get = |k: &str| c.get(k).cloned().unwrap_or(Value::Null);
            json!({
                "name": r.name,
                "backend": r.backend,
                "max_sandboxes": get("max_sandboxes"),
                "harnesses": get("harnesses"),
                "defaults": get("defaults"),
                "resources": get("resources"),
                "held_awake_ceiling": get("held_awake_ceiling"),
                "helm_keys": "runner.sandbox_cpus, runner.sandbox_memory_mib, runner.sandbox_pids, runner.max_sandboxes",
            })
        })
        .collect())
}

/// The organizations whose effective values change, their total, and section warnings.
/// `before` is the merged view in force, `after` the one the pending change would produce.
pub async fn affected(
    state: &AppState,
    section: &str,
    _diff: &[DiffItem],
    before: &Effective,
    after: &Effective,
    caller: Option<uuid::Uuid>,
) -> Result<(Vec<AffectedOrg>, i64, Vec<SettingWarning>), ApiError> {
    if section == "llm" {
        return affected_llm(state, before, after).await;
    }
    if section == "sign_in" {
        let w = super::sign_in::warnings(state, caller, before, after).await?;
        return Ok((Vec::new(), 0, w));
    }
    if section != "governance" {
        return Ok((Vec::new(), 0, Vec::new()));
    }
    // The endpoints are the registry's (SET-3 owns them); only the ceiling differs.
    let endpoints = state.gov_instance().endpoints.clone();
    let resolve = |e: &Effective| {
        e.raw
            .governance_config()
            .map(|g| instance_from_governance(&g, endpoints.clone()))
            .map_err(|_| ApiError::internal("Internal error."))
    };
    let (b, a) = (resolve(before)?, resolve(after)?);
    let (orgs, total) =
        standing::affected(&state.pool, &b, &a, limits::SETTINGS_PREVIEW_ORGS_MAX).await?;
    Ok((orgs, total, Vec::new()))
}

/// The instance ceiling a merged view would put in force, endpoints included.
fn instance_of(e: &Effective) -> Result<aurora_llm::governance::Instance, ApiError> {
    let g = e
        .raw
        .governance_config()
        .map_err(|_| ApiError::internal("Internal error."))?;
    let registry = aurora_llm::ProviderRegistry::from_config(&e.raw.llm)
        .map_err(|_| ApiError::internal("Internal error."))?;
    Ok(crate::domains::secrets::gov::instance_from_config(
        &g, &registry,
    ))
}

/// `llm` (SET-3): the organizations whose keys stop being usable (`secrets_become_non_permitted`,
/// counts per organization in `affected_orgs`) and the warnings startup validation also logs.
async fn affected_llm(
    state: &AppState,
    before: &Effective,
    after: &Effective,
) -> Result<(Vec<AffectedOrg>, i64, Vec<SettingWarning>), ApiError> {
    let (b, a) = (instance_of(before)?, instance_of(after)?);
    let (orgs, total, keys) =
        standing::keys_affected(&state.pool, &b, &a, limits::SETTINGS_PREVIEW_ORGS_MAX).await?;
    let mut warnings = Vec::new();
    if total > 0 {
        warnings.push(SettingWarning {
            code: "secrets_become_non_permitted".into(),
            key: Some("llm.endpoints".into()),
            message: format!(
                "{keys} LLM key{} in {total} organization{} would stop being usable: their \
                 endpoint is removed or no longer permits them. Owners see them as not permitted \
                 and the gateway stops injecting them.",
                if keys == 1 { "" } else { "s" },
                if total == 1 { "" } else { "s" },
            ),
        });
    }
    // FIX-019: an endpoint without a default model (startup logs the same).
    let known = aurora_config::llm_endpoint_warnings(&before.raw.llm);
    for w in aurora_config::llm_endpoint_warnings(&after.raw.llm)
        .into_iter()
        .filter(|w| !known.contains(w))
    {
        warnings.push(SettingWarning {
            code: "no_default_model".into(),
            key: Some("llm.endpoints".into()),
            message: w,
        });
    }
    Ok((orgs, total, warnings))
}

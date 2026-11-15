//! Cross-section rules on the merged view that live in the settings layer rather than in
//! `aurora-config`'s validation (D-236(d)). The real ones belong to the section briefs (SET-2..5)
//! and are added here; SET-1 carries only the test-section rule that proves a violation names
//! the other key. It only applies where the registry has the test section (tests).

use super::handle::Problem;
use aurora_config::Effective;
use serde_json::Value;

/// One refusal of a keyed-list entry's shape before it is merged: the field (below the entry),
/// the stable code and a message.
pub struct EntryProblem {
    pub field: String,
    pub code: &'static str,
    pub message: String,
}

/// Keys of an endpoint's `key_origin` object (the typed `KeyOrigin`).
const KEY_ORIGIN_KEYS: &[&str] = &[
    "method",
    "allowed_organization_ids",
    "allowed_workspace_ids",
    "allowed_team_ids",
];

/// Shape checks of one entry of a keyed list that must run on the JSON, before it is deserialized
/// into the configuration (SET-3, SEC-89/SEC-90): a key that is not part of an LLM endpoint is
/// refused by name (`credential_in_entry` for anything named like a credential, else
/// `key_unknown`), exactly as at startup — the typed configuration would only say "unknown field".
pub fn entry_problems(key: &str, entry: &Value) -> Vec<EntryProblem> {
    let mut out = Vec::new();
    if key != "llm.endpoints" {
        return out;
    }
    let Some(map) = entry.as_object() else {
        return out;
    };
    let mut refuse = |field: String, code: &'static str, what: &str| {
        let message = if code == "credential_in_entry" {
            format!(
                "`{what}` looks like a credential; credentials are never part of an endpoint (they are LLM keys)"
            )
        } else {
            format!("`{what}` is not an endpoint setting")
        };
        out.push(EntryProblem {
            field,
            code,
            message,
        });
    };
    for k in map.keys() {
        if let Some(code) = aurora_config::unknown_key_code(k) {
            refuse(k.clone(), code, k);
        }
    }
    if let Some(o) = map.get("key_origin").and_then(Value::as_object) {
        for k in o.keys().filter(|k| !KEY_ORIGIN_KEYS.contains(&k.as_str())) {
            let code = aurora_config::unknown_key_code(k).unwrap_or("key_unknown");
            refuse(format!("key_origin/{k}"), code, k);
        }
    }
    out
}

pub fn cross_rules(eff: &Effective) -> Vec<Problem> {
    let mut out = Vec::new();
    let get = |k: &str| eff.keys.get(k).map(|e| &e.value);
    out.extend(llm_rules(eff));
    // FIX-124: a store the dashboard defines is a Vault store (never a test or inline kind) and
    // never takes the implicit `inline` store's id.
    if let Some(entries) = eff
        .keys
        .get("secrets.stores")
        .and_then(|k| k.entries.as_ref())
    {
        for e in entries
            .iter()
            .filter(|e| e.source == aurora_config::Source::Dashboard)
        {
            let kind = e.value.get("kind").and_then(Value::as_str).unwrap_or("");
            if !aurora_config::dashboard_store_kind_allowed(kind) {
                out.push(Problem::new(
                    "secrets.stores",
                    "store_kind_not_allowed",
                    format!(
                        "store `{}`: kind `{kind}` cannot be configured in the dashboard (only `{}`)",
                        e.id,
                        aurora_config::VAULT_KIND
                    ),
                ));
            }
            // SEC-89/SEC-90: no environment-variable or file reference in a dashboard store.
            for (k, m) in aurora_config::dashboard_store_violations(&e.value) {
                out.push(Problem::new(
                    format!("secrets.stores/{}/{}", e.id, k.replace('.', "/")),
                    "dashboard_reference_refused",
                    m,
                ));
            }
            if e.id == "inline" {
                out.push(Problem::new(
                    "secrets.stores",
                    "store_id_reserved",
                    "`inline` is the built-in store's id",
                ));
            }
        }
    }
    if get("settings_test.mode").and_then(Value::as_str) == Some("open")
        && get("settings_test.flag").and_then(Value::as_bool) != Some(true)
    {
        out.push(Problem::new(
            "settings_test.flag",
            "requires_flag",
            "must be on while `settings_test.mode` is `open`",
        ));
    }
    // Test-only rule across two sections (S2: concurrent writes that are only invalid together).
    if get("settings_test.limit").and_then(Value::as_i64) > Some(10)
        && get("settings_test_b.count").and_then(Value::as_i64) > Some(5)
    {
        out.push(Problem::new(
            "settings_test_b.count",
            "conflicts_with_other_section",
            "must be at most 5 while `settings_test.limit` is above 10",
        ));
    }
    out
}

/// The `llm` section's rules on the merged view (SET-3). The per-entry rules (ids, urls, paths,
/// provider keys, key-origin methods, `require_key_origin`) are startup validation's and arrive
/// through `api_config`; here are the rules that need the settings' sources or the provider table.
fn llm_rules(eff: &Effective) -> Vec<Problem> {
    let mut out = Vec::new();
    let llm = &eff.raw.llm;
    let supported = aurora_llm::supported_providers();
    for p in llm
        .enabled_providers
        .iter()
        .filter(|p| !supported.contains(&p.as_str()))
    {
        out.push(Problem::new(
            "llm.enabled_providers",
            "provider_unknown",
            format!(
                "`{p}` is not a provider of this release (available: {})",
                supported.join(", ")
            ),
        ));
    }
    // A preference that names a provider that is not enabled is refused when the dashboard wrote
    // either list; a deployment configuration that already has it keeps working (appended order,
    // `LlmSection::effective_provider_preference`).
    let dashboard = |k: &str| {
        eff.keys
            .get(k)
            .is_some_and(|e| e.source == aurora_config::Source::Dashboard)
    };
    if dashboard("llm.enabled_providers") || dashboard("llm.provider_preference") {
        for p in llm
            .provider_preference
            .iter()
            .filter(|p| !llm.enabled_providers.contains(p))
        {
            out.push(Problem::new(
                "llm.provider_preference",
                "provider_not_enabled",
                format!("`{p}` is in the preference order but is not an enabled provider"),
            ));
        }
    }
    for e in &llm.endpoints {
        if !e.provider.is_empty() && !llm.enabled_providers.contains(&e.provider) {
            out.push(Problem::new(
                format!("llm.endpoints/{}/provider", e.id),
                "provider_not_enabled",
                format!("provider `{}` is not enabled", e.provider),
            ));
        }
    }
    if out.is_empty()
        && let Err(e) = aurora_llm::ProviderRegistry::from_config(llm)
    {
        // The provider table names the endpoint it refuses: "endpoint `<id>`: …".
        let msg = e.to_string();
        let key = msg
            .split_once("endpoint `")
            .and_then(|(_, rest)| rest.split_once('`'))
            .map_or_else(
                || "llm.endpoints".to_owned(),
                |(id, _)| format!("llm.endpoints/{id}"),
            );
        out.push(Problem::new(key, "invalid", msg));
    }
    out
}

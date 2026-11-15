//! Cross-section rules on the merged view that live in the settings layer rather than in
//! `aurora-config`'s validation (D-236(d)). The real ones belong to the section briefs (SET-2..5)
//! and are added here; SET-1 carries only the test-section rule that proves a violation names
//! the other key. It only applies where the registry has the test section (tests).

use super::handle::Problem;
use aurora_config::Effective;
use serde_json::Value;

pub fn cross_rules(eff: &Effective) -> Vec<Problem> {
    let mut out = Vec::new();
    let get = |k: &str| eff.keys.get(k).map(|e| &e.value);
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

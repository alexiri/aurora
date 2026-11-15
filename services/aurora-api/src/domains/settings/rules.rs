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
    if get("settings_test.mode").and_then(Value::as_str) == Some("open")
        && get("settings_test.flag").and_then(Value::as_bool) != Some(true)
    {
        out.push(Problem::new(
            "settings_test.flag",
            "requires_flag",
            "must be on while `settings_test.mode` is `open`",
        ));
    }
    out
}

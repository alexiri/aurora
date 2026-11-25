//! A tool's `body_check` (docs/09 §3.4 tools): a request-body shape the gateway enforces for
//! a tool's raw sends. The gateway refuses a request whose body is not the declared shape
//! before any rule, approval or injection, so nothing the approval card cannot show is ever
//! held for a person's approval. Fail closed: a body that is absent, truncated, not a JSON
//! object, or otherwise not checkable is refused too.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A declared body shape. Every part is a refusal, never an allowance: the check only ever
/// rejects bodies the tool's documented builder cannot send.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyCheck {
    /// Top-level keys the body must not carry (Grafana's `record` makes the rule a recording
    /// rule: it runs on every evaluation).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refuse_keys: Vec<String>,
    /// A key whose value, when the body carries it, must be an object with no keys but the
    /// listed ones (a `labels` key a notification policy could route on, a mute timing).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub only_keys: BTreeMap<String, Vec<String>>,
    /// A key whose value must be present and an array of exactly this length (the three-step
    /// query model: one data source query with its reduce and threshold — a second step could
    /// run another query the approval card does not show).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exact_len: BTreeMap<String, u32>,
}

impl BodyCheck {
    /// Why the body violates the shape, in words shown to the agent (`None` = it may pass).
    #[must_use]
    pub fn violation(&self, body: &[u8], truncated: bool) -> Option<String> {
        if truncated {
            return Some("the body is longer than the gateway can check in full".into());
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
            return Some("the body is not the JSON object this tool sends".into());
        };
        let Some(obj) = v.as_object() else {
            return Some("the body is not a JSON object".into());
        };
        for k in &self.refuse_keys {
            if obj.contains_key(k) {
                return Some(format!(
                    "the body carries `{k}`, which this tool never sends"
                ));
            }
        }
        for (k, len) in &self.exact_len {
            match obj.get(k) {
                Some(serde_json::Value::Array(a))
                    if a.len() == usize::try_from(*len).unwrap_or(0) => {}
                Some(serde_json::Value::Array(a)) => {
                    return Some(format!(
                        "`{k}` has {} entries, this tool sends exactly {len}",
                        a.len()
                    ));
                }
                _ => return Some(format!("the body has no `{k}` array")),
            }
        }
        for (k, allowed) in &self.only_keys {
            match obj.get(k) {
                None => {}
                Some(serde_json::Value::Object(o)) => {
                    if let Some(extra) = o.keys().find(|x| !allowed.contains(x)) {
                        return Some(format!(
                            "`{k}` carries `{extra}`, which this tool never sets (allowed: {})",
                            allowed.join(", ")
                        ));
                    }
                }
                Some(_) => return Some(format!("`{k}` must be a JSON object")),
            }
        }
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check() -> BodyCheck {
        serde_json::from_value(json!({
            "refuse_keys": ["record"],
            "exact_len": { "data": 3 },
            "only_keys": {
                "labels": ["managed_by", "expires", "service"],
                "notification_settings": ["receiver"]
            }
        }))
        .unwrap()
    }

    #[test]
    fn the_scripts_bodies_pass() {
        let c = check();
        let body = json!({
            "orgID": 1, "title": "p95", "ruleGroup": "g", "folderUID": "f",
            "condition": "C", "for": "300s", "noDataState": "NoData",
            "execErrState": "Error", "isPaused": false,
            "data": [ {"refId": "A"}, {"refId": "B"}, {"refId": "C"} ],
            "labels": { "managed_by": "night", "expires": "2026-12-09" },
            "annotations": { "summary": "p95" },
            "notification_settings": { "receiver": "oncall" }
        });
        assert_eq!(
            c.violation(serde_json::to_vec(&body).unwrap().as_slice(), false),
            None
        );
    }

    #[test]
    fn every_denied_shape_is_named() {
        let c = check();
        let base = serde_json::to_vec(&json!({
            "data": [ {}, {}, {} ], "labels": {}, "notification_settings": { "receiver": "r" }
        }))
        .unwrap();
        // A recording rule.
        let b = serde_json::to_vec(&json!({
            "record": { "expr": "x" }, "data": [ {}, {}, {} ]
        }))
        .unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`record`")
        );
        // A second data step (whatever it queries).
        let b = serde_json::to_vec(&json!({
            "data": [ {}, {}, {}, { "datasourceUid": "sql", "model": { "rawSql": "select 1" } } ]
        }))
        .unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`data` has 4 entries")
        );
        // A label a notification policy routes on.
        let b = serde_json::to_vec(&json!({
            "data": [ {}, {}, {} ], "labels": { "managed_by": "n", "severity": "page" }
        }))
        .unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`labels` carries `severity`")
        );
        // A receiver setting the script never sets.
        let b = serde_json::to_vec(&json!({
            "data": [ {}, {}, {} ],
            "notification_settings": { "receiver": "r", "mute_time_intervals": ["nights"] }
        }))
        .unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`notification_settings` carries `mute_time_intervals`")
        );
        let _ = base;
    }

    #[test]
    fn fail_closed_cases() {
        let c = check();
        // No body, not JSON, not an object, truncated.
        assert!(c.violation(&[], false).is_some());
        assert!(c.violation(b"not json", false).is_some());
        assert!(c.violation(b"[1, 2]", false).is_some());
        assert!(c.violation(b"{}", true).is_some());
        // `data` missing or the wrong kind.
        assert!(c.violation(b"{}", false).unwrap().contains("no `data`"));
        let b = serde_json::to_vec(&json!({ "data": "x" })).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("no `data`")
        );
        // A key of the wrong kind under `only_keys`.
        let b = serde_json::to_vec(&json!({ "data": [ {}, {}, {} ], "labels": "x" })).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`labels` must be a JSON object")
        );
        // Absent `only_keys` members are allowed (the script sets `service` only when given).
        let b = serde_json::to_vec(&json!({ "data": [ {}, {}, {} ] })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
    }
}

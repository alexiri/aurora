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
    /// Top-level keys the body must not carry (a `record` key makes the rule a recording
    /// rule: it runs on every evaluation).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refuse_keys: Vec<String>,
    /// The only top-level keys the body may carry (the keys the tool's builder sends); any
    /// other key is refused. Empty = every key is accepted unless another part refuses it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_keys: Vec<String>,
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
        // The receiver's parser matches object key names without regard to case and keeps the
        // last match. A key its fold (case, the Kelvin sign U+212A, the long s U+017F) makes
        // equal to a key this check reads is that key to the parser, whatever its spelling, so
        // it is refused at every level the check reads before any exact comparison.
        let mut known: Vec<&str> = Vec::with_capacity(
            self.refuse_keys.len()
                + self.allow_keys.len()
                + self.only_keys.len()
                + self.exact_len.len(),
        );
        known.extend(self.refuse_keys.iter().map(String::as_str));
        known.extend(self.allow_keys.iter().map(String::as_str));
        known.extend(self.only_keys.keys().map(String::as_str));
        known.extend(self.exact_len.keys().map(String::as_str));
        let mut hit: Option<(&String, &str)> = None;
        for k in obj.keys() {
            if let Some(same) = known.iter().find(|s| fold(s) == fold(k) && **s != *k) {
                hit = Some((k, same));
                break;
            }
        }
        if let Some((k, same)) = hit {
            return Some(format!(
                "the body carries `{k}`, which the receiver reads as `{same}` (key names match without case): spell it `{same}`"
            ));
        }
        for k in &self.refuse_keys {
            if obj.contains_key(k) {
                return Some(format!(
                    "the body carries `{k}`, which this tool never sends"
                ));
            }
        }
        if !self.allow_keys.is_empty()
            && let Some(extra) = obj.keys().find(|k| !self.allow_keys.contains(k))
        {
            return Some(format!(
                "the body carries `{extra}`, which this tool never sends (allowed: {})",
                self.allow_keys.join(", ")
            ));
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
                    if let Some((extra, same)) = o.keys().find_map(|x| {
                        allowed
                            .iter()
                            .find(|a| fold(a) == fold(x) && *a != x)
                            .map(|a| (x, a.as_str()))
                    }) {
                        return Some(format!(
                            "`{k}` carries `{extra}`, which the receiver reads as `{same}` (key names match without case): spell it `{same}`"
                        ));
                    }
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

/// Folds a key the way the receiver's JSON parser matches field names: without case, and with
/// the Kelvin sign (U+212A) and the long s (U+017F), which that matcher folds to `k` and `s`.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{212A}' => 'k',
            '\u{017F}' => 's',
            c => c.to_lowercase().next().unwrap_or(c),
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn check() -> BodyCheck {
        serde_json::from_value(json!({
            "refuse_keys": ["record"],
            "allow_keys": [
                "orgID", "title", "ruleGroup", "folderUID", "condition", "data", "for",
                "noDataState", "execErrState", "isPaused", "labels", "annotations",
                "notification_settings"
            ],
            "exact_len": { "data": 3 },
            "only_keys": {
                "labels": ["managed_by", "expires", "service"],
                "annotations": ["summary", "expires"],
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
    fn a_key_the_parser_folds_onto_a_checked_key_is_refused() {
        let c = check();
        let base = || {
            json!({
                "orgID": 1, "title": "p95", "ruleGroup": "g", "folderUID": "f",
                "condition": "C", "for": "300s", "noDataState": "NoData",
                "execErrState": "Error", "isPaused": false,
                "data": [ {}, {}, {} ],
                "labels": { "managed_by": "night" },
                "annotations": { "summary": "p95" },
                "notification_settings": { "receiver": "oncall" }
            })
        };
        // The receiver's parser matches key names without case and keeps the last match, so
        // `Data` would silently become the `data` the check read: refused at every level.
        for (key, value) in [
            ("Data", json!([{}, {}, {}, {}])),
            ("DATA", json!([{}, {}, {}, {}])),
            ("Record", json!({ "from": "A" })),
        ] {
            let mut body = base();
            body.as_object_mut().unwrap().insert(key.into(), value);
            let b = serde_json::to_vec(&body).unwrap();
            let why = c.violation(b.as_slice(), false).unwrap_or_default();
            assert!(why.contains(&format!("`{key}`")), "{key}: {why}");
        }
        // The fold reaches the checked objects too: the same refusal, named as the parser
        // reads it.
        for (k, inner, folded) in [
            ("labels", json!({ "Managed_By": "n" }), "Managed_By"),
            ("annotations", json!({ "Summary": "p95" }), "Summary"),
        ] {
            let mut body = base();
            body[k] = inner;
            let b = serde_json::to_vec(&body).unwrap();
            assert!(
                c.violation(b.as_slice(), false)
                    .unwrap_or_default()
                    .contains(&format!("`{folded}`")),
                "{k}: a folded key inside `{k}` passed"
            );
        }
        // And to the allow list itself: the same object under another spelling of a known key.
        let mut body = base();
        body.as_object_mut()
            .unwrap()
            .remove("notification_settings");
        body.as_object_mut().unwrap().insert(
            "Notification_Settings".into(),
            json!({ "receiver": "oncall" }),
        );
        let b = serde_json::to_vec(&body).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`Notification_Settings`")
        );
    }

    #[test]
    fn the_kelvin_sign_and_the_long_s_fold_like_the_receivers_parser() {
        // The receiver's matcher folds U+212A to `k` and U+017F to `s`; a key spelled with them
        // is the checked key to that parser and is refused.
        let c = serde_json::from_value::<BodyCheck>(json!({
            "allow_keys": ["kind", "settings"],
            "only_keys": { "settings": ["receiver"] }
        }))
        .unwrap();
        for spelling in ["\u{212A}ind", "KinD", "kind\u{017F}"] {
            let mut m = serde_json::Map::new();
            m.insert(spelling.to_owned(), json!("x"));
            let b = serde_json::to_vec(&Value::Object(m)).unwrap();
            assert!(
                c.violation(b.as_slice(), false).is_some(),
                "{spelling:?} passed"
            );
        }
        // The long s inside a checked object's key.
        let b = serde_json::to_vec(&json!({ "settings": { "receiver\u{017F}": "r" } })).unwrap();
        assert!(c.violation(b.as_slice(), false).is_some());
        // And the exact spellings still pass.
        let b =
            serde_json::to_vec(&json!({ "kind": "x", "settings": { "receiver": "r" } })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
    }

    #[test]
    fn a_duplicated_exact_key_is_still_checked_last_match_wins() {
        // Both parsers keep the last occurrence of a duplicated key, so the check reads the
        // same value the receiver does.
        let c = check();
        let b = br#"{"data":[{},{},{}],"data":[{},{},{},{}],"labels":{},"annotations":{},"notification_settings":{}}"#;
        assert!(
            c.violation(b, false)
                .unwrap()
                .contains("`data` has 4 entries")
        );
        let b = br#"{"data":[{},{},{}],"labels":{},"annotations":{},"notification_settings":{},"labels":{"severity":"page"}}"#;
        assert!(
            c.violation(b, false)
                .unwrap()
                .contains("`labels` carries `severity`")
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

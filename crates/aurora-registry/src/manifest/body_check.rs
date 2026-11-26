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
    /// A key whose value must be an array whose every entry is an object with no keys but the
    /// listed ones (every `data` step is a `{refId, datasourceUid, queryType,
    /// relativeTimeRange, model}` object; a raw query bolted onto a step is refused).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entry_keys: BTreeMap<String, Vec<String>>,
    /// A key whose value must be an array whose entries, from the `from`-th on, carry `key`
    /// with exactly this value (the three-step query model: step one queries the agent's data
    /// source, the reduce and threshold steps after it are the receiver's own `__expr__` steps,
    /// which run no query of their own).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entry_value: BTreeMap<String, EntryValue>,
    /// A key whose value must be an array whose entry at `at` carries, along the `member`
    /// path, an object with no keys but the listed ones (the first data step's `model`: the
    /// raw query object the receiver's parser reads without case, so a query bolted onto it
    /// under another spelling — or a query type the approval card does not show — is
    /// refused).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entry_member: BTreeMap<String, Vec<EntryMember>>,
}

/// An entry rule of [`BodyCheck::entry_member`]: the entry at `at` (0 counts the first)
/// carries, along the `member` path from the entry (object keys and array indexes
/// separated by `/`), an object with no keys but `keys`, and — when `value` is set — that
/// object's `value.0` carries exactly `value.1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryMember {
    pub at: u32,
    pub member: String,
    pub keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<EntryMemberValue>,
}

/// The exact value an entry member carries: the member object's `key` is `value`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryMemberValue {
    pub key: String,
    pub value: serde_json::Value,
}

/// An entry rule of [`BodyCheck::entry_value`]: from the `from`-th entry of the array on
/// (0 counts entries from the first), every entry carries `key` with exactly `value`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryValue {
    pub from: u32,
    pub key: String,
    pub value: serde_json::Value,
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
                + self.exact_len.len()
                + self.entry_keys.len()
                + self.entry_value.len(),
        );
        known.extend(self.refuse_keys.iter().map(String::as_str));
        known.extend(self.allow_keys.iter().map(String::as_str));
        known.extend(self.only_keys.keys().map(String::as_str));
        known.extend(self.exact_len.keys().map(String::as_str));
        known.extend(self.entry_keys.keys().map(String::as_str));
        known.extend(self.entry_value.keys().map(String::as_str));
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
        for (k, allowed) in &self.entry_keys {
            match obj.get(k) {
                None => {}
                Some(serde_json::Value::Array(a)) => {
                    for (i, e) in a.iter().enumerate() {
                        let Some(o) = e.as_object() else {
                            return Some(format!("`{k}`[{i}] must be a JSON object"));
                        };
                        if let Some((extra, same)) = o.keys().find_map(|x| {
                            allowed
                                .iter()
                                .find(|a| fold(a) == fold(x) && *a != x)
                                .map(|a| (x, a.as_str()))
                        }) {
                            return Some(format!(
                                "`{k}`[{i}] carries `{extra}`, which the receiver reads as `{same}` (key names match without case): spell it `{same}`"
                            ));
                        }
                        if let Some(extra) = o.keys().find(|x| !allowed.contains(x)) {
                            return Some(format!(
                                "`{k}`[{i}] carries `{extra}`, which this tool never sets (allowed: {})",
                                allowed.join(", ")
                            ));
                        }
                    }
                }
                Some(_) => return Some(format!("`{k}` must be a JSON array")),
            }
        }
        for (k, ev) in &self.entry_value {
            match obj.get(k) {
                None => {}
                Some(serde_json::Value::Array(a)) => {
                    for (i, e) in a
                        .iter()
                        .enumerate()
                        .skip(usize::try_from(ev.from).unwrap_or(usize::MAX))
                    {
                        if e.get(&ev.key).is_some_and(|v| v == &ev.value) {
                            continue;
                        }
                        let actual = e
                            .get(&ev.key)
                            .map(|v| format!("`{key}: {v}`", key = ev.key))
                            .unwrap_or_else(|| format!("no `{}`", ev.key));
                        return Some(format!(
                            "`{k}`[{i}] carries {actual}: every step after the first carries `{key}: {value}` (the receiver runs it itself; only the first step queries a data source)",
                            key = ev.key,
                            value = ev.value
                        ));
                    }
                }
                Some(_) => return Some(format!("`{k}` must be a JSON array")),
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
        for (k, members) in &self.entry_member {
            match obj.get(k) {
                None => {}
                Some(serde_json::Value::Array(a)) => {
                    for m in members {
                        let at = m.at;
                        let Some(e) = a.get(usize::try_from(at).unwrap_or(usize::MAX)) else {
                            return Some(format!("the body has no `{k}`[{at}]"));
                        };
                        // The member is reached along object keys and array indexes; the
                        // object it names is checked the way the checked objects above are:
                        // the receiver's parser matches key names without case and keeps the
                        // last match, so a spelling that folds onto a listed key is that key
                        // to it, whatever it shows.
                        let mut place = format!("`{k}`[{at}]");
                        let mut cur = e;
                        for seg in m.member.split('/') {
                            let next = match seg.parse::<usize>() {
                                Ok(n) => cur.get(n),
                                Err(_) => cur.get(seg),
                            };
                            let Some(next) = next else {
                                return Some(format!("{place} carries no `{seg}`"));
                            };
                            place.push('/');
                            place.push_str(seg);
                            cur = next;
                        }
                        let Some(o) = cur.as_object() else {
                            return Some(format!("{place} must be a JSON object"));
                        };
                        if let Some((extra, same)) = o.keys().find_map(|x| {
                            m.keys
                                .iter()
                                .find(|a| fold(a) == fold(x) && *a != x)
                                .map(|a| (x, a.as_str()))
                        }) {
                            return Some(format!(
                                "{place} carries `{extra}`, which the receiver reads as `{same}` (key names match without case): spell it `{same}`"
                            ));
                        }
                        if let Some(extra) = o.keys().find(|x| !m.keys.contains(x)) {
                            return Some(format!(
                                "{place} carries `{extra}`, which this tool never sets (allowed: {})",
                                m.keys.join(", ")
                            ));
                        }
                        if let Some(v) = &m.value {
                            if o.get(&v.key).is_some_and(|x| x == &v.value) {
                                continue;
                            }
                            let actual = o
                                .get(&v.key)
                                .map(|x| format!("`{key}: {x}`", key = v.key))
                                .unwrap_or_else(|| format!("no `{}`", v.key));
                            return Some(format!(
                                "{place} carries {actual}: this tool sends only `{key}: {value}` here",
                                key = v.key,
                                value = v.value
                            ));
                        }
                    }
                }
                Some(_) => return Some(format!("`{k}` must be a JSON array")),
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
            "entry_keys": {
                "data": ["refId", "datasourceUid", "queryType", "relativeTimeRange", "model"]
            },
            "entry_value": {
                "data": { "from": 1, "key": "datasourceUid", "value": "__expr__" }
            },
            "entry_member": {
                "data": [
                    { "at": 0, "member": "model",
                      "keys": ["editorMode", "expr", "instant", "legendFormat", "refId"] },
                    { "at": 1, "member": "model",
                      "keys": ["conditions", "datasource", "expression", "reducer", "refId", "type"],
                      "value": { "key": "type", "value": "reduce" } },
                    { "at": 1, "member": "model/conditions/0",
                      "keys": ["evaluator", "operator", "query", "reducer", "type"] },
                    { "at": 1, "member": "model/conditions/0/evaluator",
                      "keys": ["params", "type"] },
                    { "at": 2, "member": "model",
                      "keys": ["conditions", "datasource", "expression", "refId", "type"],
                      "value": { "key": "type", "value": "threshold" } },
                    { "at": 2, "member": "model/conditions/0",
                      "keys": ["evaluator", "operator", "query", "reducer", "type"] },
                    { "at": 2, "member": "model/conditions/0/evaluator",
                      "keys": ["params", "type"] }
                ]
            },
            "only_keys": {
                "labels": ["managed_by", "expires", "service"],
                "annotations": ["summary", "expires"],
                "notification_settings": ["receiver"]
            }
        }))
        .unwrap()
    }

    /// One `data` step the way the script's builder sends it: the first step queries the named
    /// data source, the steps after it are the receiver's own reduce and threshold steps, with
    /// the conditions the approval card reads its comparison and threshold from.
    fn step(ref_id: &str, ds: &str) -> Value {
        let mut s = json!({
            "refId": ref_id, "datasourceUid": ds, "queryType": "",
            "relativeTimeRange": { "from": 300, "to": 0 },
        });
        if ds == "__expr__" {
            let mut m = json!({
                "conditions": [{
                    "evaluator": { "params": [], "type": "gt" },
                    "operator": { "type": "and" },
                    "query": { "params": [ref_id] },
                    "reducer": { "params": [], "type": "last" },
                    "type": "query"
                }],
                "datasource": { "type": "__expr__", "uid": "__expr__" },
                "expression": if ref_id == "B" { "A" } else { "B" },
                "refId": ref_id,
                "type": if ref_id == "B" { "reduce" } else { "threshold" },
            });
            if ref_id == "B" {
                m["reducer"] = json!("last");
            }
            s["model"] = m;
        } else {
            s["model"] = json!({ "expr": "up", "instant": true, "refId": ref_id });
        }
        s
    }

    fn steps() -> Vec<Value> {
        vec![
            step("A", "prom-main"),
            step("B", "__expr__"),
            step("C", "__expr__"),
        ]
    }

    #[test]
    fn the_scripts_bodies_pass() {
        let c = check();
        let body = json!({
            "orgID": 1, "title": "p95", "ruleGroup": "g", "folderUID": "f",
            "condition": "C", "for": "300s", "noDataState": "NoData",
            "execErrState": "Error", "isPaused": false,
            "data": steps(),
            "labels": { "managed_by": "night", "expires": "2026-12-09" },
            "annotations": { "summary": "p95" },
            "notification_settings": { "receiver": "oncall" }
        });
        assert_eq!(
            c.violation(serde_json::to_vec(&body).unwrap().as_slice(), false),
            None
        );
        // The rule read back before an update: the first step's model carries no refId
        // (Grafana stores the model without it), everything else is the script's shape.
        let mut data = steps();
        data[0]
            .as_object_mut()
            .unwrap()
            .get_mut("model")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("refId");
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
    }

    #[test]
    fn the_first_steps_model_carries_only_the_keys_the_script_sends() {
        let c = check();
        // The script's own first step (the builder sends editorMode, expr, instant,
        // legendFormat, refId) passes, and so does the rule read back before an update,
        // whose model carries no refId (Grafana stores the model without it).
        for model in [
            json!({ "editorMode": "code", "expr": "up", "instant": true,
                    "legendFormat": "__auto", "refId": "A" }),
            json!({ "editorMode": "code", "expr": "up", "instant": true,
                    "legendFormat": "__auto" }),
        ] {
            let mut data = steps();
            data[0]["model"] = model;
            let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
            assert_eq!(
                c.violation(b.as_slice(), false),
                None,
                "the script's own first step must pass"
            );
        }
    }

    #[test]
    fn the_first_steps_model_refuses_a_query_the_card_does_not_show() {
        let c = check();
        // A SQL query bolted onto the first step's model: the card reads /data/0/model/expr,
        // which it does not show.
        let mut data = steps();
        data[0]["model"] = json!({ "editorMode": "code", "rawSql": "select 1", "refId": "A" });
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("rawSql"), "{why}");
        // A key the script never sends (whatever it carries).
        let mut data = steps();
        data[0]["model"]["intervalMs"] = json!(1000);
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("intervalMs"), "{why}");
    }

    #[test]
    fn the_first_steps_model_refuses_case_variants_of_its_keys() {
        let c = check();
        // The receiver's parser matches key names without case and keeps the last match, so
        // `{"expr": "up", "Expr": "<other query>"}` shows `up` on the card and runs the other
        // query: every spelling that folds onto a listed key is refused.
        for spelling in ["Expr", "EXPR", "in\u{212A}stant"] {
            let mut data = steps();
            data[0]["model"][spelling] = json!("histogram_quantile(0.5, x[5m])");
            let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
            let why = c.violation(b.as_slice(), false).unwrap();
            assert!(why.contains(&format!("`{spelling}`")), "{spelling}: {why}");
        }
    }

    #[test]
    fn the_first_steps_model_must_be_present_and_an_object() {
        let c = check();
        // A first step without a model: the card's Query field would show nothing at all.
        let mut data = steps();
        data[0].as_object_mut().unwrap().remove("model");
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("no `model`"), "{why}");
        // A model that is not an object.
        let mut data = steps();
        data[0]["model"] = json!("up");
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("must be a JSON object"), "{why}");
    }

    #[test]
    fn the_expression_steps_are_the_ones_the_script_builds() {
        let c = check();
        // The script's reduce and threshold steps pass with the whole body.
        let b = serde_json::to_vec(&json!({ "data": steps() })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
        // The reduce step's model carrying another expression type is refused (a `math` step
        // computes whatever it likes while the card shows the threshold step's comparison).
        for (i, ty) in [(1usize, "math"), (1, "Reduce"), (2, "threshold\u{0}")] {
            let mut data = steps();
            data[i]["model"]["type"] = json!(ty);
            let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
            let why = c.violation(b.as_slice(), false).unwrap();
            assert!(
                why.contains("type"),
                "step {i} with model type {ty:?} passed: {why}"
            );
        }
        // The threshold step's model type is pinned the same way.
        let mut data = steps();
        data[2]["model"]["type"] = json!("reduce");
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        assert!(c.violation(b.as_slice(), false).is_some());
    }

    #[test]
    fn the_expression_steps_refuse_case_variants_of_the_checked_keys() {
        let c = check();
        // A second spelling of a checked key at any level the card reads: the receiver's
        // parser keeps the last case-insensitive match, so the real conditions (or the real
        // threshold) would hide behind the ones the card shows.
        for (i, place, key) in [
            (1usize, "model", "Conditions"),
            (1, "model", "Type"),
            (2, "model/conditions/0", "Evaluator"),
            (2, "model/conditions/0/evaluator", "Params"),
        ] {
            let mut data = steps();
            let mut cur = data[i].get_mut("model").unwrap();
            for seg in place.split('/').skip(1) {
                cur = match seg.parse::<usize>() {
                    Ok(n) => cur.get_mut(n).unwrap(),
                    Err(_) => cur.get_mut(seg).unwrap(),
                };
            }
            cur.as_object_mut().unwrap().insert(key.into(), json!({}));
            let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
            let why = c.violation(b.as_slice(), false).unwrap();
            assert!(
                why.contains(&format!("`{key}`")),
                "{place}/{key} passed: {why}"
            );
        }
    }

    #[test]
    fn a_key_the_parser_folds_onto_a_checked_key_is_refused() {
        let c = check();
        let base = || {
            json!({
                "orgID": 1, "title": "p95", "ruleGroup": "g", "folderUID": "f",
                "condition": "C", "for": "300s", "noDataState": "NoData",
                "execErrState": "Error", "isPaused": false,
                "data": steps(),
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
        let b = br#"{"data":[{"refId":"A","datasourceUid":"prom"},{"refId":"B","datasourceUid":"__expr__"},{"refId":"C","datasourceUid":"__expr__"}],"labels":{},"annotations":{},"notification_settings":{},"labels":{"severity":"page"}}"#;
        assert!(
            c.violation(b, false)
                .unwrap()
                .contains("`labels` carries `severity`")
        );
    }

    #[test]
    fn every_denied_shape_is_named() {
        let c = check();
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
            "data": steps(), "labels": { "managed_by": "n", "severity": "page" }
        }))
        .unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`labels` carries `severity`")
        );
        // A receiver setting the script never sets.
        let b = serde_json::to_vec(&json!({
            "data": steps(),
            "notification_settings": { "receiver": "r", "mute_time_intervals": ["nights"] }
        }))
        .unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`notification_settings` carries `mute_time_intervals`")
        );
    }

    #[test]
    fn a_step_after_the_first_queries_no_data_source() {
        let c = check();
        // A second SQL step inside the three-step shape: the entry count is right, the step's
        // data source is not the receiver's own expression engine.
        let mut data = steps();
        data[1] = json!({
            "refId": "B", "datasourceUid": "sql-main", "queryType": "",
            "relativeTimeRange": { "from": 300, "to": 0 },
            "model": { "rawSql": "select 1", "refId": "B" }
        });
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("datasourceUid"), "{why}");
        assert!(why.contains("__expr__"), "{why}");
        // The threshold step too.
        let mut data = steps();
        data[2] = json!({
            "refId": "C", "datasourceUid": "loki-main", "queryType": "",
            "relativeTimeRange": { "from": 300, "to": 0 },
            "model": { "refId": "C" }
        });
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        assert!(c.violation(b.as_slice(), false).is_some());
        // A step after the first that carries no data source at all.
        let mut data = steps();
        data[1].as_object_mut().unwrap().remove("datasourceUid");
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("datasourceUid")
        );
        // The first step is the agent's own data source: any uid there is the declared shape.
        let b = serde_json::to_vec(&json!({ "data": [step("A", "postgres-logs"), step("B", "__expr__"), step("C", "__expr__")] }))
            .unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
    }

    #[test]
    fn every_data_step_is_the_shape_the_script_sends() {
        let c = check();
        // An entry that is not an object.
        let b = serde_json::to_vec(&json!({ "data": ["x", {}, {}] })).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`data`[0] must be a JSON object")
        );
        // An entry key the script never sets.
        let mut data = steps();
        data[1]
            .as_object_mut()
            .unwrap()
            .insert("rawSql".into(), json!("select 1"));
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`data`[1] carries `rawSql`")
        );
        // A key the receiver's parser folds onto a checked entry key.
        let mut data = steps();
        data[2]
            .as_object_mut()
            .unwrap()
            .insert("RefId".into(), json!("C"));
        let b = serde_json::to_vec(&json!({ "data": data })).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`RefId`")
        );
        // Not an array: refused. (`exact_len` would name the missing array first on the
        // shared check, so this one carries only the entry rules.)
        let entry_only: BodyCheck =
            serde_json::from_value(json!({ "entry_keys": { "data": ["a"] } })).unwrap();
        let b = serde_json::to_vec(&json!({ "data": "x" })).unwrap();
        assert!(
            entry_only
                .violation(b.as_slice(), false)
                .unwrap()
                .contains("`data` must be a JSON array")
        );
        assert!(c.violation(b"{}", true).is_some());
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
        let b = serde_json::to_vec(&json!({ "data": steps(), "labels": "x" })).unwrap();
        assert!(
            c.violation(b.as_slice(), false)
                .unwrap()
                .contains("`labels` must be a JSON object")
        );
        // Absent `only_keys` members are allowed (the script sets `service` only when given).
        let b = serde_json::to_vec(&json!({ "data": steps() })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
    }
}

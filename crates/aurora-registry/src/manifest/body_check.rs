//! A tool's `body_check` (docs/09 §3.4 tools): a request-body shape the gateway enforces for
//! a tool's raw sends. The gateway refuses a request whose body is not the declared shape
//! before any rule, approval or injection, so nothing the approval card cannot show is ever
//! held for a person's approval. Fail closed: a body that is absent, truncated, not a JSON
//! object, or otherwise not checkable is refused too.
use aurora_policy::json::read_unrepeated;
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
    /// A key whose value must be an array whose entry at `at` carries `key` — on the entry
    /// itself, or, along the optional `member` path of object keys and array indexes, on
    /// the object it names — with exactly this value (the steps' `refId`s and the
    /// expression links between them: the card shows the step the top-level `condition`
    /// names, and a swapped refId would name another step than the one the card reads).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entry_at: BTreeMap<String, Vec<EntryAt>>,
    /// A key the body must carry with exactly this scalar value (the top-level
    /// `condition`: the card shows the decision it names, so it names the threshold step
    /// the script builds or the body is refused). A key the body does not carry is
    /// refused too.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub value: BTreeMap<String, serde_json::Value>,
    /// A key whose value, when the body carries it, must be of one of the listed JSON
    /// kinds (a duration a newer server answers with is a string or a number, never an
    /// object; a count it adds is an integer). A key the body does not carry is allowed:
    /// the server adds these keys only when it runs the feature.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, Vec<JsonKind>>,
    /// A key the body must carry as an array whose entry at `at` holds, along the `member`
    /// path, an array of exactly `len` entries (one metric and one bucket: a second one
    /// would run without any approval card field showing it).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entry_len: BTreeMap<String, Vec<EntryLen>>,
    /// A key the body must carry as an array whose entry at `at` holds, along the `member`
    /// path, a string of 1 to `max` characters (a value an approval card shows in full: a
    /// longer one would be cut, and one that is not a string would be left off the card).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entry_max_len: BTreeMap<String, Vec<EntryMaxLen>>,
    /// A key the body must carry as an array whose entry at `at` holds, along the `member`
    /// path, an object whose `key` and `than` are whole numbers (0 or more), `key` the
    /// larger: a time range `{from, to}` in seconds before now that is neither empty nor
    /// inverted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entry_greater: BTreeMap<String, Vec<EntryGreater>>,
    /// Further shapes: the body must pass this check **and** at least one of these. A
    /// variant carries no `one_of` of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<BodyCheck>,
}

/// An entry rule of [`BodyCheck::entry_len`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryLen {
    pub at: u32,
    pub member: String,
    pub len: u32,
}

/// An entry rule of [`BodyCheck::entry_greater`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryGreater {
    pub at: u32,
    pub member: String,
    pub key: String,
    pub than: String,
}

/// An entry rule of [`BodyCheck::entry_max_len`]. With `optional`, the object the path's
/// last segment is read from may lack it; everything before it must be there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryMaxLen {
    pub at: u32,
    pub member: String,
    pub max: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// A JSON kind a [`BodyCheck::kinds`] rule allows for a key's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonKind {
    String,
    Number,
    Integer,
    Boolean,
}

impl JsonKind {
    /// Whether `v` is of this kind: an `integer` is a JSON number without a fractional
    /// part, so `2.5` is a number and never an integer.
    fn matches(self, v: &serde_json::Value) -> bool {
        match self {
            Self::String => v.is_string(),
            Self::Number => v.is_number(),
            Self::Integer => v.is_i64() || v.is_u64(),
            Self::Boolean => v.is_boolean(),
        }
    }

    /// The kind's name in a refusal message.
    fn name(self) -> &'static str {
        match self {
            Self::String => "a string",
            Self::Number => "a number",
            Self::Integer => "an integer",
            Self::Boolean => "a boolean",
        }
    }
}

/// An entry rule of [`BodyCheck::entry_at`]: the entry at `at` (0 counts the first)
/// carries, on itself or along the optional `member` path, `key` with exactly `value`, or
/// with one of `values` (a manifest declares exactly one of the two).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryAt {
    pub at: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<serde_json::Value>,
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

/// The value an entry member carries: the member object's `key` is `value`, or one of
/// `values` (a manifest declares exactly one of the two).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryMemberValue {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<serde_json::Value>,
}

/// Whether `found` is the pinned `value`, or one of `values`. A pin with neither accepts
/// nothing.
fn pinned(
    found: Option<&serde_json::Value>,
    value: Option<&serde_json::Value>,
    values: &[serde_json::Value],
) -> bool {
    found.is_some_and(|f| value == Some(f) || values.contains(f))
}

/// What a pin accepts for `key`, as a refusal message says it.
fn accepted(key: &str, value: Option<&serde_json::Value>, values: &[serde_json::Value]) -> String {
    match value {
        Some(v) => format!("`{key}: {v}`"),
        None => {
            let list = values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("`{key}` as one of {list}")
        }
    }
}

/// Characters of one variant's reason quoted when a body fits none.
const VARIANT_WHY_MAX: usize = 240;

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
        let v = match read_unrepeated(body) {
            Ok(v) => v,
            Err(Some(key)) => {
                return Some(format!(
                    "the body carries `{key}` twice: send each key once"
                ));
            }
            Err(None) => return Some("the body is not the JSON object this tool sends".into()),
        };
        let Some(obj) = v.as_object() else {
            return Some("the body is not a JSON object".into());
        };
        self.object_violation(obj)
    }

    /// [`BodyCheck::violation`] on the body as read: this check's own parts, then `one_of`.
    fn object_violation(&self, obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
        if let Some(why) = self.parts_violation(obj) {
            return Some(why);
        }
        if self.one_of.is_empty() {
            return None;
        }
        let mut whys = Vec::with_capacity(self.one_of.len());
        for (i, variant) in self.one_of.iter().enumerate() {
            let why = variant.object_violation(obj)?;
            let why: String = why.chars().take(VARIANT_WHY_MAX).collect();
            whys.push(format!("shape {}: {why}", i + 1));
        }
        Some(format!(
            "the body fits none of the shapes this tool sends ({})",
            whys.join("; ")
        ))
    }

    fn parts_violation(&self, obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
        // The receiver's parser matches object key names without regard to case. A key its fold (case, the Kelvin sign U+212A, the long s U+017F) makes
        // equal to a key this check reads is that key to the parser, whatever its spelling, so
        // it is refused at every level the check reads before any exact comparison.
        let mut known: Vec<&str> = Vec::with_capacity(
            self.refuse_keys.len()
                + self.allow_keys.len()
                + self.only_keys.len()
                + self.exact_len.len()
                + self.entry_keys.len()
                + self.entry_value.len()
                + self.entry_at.len()
                + self.value.len()
                + self.kinds.len(),
        );
        known.extend(self.refuse_keys.iter().map(String::as_str));
        known.extend(self.allow_keys.iter().map(String::as_str));
        known.extend(self.only_keys.keys().map(String::as_str));
        known.extend(self.exact_len.keys().map(String::as_str));
        known.extend(self.entry_keys.keys().map(String::as_str));
        known.extend(self.entry_value.keys().map(String::as_str));
        known.extend(self.entry_at.keys().map(String::as_str));
        known.extend(self.value.keys().map(String::as_str));
        known.extend(self.kinds.keys().map(String::as_str));
        known.extend(self.entry_member.keys().map(String::as_str));
        known.extend(self.entry_len.keys().map(String::as_str));
        known.extend(self.entry_max_len.keys().map(String::as_str));
        known.extend(self.entry_greater.keys().map(String::as_str));
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
                        // the receiver's parser matches key names without case, so a spelling
                        // that folds onto a listed key is that key to it, whatever it shows.
                        let place = format!("`{k}`[{at}]");
                        let (cur, place) = match member_value(e, &m.member, &place) {
                            Ok(found) => found,
                            Err(why) => return Some(why),
                        };
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
                            if pinned(o.get(&v.key), v.value.as_ref(), &v.values) {
                                continue;
                            }
                            let actual = o
                                .get(&v.key)
                                .map(|x| format!("`{key}: {x}`", key = v.key))
                                .unwrap_or_else(|| format!("no `{}`", v.key));
                            return Some(format!(
                                "{place} carries {actual}: this tool sends only {} here",
                                accepted(&v.key, v.value.as_ref(), &v.values)
                            ));
                        }
                    }
                }
                Some(_) => return Some(format!("`{k}` must be a JSON array")),
            }
        }
        // The kind checks run before the pins: a key a newer server adds to a rule it
        // answers may carry a scalar of the declared kind only, whatever its value.
        for (k, kinds) in &self.kinds {
            let Some(v) = obj.get(k) else {
                continue;
            };
            if kinds.iter().any(|kind| kind.matches(v)) {
                continue;
            }
            let allowed = kinds
                .iter()
                .map(|kind| kind.name())
                .collect::<Vec<_>>()
                .join(" or ");
            return Some(format!(
                "`{k}` carries `{v}`: this tool sends `{k}` as {allowed} there"
            ));
        }
        // The pins run last: a body that fails a shape rule above is refused for that, and
        // a pinned key's folded spellings are refused by the fold checks those rules already
        // run (and by the top-level one before them).
        for (k, want) in &self.value {
            match obj.get(k) {
                Some(v) if v == want => {}
                Some(v) => {
                    return Some(format!(
                        "the body carries `{k}: {v}`: this tool sends only `{k}: {want}`"
                    ));
                }
                None => {
                    return Some(format!(
                        "the body carries no `{k}`: this tool sends only `{k}: {want}`"
                    ));
                }
            }
        }
        for (k, pins) in &self.entry_at {
            match obj.get(k) {
                None => {}
                Some(serde_json::Value::Array(a)) => {
                    for p in pins {
                        let at = p.at;
                        let Some(e) = a.get(usize::try_from(at).unwrap_or(usize::MAX)) else {
                            return Some(format!("the body has no `{k}`[{at}]"));
                        };
                        let place = format!("`{k}`[{at}]");
                        let (cur, place) = match &p.member {
                            Some(member) => match member_value(e, member, &place) {
                                Ok(found) => found,
                                Err(why) => return Some(why),
                            },
                            None => (e, place),
                        };
                        let Some(o) = cur.as_object() else {
                            return Some(format!("{place} must be a JSON object"));
                        };
                        // The same fold as the checked objects above: the receiver reads the
                        // pinned key's name without case, so a spelling that folds onto it is
                        // that key to it, whatever it shows.
                        if let Some(extra) =
                            o.keys().find(|x| fold(x) == fold(&p.key) && **x != p.key)
                        {
                            return Some(format!(
                                "{place} carries `{extra}`, which the receiver reads as `{}` (key names match without case): spell it `{}`",
                                p.key, p.key
                            ));
                        }
                        if pinned(o.get(&p.key), p.value.as_ref(), &p.values) {
                            continue;
                        }
                        let actual = o
                            .get(&p.key)
                            .map(|v| format!("`{key}: {v}`", key = p.key))
                            .unwrap_or_else(|| format!("no `{}`", p.key));
                        return Some(format!(
                            "{place} carries {actual}: this tool sends only {} there",
                            accepted(&p.key, p.value.as_ref(), &p.values)
                        ));
                    }
                }
                Some(_) => return Some(format!("`{k}` must be a JSON array")),
            }
        }
        // The length rules refuse whatever they cannot find: an absent array, entry or
        // member is no body of the shape.
        for (k, lens) in &self.entry_len {
            for l in lens {
                let (e, place) = match entry(obj, k, l.at) {
                    Ok(found) => found,
                    Err(why) => return Some(why),
                };
                let (cur, place) = match member_value(e, &l.member, &place) {
                    Ok(found) => found,
                    Err(why) => return Some(why),
                };
                let Some(a) = cur.as_array() else {
                    return Some(format!("{place} must be a JSON array"));
                };
                if a.len() != usize::try_from(l.len).unwrap_or(0) {
                    return Some(format!(
                        "{place} has {} entries, this tool sends exactly {}",
                        a.len(),
                        l.len
                    ));
                }
            }
        }
        for (k, pairs) in &self.entry_greater {
            for g in pairs {
                let (e, place) = match entry(obj, k, g.at) {
                    Ok(found) => found,
                    Err(why) => return Some(why),
                };
                let (cur, place) = match member_value(e, &g.member, &place) {
                    Ok(found) => found,
                    Err(why) => return Some(why),
                };
                let Some(o) = cur.as_object() else {
                    return Some(format!("{place} must be a JSON object"));
                };
                for name in [&g.key, &g.than] {
                    if let Some(why) = folded_sibling(o, name, &place) {
                        return Some(why);
                    }
                    if !o.contains_key(name) {
                        return Some(format!("{place} carries no `{name}`"));
                    }
                }
                let number = |name: &str| o.get(name).and_then(serde_json::Value::as_u64);
                let (Some(larger), Some(smaller)) = (number(&g.key), number(&g.than)) else {
                    return Some(format!(
                        "{place}: `{}` and `{}` must be whole numbers, 0 or more",
                        g.key, g.than
                    ));
                };
                if larger <= smaller {
                    return Some(format!(
                        "{place}: `{key}` ({larger}) must be greater than `{than}` ({smaller})",
                        key = g.key,
                        than = g.than
                    ));
                }
            }
        }
        for (k, limits) in &self.entry_max_len {
            for l in limits {
                let (e, place) = match entry(obj, k, l.at) {
                    Ok(found) => found,
                    Err(why) => return Some(why),
                };
                let (parent, last) = match l.member.rsplit_once('/') {
                    Some((parent, last)) => (Some(parent), last),
                    None => (None, l.member.as_str()),
                };
                let (holder, place) = match parent {
                    Some(parent) => match member_value(e, parent, &place) {
                        Ok(found) => found,
                        Err(why) => return Some(why),
                    },
                    None => (e, place),
                };
                let Some(o) = holder.as_object() else {
                    return Some(format!("{place} must be a JSON object"));
                };
                if let Some(why) = folded_sibling(o, last, &place) {
                    return Some(why);
                }
                let Some(found) = o.get(last) else {
                    if l.optional {
                        continue;
                    }
                    return Some(format!("{place} carries no `{last}`"));
                };
                let Some(text) = found.as_str() else {
                    return Some(format!("{place}/{last} must be a string"));
                };
                let n = text.chars().count();
                if n == 0 || n > usize::try_from(l.max).unwrap_or(0) {
                    return Some(format!(
                        "{place}/{last} has {n} characters, this tool sends 1 to {} characters there",
                        l.max
                    ));
                }
            }
        }
        None
    }
}

/// The entry at `at` of the array `key` names, with the place a refusal message quotes.
fn entry<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: u32,
) -> Result<(&'a serde_json::Value, String), String> {
    let Some(serde_json::Value::Array(a)) = obj.get(key) else {
        return Err(format!("the body has no `{key}` array"));
    };
    a.get(usize::try_from(at).unwrap_or(usize::MAX))
        .map(|e| (e, format!("`{key}`[{at}]")))
        .ok_or_else(|| format!("the body has no `{key}`[{at}]"))
}

/// The refusal for a key of `o` the receiver reads as `name` without being spelled so.
fn folded_sibling(
    o: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    place: &str,
) -> Option<String> {
    o.keys()
        .find(|x| fold(x) == fold(name) && *x != name)
        .map(|x| {
            format!(
                "{place} carries `{x}`, which the receiver reads as `{name}` (key names match without case): spell it `{name}`"
            )
        })
}

/// The value `member` names from `entry` (object keys and array indexes separated by `/`),
/// with the place it was found at, quoted in the refusal message; `Err` names how far the
/// walk came when a segment is missing.
fn member_value<'a>(
    entry: &'a serde_json::Value,
    member: &str,
    base: &str,
) -> Result<(&'a serde_json::Value, String), String> {
    let mut place = base.to_owned();
    let mut cur = entry;
    for seg in member.split('/') {
        let next = match seg.parse::<usize>() {
            Ok(n) => cur.get(n),
            Err(_) => {
                if let Some(why) = cur.as_object().and_then(|o| folded_sibling(o, seg, &place)) {
                    return Err(why);
                }
                cur.get(seg)
            }
        };
        let Some(next) = next else {
            return Err(format!("{place} carries no `{seg}`"));
        };
        place.push('/');
        place.push_str(seg);
        cur = next;
    }
    Ok((cur, place))
}

/// Folds a key the way the receiver's JSON parser matches field names: without case, and with
/// the Kelvin sign (U+212A) and the long s (U+017F), which that matcher folds to `k` and `s`.
fn fold(s: &str) -> String {
    aurora_policy::json::fold(s).collect()
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
            "value": { "condition": "C" },
            "entry_at": {
                "data": [
                    { "at": 0, "key": "refId", "value": "A" },
                    { "at": 1, "key": "refId", "value": "B" },
                    { "at": 2, "key": "refId", "value": "C" },
                    { "at": 1, "member": "model", "key": "expression", "value": "A" },
                    { "at": 2, "member": "model", "key": "expression", "value": "B" }
                ]
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
        // (the server stores the model without it), everything else is the script's shape.
        let mut data = steps();
        data[0]
            .as_object_mut()
            .unwrap()
            .get_mut("model")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("refId");
        let b = serde_json::to_vec(&json!({ "condition": "C", "data": data })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
    }

    #[test]
    fn the_steps_refids_the_expression_links_and_the_condition_are_pinned() {
        let c = check();
        // The script's own body passes with the pins declared.
        let b = serde_json::to_vec(&json!({ "condition": "C", "data": steps() })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);

        // The exploit: the query step named `C` and the threshold step named `A`, the two
        // expression links pointed back through them, and the condition left at `C` — the
        // card shows `Condition step: C` and the threshold step's comparison while the
        // receiver fires on the raw query's value.
        let mut data = steps();
        data[0]["refId"] = json!("C");
        data[2]["refId"] = json!("A");
        data[1]["model"]["expression"] = json!("C");
        let b = serde_json::to_vec(&json!({ "condition": "C", "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("refId"), "{why}");

        // The condition naming the reduce step: the card would show a decision the
        // threshold step does not make.
        let b = serde_json::to_vec(&json!({ "condition": "B", "data": steps() })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("condition"), "{why}");

        // No condition at all: the card's Condition step field would show nothing.
        let b = serde_json::to_vec(&json!({ "data": steps() })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("no `condition`"), "{why}");

        // The two expression links swapped: the reduce step reads the threshold step.
        let mut data = steps();
        data[1]["model"]["expression"] = json!("C");
        data[2]["model"]["expression"] = json!("A");
        let b = serde_json::to_vec(&json!({ "condition": "C", "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("expression"), "{why}");

        // One swapped step refId, the rest the script's: the threshold step named `B`.
        let mut data = steps();
        data[2]["refId"] = json!("B");
        let b = serde_json::to_vec(&json!({ "condition": "C", "data": data })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("refId"), "{why}");
    }

    #[test]
    fn a_spelling_that_folds_onto_a_pinned_key_is_refused() {
        // The pin's own fold, with no other rule to lean on: the receiver reads key names
        // without case and keeps the last match, so `RefId` is `refId` to it whatever the
        // pin compares.
        let c = serde_json::from_value::<BodyCheck>(json!({
            "entry_at": { "data": [{ "at": 0, "key": "refId", "value": "A" }] }
        }))
        .unwrap();
        let b = serde_json::to_vec(&json!({ "data": [{ "refId": "A" }] })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
        for spelling in ["RefId", "REFID", "rEfid"] {
            let mut o = serde_json::Map::new();
            o.insert(spelling.to_owned(), json!("A"));
            let b = serde_json::to_vec(&json!({ "data": [Value::Object(o)] })).unwrap();
            let why = c.violation(b.as_slice(), false).unwrap();
            assert!(why.contains(&format!("`{spelling}`")), "{spelling}: {why}");
        }
        // A pin naming an entry the body does not carry is refused too (fail closed).
        let c = serde_json::from_value::<BodyCheck>(json!({
            "entry_at": { "data": [{ "at": 5, "key": "refId", "value": "A" }] }
        }))
        .unwrap();
        let b = serde_json::to_vec(&json!({ "data": [{ "refId": "A" }] })).unwrap();
        let why = c.violation(b.as_slice(), false).unwrap();
        assert!(why.contains("no `data`[5]"), "{why}");
    }

    #[test]
    fn the_first_steps_model_carries_only_the_keys_the_script_sends() {
        let c = check();
        // The script's own first step (the builder sends editorMode, expr, instant,
        // legendFormat, refId) passes, and so does the rule read back before an update,
        // whose model carries no refId (the server stores the model without it).
        for model in [
            json!({ "editorMode": "code", "expr": "up", "instant": true,
                    "legendFormat": "__auto", "refId": "A" }),
            json!({ "editorMode": "code", "expr": "up", "instant": true,
                    "legendFormat": "__auto" }),
        ] {
            let mut data = steps();
            data[0]["model"] = model;
            let b = serde_json::to_vec(&json!({ "condition": "C", "data": data })).unwrap();
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
        let b = serde_json::to_vec(&json!({ "condition": "C", "data": steps() })).unwrap();
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
        // The receiver's parser matches key names without case, so `Data` would silently
        // become part of the `data` the check read: refused at every level.
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

    /// The script's whole body as text, so a test can repeat a key in it.
    fn script_body() -> String {
        serde_json::to_string(&json!({
            "condition": "C", "data": steps(),
            "labels": { "managed_by": "night" },
            "annotations": { "summary": "p95" },
            "notification_settings": { "receiver": "oncall" }
        }))
        .unwrap()
    }

    /// `body` with `extra` added as the first member(s) of the object that starts `after` the
    /// given text.
    fn with_member(body: &str, after: &str, extra: &str) -> String {
        assert!(body.contains(after), "{after} is not in {body}");
        body.replacen(after, &format!("{after}{extra},"), 1)
    }

    fn twice(key: &str) -> String {
        format!("the body carries `{key}` twice: send each key once")
    }

    #[test]
    fn a_repeated_key_is_refused() {
        // A receiver need not keep only the last of a repeat: it may decode a repeated object
        // into the one it already filled, and keep what the first one added. The first value
        // here is what the check and the approval card would never see.
        let c = check();
        let base = script_body();
        assert_eq!(c.violation(base.as_bytes(), false), None);
        for (key, first) in [
            // An object key: routing labels, annotations, the receiver settings.
            ("labels", r#""labels":{"severity":"page"}"#),
            ("annotations", r#""annotations":{"runbook_url":"x"}"#),
            (
                "notification_settings",
                r#""notification_settings":{"mute_time_intervals":["nights"]}"#,
            ),
            // An array key.
            ("data", r#""data":[]"#),
            // A scalar, with the same value both times.
            ("condition", r#""condition":"C""#),
        ] {
            let b = with_member(&base, "{", first);
            assert_eq!(c.violation(b.as_bytes(), false), Some(twice(key)), "{b}");
        }
    }

    #[test]
    fn a_repeated_nested_key_is_refused() {
        let c = check();
        let base = script_body();
        for (key, after, first) in [
            (
                "receiver",
                r#""notification_settings":{"#,
                r#""receiver":"other""#,
            ),
            // Inside a data step, and inside the query step's model.
            ("refId", r#""data":[{"#, r#""refId":"A""#),
            ("expr", r#""model":{"#, r#""expr":"other""#),
            // The same key spelled with an escape is the same key.
            ("managed_by", r#""labels":{"#, r#""managed_by":"x""#),
        ] {
            let b = with_member(&base, after, first);
            assert_eq!(c.violation(b.as_bytes(), false), Some(twice(key)), "{b}");
        }
        // Wherever it appears: in an object no rule reads.
        let b = with_member(&base, r#""relativeTimeRange":{"#, r#""from":1"#);
        assert_eq!(c.violation(b.as_bytes(), false), Some(twice("from")), "{b}");
    }

    #[test]
    fn the_check_reads_every_object_as_the_bytes_spell_it() {
        // The JSON library's own `Value` parser (with its raw-value feature on) replaces an
        // object whose only member has this private name with the JSON text in its string.
        // The receiver sees an object with that member, so the check reads the same.
        let c = check();
        let b = serde_json::to_vec(&json!({
            "condition": "C", "data": steps(),
            "labels": { "$serde_json::private::RawValue": r#"{"managed_by":"night"}"# }
        }))
        .unwrap();
        let why = c.violation(&b, false).unwrap_or_default();
        assert!(why.contains("$serde_json::private::RawValue"), "{why}");
    }

    #[test]
    fn the_same_key_in_different_objects_is_no_repeat_and_trailing_data_is_refused() {
        let c = check();
        let base = script_body();
        // `refId`, `type` and `params` appear in many objects of the script's own body.
        assert_eq!(c.violation(base.as_bytes(), false), None);
        let b = format!("{base} {{}}");
        assert_eq!(
            c.violation(b.as_bytes(), false).unwrap(),
            "the body is not the JSON object this tool sends"
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
        let b = serde_json::to_vec(
            &json!({ "condition": "C", "data": [step("A", "postgres-logs"), step("B", "__expr__"), step("C", "__expr__")] }),
        )
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
        let b = serde_json::to_vec(&json!({ "condition": "C", "data": steps() })).unwrap();
        assert_eq!(c.violation(b.as_slice(), false), None);
    }

    // A `kinds` check of the keys a newer server adds to a rule it answers: the script
    // resends them unchanged, so neither may carry an object, and a count may not arrive
    // with a fractional part or as a string.
    fn kinds_check() -> BodyCheck {
        serde_json::from_value(json!({
            "allow_keys": ["title", "keep_firing_for", "missingSeriesEvalsToResolve"],
            "kinds": {
                "keep_firing_for": ["string", "number"],
                "missingSeriesEvalsToResolve": ["integer"]
            }
        }))
        .unwrap()
    }

    #[test]
    fn a_kinds_key_carries_the_declared_kind_when_present() {
        let c = kinds_check();
        // Keys the body does not carry are allowed (the server adds them only sometimes).
        assert_eq!(c.violation(br#"{"title":"r"}"#, false), None);
        // The declared kinds pass.
        assert_eq!(
            c.violation(
                br#"{"title":"r","keep_firing_for":"5m","missingSeriesEvalsToResolve":2}"#,
                false
            ),
            None
        );
        assert_eq!(
            c.violation(br#"{"title":"r","keep_firing_for":5}"#, false),
            None
        );
        assert_eq!(
            c.violation(br#"{"title":"r","missingSeriesEvalsToResolve":0}"#, false),
            None
        );
    }

    #[test]
    fn a_kinds_key_of_another_kind_is_refused() {
        let c = kinds_check();
        // An object where a scalar belongs.
        let b = br#"{"title":"r","keep_firing_for":{"for":"5m"}}"#;
        assert!(c
            .violation(b, false)
            .unwrap()
            .contains("`keep_firing_for` carries `{\"for\":\"5m\"}`: this tool sends `keep_firing_for` as a string or a number there"));
        // A fractional number is not an integer, and neither is a string.
        let b = br#"{"title":"r","missingSeriesEvalsToResolve":2.5}"#;
        assert!(c
            .violation(b, false)
            .unwrap()
            .contains("`missingSeriesEvalsToResolve` carries `2.5`: this tool sends `missingSeriesEvalsToResolve` as an integer there"));
        let b = br#"{"title":"r","missingSeriesEvalsToResolve":"2"}"#;
        assert!(c
            .violation(b, false)
            .unwrap()
            .contains("`missingSeriesEvalsToResolve` carries `\"2\"`: this tool sends `missingSeriesEvalsToResolve` as an integer there"));
    }

    // --- `one_of`, `entry_len`, `entry_max_len` and `values` ----------------------------

    /// Two shapes of one step's model behind a common base: a string query (`expr`), or a
    /// log query with exactly one metric.
    fn two_shapes() -> BodyCheck {
        serde_json::from_value(json!({
            "allow_keys": ["data"],
            "exact_len": { "data": 1 },
            "one_of": [
                { "entry_member": { "data": [
                    { "at": 0, "member": "model", "keys": ["expr", "refId"] }
                ] } },
                {
                    "entry_member": { "data": [
                        { "at": 0, "member": "model", "keys": ["query", "metrics", "refId"] },
                        { "at": 0, "member": "model/metrics/0", "keys": ["type", "field"],
                          "value": { "key": "type", "values": ["count", "sum"] } }
                    ] },
                    "entry_len": { "data": [ { "at": 0, "member": "model/metrics", "len": 1 } ] },
                    "entry_max_len": { "data": [
                        { "at": 0, "member": "model/query", "max": 10 },
                        { "at": 0, "member": "model/metrics/0/field", "max": 5, "optional": true }
                    ] }
                }
            ]
        }))
        .unwrap()
    }

    fn why(c: &BodyCheck, body: &Value) -> Option<String> {
        c.violation(&serde_json::to_vec(body).unwrap(), false)
    }

    fn model(m: Value) -> Value {
        json!({ "data": [ { "model": m } ] })
    }

    fn log_model() -> Value {
        json!({ "query": "status:500", "refId": "A", "metrics": [ { "type": "count" } ] })
    }

    #[test]
    fn one_of_passes_a_body_that_fits_any_variant_and_the_base() {
        let c = two_shapes();
        assert_eq!(why(&c, &model(json!({ "expr": "up" }))), None);
        assert_eq!(why(&c, &model(log_model())), None);
    }

    #[test]
    fn one_of_refuses_a_body_that_passes_the_base_but_no_variant_and_says_why_for_each() {
        let c = two_shapes();
        let w = why(&c, &model(json!({ "rawSql": "select 1" }))).unwrap();
        assert!(w.contains("none of the shapes"), "{w}");
        assert!(w.contains("shape 1: ") && w.contains("shape 2: "), "{w}");
        assert!(w.contains("`rawSql`"), "{w}");
        // A variant never loosens the base.
        let w = why(
            &c,
            &json!({ "data": [ { "model": { "expr": "up" } }, {} ] }),
        )
        .unwrap();
        assert!(w.contains("`data` has 2 entries"), "{w}");
        let w = why(
            &c,
            &json!({ "data": [ { "model": { "expr": "up" } } ], "x": 1 }),
        )
        .unwrap();
        assert!(w.contains("`x`"), "{w}");
    }

    #[test]
    fn one_of_refuses_a_body_with_one_variants_keys_and_the_others_pins() {
        let c = two_shapes();
        // The first variant's key beside the second's: neither key list holds both.
        let mut m = log_model();
        m["expr"] = json!("up");
        assert!(why(&c, &model(m)).is_some());
        // The first variant's model with a key only the second one reads.
        assert!(why(&c, &model(json!({ "expr": "up", "query": "x" }))).is_some());
        // The second variant's keys with a metric its value list does not hold.
        let mut m = log_model();
        m["metrics"][0]["type"] = json!("raw_data");
        let w = why(&c, &model(m)).unwrap();
        assert!(w.contains("raw_data") && w.contains("count"), "{w}");
    }

    #[test]
    fn one_of_refuses_a_repeated_key_and_a_truncated_body_before_any_variant() {
        let c = two_shapes();
        let b = br#"{"data":[{"model":{"expr":"up","expr":"down"}}]}"#;
        assert_eq!(c.violation(b, false), Some(twice("expr")));
        assert!(
            c.violation(br#"{"data":[{"model":{"expr":"up"}}]}"#, true)
                .is_some()
        );
    }

    #[test]
    fn entry_len_pins_the_length_of_an_array_along_a_path() {
        let c: BodyCheck = serde_json::from_value(json!({
            "entry_len": { "data": [ { "at": 0, "member": "model/metrics", "len": 1 } ] }
        }))
        .unwrap();
        assert_eq!(why(&c, &model(json!({ "metrics": [1] }))), None);
        for (m, want) in [
            (json!({ "metrics": [1, 2] }), "has 2 entries"),
            (json!({ "metrics": [] }), "has 0 entries"),
            (json!({ "metrics": { "0": {} } }), "must be a JSON array"),
            (json!({ "metrics": "x" }), "must be a JSON array"),
            (json!({}), "carries no `metrics`"),
        ] {
            let w = why(&c, &model(m.clone())).unwrap();
            assert!(w.contains(want), "{m}: {w}");
        }
        // Fail closed: no entry, no array, no key at all.
        assert!(why(&c, &json!({ "data": [] })).is_some());
        assert!(why(&c, &json!({ "data": "x" })).is_some());
        assert!(why(&c, &json!({})).is_some());
    }

    #[test]
    fn entry_max_len_wants_a_string_of_one_to_max_characters() {
        let c: BodyCheck = serde_json::from_value(json!({
            "entry_max_len": { "data": [ { "at": 0, "member": "model/query", "max": 5 } ] }
        }))
        .unwrap();
        assert_eq!(why(&c, &model(json!({ "query": "abcde" }))), None);
        // Characters, not bytes.
        assert_eq!(why(&c, &model(json!({ "query": "äöüäö" }))), None);
        for (m, want) in [
            (json!({ "query": "abcdef" }), "6 characters"),
            (json!({ "query": "" }), "1 to 5 characters"),
            (json!({ "query": { "a": 1 } }), "must be a string"),
            (json!({ "query": ["a"] }), "must be a string"),
            (json!({ "query": 5 }), "must be a string"),
            (json!({ "query": null }), "must be a string"),
            (json!({}), "carries no `query`"),
        ] {
            let w = why(&c, &model(m.clone())).unwrap();
            assert!(w.contains(want), "{m}: {w}");
        }
        assert!(why(&c, &json!({})).is_some());
    }

    #[test]
    fn an_optional_entry_max_len_member_may_be_absent_but_never_another_kind() {
        let c = two_shapes();
        // `field` is optional: absent passes, present is a string within the limit.
        let mut m = log_model();
        m["metrics"][0]["field"] = json!("ms");
        assert_eq!(why(&c, &model(m)), None);
        for field in [
            json!("abcdef"),
            json!(""),
            json!({ "script": "x" }),
            json!(1),
        ] {
            let mut m = log_model();
            m["metrics"][0]["field"] = field.clone();
            assert!(why(&c, &model(m)).is_some(), "{field} passed");
        }
        // Only the last segment may be absent: a missing parent is refused.
        let c: BodyCheck = serde_json::from_value(json!({
            "entry_max_len": { "data": [
                { "at": 0, "member": "model/metrics/0/field", "max": 5, "optional": true }
            ] }
        }))
        .unwrap();
        assert!(why(&c, &model(json!({ "metrics": [] }))).is_some());
        assert!(why(&c, &model(json!({}))).is_some());
        assert_eq!(why(&c, &model(json!({ "metrics": [ {} ] }))), None);
    }

    #[test]
    fn entry_greater_wants_two_whole_numbers_the_first_the_larger() {
        let c: BodyCheck = serde_json::from_value(json!({
            "entry_greater": { "data": [
                { "at": 0, "member": "range", "key": "from", "than": "to" }
            ] }
        }))
        .unwrap();
        let body = |range: Value| json!({ "data": [ { "range": range } ] });
        assert_eq!(why(&c, &body(json!({ "from": 360, "to": 60 }))), None);
        assert_eq!(why(&c, &body(json!({ "from": 1, "to": 0 }))), None);
        for (range, want) in [
            (json!({ "from": 60, "to": 60 }), "must be greater than"),
            (json!({ "from": 300, "to": 900 }), "must be greater than"),
            (json!({ "from": 300, "to": -60 }), "whole numbers"),
            (json!({ "from": "300", "to": 0 }), "whole numbers"),
            (json!({ "from": 300.5, "to": 0 }), "whole numbers"),
            (json!({ "from": 300, "to": { "a": 1 } }), "whole numbers"),
            (json!({ "from": 300 }), "carries no `to`"),
            (json!({ "to": 0 }), "carries no `from`"),
            (json!({ "from": 300, "to": 0, "From": 0 }), "without case"),
            (json!([300, 0]), "must be a JSON object"),
        ] {
            let w = why(&c, &body(range.clone())).unwrap_or_default();
            assert!(w.contains(want), "{range}: {w}");
        }
        assert!(why(&c, &json!({ "data": [ {} ] })).is_some());
        assert!(why(&c, &json!({ "data": [] })).is_some());
        assert!(why(&c, &json!({})).is_some());
    }

    #[test]
    fn a_folded_spelling_anywhere_on_a_new_rules_path_is_refused() {
        // With no key list to lean on: the receiver reads these names without case.
        for rule in [
            json!({ "entry_len": { "data": [ { "at": 0, "member": "model/skills", "len": 1 } ] } }),
            json!({ "entry_max_len": { "data": [
                { "at": 0, "member": "model/skills/0/kind", "max": 9, "optional": true }
            ] } }),
        ] {
            let c: BodyCheck = serde_json::from_value(rule).unwrap();
            let plain = json!({ "skills": [ { "kind": "a" } ] });
            assert_eq!(why(&c, &model(plain)), None);
            // `Model` beside nothing, the long s in `skills`, and (where the path reaches
            // it) the Kelvin sign in `kind`.
            for m in [
                json!({ "skills": [ { "kind": "a" } ], "Skills": [] }),
                json!({ "skills": [ { "kind": "a" } ], "\u{017F}kills": [] }),
            ] {
                let w = why(&c, &model(m.clone())).unwrap_or_default();
                assert!(w.contains("without case"), "{m}: {w}");
            }
        }
        let c: BodyCheck = serde_json::from_value(json!({
            "entry_max_len": { "data": [
                { "at": 0, "member": "model/kind", "max": 9, "optional": true }
            ] }
        }))
        .unwrap();
        let w = why(&c, &model(json!({ "\u{212A}ind": { "a": 1 } }))).unwrap_or_default();
        assert!(w.contains("without case"), "{w}");
        let w = why(&c, &json!({ "data": [ {} ], "Data": [] })).unwrap_or_default();
        assert!(w.contains("`Data`"), "{w}");
    }

    #[test]
    fn values_accept_any_of_the_listed_scalars_and_nothing_else() {
        let c: BodyCheck = serde_json::from_value(json!({
            "entry_at": { "data": [
                { "at": 0, "member": "model", "key": "reducer", "values": ["last", "sum", 3] }
            ] }
        }))
        .unwrap();
        for ok in [json!("last"), json!("sum"), json!(3)] {
            assert_eq!(why(&c, &model(json!({ "reducer": ok }))), None);
        }
        for bad in [
            json!("count"),
            json!("Sum"),
            json!("3"),
            json!(["sum"]),
            json!(null),
        ] {
            let w = why(&c, &model(json!({ "reducer": bad }))).unwrap();
            assert!(w.contains("one of"), "{bad}: {w}");
        }
        let w = why(&c, &model(json!({}))).unwrap();
        assert!(w.contains("no `reducer`"), "{w}");
        let w = why(&c, &model(json!({ "Reducer": "sum" }))).unwrap();
        assert!(w.contains("without case"), "{w}");
        // The same list on a member rule's value.
        let c = two_shapes();
        let mut m = log_model();
        m["metrics"][0]["type"] = json!("sum");
        assert_eq!(why(&c, &model(m)), None);
        let mut m = log_model();
        m["metrics"][0].as_object_mut().unwrap().remove("type");
        assert!(why(&c, &model(m)).is_some());
    }

    #[test]
    fn the_new_parts_change_nothing_in_a_serialized_check_that_does_not_use_them() {
        let text = serde_json::to_string(&check()).unwrap();
        for part in [
            "one_of",
            "entry_len",
            "entry_max_len",
            "entry_greater",
            "values",
        ] {
            assert!(!text.contains(part), "{part} appears in {text}");
        }
    }

    #[test]
    fn a_spelling_that_folds_onto_a_kinds_key_is_refused() {
        let c = kinds_check();
        let b = br#"{"title":"r","KEEP_FIRING_FOR":"5m"}"#;
        assert!(
            c.violation(b, false)
                .unwrap()
                .contains("which the receiver reads as `keep_firing_for`")
        );
    }
}

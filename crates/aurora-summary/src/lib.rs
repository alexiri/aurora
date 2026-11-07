//! aurora-summary: approval summaries (docs/09 §3.4 `approvals`, §5 decoders). The
//! generic redacting summarizer plus the template summarizer ([`template`]); the hook
//! call (`summarize: hook`) is made by the caller before [`template::summarize`].
#![forbid(unsafe_code)]

pub mod template;

use aurora_core::limits::{
    SUMMARY_FIELDS_MAX, SUMMARY_TEXT_MAX, SUMMARY_TITLE_MAX, SUMMARY_VALUE_MAX,
};
use aurora_core::sanitize::{block, one_line};
use serde::{Deserialize, Serialize};

/// Top-level JSON keys shown by the generic summarizer.
pub const GENERIC_JSON_KEYS_MAX: usize = 8;
const LABEL_MAX: usize = 60;
const LINK_MAX: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummarySource {
    Template,
    Hook,
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryField {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalSummary {
    pub title: String,
    pub fields: Vec<SummaryField>,
    pub text: Option<String>,
    pub link: Option<String>,
    pub group: Option<String>,
    pub source: SummarySource,
}

impl ApprovalSummary {
    /// Sanitizes every string and clamps to `SUMMARY_LIMITS`.
    #[must_use]
    pub fn clamp(self) -> Self {
        let link = self
            .link
            .map(|l| one_line(&l, LINK_MAX))
            .filter(|l| l.starts_with("https://") || l.starts_with("http://"));
        Self {
            title: one_line(&self.title, SUMMARY_TITLE_MAX),
            fields: self
                .fields
                .into_iter()
                .take(SUMMARY_FIELDS_MAX as usize)
                .map(|f| SummaryField {
                    label: one_line(&f.label, LABEL_MAX),
                    value: one_line(&f.value, SUMMARY_VALUE_MAX),
                })
                .collect(),
            text: self.text.map(|t| block(&t, SUMMARY_TEXT_MAX)),
            link,
            group: self.group.map(|g| one_line(&g, LABEL_MAX)),
            source: self.source,
        }
    }
}

/// `a=1&b=2` -> `a=***&b=***` (keys kept, values never shown).
#[must_use]
pub fn mask_query(query: &str) -> String {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, _)) => format!("{k}=***"),
            None => p.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn describe(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::String(s) => format!("string({})", s.chars().count()),
        Value::Number(_) => "number".to_owned(),
        Value::Bool(_) => "boolean".to_owned(),
        Value::Null => "null".to_owned(),
        Value::Array(a) => format!("array({} items)", a.len()),
        Value::Object(o) => format!("object({} keys)", o.len()),
    }
}

fn field(label: &str, value: impl Into<String>) -> SummaryField {
    SummaryField {
        label: label.to_owned(),
        value: value.into(),
    }
}

fn is_json(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|c| {
        let c = c
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        c == "application/json" || c.ends_with("+json")
    })
}

/// The generic redacting summary. Raw body values are never included: JSON
/// bodies show up to 8 top-level keys with the value's type/length only.
#[must_use]
pub fn generic(
    method: &str,
    host: &str,
    path: &str,
    query: Option<&str>,
    content_type: Option<&str>,
    body_prefix: &[u8],
    truncated: bool,
) -> ApprovalSummary {
    let full_path = match query.map(mask_query).filter(|q| !q.is_empty()) {
        Some(q) => format!("{path}?{q}"),
        None => path.to_owned(),
    };
    let mut fields = vec![
        field("Method", method),
        field("Host", host),
        field("Path", full_path.clone()),
    ];
    let parsed = if is_json(content_type) && !truncated {
        serde_json::from_slice::<serde_json::Value>(body_prefix).ok()
    } else {
        None
    };
    match parsed {
        Some(serde_json::Value::Object(o)) => {
            fields.extend(
                o.iter()
                    .take(GENERIC_JSON_KEYS_MAX)
                    .map(|(k, v)| field(k, describe(v))),
            );
            if o.len() > GENERIC_JSON_KEYS_MAX {
                fields.push(field(
                    "More",
                    format!("{} more keys", o.len() - GENERIC_JSON_KEYS_MAX),
                ));
            }
        }
        Some(other) => fields.push(field("Body", format!("JSON {}", describe(&other)))),
        None => {
            let n = body_prefix.len();
            let ct = content_type.unwrap_or("unknown type");
            let more = if truncated { "at least " } else { "" };
            fields.push(field("Body", format!("{more}{n} bytes ({ct})")));
        }
    }
    if truncated {
        fields.push(field("Note", "Body truncated; only a prefix was inspected"));
    }
    ApprovalSummary {
        title: format!("{method} {host}{full_path}"),
        fields,
        text: None,
        link: None,
        group: None,
        source: SummarySource::Generic,
    }
    .clamp()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn json_values_are_redacted() {
        let body = br#"{"to":"alice@example.com","amount":42,"nested":{"a":1,"b":2},"tags":[1,2]}"#;
        let s = generic(
            "POST",
            "api.example.com",
            "/v1/send",
            Some("token=abc&x=1"),
            Some("application/json; charset=utf-8"),
            body,
            false,
        );
        let j = serde_json::to_string(&s).unwrap();
        assert!(!j.contains("alice"), "{j}");
        assert!(!j.contains("abc"), "{j}");
        assert_eq!(s.title, "POST api.example.com/v1/send?token=***&x=***");
        let get = |l: &str| {
            s.fields
                .iter()
                .find(|f| f.label == l)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(get("to"), "string(17)");
        assert_eq!(get("amount"), "number");
        assert_eq!(get("nested"), "object(2 keys)");
        assert_eq!(get("tags"), "array(2 items)");
        assert_eq!(get("Method"), "POST");
        assert_eq!(s.source, SummarySource::Generic);
    }

    #[test]
    fn caps_json_keys_and_fields() {
        let mut m = serde_json::Map::new();
        for i in 0..30 {
            m.insert(format!("k{i:02}"), serde_json::json!("v"));
        }
        let body = serde_json::to_vec(&m).unwrap();
        let s = generic(
            "POST",
            "h",
            "/",
            None,
            Some("application/json"),
            &body,
            false,
        );
        assert!(s.fields.len() <= SUMMARY_FIELDS_MAX as usize);
        assert!(s.fields.iter().any(|f| f.label == "More"));
        assert_eq!(
            s.fields.iter().filter(|f| f.value == "string(1)").count(),
            8
        );
    }

    #[test]
    fn non_json_and_truncated() {
        let s = generic(
            "PUT",
            "h",
            "/f",
            None,
            Some("text/plain"),
            b"secret text",
            false,
        );
        assert_eq!(s.fields.last().unwrap().value, "11 bytes (text/plain)");
        assert!(!serde_json::to_string(&s).unwrap().contains("secret"));
        let s = generic(
            "PUT",
            "h",
            "/f",
            None,
            Some("application/json"),
            b"{\"a\":",
            true,
        );
        assert!(s.fields.iter().any(|f| f.label == "Note"));
        assert!(
            s.fields
                .iter()
                .any(|f| f.value.starts_with("at least 5 bytes"))
        );
        let s = generic("GET", "h", "/", None, None, b"", false);
        assert_eq!(s.fields.last().unwrap().value, "0 bytes (unknown type)");
    }

    #[test]
    fn clamps_and_sanitizes() {
        let s = ApprovalSummary {
            title: format!("a\nb\u{202e}{}", "x".repeat(500)),
            fields: (0..40)
                .map(|i| SummaryField {
                    label: format!("l{i}"),
                    value: "v\n".repeat(400),
                })
                .collect(),
            text: Some("t".repeat(5000)),
            link: Some("javascript:alert(1)".into()),
            group: None,
            source: SummarySource::Hook,
        }
        .clamp();
        assert!(s.title.chars().count() <= SUMMARY_TITLE_MAX);
        assert!(!s.title.contains('\n') && !s.title.contains('\u{202e}'));
        assert_eq!(s.fields.len(), 12);
        assert!(
            s.fields
                .iter()
                .all(|f| f.value.chars().count() <= SUMMARY_VALUE_MAX && !f.value.contains('\n'))
        );
        assert!(s.text.unwrap().chars().count() <= SUMMARY_TEXT_MAX);
        assert!(s.link.is_none());
    }

    #[test]
    fn serde_shape() {
        let s = generic("GET", "h", "/", None, None, b"", false);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["source"], "generic");
        assert!(v["text"].is_null() && v["link"].is_null() && v["group"].is_null());
    }
}

//! (P5-WEBHOOK-2, C-910, `07`, `09 §Decoders`) The `slots` summary: what an agent's send to an
//! outgoing webhook asks the person to approve. It shows the slot values by name and the host and
//! method, never a fixed part of the template, a header, a secret value or the URL. Each value is
//! clamped to [`VALUE_MAX`] characters with the full length shown.
use aurora_core::limits::{SUMMARY_FIELDS_MAX, SUMMARY_TITLE_MAX};
use aurora_core::sanitize::{block, one_line};
use serde_json::Value;

use crate::{ApprovalSummary, SummaryField, SummarySource};

/// Characters of one slot value shown.
pub const VALUE_MAX: usize = 500;
const LABEL_MAX: usize = 60;
const TEXT_MAX: usize = 4_000;

/// `title`, the receiver's `host` and `method`, and each given slot as `name: value`. A `json`
/// value is pretty-printed first; a value over [`VALUE_MAX`] characters is cut and followed by its
/// full length. Slots past the field limit go to `text`.
#[must_use]
pub fn summary<'a>(
    name: &str,
    host: &str,
    method: &str,
    slots: impl IntoIterator<Item = (&'a str, &'a Value)>,
) -> ApprovalSummary {
    let mut fields = vec![
        SummaryField {
            label: "Host".into(),
            value: one_line(host, VALUE_MAX),
        },
        SummaryField {
            label: "Method".into(),
            value: one_line(method, 16),
        },
    ];
    let mut overflow = String::new();
    for (slot, v) in slots {
        let shown = shown(v);
        if fields.len() < SUMMARY_FIELDS_MAX as usize {
            fields.push(SummaryField {
                label: one_line(slot, LABEL_MAX),
                value: shown,
            });
        } else {
            overflow.push_str(&format!("{}: {shown}\n", one_line(slot, LABEL_MAX)));
        }
    }
    ApprovalSummary {
        title: one_line(&format!("Send to {name}"), SUMMARY_TITLE_MAX),
        fields,
        text: (!overflow.is_empty()).then(|| block(&overflow, TEXT_MAX)),
        link: None,
        group: None,
        source: SummarySource::Template,
    }
}

fn shown(v: &Value) -> String {
    let full = match v {
        Value::String(s) => s.clone(),
        Value::Array(a) if a.iter().all(Value::is_string) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) | Value::Array(_) => serde_json::to_string_pretty(v).unwrap_or_default(),
        other => other.to_string(),
    };
    let n = full.chars().count();
    if n > VALUE_MAX {
        format!("{} … ({n} characters)", one_line(&full, VALUE_MAX))
    } else {
        one_line(&full, VALUE_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_are_shown_by_name_and_long_ones_are_cut_with_their_length() {
        let long = "x".repeat(900);
        let a = json!(["a", "b"]);
        let j = json!({"k": [1, 2]});
        let s = summary(
            "Ops alerts",
            "hooks.example.com",
            "POST",
            [
                ("text", &json!("Deploy 42 failed")),
                ("long", &json!(long)),
                ("tags", &a),
                ("j", &j),
                ("n", &json!(3)),
            ],
        );
        assert_eq!(s.title, "Send to Ops alerts");
        let get = |l: &str| {
            s.fields
                .iter()
                .find(|f| f.label == l)
                .map(|f| f.value.clone())
        };
        assert_eq!(get("Host").as_deref(), Some("hooks.example.com"));
        assert_eq!(get("Method").as_deref(), Some("POST"));
        assert_eq!(get("text").as_deref(), Some("Deploy 42 failed"));
        assert_eq!(get("tags").as_deref(), Some("a, b"));
        assert_eq!(get("n").as_deref(), Some("3"));
        let l = get("long").unwrap();
        assert!(
            l.ends_with("(900 characters)") && l.chars().count() < 540,
            "{l}"
        );
        assert!(get("j").unwrap().contains('k'));
    }

    #[test]
    fn a_card_holds_thirty_two_fields() {
        let vals: Vec<(String, Value)> = (0..30).map(|i| (format!("s{i}"), json!(i))).collect();
        let s = summary(
            "W",
            "h.example",
            "POST",
            vals.iter().map(|(k, v)| (k.as_str(), v)),
        );
        assert_eq!(s.fields.len(), 32);
        assert_eq!(s.text, None);
    }

    #[test]
    fn slots_past_the_field_limit_go_to_the_text() {
        let vals: Vec<(String, Value)> = (0..32).map(|i| (format!("s{i}"), json!(i))).collect();
        let s = summary(
            "W",
            "h.example",
            "POST",
            vals.iter().map(|(k, v)| (k.as_str(), v)),
        );
        assert_eq!(s.fields.len(), SUMMARY_FIELDS_MAX as usize);
        assert!(s.text.unwrap().contains("s31: 31"));
    }
}

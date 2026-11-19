//! (P5-GUIDE-6, `docs/24 §10a`) The bounded stdout summary of a response: the result discipline of
//! `24 §6` enforced by the tool. Never prints a request or response header (SEC-120).
use serde_json::Value;

use super::text::{bound, oneline};

const LINE_MAX: usize = 160;
const TEXT_LINES: usize = 12;
const KEYS_MAX: usize = 24;

/// What the preview says about one finished call.
pub struct Summary<'a> {
    pub status: u16,
    pub content_type: &'a str,
    pub bytes: usize,
    pub pages: Option<usize>,
    pub saved: Option<&'a str>,
    /// `Some(true)` more pages exist, `Some(false)` none, `None` unknown / not applicable.
    pub more: Option<bool>,
    pub notes: &'a [String],
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn short(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{:?}", oneline(s, 40)),
        Value::Number(_) | Value::Bool(_) | Value::Null => v.to_string(),
        Value::Array(a) => format!("[{} items]", a.len()),
        Value::Object(o) => format!("{{{} keys}}", o.len()),
    }
}

fn keys_of(v: &Value) -> String {
    match v {
        Value::Object(o) => {
            let mut k: Vec<&str> = o.keys().take(KEYS_MAX).map(String::as_str).collect();
            let more = o.len().saturating_sub(k.len());
            let mut s = k
                .drain(..)
                .map(|k| oneline(k, 40))
                .collect::<Vec<_>>()
                .join(", ");
            if more > 0 {
                s.push_str(&format!(", … (+{more})"));
            }
            s
        }
        other => kind(other).to_owned(),
    }
}

/// The body part of the preview: items of an array, keys of an object, first lines of text.
fn body_part(body: &[u8], content_type: &str) -> String {
    let is_json =
        content_type.contains("json") || body.first().is_some_and(|b| matches!(b, b'{' | b'['));
    if is_json && let Ok(v) = serde_json::from_slice::<Value>(body) {
        return json_part(&v);
    }
    match std::str::from_utf8(body) {
        Ok(t) => {
            let lines: Vec<String> = t
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(TEXT_LINES)
                .map(|l| oneline(l, LINE_MAX))
                .collect();
            if lines.is_empty() {
                "(empty body)\n".to_owned()
            } else {
                format!("{}\n", lines.join("\n"))
            }
        }
        Err(_) => "(binary body)\n".to_owned(),
    }
}

#[must_use]
pub fn json_part(v: &Value) -> String {
    match v {
        Value::Array(a) => {
            let mut o = format!("items: {}\n", a.len());
            if let Some(first) = a.first() {
                o.push_str(&format!("keys of the first item: {}\n", keys_of(first)));
            }
            for (i, item) in a.iter().take(3).enumerate() {
                o.push_str(&format!(
                    "[{}] {}\n",
                    i + 1,
                    oneline(&item.to_string(), LINE_MAX)
                ));
            }
            o
        }
        Value::Object(m) => {
            let mut o = String::from("keys:\n");
            for (k, x) in m.iter().take(KEYS_MAX) {
                o.push_str(&format!(
                    "  {} ({}) {}\n",
                    oneline(k, 40),
                    kind(x),
                    short(x)
                ));
            }
            if m.len() > KEYS_MAX {
                o.push_str(&format!("  … (+{} more)\n", m.len() - KEYS_MAX));
            }
            o
        }
        other => format!("{}\n", oneline(&other.to_string(), LINE_MAX)),
    }
}

/// The whole preview, at most `AGENT_CLI_PREVIEW_MAX` characters.
#[must_use]
pub fn render(s: &Summary<'_>, body: &[u8]) -> String {
    let mut o = format!(
        "HTTP {} · {} · {} bytes",
        s.status,
        if s.content_type.is_empty() {
            "no content type".to_owned()
        } else {
            oneline(s.content_type, 80)
        },
        s.bytes
    );
    if let Some(p) = s.pages {
        o.push_str(&format!(" · {p} page{}", if p == 1 { "" } else { "s" }));
    }
    o.push('\n');
    if let Some(path) = s.saved {
        o.push_str(&format!("saved: {}\n", oneline(path, 200)));
    }
    match s.more {
        Some(true) => o.push_str("more pages: yes (add --all to fetch them)\n"),
        Some(false) => o.push_str("more pages: no\n"),
        None => {}
    }
    for n in s.notes {
        o.push_str(&format!("note: {}\n", oneline(n, LINE_MAX)));
    }
    o.push_str(&body_part(body, s.content_type));
    bound(&o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_core::limits::AGENT_CLI_PREVIEW_MAX;

    fn sum<'a>(notes: &'a [String]) -> Summary<'a> {
        Summary {
            status: 200,
            content_type: "application/json",
            bytes: 1234,
            pages: Some(2),
            saved: Some("/tmp/aurora-agent/demo-list-20260101T000000Z.json"),
            more: Some(false),
            notes,
        }
    }

    #[test]
    fn an_array_shows_count_keys_and_three_items() {
        let body = serde_json::to_vec(&serde_json::json!([
            {"id": 1, "name": "a"}, {"id": 2, "name": "b"}, {"id": 3, "name": "c"}, {"id": 4, "name": "d"}
        ]))
        .unwrap();
        insta::assert_snapshot!("preview_array", render(&sum(&[]), &body));
    }

    #[test]
    fn an_object_shows_keys_with_types_and_short_values() {
        let body = serde_json::to_vec(&serde_json::json!({
            "total": 3, "ok": true, "name": "x".repeat(100), "items": [1, 2], "meta": {"a": 1}
        }))
        .unwrap();
        insta::assert_snapshot!("preview_object", render(&sum(&[]), &body));
    }

    #[test]
    fn text_shows_the_first_lines() {
        let mut s = sum(&[]);
        s.content_type = "text/plain";
        s.pages = None;
        s.more = None;
        let body = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
        insta::assert_snapshot!("preview_text", render(&s, body.as_bytes()));
    }

    #[test]
    fn a_hostile_or_huge_body_stays_bounded_and_on_one_line_per_item() {
        let huge: Vec<_> = (0..500)
            .map(|i| serde_json::json!({"k": format!("v{i}\n\u{1b}[31m{}", "x".repeat(500))}))
            .collect();
        let body = serde_json::to_vec(&huge).unwrap();
        let notes = vec!["stopped at the size limit".to_owned()];
        let out = render(&sum(&notes), &body);
        assert!(
            out.chars().count() <= AGENT_CLI_PREVIEW_MAX,
            "{}",
            out.len()
        );
        assert!(!out.contains('\u{1b}'));
        assert!(out.lines().all(|l| l.chars().count() <= LINE_MAX + 8));
    }
}

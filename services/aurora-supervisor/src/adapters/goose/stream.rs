//! `goose run --output-format stream-json` line -> [`HarnessEvent`] mapping
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`).
//!
//! stdout also carries a banner (a logo, the session line): only lines that start with `{` are
//! parsed. Mapping (pinned for [`super::PINNED_VERSION`]; the shapes in the tests were recorded from
//! a real 1.59.2 run against the mock upstream of `aurora-testkit`):
//! - `message` / `content` items `text` -> `TextDelta` (deltas under one message id), `toolRequest`
//!   -> `ToolStarted`, `toolResponse` -> `ToolFinished`; `systemNotification` and the rest are ignored.
//!   The platform tools (`aurora__<tool>`) are named `mcp__aurora__<tool>`, as for the other harnesses.
//! - `complete` -> the terminal `Done` with usage. goose's `input_tokens` **includes** the cached
//!   tokens, so Aurora's input is `input_tokens - cache_read - cache_write`. goose reports **no model**
//!   and no per-model split: one `by_model` entry, for the model the turn ran on (D-304). The cost
//!   field is never read.
//! - `error` -> the terminal `Error`, classified through the shared table (see [`classify`]).
//! - The stream ending without either (the process died, or refused to start) is a failed turn with
//!   goose's last stderr lines as the message.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::adapters::skill_use;
use crate::harness::HarnessEvent;

const SUMMARY_MAX: usize = 120;
const MESSAGE_MAX: usize = 500;

#[derive(Debug, Default, PartialEq)]
pub struct Mapped {
    pub events: Vec<HarnessEvent>,
    /// Set for `complete` / `error`; always a `Done` or `Error`.
    pub terminal: Option<HarnessEvent>,
    /// The line was a goose JSON event (the session exists from here on).
    pub json: bool,
}

#[derive(Debug)]
pub struct StreamMapper {
    /// The model the turn runs on: goose does not say.
    model: String,
    started: HashSet<String>,
    open: Vec<String>,
}

fn trunc(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn platform_name(name: &str) -> String {
    if name.starts_with("aurora__") {
        format!("mcp__{name}")
    } else {
        name.to_owned()
    }
}

/// (C-1175) The skill a call loads: a view of a skill's `SKILL.md` with goose's text editor, or
/// a read command in its shell.
fn loaded_skill(name: &str, args: Option<&Value>) -> Option<String> {
    let tool = name.strip_prefix("developer__").unwrap_or(name);
    match tool {
        "text_editor"
            if args.and_then(|a| a.get("command")).and_then(Value::as_str) == Some("view") =>
        {
            skill_use::of_read(args, super::SKILLS_DIR)
        }
        "shell" => skill_use::of_shell(args, super::SKILLS_DIR),
        _ => None,
    }
}

fn summary(name: &str, args: Option<&Value>) -> String {
    let obj = args.and_then(Value::as_object);
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| obj.and_then(|o| o.get(*k)).and_then(Value::as_str))
    };
    if let Some(c) = pick(&["command", "path", "file_path", "instructions"]) {
        return format!(
            "{name}: {}",
            trunc(c.lines().next().unwrap_or(c), SUMMARY_MAX)
        );
    }
    format!("{name} ({} args)", obj.map_or(0, serde_json::Map::len))
}

impl StreamMapper {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            started: HashSet::new(),
            open: Vec::new(),
        }
    }

    pub fn map_line(&mut self, line: &str) -> Mapped {
        let mut out = Mapped::default();
        if line == super::launcher::OVERLONG_LINE {
            out.terminal = Some(HarnessEvent::Error {
                code: "harness_failed".into(),
                message: "goose produced an output line over the size limit".into(),
            });
            return out;
        }
        if !line.starts_with('{') {
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return out;
        };
        out.json = true;
        match v.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(m) = v.get("message") {
                    self.message(m, &mut out);
                }
            }
            Some("complete") => {
                self.finish_open(&mut out);
                out.terminal = Some(HarnessEvent::Done {
                    usage: Some(self.usage(&v)),
                });
            }
            Some("error") => {
                self.finish_open(&mut out);
                let text = v.get("error").and_then(Value::as_str).unwrap_or_default();
                let (code, message) = classify(text);
                out.terminal = Some(HarnessEvent::Error { code, message });
            }
            _ => {}
        }
        out
    }

    fn message(&mut self, m: &Value, out: &mut Mapped) {
        let id = m.get("id").and_then(Value::as_str).unwrap_or_default();
        let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
        let Some(items) = m.get("content").and_then(Value::as_array) else {
            return;
        };
        for c in items {
            match c.get("type").and_then(Value::as_str) {
                Some("text") if role == "assistant" => {
                    let t = c.get("text").and_then(Value::as_str).unwrap_or_default();
                    if !t.is_empty() {
                        out.events.push(HarnessEvent::TextDelta {
                            message_id: id.to_owned(),
                            delta: t.to_owned(),
                            reply_to: None,
                        });
                    }
                }
                Some("toolRequest") => {
                    let call = c.get("id").and_then(Value::as_str).unwrap_or_default();
                    let value = c.pointer("/toolCall/value");
                    let name = value
                        .and_then(|v| v.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    if !call.is_empty() && self.started.insert(call.to_owned()) {
                        self.open.push(call.to_owned());
                        let name = platform_name(name);
                        let args = value.and_then(|v| v.get("arguments"));
                        out.events.push(HarnessEvent::ToolStarted {
                            call_id: call.to_owned(),
                            summary: summary(&name, args),
                            skill: loaded_skill(&name, args),
                            name,
                        });
                    }
                }
                Some("toolResponse") => {
                    let call = c.get("id").and_then(Value::as_str).unwrap_or_default();
                    let Some(i) = self.open.iter().position(|o| o == call) else {
                        continue;
                    };
                    self.open.remove(i);
                    let r = c.get("toolResult");
                    let ok = r.and_then(|r| r.get("status")).and_then(Value::as_str)
                        == Some("success")
                        && !r
                            .and_then(|r| r.pointer("/value/isError"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                    let text = r
                        .and_then(|r| r.pointer("/value/content"))
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|b| b.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .or_else(|| {
                            r.and_then(|r| r.pointer("/error/message"))
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    out.events.push(HarnessEvent::ToolFinished {
                        call_id: call.to_owned(),
                        ok,
                        summary: (!text.is_empty()).then(|| trunc(text.trim_end(), SUMMARY_MAX)),
                    });
                }
                _ => {}
            }
        }
    }

    /// A call still open when the turn ends is finished as failed, so every start has its finish.
    fn finish_open(&mut self, out: &mut Mapped) {
        for call in std::mem::take(&mut self.open) {
            out.events.push(HarnessEvent::ToolFinished {
                call_id: call,
                ok: false,
                summary: Some("the turn ended before the tool finished".into()),
            });
        }
    }

    fn usage(&self, v: &Value) -> Value {
        let n = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0).max(0);
        let (read, write) = (n("cache_read_input_tokens"), n("cache_write_input_tokens"));
        let input = (n("input_tokens") - read - write).max(0);
        let output = n("output_tokens");
        json!({
            "input_tokens": input,
            "output_tokens": output,
            "cache_read_tokens": read,
            "cache_write_tokens": write,
            "by_model": [{
                "model": self.model,
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_tokens": read,
                "cache_write_tokens": write,
            }],
        })
    }
}

/// Maps goose's error text (`Ran into this error: Authentication error: … Status: 401 …`,
/// `Rate limit exceeded: …`, `Server error (500 …)`) to `(code, message)` through the shared
/// classification (`12 §Provider refusal classification`). goose calls every 429 a rate limit, so
/// a quota or billing phrase is read first.
pub fn classify(text: &str) -> (String, String) {
    let text = text
        .strip_prefix("Ran into this error: ")
        .unwrap_or(text)
        .split("\n\nPlease retry")
        .next()
        .unwrap_or(text);
    if text.trim().is_empty() {
        return ("harness_failed".into(), "goose failed".into());
    }
    let lower = text.to_ascii_lowercase();
    // goose writes a server error as `Server error (500 Internal Server Error) at …`: the shared
    // classification reads a status after the word `status`.
    let status = text.find("rror (").and_then(|i| {
        let d = text.get(i + 6..i + 9)?;
        (d.bytes().all(|b| b.is_ascii_digit())).then_some(d)
    });
    let with_status;
    let for_table = if let Some(s) = status
        && !lower.contains("status")
    {
        with_status = format!("status {s} {text}");
        with_status.as_str()
    } else {
        text
    };
    let (mut code, mut message) = crate::adapters::codex::stream::classify_failure(for_table);
    if lower.contains("quota") || lower.contains("billing") || lower.contains("credit balance") {
        code = "provider_quota".into();
    }
    if message == "codex failed" {
        message = "goose failed".into();
    }
    if let Some(s) = status
        && let Some(rest) = message.strip_prefix(&format!("status {s} "))
    {
        message = rest.to_owned();
    }
    (code, trunc(&message, MESSAGE_MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> (Vec<HarnessEvent>, Option<HarnessEvent>) {
        let mut m = StreamMapper::new("m1");
        let (mut ev, mut term) = (Vec::new(), None);
        for l in lines {
            let r = m.map_line(l);
            ev.extend(r.events);
            term = term.or(r.terminal);
        }
        (ev, term)
    }

    /// (C-1175) goose reads a skill with a text-editor view or a command in its shell.
    #[test]
    fn viewing_a_skill_md_names_the_skill() {
        let req = |id: &str, name: &str, args: &str| {
            format!(
                r#"{{"type":"message","message":{{"id":"m{id}","role":"assistant","content":[{{"type":"toolRequest","id":"{id}","toolCall":{{"status":"success","value":{{"name":"{name}","arguments":{args}}}}}}}]}}}}"#
            )
        };
        let path = r#""/workspace/.agents/skills/pdf/SKILL.md""#;
        let lines = [
            req(
                "a",
                "developer__text_editor",
                &format!(r#"{{"command":"view","path":{path}}}"#),
            ),
            req(
                "b",
                "developer__text_editor",
                &format!(r#"{{"command":"write","path":{path}}}"#),
            ),
            req(
                "c",
                "shell",
                &format!(r#"{{"command":"cat {}"}}"#, path.trim_matches('"')),
            ),
        ];
        let (ev, _) = run(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        let got: Vec<Option<String>> = ev
            .iter()
            .filter_map(|e| match e {
                HarnessEvent::ToolStarted { skill, .. } => Some(skill.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(got, [Some("pdf".into()), None, Some("pdf".into())]);
    }

    // Lines recorded from goose 1.59.2 (trimmed: the metadata objects are dropped).
    const TEXT1: &str = r#"{"type":"message","message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"po"}]}}"#;
    const TEXT2: &str = r#"{"type":"message","message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"ng"}]}}"#;
    const REQ: &str = r#"{"type":"message","message":{"id":"msg_2","role":"assistant","content":[{"type":"toolRequest","id":"toolu_1","toolCall":{"status":"success","value":{"name":"shell","arguments":{"command":"echo hi\nmore"}}}}]}}"#;
    const RESP: &str = r#"{"type":"message","message":{"id":"msg_3","role":"user","content":[{"type":"toolResponse","id":"toolu_1","toolResult":{"status":"success","value":{"content":[{"type":"text","text":"hi\n"}]}}}]}}"#;
    const COMPLETE: &str = r#"{"type":"complete","total_tokens":20,"input_tokens":16,"output_tokens":4,"cache_read_input_tokens":3,"cache_write_input_tokens":2}"#;

    #[test]
    fn a_text_turn_streams_deltas_and_ends_with_usage_whose_input_excludes_the_cache() {
        let (ev, term) = run(&[
            "   __( O)>  ● new session · anthropic say",
            "goose is ready",
            TEXT1,
            TEXT2,
            COMPLETE,
        ]);
        assert_eq!(
            ev,
            [
                HarnessEvent::TextDelta {
                    message_id: "msg_1".into(),
                    delta: "po".into(),
                    reply_to: None
                },
                HarnessEvent::TextDelta {
                    message_id: "msg_1".into(),
                    delta: "ng".into(),
                    reply_to: None
                },
            ]
        );
        let Some(HarnessEvent::Done { usage: Some(u) }) = term else {
            panic!("{term:?}")
        };
        // 16 reported = 11 fresh + 3 read + 2 written.
        assert_eq!(
            (
                u["input_tokens"].clone(),
                u["output_tokens"].clone(),
                u["cache_read_tokens"].clone(),
                u["cache_write_tokens"].clone()
            ),
            (json!(11), json!(4), json!(3), json!(2))
        );
        assert_eq!(
            u["by_model"],
            json!([{"model": "m1", "input_tokens": 11, "output_tokens": 4, "cache_read_tokens": 3, "cache_write_tokens": 2}]),
            "one entry for the turn's model: goose reports no model"
        );
        assert!(!u.to_string().contains("cost"));
    }

    #[test]
    fn tools_start_and_finish_with_their_ids_and_platform_tools_get_the_mcp_name() {
        let plat = r#"{"type":"message","message":{"id":"m","role":"assistant","content":[{"type":"toolRequest","id":"t2","toolCall":{"status":"success","value":{"name":"aurora__ping","arguments":{"a":1}}}}]}}"#;
        let (ev, term) = run(&[REQ, RESP, plat, COMPLETE]);
        assert_eq!(
            ev,
            [
                HarnessEvent::ToolStarted {
                    call_id: "toolu_1".into(),
                    name: "shell".into(),
                    summary: "shell: echo hi".into(),
                    skill: None
                },
                HarnessEvent::ToolFinished {
                    call_id: "toolu_1".into(),
                    ok: true,
                    summary: Some("hi".into())
                },
                HarnessEvent::ToolStarted {
                    call_id: "t2".into(),
                    name: "mcp__aurora__ping".into(),
                    summary: "mcp__aurora__ping (1 args)".into(),
                    skill: None
                },
                HarnessEvent::ToolFinished {
                    call_id: "t2".into(),
                    ok: false,
                    summary: Some("the turn ended before the tool finished".into())
                },
            ]
        );
        assert!(matches!(term, Some(HarnessEvent::Done { .. })));
    }

    #[test]
    fn a_tool_error_is_a_finished_call_that_is_not_ok() {
        let bad = r#"{"type":"message","message":{"id":"x","role":"user","content":[{"type":"toolResponse","id":"toolu_1","toolResult":{"status":"error","error":{"message":"denied"}}}]}}"#;
        let (ev, _) = run(&[REQ, bad]);
        assert!(
            matches!(&ev[1], HarnessEvent::ToolFinished { ok: false, summary: Some(s), .. } if s == "denied"),
            "{ev:?}"
        );
    }

    #[test]
    fn errors_are_the_terminal_event_classified_through_the_shared_table() {
        for (text, want) in [
            (
                "Ran into this error: Authentication error: Authentication failed for http://x/v1/messages. Status: 401 Unauthorized. Response: bad.\n\nPlease retry if you think this is a transient or recoverable error.",
                "provider_auth",
            ),
            (
                "Ran into this error: Rate limit exceeded: slow down.\n\nPlease retry if you think this is a transient or recoverable error.",
                "provider_rate_limited",
            ),
            (
                "Ran into this error: Rate limit exceeded: You exceeded your current quota, please check your plan and billing details.",
                "provider_quota",
            ),
            (
                "Ran into this error: Server error: Server error (500 Internal Server Error) at http://x: boom.",
                "provider_overloaded",
            ),
            (
                "Ran into this error: Network error: Stream decode error: x",
                "harness_failed",
            ),
        ] {
            let line = json!({"type": "error", "error": text}).to_string();
            let (_, term) = run(&[&line]);
            let Some(HarnessEvent::Error { code, message }) = term else {
                panic!("{term:?}")
            };
            assert_eq!(code, want, "{text}");
            assert!(
                !message.contains("Ran into this error") && !message.contains("Please retry"),
                "{message}"
            );
        }
        assert_eq!(classify("").0, "harness_failed");
        assert_eq!(classify("").1, "goose failed");
    }

    #[test]
    fn banner_noise_overlong_lines_and_unknown_events_are_handled() {
        let mut m = StreamMapper::new("m");
        for noise in [
            "",
            "   \\____)    20261118_1 · /tmp/x",
            "goose is ready",
            "{not json",
            r#"{"type":"notification"}"#,
        ] {
            assert_eq!(m.map_line(noise).terminal, None, "{noise}");
            assert!(m.map_line(noise).events.is_empty(), "{noise}");
        }
        assert!(!m.map_line("   banner").json);
        assert!(m.map_line(r#"{"type":"notification"}"#).json);
        let r = m.map_line(super::super::launcher::OVERLONG_LINE);
        assert!(
            matches!(r.terminal, Some(HarnessEvent::Error { ref code, .. }) if code == "harness_failed")
        );
    }
}

//! `codex exec --json` JSONL line -> [`HarnessEvent`] mapping and error classification.
//!
//! Mapping (pinned for [`super::PINNED_VERSION`]; `fixtures/text_turn.jsonl`, `error_*.jsonl` and
//! the command item of `tool_turn.jsonl` were recorded from a real 0.167.0 run against a scripted
//! Responses upstream, the MCP and file-change items are hand-authored from the documented item
//! shapes; the live suite `AURORA_TEST_HARNESS_CODEX_LIVE=1` verifies them):
//! - `thread.started` -> `SessionRef` (the Codex thread id is the session reference).
//! - `item.started` / `item.completed` of `command_execution` and `mcp_tool_call` ->
//!   `ToolStarted` / `ToolFinished`; `file_change` -> a started+finished pair;
//!   `web_search` -> an activity line.
//! - `item.completed` `agent_message` -> `TextDelta` (the whole message: `exec --json` does not
//!   stream deltas); `reasoning` -> `ThinkingDelta`; `error` items are non-fatal notes and are
//!   only logged; non-terminal `error` events (retries) become activity lines.
//! - `turn.completed` -> terminal `Done{usage}`; `turn.failed` -> terminal `Error{code,message}`
//!   (see [`classify_failure`]).
//! - Unknown lines and types are ignored.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::harness::HarnessEvent;

const SUMMARY_MAX: usize = 120;
const MESSAGE_MAX: usize = 500;
/// Prefix of platform tools, as `claude-code` names them.
const MCP_TOOL_PREFIX: &str = "mcp__";

#[derive(Debug, Default, PartialEq)]
pub struct Mapped {
    pub events: Vec<HarnessEvent>,
    /// Set for `turn.completed` / `turn.failed`; always a `Done` or `Error`.
    pub terminal: Option<HarnessEvent>,
}

#[derive(Debug, Default)]
pub struct StreamMapper {
    started: HashSet<String>,
}

fn trunc(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// `/bin/bash -lc 'echo hi'` -> `echo hi`.
fn unwrap_shell(cmd: &str) -> &str {
    for prefix in ["/bin/bash -lc ", "bash -lc ", "/bin/sh -c ", "sh -c "] {
        if let Some(rest) = cmd.strip_prefix(prefix) {
            let r = rest.trim();
            for q in ['\'', '"'] {
                if let Some(inner) = r.strip_prefix(q).and_then(|x| x.strip_suffix(q)) {
                    return inner;
                }
            }
            return r;
        }
    }
    cmd
}

/// A Codex thread id is a canonical hyphenated UUID.
pub fn is_thread_id(id: &str) -> bool {
    id.len() == 36 && uuid::Uuid::parse_str(id).is_ok()
}

impl StreamMapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears per-turn state (call when a new prompt is sent).
    pub fn reset_turn(&mut self) {
        self.started.clear();
    }

    pub fn map_line(&mut self, line: &str) -> Mapped {
        let mut out = Mapped::default();
        if line == super::launcher::OVERLONG_LINE {
            out.terminal = Some(HarnessEvent::Error {
                code: "harness_failed".into(),
                message: "codex produced an output line over the size limit".into(),
            });
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return out;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("thread.started") => {
                // The id names rollout files and becomes an argv word: only a thread-id shaped
                // value (a UUID) is kept, anything else is ignored (FIX-095).
                if let Some(id) = v
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .filter(|id| is_thread_id(id))
                {
                    out.events.push(HarnessEvent::SessionRef {
                        session_ref: id.to_owned(),
                    });
                }
            }
            Some("item.started") => {
                if let Some(i) = v.get("item") {
                    self.item(i, false, &mut out);
                }
            }
            Some("item.completed") => {
                if let Some(i) = v.get("item") {
                    self.item(i, true, &mut out);
                }
            }
            Some("error") => {
                // A non-terminal error event is a retry note ("Reconnecting... 2/5 (...)").
                if let Some(m) = v.get("message").and_then(Value::as_str)
                    && m.starts_with("Reconnecting")
                {
                    out.events.push(HarnessEvent::Activity {
                        line: "retrying the model request".into(),
                    });
                }
            }
            Some("turn.completed") => out.terminal = Some(done(&v)),
            Some("turn.failed") => {
                let m = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let (code, message) = classify_failure(m);
                out.terminal = Some(HarnessEvent::Error { code, message });
            }
            _ => {}
        }
        out
    }

    fn item(&mut self, i: &Value, completed: bool, out: &mut Mapped) {
        let id = i.get("id").and_then(Value::as_str).unwrap_or_default();
        match i.get("type").and_then(Value::as_str) {
            Some("agent_message") if completed => {
                if let Some(t) = i.get("text").and_then(Value::as_str)
                    && !t.is_empty()
                {
                    out.events.push(HarnessEvent::TextDelta {
                        message_id: id.to_owned(),
                        delta: t.to_owned(),
                        reply_to: None,
                    });
                }
            }
            Some("reasoning") if completed => {
                if let Some(t) = i.get("text").and_then(Value::as_str)
                    && !t.is_empty()
                {
                    out.events.push(HarnessEvent::ThinkingDelta {
                        delta: t.to_owned(),
                    });
                }
            }
            Some("command_execution") => {
                let cmd = i.get("command").and_then(Value::as_str).unwrap_or_default();
                if self.started.insert(id.to_owned()) {
                    out.events.push(HarnessEvent::ToolStarted {
                        call_id: id.to_owned(),
                        name: "shell".into(),
                        summary: format!("shell: {}", trunc(unwrap_shell(cmd), SUMMARY_MAX)),
                    });
                }
                if completed {
                    let code = i.get("exit_code").and_then(Value::as_i64);
                    let status = i.get("status").and_then(Value::as_str);
                    let ok = status != Some("failed") && code.is_none_or(|c| c == 0);
                    let text = i
                        .get("aggregated_output")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    out.events.push(HarnessEvent::ToolFinished {
                        call_id: id.to_owned(),
                        ok,
                        summary: (!text.is_empty()).then(|| trunc(text.trim_end(), SUMMARY_MAX)),
                    });
                }
            }
            Some("mcp_tool_call") => {
                let server = i.get("server").and_then(Value::as_str).unwrap_or("mcp");
                let tool = i.get("tool").and_then(Value::as_str).unwrap_or("tool");
                if self.started.insert(id.to_owned()) {
                    let name = format!("{MCP_TOOL_PREFIX}{server}__{tool}");
                    let n = i
                        .get("arguments")
                        .and_then(Value::as_object)
                        .map_or(0, serde_json::Map::len);
                    out.events.push(HarnessEvent::ToolStarted {
                        call_id: id.to_owned(),
                        summary: format!("{name} ({n} args)"),
                        name,
                    });
                }
                if completed {
                    let failed = i.get("status").and_then(Value::as_str) == Some("failed")
                        || !i.get("error").is_none_or(Value::is_null);
                    let text = i
                        .pointer("/result/content")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|b| b.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .or_else(|| {
                            i.pointer("/error/message")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    out.events.push(HarnessEvent::ToolFinished {
                        call_id: id.to_owned(),
                        ok: !failed,
                        summary: (!text.is_empty()).then(|| trunc(&text, SUMMARY_MAX)),
                    });
                }
            }
            Some("file_change") if completed => {
                let paths: Vec<&str> = i
                    .get("changes")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.get("path").and_then(Value::as_str))
                            .collect()
                    })
                    .unwrap_or_default();
                let summary = format!("apply_patch: {}", trunc(&paths.join(", "), SUMMARY_MAX));
                out.events.push(HarnessEvent::ToolStarted {
                    call_id: id.to_owned(),
                    name: "apply_patch".into(),
                    summary: summary.clone(),
                });
                out.events.push(HarnessEvent::ToolFinished {
                    call_id: id.to_owned(),
                    ok: i.get("status").and_then(Value::as_str) != Some("failed"),
                    summary: Some(summary),
                });
            }
            Some("web_search") if !completed => {
                let q = i.get("query").and_then(Value::as_str).unwrap_or_default();
                out.events.push(HarnessEvent::Activity {
                    line: format!("web search: {}", trunc(q, SUMMARY_MAX)),
                });
            }
            Some("error") => {
                let m = i.get("message").and_then(Value::as_str).unwrap_or_default();
                tracing::debug!(target: "codex", message = %trunc(m, MESSAGE_MAX), "non-fatal codex note");
            }
            _ => {}
        }
    }
}

fn done(v: &Value) -> HarnessEvent {
    let u = v.get("usage");
    let n = |k: &str| u.and_then(|u| u.get(k)).and_then(Value::as_i64);
    HarnessEvent::Done {
        usage: u.map(|_| {
            json!({
                "input_tokens": n("input_tokens").unwrap_or(0),
                "output_tokens": n("output_tokens").unwrap_or(0),
                "cache_read_tokens": n("cached_input_tokens"),
                "reasoning_tokens": n("reasoning_output_tokens"),
            })
        }),
    }
}

/// First `4xx`/`5xx` number that follows the word `status` in `text`.
fn parse_status(lower: &str) -> Option<u64> {
    let i = lower.find("status")?;
    lower[i + 6..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|t| t.len() == 3)
        .and_then(|t| t.parse().ok())
}

/// Maps a failed turn's message to `(harness error code, message)` (`12 §Provider refusal
/// classification`). Codex reports provider errors as free text (`unexpected status 401
/// Unauthorized: <body>`), so classification reads the status and the documented provider
/// phrases. A gateway envelope (`{"error":{"code":…}}`, which provider errors never carry: they
/// use `error.type`) yields `gateway_refused` with the message `[<gateway code>] <message>`.
pub fn classify_failure(text: &str) -> (String, String) {
    let body: Option<Value> = text
        .find('{')
        .and_then(|i| serde_json::from_str(&text[i..]).ok())
        .or_else(|| {
            // `unexpected status …: {json}, url: …`: the JSON ends before the trailing `, url:`.
            let start = text.find('{')?;
            let end = text.rfind('}')?;
            serde_json::from_str(text.get(start..=end)?).ok()
        });
    let err = body.as_ref().and_then(|b| b.get("error"));
    let inner = err
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(text);
    if let Some(code) = err.and_then(|e| e.get("code")).and_then(Value::as_str)
        && err.and_then(|e| e.get("type")).is_none()
    {
        return (
            "gateway_refused".into(),
            trunc(&format!("[{code}] {inner}"), MESSAGE_MAX),
        );
    }
    let lower = text.to_ascii_lowercase();
    let status = parse_status(&lower);
    let err_type = err
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let err_code = err
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let quota = lower.contains("quota exceeded")
        || lower.contains("insufficient_quota")
        || err_code == "insufficient_quota"
        || err_type == "insufficient_quota"
        || lower.contains("billing")
        || status == Some(402);
    let code = if quota {
        "provider_quota"
    } else if matches!(status, Some(401 | 403))
        || lower.contains("unauthorized")
        || lower.contains("incorrect api key")
        || lower.contains("invalid api key")
        || err_type == "authentication_error"
        || err_type == "permission_error"
    {
        "provider_auth"
    } else if status == Some(429)
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || err_type == "rate_limit_error"
    {
        "provider_rate_limited"
    } else if matches!(status, Some(500..=504 | 529))
        || lower.contains("high demand")
        || lower.contains("overloaded")
        || err_type == "overloaded_error"
    {
        "provider_overloaded"
    } else {
        "harness_failed"
    };
    let message = if inner.is_empty() {
        "codex failed".to_owned()
    } else {
        trunc(inner, MESSAGE_MAX)
    };
    (code.into(), message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = include_str!("fixtures/text_turn.jsonl");
    const TOOL: &str = include_str!("fixtures/tool_turn.jsonl");

    fn run(fixture: &str) -> (Vec<HarnessEvent>, Option<HarnessEvent>) {
        let mut m = StreamMapper::new();
        let (mut ev, mut term) = (Vec::new(), None);
        for l in fixture.lines() {
            let r = m.map_line(l);
            ev.extend(r.events);
            term = term.or(r.terminal);
        }
        (ev, term)
    }

    fn terminal(fixture: &str) -> (String, String) {
        match run(fixture).1 {
            Some(HarnessEvent::Error { code, message }) => (code, message),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn text_turn_maps_ref_thinking_text_and_usage() {
        let (ev, term) = run(TEXT);
        assert_eq!(
            ev,
            vec![
                HarnessEvent::SessionRef {
                    session_ref: "01a1c3dc-f235-78e1-920f-5c889ce01480".into()
                },
                HarnessEvent::ThinkingDelta {
                    delta: "Checking the request.".into()
                },
                HarnessEvent::TextDelta {
                    message_id: "item_2".into(),
                    delta: "pong".into(),
                    reply_to: None
                },
            ]
        );
        assert_eq!(
            term,
            Some(HarnessEvent::Done {
                usage: Some(json!({"input_tokens": 5, "output_tokens": 2,
                                   "cache_read_tokens": 2, "reasoning_tokens": 1}))
            })
        );
    }

    #[test]
    fn tool_turn_pairs_every_start_with_a_finish() {
        let (ev, term) = run(TOOL);
        assert!(matches!(term, Some(HarnessEvent::Done { .. })));
        let started: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                HarnessEvent::ToolStarted { call_id, name, .. } => {
                    Some((call_id.as_str(), name.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            started,
            [
                ("item_1", "shell"),
                ("item_2", "mcp__aurora__memory_save"),
                ("item_3", "shell"),
                ("item_4", "apply_patch")
            ]
        );
        for (id, _) in &started {
            assert_eq!(
                ev.iter()
                    .filter(
                        |e| matches!(e, HarnessEvent::ToolFinished { call_id, .. } if call_id == id)
                    )
                    .count(),
                1,
                "{id}"
            );
        }
        let ok = |id: &str| {
            ev.iter().find_map(|e| match e {
                HarnessEvent::ToolFinished { call_id, ok, .. } if call_id == id => Some(*ok),
                _ => None,
            })
        };
        assert_eq!(ok("item_1"), Some(true));
        assert_eq!(ok("item_2"), Some(true));
        assert_eq!(ok("item_3"), Some(false));
        assert!(ev.iter().any(|e| matches!(e,
            HarnessEvent::ToolStarted { summary, .. } if summary == "shell: echo hi")));
    }

    #[test]
    fn provider_errors_are_classified_through_the_shared_codes() {
        let (code, _) = terminal(include_str!("fixtures/error_auth.jsonl"));
        assert_eq!(code, "provider_auth");
        let (code, _) = terminal(include_str!("fixtures/error_quota.jsonl"));
        assert_eq!(code, "provider_quota");
        let (code, _) = terminal(include_str!("fixtures/error_rate_limit.jsonl"));
        assert_eq!(code, "provider_rate_limited");
        let (code, _) = terminal(include_str!("fixtures/error_overloaded.jsonl"));
        assert_eq!(code, "provider_overloaded");
    }

    #[test]
    fn gateway_envelope_is_not_a_provider_refusal() {
        let (code, message) = terminal(include_str!("fixtures/error_gateway.jsonl"));
        assert_eq!(code, "gateway_refused");
        assert!(
            message.starts_with("[llm_path_not_permitted] "),
            "{message}"
        );
    }

    #[test]
    fn classify_covers_documented_openai_shapes() {
        let c = |t: &str| classify_failure(t).0;
        assert_eq!(
            c(
                r#"unexpected status 429 Too Many Requests: {"error":{"type":"insufficient_quota","code":"insufficient_quota","message":"You exceeded your current quota"}}"#
            ),
            "provider_quota"
        );
        assert_eq!(
            c(
                r#"unexpected status 429 Too Many Requests: {"error":{"type":"rate_limit_exceeded","message":"slow down"}}"#
            ),
            "provider_rate_limited"
        );
        assert_eq!(
            c("unexpected status 503 Service Unavailable: upstream"),
            "provider_overloaded"
        );
        assert_eq!(
            c("unexpected status 402 Payment Required"),
            "provider_quota"
        );
        assert_eq!(c("stream disconnected before completion"), "harness_failed");
        assert_eq!(c(""), "harness_failed");
    }

    #[test]
    fn retries_become_activity_and_junk_is_ignored() {
        let mut m = StreamMapper::new();
        let r = m.map_line(r#"{"type":"error","message":"Reconnecting... 2/5 (x)"}"#);
        assert_eq!(
            r.events,
            [HarnessEvent::Activity {
                line: "retrying the model request".into()
            }]
        );
        assert_eq!(m.map_line("not json"), Mapped::default());
        assert_eq!(m.map_line(r#"{"type":"mystery"}"#), Mapped::default());
        assert_eq!(
            m.map_line(
                r#"{"type":"item.completed","item":{"id":"i","type":"error","message":"warn"}}"#
            ),
            Mapped::default()
        );
    }

    #[test]
    fn summaries_are_truncated() {
        let long = "x".repeat(500);
        let line = format!(
            r#"{{"type":"item.started","item":{{"id":"c","type":"command_execution","command":"{long}","status":"in_progress"}}}}"#
        );
        let r = StreamMapper::new().map_line(&line);
        let HarnessEvent::ToolStarted { summary, .. } = &r.events[0] else {
            panic!()
        };
        assert!(summary.chars().count() <= "shell: ".len() + SUMMARY_MAX + 1);
    }
}

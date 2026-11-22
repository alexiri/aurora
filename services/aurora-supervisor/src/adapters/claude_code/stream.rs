//! Claude Code `--output-format stream-json` line -> [`HarnessEvent`] mapping and error classification.
//!
//! Mapping (pinned for [`super::PINNED_VERSION`]; fixtures in `fixtures/` are hand-authored):
//! - `system/init` -> [`Mapped::init`] (session id, MCP readiness) and a `SessionRef` event.
//! - `stream_event` `message_start` -> current message id; `content_block_delta` `text_delta` ->
//!   `TextDelta`; `thinking_delta` -> `ThinkingDelta`; `message_delta` with usage -> `Usage`,
//!   `stop_reason` remembered (a `refusal` becomes `provider_refusal` at the result).
//! - `assistant` messages: `tool_use` blocks -> `ToolStarted`; text/thinking blocks only when the
//!   message was not already streamed through `stream_event`s.
//! - `user` messages: `tool_result` blocks -> `ToolFinished`; a replayed text message (`isReplay`,
//!   `--replay-user-messages`) carrying the text of a steer handed to the CLI -> `MessageJoined`.
//! - `result` -> terminal `Done{usage}` or `Error{code,message}` (see [`classify_failure`]).
//! - Events with a non-null `parent_tool_use_id` (sub-agents) and unknown types are ignored.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::adapters::provider_error;
use crate::harness::HarnessEvent;

/// Name of the MCP server the adapter configures.
pub const MCP_SERVER: &str = "aurora";
const MCP_TOOL_PREFIX: &str = "mcp__aurora__";
const SUMMARY_MAX: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitInfo {
    pub session_id: Option<String>,
    /// The `aurora` MCP server is connected and its tools are listed.
    pub mcp_ready: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Mapped {
    pub events: Vec<HarnessEvent>,
    pub init: Option<InitInfo>,
    /// Set for the `result` line; always a `Done` or `Error`.
    pub terminal: Option<HarnessEvent>,
}

/// Per model: input, output, cache-read, cache-write tokens, as of the child's last `result`.
pub type Baseline = Arc<StdMutex<BTreeMap<String, [i64; 4]>>>;

/// (FIX-225) What a live child may still be handed: whether a run is taking steers, and the steers
/// written to its stdin that it has not echoed yet (`(followup id, rendered text)`). One lock, so a
/// steer can never be registered after the turn closed the door.
#[derive(Debug, Default)]
pub struct SteerState {
    pub accepting: bool,
    pub pending: Vec<(Uuid, String)>,
}

pub type Steers = Arc<StdMutex<SteerState>>;

#[derive(Debug, Default)]
pub struct StreamMapper {
    steers: Steers,
    /// Shared across the turns of one child (the CLI's per-model report is cumulative for the process).
    baseline: Baseline,
    message_id: String,
    streamed: HashSet<String>,
    block_kinds: HashMap<u64, String>,
    stop_reason: Option<String>,
    tool_names: HashMap<String, String>,
}

fn trunc(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// Short, secret-free description of a tool call.
pub fn summarize(name: &str, input: &Value) -> String {
    for key in ["command", "file_path", "path", "url", "pattern", "query"] {
        if let Some(v) = input.get(key).and_then(Value::as_str) {
            return format!("{name}: {}", trunc(v, SUMMARY_MAX));
        }
    }
    let n = input.as_object().map_or(0, serde_json::Map::len);
    format!("{name} ({n} args)")
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl StreamMapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// A mapper for one turn of a child whose earlier turns left `baseline`; the CLI's echo of a
    /// steer in `steers` becomes `MessageJoined`.
    pub fn with_baseline(baseline: Baseline, steers: Steers) -> Self {
        Self {
            baseline,
            steers,
            ..Self::default()
        }
    }

    /// Clears per-turn state (call when a new user message is sent).
    pub fn reset_turn(&mut self) {
        self.stop_reason = None;
        self.block_kinds.clear();
    }

    pub fn map_line(&mut self, line: &str) -> Mapped {
        let mut out = Mapped::default();
        if line == super::launcher::OVERLONG_LINE {
            out.terminal = Some(HarnessEvent::Error {
                code: "harness_failed".into(),
                message: "claude produced an output line over the size limit".into(),
            });
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return out;
        };
        if !v.get("parent_tool_use_id").is_none_or(Value::is_null) {
            return out;
        }
        match v.get("type").and_then(Value::as_str) {
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
                self.init(&v, &mut out);
            }
            Some("stream_event") => {
                if let Some(e) = v.get("event") {
                    self.stream_event(e, &mut out);
                }
            }
            Some("assistant") => self.assistant(&v, &mut out),
            Some("user") => self.user(&v, &mut out),
            Some("result") => out.terminal = Some(self.result(&v)),
            _ => {}
        }
        out
    }

    fn init(&mut self, v: &Value, out: &mut Mapped) {
        let session_id = v
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let connected = v
            .get("mcp_servers")
            .and_then(Value::as_array)
            .is_some_and(|s| {
                s.iter().any(|m| {
                    m.get("name").and_then(Value::as_str) == Some(MCP_SERVER)
                        && m.get("status").and_then(Value::as_str) == Some("connected")
                })
            });
        let listed = v.get("tools").and_then(Value::as_array).is_some_and(|t| {
            t.iter()
                .filter_map(Value::as_str)
                .any(|n| n.starts_with(MCP_TOOL_PREFIX))
        });
        if let Some(id) = &session_id {
            out.events.push(HarnessEvent::SessionRef {
                session_ref: id.clone(),
            });
        }
        out.init = Some(InitInfo {
            session_id,
            mcp_ready: connected && listed,
        });
    }

    fn stream_event(&mut self, e: &Value, out: &mut Mapped) {
        match e.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(id) = e.pointer("/message/id").and_then(Value::as_str) {
                    self.message_id = id.to_owned();
                }
            }
            Some("content_block_start") => {
                let idx = e.get("index").and_then(Value::as_u64).unwrap_or(0);
                let kind = e
                    .pointer("/content_block/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.block_kinds.insert(idx, kind.to_owned());
            }
            Some("content_block_delta") => {
                let d = e.get("delta");
                match d.and_then(|d| d.get("type")).and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(t) = d.and_then(|d| d.get("text")).and_then(Value::as_str)
                            && !t.is_empty()
                        {
                            self.streamed.insert(self.message_id.clone());
                            out.events.push(HarnessEvent::TextDelta {
                                message_id: self.message_id.clone(),
                                delta: t.to_owned(),
                                reply_to: None,
                            });
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = d.and_then(|d| d.get("thinking")).and_then(Value::as_str)
                            && !t.is_empty()
                        {
                            self.streamed.insert(self.message_id.clone());
                            out.events.push(HarnessEvent::ThinkingDelta {
                                delta: t.to_owned(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(r) = e.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(r.to_owned());
                }
                if let Some(u) = e.get("usage") {
                    out.events.push(HarnessEvent::Usage { usage: u.clone() });
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, v: &Value, out: &mut Mapped) {
        let Some(msg) = v.get("message") else { return };
        let id = msg.get("id").and_then(Value::as_str).unwrap_or_default();
        let streamed = self.streamed.contains(id);
        if let Some(r) = msg.get("stop_reason").and_then(Value::as_str) {
            self.stop_reason = Some(r.to_owned());
        }
        let blocks = msg
            .get("content")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("text") if !streamed => {
                    if let Some(t) = b.get("text").and_then(Value::as_str)
                        && !t.is_empty()
                    {
                        out.events.push(HarnessEvent::TextDelta {
                            message_id: id.to_owned(),
                            delta: t.to_owned(),
                            reply_to: None,
                        });
                    }
                }
                Some("thinking") if !streamed => {
                    if let Some(t) = b.get("thinking").and_then(Value::as_str)
                        && !t.is_empty()
                    {
                        out.events.push(HarnessEvent::ThinkingDelta {
                            delta: t.to_owned(),
                        });
                    }
                }
                Some("tool_use") => {
                    let call_id = b.get("id").and_then(Value::as_str).unwrap_or_default();
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let input = b.get("input").unwrap_or(&Value::Null);
                    self.tool_names.insert(call_id.to_owned(), name.to_owned());
                    out.events.push(HarnessEvent::ToolStarted {
                        call_id: call_id.to_owned(),
                        name: name.to_owned(),
                        summary: summarize(name, input),
                    });
                }
                _ => {}
            }
        }
    }

    fn user(&mut self, v: &Value, out: &mut Mapped) {
        let blocks = v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        // `--replay-user-messages`: the CLI echoes a user line it has taken. The echo of a steer
        // (its text matches one handed over) is the join; the turn's own message is not a steer.
        if v.get("isReplay").and_then(Value::as_bool) == Some(true) {
            let text = blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if let Ok(mut s) = self.steers.lock()
                && let Some(i) = s.pending.iter().position(|(_, t)| *t == text)
            {
                let (id, _) = s.pending.remove(i);
                out.events
                    .push(HarnessEvent::MessageJoined { followup_id: id });
            }
        }
        for b in blocks {
            if b.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let call_id = b
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.tool_names.remove(call_id);
            let text = b.get("content").map(result_text).unwrap_or_default();
            out.events.push(HarnessEvent::ToolFinished {
                call_id: call_id.to_owned(),
                ok: !b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                summary: (!text.is_empty()).then(|| trunc(&text, SUMMARY_MAX)),
            });
        }
    }

    fn result(&mut self, v: &Value) -> HarnessEvent {
        // Taken for every result, so that a failed turn's tokens are not carried into the next one.
        let usage = self.turn_usage(v);
        let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let refusal = self.stop_reason.as_deref() == Some("refusal");
        if refusal && !is_error {
            return HarnessEvent::Error {
                code: "provider_refusal".into(),
                message: "the model declined to answer".into(),
            };
        }
        let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or_default();
        // `error_max_turns`, `error_during_execution`, … are failed turns whatever `is_error`
        // says (the pinned CLI may report them with `is_error: false`; FIX-095).
        if !is_error && subtype.starts_with("error") {
            return HarnessEvent::Error {
                code: "harness_failed".into(),
                message: trunc(&format!("the CLI ended the turn: {subtype}"), 500),
            };
        }
        if is_error {
            let status = v.get("api_error_status").and_then(Value::as_u64);
            let text = v.get("result").and_then(Value::as_str).unwrap_or_default();
            let (code, message) = classify_failure(status, text, subtype);
            return HarnessEvent::Error { code, message };
        }
        HarnessEvent::Done { usage }
    }

    /// (P5-MODELS-2, D-304, C-879) This turn's usage, per model. The result's `modelUsage` (pinned 2.1.319)
    /// is keyed by model id and includes subagents; the top-level `usage` covers the main loop only and is
    /// used only when the report is absent. The CLI's report is **cumulative for the child process** ("read
    /// the latest result rather than summing across results"), so the turn's share is the difference to the
    /// previous result of the same child (a counter that went down means a fresh child: its value is taken
    /// whole). `costUSD` and `total_cost_usd` are never read: Aurora records no money.
    fn turn_usage(&mut self, v: &Value) -> Option<Value> {
        let Some(report) = v
            .get("modelUsage")
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty())
        else {
            return v.get("usage").map(|u| {
                let n = |k: &str| u.get(k).and_then(Value::as_i64).unwrap_or(0);
                json!({
                    "input_tokens": n("input_tokens"),
                    "output_tokens": n("output_tokens"),
                    "cache_read_tokens": n("cache_read_input_tokens"),
                    "cache_write_tokens": n("cache_creation_input_tokens"),
                })
            });
        };
        let mut base = self
            .baseline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut by_model = Vec::new();
        let mut total = [0_i64; 4];
        for (model, e) in report {
            let n = |k: &str| e.get(k).and_then(Value::as_i64).unwrap_or(0).max(0);
            let cur = [
                n("inputTokens"),
                n("outputTokens"),
                n("cacheReadInputTokens"),
                n("cacheCreationInputTokens"),
            ];
            let prev = base.get(model).copied().unwrap_or_default();
            let delta = if cur.iter().zip(prev).all(|(c, p)| *c >= p) {
                [
                    cur[0] - prev[0],
                    cur[1] - prev[1],
                    cur[2] - prev[2],
                    cur[3] - prev[3],
                ]
            } else {
                cur
            };
            base.insert(model.clone(), cur);
            if delta == [0; 4] {
                continue;
            }
            for (t, d) in total.iter_mut().zip(delta) {
                *t = t.saturating_add(d);
            }
            by_model.push(json!({
                "model": model,
                "input_tokens": delta[0],
                "output_tokens": delta[1],
                "cache_read_tokens": delta[2],
                "cache_write_tokens": delta[3],
            }));
        }
        by_model.sort_by(|a, b| a["model"].as_str().cmp(&b["model"].as_str()));
        Some(json!({
            "input_tokens": total[0],
            "output_tokens": total[1],
            "cache_read_tokens": total[2],
            "cache_write_tokens": total[3],
            "by_model": by_model,
        }))
    }
}

fn parse_status(text: &str) -> Option<u64> {
    let rest = text.strip_prefix("API Error: ")?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|d| d.parse().ok())
}

/// Maps a failed `result` to `(harness error code, message)` (`12 §Provider refusal classification`).
///
/// A gateway envelope (`{"error":{"code":…}}`, which provider errors never carry: they use
/// `error.type`) yields `gateway_refused` with the message `[<gateway code>] <message>`.
pub fn classify_failure(status: Option<u64>, text: &str, subtype: &str) -> (String, String) {
    let body: Option<Value> = text
        .find('{')
        .and_then(|i| serde_json::from_str(&text[i..]).ok());
    let err = body.as_ref().and_then(|b| b.get("error"));
    let message = err
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(text);
    if let Some(e) = err
        && let Some(code) = e.get("code").and_then(Value::as_str)
        && e.get("type").is_none()
    {
        // (P4-LLM-5, D-261) A rejected LLM credential mint is `provider_auth`; every other gateway code stays
        // `gateway_refused`.
        return (
            provider_error::gateway_code(e).into(),
            trunc(&format!("[{code}] {message}"), 500),
        );
    }
    let err_type = err
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lower = text.to_ascii_lowercase();
    let status = status.or_else(|| parse_status(text));
    let quota = lower.contains("credit balance")
        || lower.contains("insufficient_quota")
        || lower.contains("billing")
        || status == Some(402);
    // (P4-LLM-5, C-808) The shared table first, on the error type the body or stream event carries; then the status
    // and phrase rules for what the CLI reports as free text.
    let code = if let Some(c) = provider_error::table_code(err, body.as_ref(), text) {
        c
    } else {
        match status {
            _ if quota => "provider_quota",
            Some(401 | 403) => "provider_auth",
            Some(429) => "provider_rate_limited",
            Some(503 | 529) => "provider_overloaded",
            _ if err_type == "authentication_error" || err_type == "permission_error" => {
                "provider_auth"
            }
            _ if err_type == "rate_limit_error" => "provider_rate_limited",
            _ if err_type == "overloaded_error" => "provider_overloaded",
            _ => "harness_failed",
        }
    };
    let message = if message.is_empty() {
        format!("claude-code failed ({subtype})")
    } else {
        trunc(message, 500)
    };
    (code.into(), message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = include_str!("fixtures/text_turn.jsonl");
    const TOOL: &str = include_str!("fixtures/tool_turn.jsonl");

    fn run(fixture: &str) -> (Vec<HarnessEvent>, Option<HarnessEvent>, Option<InitInfo>) {
        let mut m = StreamMapper::new();
        let (mut ev, mut term, mut init) = (Vec::new(), None, None);
        for l in fixture.lines() {
            let r = m.map_line(l);
            ev.extend(r.events);
            term = term.or(r.terminal);
            init = init.or(r.init);
        }
        (ev, term, init)
    }

    #[test]
    fn text_turn_maps_deltas_thinking_usage_session_done() {
        let (ev, term, init) = run(TEXT);
        assert_eq!(
            init,
            Some(InitInfo {
                session_id: Some("5d1e0c1a-0000-4000-8000-000000000001".into()),
                mcp_ready: true
            })
        );
        assert!(
            matches!(&ev[0], HarnessEvent::SessionRef { session_ref } if session_ref.starts_with("5d1e"))
        );
        let text: String = ev
            .iter()
            .filter_map(|e| match e {
                HarnessEvent::TextDelta {
                    message_id, delta, ..
                } => {
                    assert_eq!(message_id, "msg_01");
                    Some(delta.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            text, "Hello there!",
            "assistant duplicate must not re-emit text"
        );
        assert!(ev.contains(&HarnessEvent::ThinkingDelta {
            delta: "The user greets me.".into()
        }));
        assert!(
            ev.iter()
                .any(|e| matches!(e, HarnessEvent::Usage { usage } if usage["output_tokens"] == 9))
        );
        assert!(
            matches!(term, Some(HarnessEvent::Done { usage: Some(u) }) if u["input_tokens"] == 12)
        );
    }

    #[test]
    fn tool_turn_maps_start_finish_and_unstreamed_text() {
        let (ev, term, _) = run(TOOL);
        let started: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                HarnessEvent::ToolStarted {
                    call_id,
                    name,
                    summary,
                } => Some((call_id.as_str(), name.as_str(), summary.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            started[0],
            (
                "toolu_01",
                "mcp__aurora__memory_save",
                "mcp__aurora__memory_save (1 args)"
            )
        );
        assert_eq!(started[1], ("toolu_02", "Bash", "Bash: false"));
        assert!(ev.contains(&HarnessEvent::ToolFinished {
            call_id: "toolu_01".into(),
            ok: true,
            summary: Some("saved".into())
        }));
        assert!(ev.contains(&HarnessEvent::ToolFinished {
            call_id: "toolu_02".into(),
            ok: false,
            summary: Some("exit 1".into())
        }));
        let texts: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                HarnessEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Saving.", "Done."]);
        assert!(matches!(term, Some(HarnessEvent::Done { .. })));
    }

    fn err(fixture: &str) -> (String, String) {
        match run(fixture).1 {
            Some(HarnessEvent::Error { code, message }) => (code, message),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn gateway_envelope_is_gateway_refused_with_code_never_provider_refusal() {
        let (code, msg) = err(include_str!("fixtures/error_gateway.jsonl"));
        assert_eq!(code, "gateway_refused");
        assert!(
            msg.starts_with("[blocked_by_policy] Blocked by policy"),
            "{msg}"
        );
    }

    /// (P4-LLM-5, C-808, D-261) The shared table on provider error types, and the gateway's codes.
    #[test]
    fn the_shared_table_and_the_gateway_codes_drive_the_turn_code() {
        let c = |status, text: &str| classify_failure(status, text, "").0;
        // A Bedrock event-stream exception the CLI prints, an OpenAI 429 whose body says the quota is gone.
        assert_eq!(
            c(
                None,
                "API Error: ThrottlingException: Too many requests, please wait"
            ),
            "provider_rate_limited"
        );
        assert_eq!(
            c(
                Some(429),
                r#"API Error: 429 {"error":{"type":"insufficient_quota","code":"insufficient_quota"}}"#
            ),
            "provider_quota"
        );
        assert_eq!(
            c(
                None,
                r#"{"error":{"status":"PERMISSION_DENIED","message":"no"}}"#
            ),
            "provider_auth"
        );
        // A gateway refusal stays the gateway's, except a rejected mint: the credential is at fault.
        let gw = |code: &str, details: &str| {
            c(
                Some(502),
                &format!(
                    r#"API Error: 502 {{"error":{{"code":"{code}","message":"m","details":{details}}}}}"#
                ),
            )
        };
        assert_eq!(gw("llm_path_not_permitted", "{}"), "gateway_refused");
        assert_eq!(gw("llm_request_too_large", "{}"), "gateway_refused");
        assert_eq!(
            gw(
                "credential_unavailable",
                r#"{"kind":"llm_mint","reason":"mint_rejected"}"#
            ),
            "provider_auth"
        );
        assert_eq!(
            gw(
                "credential_unavailable",
                r#"{"kind":"llm_mint","reason":"mint_unavailable"}"#
            ),
            "gateway_refused"
        );
        let (code, msg) = classify_failure(
            Some(502),
            r#"API Error: 502 {"error":{"code":"credential_unavailable","message":"rejected","details":{"kind":"llm_mint","reason":"mint_rejected"}}}"#,
            "",
        );
        assert_eq!(code, "provider_auth");
        assert!(msg.starts_with("[credential_unavailable]"), "{msg}");
    }

    #[test]
    fn an_error_subtype_is_a_failed_turn_even_when_is_error_is_false() {
        for subtype in ["error_max_turns", "error_during_execution"] {
            let line = format!(
                r#"{{"type":"result","subtype":"{subtype}","is_error":false,"result":"","session_id":"s"}}"#
            );
            let (_, term, _) = run(&line);
            assert!(
                matches!(&term, Some(HarnessEvent::Error { code, .. }) if code == "harness_failed"),
                "{subtype}: {term:?}"
            );
        }
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":"x","session_id":"s"}"#;
        assert!(matches!(run(ok).1, Some(HarnessEvent::Done { .. })));
    }

    #[test]
    fn provider_errors_classified() {
        assert_eq!(
            err(include_str!("fixtures/error_rate_limit.jsonl")).0,
            "provider_rate_limited"
        );
        assert_eq!(
            err(include_str!("fixtures/error_quota.jsonl")).0,
            "provider_quota"
        );
        assert_eq!(
            err(include_str!("fixtures/error_refusal.jsonl")).0,
            "provider_refusal"
        );
        assert_eq!(
            err(include_str!("fixtures/error_execution.jsonl")).0,
            "harness_failed"
        );
        assert_eq!(
            classify_failure(None, "API Error: 401 nope", "").0,
            "provider_auth"
        );
        assert_eq!(
            classify_failure(Some(529), "x", "").0,
            "provider_overloaded"
        );
        assert_eq!(
            classify_failure(None, "boom", "error_max_turns").0,
            "harness_failed"
        );
    }

    #[test]
    fn mcp_not_ready_when_server_failed_or_tools_missing() {
        let mut m = StreamMapper::new();
        let failed = r#"{"type":"system","subtype":"init","session_id":"s","tools":["mcp__aurora__x"],"mcp_servers":[{"name":"aurora","status":"failed"}]}"#;
        assert!(!m.map_line(failed).init.unwrap().mcp_ready);
        let none = r#"{"type":"system","subtype":"init","session_id":"s","tools":["Bash"],"mcp_servers":[{"name":"aurora","status":"connected"}]}"#;
        assert!(!m.map_line(none).init.unwrap().mcp_ready);
    }

    #[test]
    fn subagent_and_garbage_lines_ignored() {
        let mut m = StreamMapper::new();
        assert_eq!(m.map_line("not json"), Mapped::default());
        let sub = r#"{"type":"assistant","parent_tool_use_id":"toolu_9","message":{"id":"m","content":[{"type":"text","text":"x"}]}}"#;
        assert_eq!(m.map_line(sub), Mapped::default());
    }

    const SUBAGENT: &str = include_str!("fixtures/subagent_turn.jsonl");

    fn done_usage(t: Option<HarnessEvent>) -> Value {
        match t {
            Some(HarnessEvent::Done { usage: Some(u) }) => u,
            other => panic!("not a Done with usage: {other:?}"),
        }
    }

    /// (D-304) The same fixture read the old way (the result's main-loop `usage`) leaves the subagent's
    /// tokens out; the per-model report counts them.
    #[test]
    fn a_subagent_turn_counts_the_subagents_tokens_and_drops_the_cost() {
        let u = done_usage(run(SUBAGENT).1);
        let old: Value = serde_json::from_str(SUBAGENT.lines().last().unwrap()).unwrap();
        assert_eq!(old["usage"]["input_tokens"], 100, "the old reading");
        assert_eq!(u["input_tokens"], 400, "main 100 + subagent 300");
        assert_eq!(u["output_tokens"], 130);
        assert_eq!(u["cache_read_tokens"], 10);
        assert_eq!(u["cache_write_tokens"], 5);
        assert_eq!(
            u["by_model"],
            json!([
                {"model": "m-main", "input_tokens": 100, "output_tokens": 40,
                 "cache_read_tokens": 10, "cache_write_tokens": 5},
                {"model": "m-small", "input_tokens": 300, "output_tokens": 90,
                 "cache_read_tokens": 0, "cache_write_tokens": 0},
            ])
        );
        let text = u.to_string();
        assert!(
            !text.contains("cost") && !text.contains("0.5"),
            "no money is stored: {text}"
        );
    }

    #[test]
    fn without_the_per_model_report_the_main_loop_usage_is_the_fallback() {
        let no_report: String = SUBAGENT
            .lines()
            .map(|l| {
                if l.contains(r#""type":"result""#) {
                    let mut v: Value = serde_json::from_str(l).unwrap();
                    v.as_object_mut().unwrap().remove("modelUsage");
                    v.to_string()
                } else {
                    l.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let u = done_usage(run(&no_report).1);
        assert_eq!(u["input_tokens"], 100);
        assert_eq!(u["cache_read_tokens"], 10);
        assert_eq!(u["cache_write_tokens"], 5);
        assert!(u.get("by_model").is_none(), "{u}");
    }

    /// (FIX-225) The echo of a steer (`isReplay`, `--replay-user-messages`) is its join; the turn's own
    /// message, a tool result and an echo nobody asked for are not.
    #[test]
    fn a_replayed_user_line_joins_only_a_steer_that_was_handed_over() {
        let steers = Steers::default();
        let id = Uuid::now_v7();
        steers
            .lock()
            .unwrap()
            .pending
            .push((id, "Ann: more".into()));
        let mut m = StreamMapper::with_baseline(Baseline::default(), steers.clone());
        let echo = |text: &str, replay: bool| {
            json!({"type": "user", "isReplay": replay, "parent_tool_use_id": null,
                "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
            .to_string()
        };
        assert!(m.map_line(&echo("first message", true)).events.is_empty());
        assert!(
            m.map_line(&echo("Ann: more", false)).events.is_empty(),
            "only the CLI's replay counts"
        );
        assert_eq!(steers.lock().unwrap().pending.len(), 1);
        assert_eq!(
            m.map_line(&echo("Ann: more", true)).events,
            [HarnessEvent::MessageJoined { followup_id: id }]
        );
        assert!(steers.lock().unwrap().pending.is_empty());
        assert!(
            m.map_line(&echo("Ann: more", true)).events.is_empty(),
            "joined once"
        );
    }

    /// The CLI's report is cumulative for the child: a second turn reports its own share, and a counter
    /// that went down (a fresh child) is taken whole.
    #[test]
    fn the_cumulative_report_is_reduced_to_each_turns_share() {
        let result = |main: i64, small: i64| {
            format!(
                r#"{{"type":"result","subtype":"success","is_error":false,"modelUsage":{{"m-main":{{"inputTokens":{main},"outputTokens":0,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"costUSD":1.0}},"m-small":{{"inputTokens":{small},"outputTokens":0,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"costUSD":1.0}}}}}}"#
            )
        };
        let base = Baseline::default();
        let turn = |line: String| {
            let mut m = StreamMapper::with_baseline(base.clone(), Steers::default());
            done_usage(m.map_line(&line).terminal)
        };
        let first = turn(result(100, 300));
        assert_eq!(first["input_tokens"], 400);
        let second = turn(result(150, 300));
        assert_eq!(second["input_tokens"], 50, "only the main model grew");
        assert_eq!(second["by_model"].as_array().unwrap().len(), 1, "{second}");
        assert_eq!(second["by_model"][0]["model"], "m-main");
        let fresh = turn(result(10, 20));
        assert_eq!(fresh["input_tokens"], 30, "counters went down: a new child");
    }
}

//! Pi `--mode rpc` JSONL line -> [`HarnessEvent`] mapping (`docs/12-supervisor-harness.md` §Adapters
//! `pi`, `goose` and `opencode`).
//!
//! Mapping (pinned for [`super::PINNED_VERSION`]; the line shapes in the tests were recorded from a
//! real 1.5.2 run against the mock upstream of `aurora-testkit`):
//! - `message_start` of a `user` message that carries the text of a steer the adapter has queued ->
//!   `MessageJoined` (the join is confirmed by Pi's own transcript, not by the queue acknowledgement).
//! - `message_update` `assistantMessageEvent` `text_delta` / `thinking_delta` -> `TextDelta` /
//!   `ThinkingDelta`.
//! - `tool_execution_start` / `tool_execution_end` -> `ToolStarted` / `ToolFinished`; a call that is
//!   still open when the run ends is finished as failed, so every start has its finish.
//! - assistant `message_end` -> the per-model usage of the turn (D-304); the last assistant message's
//!   `stopReason: "error"` with its `errorMessage` (`"<status> <body>"`) is the failure of the run.
//! - `auto_retry_start` -> an activity line (Pi retries a refused request, [`super::config::MAX_RETRIES`]
//!   times, before it gives up).
//! - `agent_settled` is the **terminal** event: `Done`, or `Error` for an aborted run or a failed last
//!   message. Unknown lines and types are ignored.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::harness::HarnessEvent;

const SUMMARY_MAX: usize = 120;
const MESSAGE_MAX: usize = 500;

/// A steer the adapter has handed to Pi and not yet seen join: `(followup id, rendered text)`.
pub type PendingSteers = Arc<Mutex<Vec<(Uuid, String)>>>;

/// The answer to one RPC command (`{"type":"response",…}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub command: String,
    pub success: bool,
    /// `started` | `queued` for a `prompt`.
    pub disposition: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Mapped {
    pub events: Vec<HarnessEvent>,
    /// Set for `agent_settled`; always a `Done` or `Error`.
    pub terminal: Option<HarnessEvent>,
    pub response: Option<Response>,
}

#[derive(Debug, Default)]
pub struct StreamMapper {
    messages: u32,
    current: Option<String>,
    open_tools: Vec<String>,
    seen_tools: HashSet<String>,
    /// model -> [input, output, cache read, cache write] of this turn.
    usage: BTreeMap<String, [i64; 4]>,
    last_error: Option<String>,
    pending: PendingSteers,
}

fn trunc(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// What a person sees for a tool call: `bash: <command>`, `read: <path>`, `mcp__… (2 args)`.
fn tool_summary(name: &str, args: Option<&Value>) -> String {
    let obj = args.and_then(Value::as_object);
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| obj.and_then(|o| o.get(*k)).and_then(Value::as_str))
    };
    if let Some(c) = pick(&["command", "path", "file_path", "pattern"]) {
        return format!(
            "{name}: {}",
            trunc(c.lines().next().unwrap_or(c), SUMMARY_MAX)
        );
    }
    format!("{name} ({} args)", obj.map_or(0, serde_json::Map::len))
}

fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl StreamMapper {
    pub fn new(pending: PendingSteers) -> Self {
        Self {
            pending,
            ..Self::default()
        }
    }

    /// Clears per-turn state (call when a new prompt is sent).
    pub fn reset_turn(&mut self) {
        self.current = None;
        self.open_tools.clear();
        self.seen_tools.clear();
        self.usage.clear();
        self.last_error = None;
    }

    pub fn map_line(&mut self, line: &str) -> Mapped {
        let mut out = Mapped::default();
        if line == super::launcher::OVERLONG_LINE {
            out.terminal = Some(HarnessEvent::Error {
                code: "harness_failed".into(),
                message: "pi produced an output line over the size limit".into(),
            });
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return out;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("response") => {
                out.response = Some(Response {
                    command: v
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
                    disposition: v
                        .pointer("/data/disposition")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    error: v.get("error").and_then(Value::as_str).map(str::to_owned),
                });
            }
            Some("message_start") => self.message_start(&v, &mut out),
            Some("message_update") => self.message_update(&v, &mut out),
            Some("message_end") => self.message_end(&v),
            Some("tool_execution_start") => self.tool_start(&v, &mut out),
            Some("tool_execution_end") => self.tool_end(&v, &mut out),
            Some("auto_retry_start") => {
                let n = v.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                let m = v.get("maxAttempts").and_then(Value::as_u64).unwrap_or(0);
                out.events.push(HarnessEvent::Activity {
                    line: format!("retrying the model request ({n}/{m})"),
                });
            }
            Some("agent_settled") => {
                self.finish_open_tools(&mut out);
                let aborted = v.get("aborted").and_then(Value::as_bool).unwrap_or(false);
                out.terminal = Some(if aborted {
                    HarnessEvent::Error {
                        code: "aborted".into(),
                        message: "turn aborted".into(),
                    }
                } else if let Some(m) = self.last_error.take() {
                    let (code, message) = classify(&m);
                    HarnessEvent::Error { code, message }
                } else {
                    HarnessEvent::Done {
                        usage: self.turn_usage(),
                    }
                });
            }
            _ => {}
        }
        out
    }

    fn message_start(&mut self, v: &Value, out: &mut Mapped) {
        let m = v.get("message");
        match m.and_then(|m| m.get("role")).and_then(Value::as_str) {
            Some("assistant") => {
                self.messages += 1;
                self.current = Some(format!("pi-m{}", self.messages));
            }
            Some("user") => {
                let text = text_of(m.and_then(|m| m.get("content")));
                if let Ok(mut p) = self.pending.lock()
                    && let Some(i) = p.iter().position(|(_, t)| *t == text)
                {
                    let (id, _) = p.remove(i);
                    out.events
                        .push(HarnessEvent::MessageJoined { followup_id: id });
                }
            }
            _ => {}
        }
    }

    fn message_update(&mut self, v: &Value, out: &mut Mapped) {
        let Some(e) = v.get("assistantMessageEvent") else {
            return;
        };
        let delta = e.get("delta").and_then(Value::as_str).unwrap_or_default();
        if delta.is_empty() {
            return;
        }
        match e.get("type").and_then(Value::as_str) {
            Some("text_delta") => out.events.push(HarnessEvent::TextDelta {
                message_id: self
                    .current
                    .clone()
                    .unwrap_or_else(|| format!("pi-m{}", self.messages)),
                delta: delta.to_owned(),
                reply_to: None,
            }),
            Some("thinking_delta") => out.events.push(HarnessEvent::ThinkingDelta {
                delta: delta.to_owned(),
            }),
            _ => {}
        }
    }

    fn message_end(&mut self, v: &Value) {
        let Some(m) = v.get("message") else { return };
        if m.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        if m.get("stopReason").and_then(Value::as_str) == Some("error") {
            let e = m
                .get("errorMessage")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.last_error = Some(e.to_owned());
        } else {
            // A later success after a retried failure clears it.
            self.last_error = None;
        }
        if let (Some(model), Some(u)) = (m.get("model").and_then(Value::as_str), m.get("usage")) {
            let n = |k: &str| u.get(k).and_then(Value::as_i64).unwrap_or(0).max(0);
            let e = self.usage.entry(model.to_owned()).or_default();
            for (t, d) in
                e.iter_mut()
                    .zip([n("input"), n("output"), n("cacheRead"), n("cacheWrite")])
            {
                *t = t.saturating_add(d);
            }
        }
    }

    fn tool_start(&mut self, v: &Value, out: &mut Mapped) {
        let id = v
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = v.get("toolName").and_then(Value::as_str).unwrap_or("tool");
        if id.is_empty() || !self.seen_tools.insert(id.to_owned()) {
            return;
        }
        self.open_tools.push(id.to_owned());
        out.events.push(HarnessEvent::ToolStarted {
            call_id: id.to_owned(),
            name: name.to_owned(),
            summary: tool_summary(name, v.get("args")),
        });
    }

    fn tool_end(&mut self, v: &Value, out: &mut Mapped) {
        let id = v
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(i) = self.open_tools.iter().position(|o| o == id) else {
            return;
        };
        self.open_tools.remove(i);
        let text = text_of(v.pointer("/result/content"));
        out.events.push(HarnessEvent::ToolFinished {
            call_id: id.to_owned(),
            ok: !v.get("isError").and_then(Value::as_bool).unwrap_or(false),
            summary: (!text.is_empty()).then(|| trunc(text.trim_end(), SUMMARY_MAX)),
        });
    }

    fn finish_open_tools(&mut self, out: &mut Mapped) {
        for id in std::mem::take(&mut self.open_tools) {
            out.events.push(HarnessEvent::ToolFinished {
                call_id: id,
                ok: false,
                summary: Some("the run ended before the tool finished".into()),
            });
        }
    }

    /// (D-304) This turn's usage per model, from each assistant message's own report; the totals are
    /// their sum. Pi's `cost` is never read: Aurora records no money. `None` when no model answered.
    fn turn_usage(&mut self) -> Option<Value> {
        if self.usage.is_empty() {
            return None;
        }
        let mut total = [0_i64; 4];
        let by_model: Vec<Value> = std::mem::take(&mut self.usage)
            .into_iter()
            .map(|(model, u)| {
                for (t, d) in total.iter_mut().zip(u) {
                    *t = t.saturating_add(d);
                }
                json!({
                    "model": model,
                    "input_tokens": u[0],
                    "output_tokens": u[1],
                    "cache_read_tokens": u[2],
                    "cache_write_tokens": u[3],
                })
            })
            .collect();
        Some(json!({
            "input_tokens": total[0],
            "output_tokens": total[1],
            "cache_read_tokens": total[2],
            "cache_write_tokens": total[3],
            "by_model": by_model,
        }))
    }
}

/// Maps the last assistant message's `errorMessage` (`"<status> <response body>"`, e.g.
/// `429 {"type":"error","error":{"type":"rate_limit_error",…}}`) to `(code, message)` through the
/// shared classification (`12 §Provider refusal classification`).
pub fn classify(error_message: &str) -> (String, String) {
    if error_message.trim().is_empty() {
        return ("harness_failed".into(), "pi failed".into());
    }
    let (code, message) =
        crate::adapters::codex::stream::classify_failure(&format!("status {error_message}"));
    let message = if message == "codex failed" {
        "pi failed".to_owned()
    } else {
        trunc(&message, MESSAGE_MAX)
    };
    (code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> (Vec<HarnessEvent>, Option<HarnessEvent>) {
        run_with(lines, PendingSteers::default())
    }

    fn run_with(lines: &[&str], p: PendingSteers) -> (Vec<HarnessEvent>, Option<HarnessEvent>) {
        let mut m = StreamMapper::new(p);
        let (mut ev, mut term) = (Vec::new(), None);
        for l in lines {
            let r = m.map_line(l);
            ev.extend(r.events);
            term = term.or(r.terminal);
        }
        (ev, term)
    }

    // Lines recorded from Pi 1.5.2 (trimmed: the system message and cost objects are dropped).
    const USER: &str = r#"{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"go"}]}}"#;
    const A_START: &str = r#"{"type":"message_start","message":{"role":"assistant","content":[],"model":"m1","stopReason":"pending"}}"#;
    const DELTA_PO: &str = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"po"}}"#;
    const DELTA_NG: &str = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"ng"}}"#;
    const A_END: &str = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"pong"}],"model":"m1","usage":{"input":11,"output":4,"cacheRead":3,"cacheWrite":2,"totalTokens":20,"cost":{"total":9.5}},"stopReason":"stop"}}"#;
    const SETTLED: &str = r#"{"type":"agent_settled","aborted":false}"#;

    #[test]
    fn a_text_turn_streams_deltas_under_one_message_id_and_ends_with_per_model_usage() {
        let (ev, term) = run(&[USER, A_START, DELTA_PO, DELTA_NG, A_END, SETTLED]);
        let texts: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                HarnessEvent::TextDelta {
                    message_id, delta, ..
                } => Some((message_id.as_str(), delta.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(texts, [("pi-m1", "po"), ("pi-m1", "ng")]);
        let Some(HarnessEvent::Done { usage: Some(u) }) = term else {
            panic!("{term:?}")
        };
        assert_eq!(u["input_tokens"], 11);
        assert_eq!(u["output_tokens"], 4);
        assert_eq!(u["cache_read_tokens"], 3);
        assert_eq!(u["cache_write_tokens"], 2);
        assert_eq!(
            u["by_model"],
            json!([{"model": "m1", "input_tokens": 11, "output_tokens": 4, "cache_read_tokens": 3, "cache_write_tokens": 2}])
        );
        assert!(!u.to_string().contains("9.5"), "cost is never read: {u}");
    }

    #[test]
    fn usage_is_summed_per_model_over_the_messages_of_the_turn_and_totals_are_the_sum() {
        let end = |model: &str, i: i64, o: i64, r: i64, w: i64| {
            format!(
                r#"{{"type":"message_end","message":{{"role":"assistant","model":"{model}","usage":{{"input":{i},"output":{o},"cacheRead":{r},"cacheWrite":{w}}},"stopReason":"stop"}}}}"#
            )
        };
        let lines = [
            end("big", 10, 1, 2, 3),
            end("small", 5, 6, 0, 1),
            end("big", 20, 2, 4, 0),
            SETTLED.to_owned(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (_, term) = run(&refs);
        let Some(HarnessEvent::Done { usage: Some(u) }) = term else {
            panic!("{term:?}")
        };
        assert_eq!(
            u["by_model"],
            json!([
                {"model": "big", "input_tokens": 30, "output_tokens": 3, "cache_read_tokens": 6, "cache_write_tokens": 3},
                {"model": "small", "input_tokens": 5, "output_tokens": 6, "cache_read_tokens": 0, "cache_write_tokens": 1}
            ])
        );
        assert_eq!(
            (
                u["input_tokens"].clone(),
                u["output_tokens"].clone(),
                u["cache_read_tokens"].clone(),
                u["cache_write_tokens"].clone()
            ),
            (json!(35), json!(9), json!(6), json!(4))
        );
    }

    #[test]
    fn tools_start_and_finish_with_their_ids_and_an_open_call_is_finished_when_the_run_ends() {
        let (ev, term) = run(&[
            r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"bash","args":{"command":"echo hi\nsecond"}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"bash","args":{}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"bash","result":{"content":[{"type":"text","text":"hi\n"}]},"isError":false}"#,
            r#"{"type":"tool_execution_start","toolCallId":"t2","toolName":"mcp__aurora__ping","args":{"a":1,"b":2}}"#,
            SETTLED,
        ]);
        assert_eq!(
            ev,
            [
                HarnessEvent::ToolStarted {
                    call_id: "t1".into(),
                    name: "bash".into(),
                    summary: "bash: echo hi".into()
                },
                HarnessEvent::ToolFinished {
                    call_id: "t1".into(),
                    ok: true,
                    summary: Some("hi".into())
                },
                HarnessEvent::ToolStarted {
                    call_id: "t2".into(),
                    name: "mcp__aurora__ping".into(),
                    summary: "mcp__aurora__ping (2 args)".into()
                },
                HarnessEvent::ToolFinished {
                    call_id: "t2".into(),
                    ok: false,
                    summary: Some("the run ended before the tool finished".into())
                },
            ]
        );
        assert!(matches!(term, Some(HarnessEvent::Done { usage: None })));
    }

    #[test]
    fn a_steer_joins_when_pi_puts_its_text_in_the_transcript_and_not_before() {
        let p = PendingSteers::default();
        let id = Uuid::now_v7();
        p.lock().unwrap().push((id, "Ann: more".into()));
        let queue = r#"{"type":"queue_update","steering":["Ann: more"],"followUp":[]}"#;
        let own = USER; // the turn's own prompt is not a steer
        let joined = r#"{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"Ann: more"}]}}"#;
        let (ev, _) = run_with(&[own, queue], p.clone());
        assert!(ev.is_empty(), "{ev:?}");
        assert_eq!(p.lock().unwrap().len(), 1);
        let (ev, _) = run_with(&[joined, joined], p.clone());
        assert_eq!(
            ev,
            [HarnessEvent::MessageJoined { followup_id: id }],
            "once"
        );
        assert!(p.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_last_message_is_the_error_after_the_retries_and_a_retry_that_succeeds_is_not() {
        let err = r#"{"type":"message_end","message":{"role":"assistant","model":"m1","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"stopReason":"error","errorMessage":"429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"mock fail-429\"}}"}}"#;
        let retry = r#"{"type":"auto_retry_start","attempt":1,"maxAttempts":2,"delayMs":2000,"errorMessage":"429 x"}"#;
        let (ev, term) = run(&[err, retry, err, SETTLED]);
        assert_eq!(
            ev,
            [HarnessEvent::Activity {
                line: "retrying the model request (1/2)".into()
            }]
        );
        let Some(HarnessEvent::Error { code, message }) = term else {
            panic!("{term:?}")
        };
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("provider_rate_limited", "mock fail-429")
        );
        // The retry answered: the earlier failure is forgotten.
        let (_, term) = run(&[err, retry, A_START, A_END, SETTLED]);
        assert!(matches!(term, Some(HarnessEvent::Done { .. })), "{term:?}");
    }

    #[test]
    fn provider_errors_are_classified_through_the_shared_codes() {
        for (msg, want) in [
            (
                r#"401 {"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#,
                "provider_auth",
            ),
            (
                r#"429 {"error":{"type":"insufficient_quota","code":"insufficient_quota","message":"q"}}"#,
                "provider_quota",
            ),
            (
                r#"529 {"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
                "provider_overloaded",
            ),
            (r#"500 {"error":{"message":"boom"}}"#, "provider_overloaded"),
            (
                r#"403 {"error":{"code":"llm_path_not_permitted","message":"no"}}"#,
                "gateway_refused",
            ),
            ("connection refused", "harness_failed"),
        ] {
            assert_eq!(classify(msg).0, want, "{msg}");
        }
        assert_eq!(classify("").1, "pi failed");
    }

    #[test]
    fn abort_is_an_aborted_error_and_responses_and_noise_are_handled() {
        let (_, term) = run(&[r#"{"type":"agent_settled","aborted":true}"#]);
        assert!(matches!(term, Some(HarnessEvent::Error { ref code, .. }) if code == "aborted"));
        let mut m = StreamMapper::default();
        let r = m.map_line(r#"{"type":"response","command":"prompt","success":true,"data":{"disposition":"queued"}}"#);
        assert_eq!(
            r.response,
            Some(Response {
                command: "prompt".into(),
                success: true,
                disposition: Some("queued".into()),
                error: None
            })
        );
        let r =
            m.map_line(r#"{"type":"response","command":"prompt","success":false,"error":"nope"}"#);
        assert_eq!(r.response.unwrap().error.as_deref(), Some("nope"));
        for noise in [
            "",
            "not json",
            r#"{"type":"queue_update"}"#,
            r#"{"type":"entry_appended"}"#,
        ] {
            assert_eq!(m.map_line(noise), Mapped::default(), "{noise}");
        }
        let r = m.map_line(super::super::launcher::OVERLONG_LINE);
        assert!(
            matches!(r.terminal, Some(HarnessEvent::Error { ref code, .. }) if code == "harness_failed")
        );
    }
}

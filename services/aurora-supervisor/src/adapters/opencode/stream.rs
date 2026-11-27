//! `opencode run --format json` line -> [`HarnessEvent`] mapping
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`).
//!
//! One JSON object per line, each with a `sessionID`; the shapes in the tests were recorded from a
//! real 1.19.6 run against the mock upstream of `aurora-testkit`. Mapping (for
//! [`super::PINNED_VERSION`]):
//! - `text` (the **whole** part: no streaming deltas) -> `TextDelta`; `tool_use` (always already
//!   completed, with `state.input`/`state.output`) -> `ToolStarted` + `ToolFinished`. The platform
//!   tools (`aurora_<tool>`) are named `mcp__aurora__<tool>`, as for the other harnesses.
//! - `step_finish` (`reason`, `tokens`): a reason other than `tool-calls` is the end of the answer; its
//!   tokens add up to the fallback usage. OpenCode's events carry **no model**: the per-model usage
//!   comes from `opencode db` after the turn ([`usage_from_rows`]); without it, one entry for the
//!   default model.
//! - `error` -> the terminal `Error`, classified through the shared table (see [`classify`]).
//! - The end of the turn is the process's exit (exit 0 -> `Done`), which the session decides.
//! - Events of a session other than the turn's own (a subagent's, if one is ever printed) are ignored,
//!   so a subagent's `step_finish` never ends the turn.

use serde_json::{Value, json};

use crate::adapters::skill_use;
use crate::harness::HarnessEvent;

const SUMMARY_MAX: usize = 120;
const MESSAGE_MAX: usize = 500;

/// OpenCode retries a 429 or 5xx (`isRetryable`) about six times with a growing delay before it
/// reports the error (*verified*: about 72 s for a 429 on every request, no setting in 1.19.6). The
/// message says so, so the wait is explained.
pub const RETRY_NOTE: &str = "OpenCode retried the request for about 70 seconds before giving up";

#[derive(Debug, Default, PartialEq)]
pub struct Mapped {
    pub events: Vec<HarnessEvent>,
    /// Set for `error`; always an `Error`.
    pub error: Option<HarnessEvent>,
    /// The line was an OpenCode JSON event of the turn's session (the session exists from here on).
    pub json: bool,
    /// The turn's session id, from the first event.
    pub session: Option<String>,
    /// A `step_finish` that is not `tool-calls`: the answer is complete, the process is about to exit.
    pub finished: bool,
}

#[derive(Debug, Default)]
pub struct StreamMapper {
    /// The turn's own session: the first event's (or the resumed one).
    session: Option<String>,
    /// The model the turn ran on: the events do not say.
    model: String,
    open: Vec<String>,
    /// Sum of the `step_finish` tokens: input, output, cache read, cache write.
    steps: [i64; 4],
}

fn trunc(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn platform_name(name: &str) -> String {
    match name.strip_prefix("aurora_") {
        Some(rest) => format!("mcp__aurora__{rest}"),
        None => name.to_owned(),
    }
}

fn summary(name: &str, state: &Value) -> String {
    let input = state.get("input").and_then(Value::as_object);
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input.and_then(|o| o.get(*k)).and_then(Value::as_str))
    };
    if let Some(c) = pick(&[
        "command",
        "filePath",
        "path",
        "pattern",
        "description",
        "url",
    ]) {
        return format!(
            "{name}: {}",
            trunc(c.lines().next().unwrap_or(c), SUMMARY_MAX)
        );
    }
    format!("{name} ({} args)", input.map_or(0, serde_json::Map::len))
}

impl StreamMapper {
    /// `resume` = the session id of a resumed turn, so its events are told from others at once.
    pub fn new(model: &str, resume: Option<&str>) -> Self {
        Self {
            session: resume.map(str::to_owned),
            model: model.to_owned(),
            ..Self::default()
        }
    }

    pub fn map_line(&mut self, line: &str) -> Mapped {
        let mut out = Mapped::default();
        if line == super::launcher::OVERLONG_LINE {
            out.error = Some(HarnessEvent::Error {
                code: "harness_failed".into(),
                message: "opencode produced an output line over the size limit".into(),
            });
            return out;
        }
        if !line.starts_with('{') {
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return out;
        };
        let sid = v.get("sessionID").and_then(Value::as_str);
        if self.session.is_none() {
            self.session = sid.map(str::to_owned);
        }
        if sid.is_some() && sid != self.session.as_deref() {
            return out; // another session's event
        }
        out.json = true;
        out.session.clone_from(&self.session);
        let part = v.get("part");
        match v.get("type").and_then(Value::as_str) {
            Some("text") => {
                let t = part
                    .and_then(|p| p.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !t.is_empty() {
                    let id = part
                        .and_then(|p| p.get("messageID"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    out.events.push(HarnessEvent::TextDelta {
                        message_id: id.to_owned(),
                        delta: t.to_owned(),
                        reply_to: None,
                    });
                }
            }
            Some("tool_use") => {
                if let Some(p) = part {
                    self.tool(p, &mut out);
                }
            }
            Some("step_finish") => {
                if let Some(p) = part {
                    self.step_finish(p, &mut out);
                }
            }
            Some("error") => {
                self.finish_open(&mut out);
                let (code, message) = classify(v.get("error"));
                out.error = Some(HarnessEvent::Error { code, message });
            }
            _ => {}
        }
        out
    }

    fn tool(&mut self, p: &Value, out: &mut Mapped) {
        let call = p.get("callID").and_then(Value::as_str).unwrap_or_default();
        let name = platform_name(p.get("tool").and_then(Value::as_str).unwrap_or("tool"));
        let state = p.get("state").cloned().unwrap_or(Value::Null);
        if call.is_empty() {
            return;
        }
        // (C-1175) OpenCode's own `skill` tool, a `read` of a skill's `SKILL.md`, or a read
        // command in `bash`.
        let input = state.get("input");
        let skill = match name.as_str() {
            "skill" => skill_use::of_skill_tool(input, &["name"]),
            "read" => skill_use::of_read(input, super::SKILLS_DIR),
            "bash" => skill_use::of_shell(input, super::SKILLS_DIR),
            _ => None,
        };
        out.events.push(HarnessEvent::ToolStarted {
            call_id: call.to_owned(),
            summary: summary(&name, &state),
            name,
            skill,
        });
        let ok = state.get("status").and_then(Value::as_str) == Some("completed");
        let text = state
            .get("output")
            .or_else(|| state.get("error"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        out.events.push(HarnessEvent::ToolFinished {
            call_id: call.to_owned(),
            ok,
            summary: (!text.trim().is_empty()).then(|| trunc(text.trim_end(), SUMMARY_MAX)),
        });
    }

    fn step_finish(&mut self, p: &Value, out: &mut Mapped) {
        let t = p.get("tokens");
        let n = |ptr: &str| {
            t.and_then(|t| t.pointer(ptr))
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0)
        };
        for (slot, ptr) in
            self.steps
                .iter_mut()
                .zip(["/input", "/output", "/cache/read", "/cache/write"])
        {
            *slot += n(ptr);
        }
        if p.get("reason").and_then(Value::as_str) != Some("tool-calls") {
            out.finished = true;
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

    /// The usage when `opencode db` is not available: one entry, for the default model, from the
    /// `step_finish` tokens (D-304: the model is the one the turn was asked to run).
    pub fn usage_fallback(&self) -> Value {
        usage_json(&[(
            self.model.clone(),
            self.steps[0],
            self.steps[1],
            self.steps[2],
            self.steps[3],
        )])
    }
}

type Row = (String, i64, i64, i64, i64);

fn usage_json(rows: &[Row]) -> Value {
    let sum = |f: fn(&Row) -> i64| rows.iter().map(f).sum::<i64>();
    json!({
        "input_tokens": sum(|r| r.1),
        "output_tokens": sum(|r| r.2),
        "cache_read_tokens": sum(|r| r.3),
        "cache_write_tokens": sum(|r| r.4),
        "by_model": rows.iter().map(|(m, i, o, r, w)| json!({
            "model": m,
            "input_tokens": i,
            "output_tokens": o,
            "cache_read_tokens": r,
            "cache_write_tokens": w,
        })).collect::<Vec<_>>(),
    })
}

/// The per-model usage from the output of the `opencode db` query (`--format json`: an array of
/// `{model, input, output, cache_read, cache_write}`). `None` when the output is not that, or has
/// no row: the caller uses [`StreamMapper::usage_fallback`]. The cost is never read (D-304).
pub fn usage_from_rows(text: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let rows: Vec<Row> = v
        .as_array()?
        .iter()
        .filter_map(|r| {
            let n = |k: &str| r.get(k).and_then(Value::as_i64).unwrap_or(0).max(0);
            Some((
                r.get("model")?.as_str()?.to_owned(),
                n("input"),
                n("output"),
                n("cache_read"),
                n("cache_write"),
            ))
        })
        .collect();
    (!rows.is_empty()).then(|| usage_json(&rows))
}

/// The session id from the output of `ROOT_SESSION_SQL`.
pub fn session_from_rows(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let id = v.as_array()?.first()?.get("id")?.as_str()?;
    super::config::is_session_id(id).then(|| id.to_owned())
}

/// Maps OpenCode's `error` object (`{name: "APIError", data: {message, statusCode, isRetryable,
/// responseBody}}`, or another name with `data.message`) to `(code, message)` through the shared
/// classification (`12 §Provider refusal classification`): the status and the response body are
/// handed over as the table reads them. A retryable error says how long OpenCode retried.
pub fn classify(error: Option<&Value>) -> (String, String) {
    let data = error.and_then(|e| e.get("data"));
    let message = data
        .and_then(|d| d.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = data
        .and_then(|d| d.get("responseBody"))
        .and_then(Value::as_str)
        .filter(|b| !b.trim().is_empty());
    let status = data
        .and_then(|d| d.get("statusCode"))
        .and_then(Value::as_i64);
    let retried = data
        .and_then(|d| d.get("isRetryable"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = error
        .and_then(|e| e.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if message.is_empty() && body.is_none() {
        return (
            "harness_failed".into(),
            trunc(&format!("opencode failed ({name})"), MESSAGE_MAX),
        );
    }
    let text = match (status, body) {
        (Some(s), Some(b)) => format!("status {s} {b}"),
        (Some(s), None) => format!("status {s} {message}"),
        (None, Some(b)) => b.to_owned(),
        (None, None) => message.to_owned(),
    };
    let (code, mut m) = crate::adapters::codex::stream::classify_failure(&text);
    if m == "codex failed" {
        m = "opencode failed".into();
    }
    if let Some(s) = status
        && let Some(rest) = m.strip_prefix(&format!("status {s} "))
    {
        m = rest.to_owned();
    }
    let mut m = trunc(&m, MESSAGE_MAX);
    if retried {
        m = format!("{m} ({RETRY_NOTE})");
    }
    (code, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> (Vec<HarnessEvent>, Vec<Mapped>) {
        let mut m = StreamMapper::new("m1", None);
        let (mut ev, mut all) = (Vec::new(), Vec::new());
        for l in lines {
            let r = m.map_line(l);
            ev.extend(r.events.clone());
            all.push(r);
        }
        (ev, all)
    }

    /// (C-1175) OpenCode's `skill` tool, a `read` of a skill's `SKILL.md` and a `bash` read.
    #[test]
    fn loading_a_skill_names_it() {
        let call = |id: &str, tool: &str, input: &str| {
            format!(
                r#"{{"type":"tool_use","sessionID":"ses_a","part":{{"type":"tool","tool":"{tool}","callID":"{id}","state":{{"status":"completed","input":{input},"output":"x"}}}}}}"#
            )
        };
        let lines = [
            call("a", "skill", r#"{"name":"pdf"}"#),
            call(
                "b",
                "read",
                r#"{"filePath":"/workspace/.agents/skills/triage/SKILL.md"}"#,
            ),
            call(
                "c",
                "bash",
                r#"{"command":"cat .agents/skills/notes/SKILL.md"}"#,
            ),
            call("d", "aurora_skill", r#"{"name":"pdf"}"#),
        ];
        let (ev, _) = run(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        let got: Vec<Option<String>> = ev
            .iter()
            .filter_map(|e| match e {
                HarnessEvent::ToolStarted { skill, .. } => Some(skill.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            [
                Some("pdf".into()),
                Some("triage".into()),
                Some("notes".into()),
                None
            ]
        );
    }

    // Lines recorded from OpenCode 1.19.6 (trimmed: the snapshot hashes and timestamps are dropped).
    const START: &str = r#"{"type":"step_start","sessionID":"ses_a","part":{"type":"step-start"}}"#;
    const TEXT: &str = r#"{"type":"text","sessionID":"ses_a","part":{"type":"text","text":"pong","messageID":"msg_1"}}"#;
    const TOOL: &str = r#"{"type":"tool_use","sessionID":"ses_a","part":{"type":"tool","tool":"bash","callID":"toolu_1","state":{"status":"completed","input":{"command":"echo hi\nmore"},"output":"hi\n","metadata":{"exit":0},"title":"echo hi"},"messageID":"msg_2"}}"#;
    const MCP: &str = r#"{"type":"tool_use","sessionID":"ses_a","part":{"type":"tool","tool":"aurora_ping","callID":"toolu_2","state":{"status":"completed","input":{"text":"x"},"output":"pong"}}}"#;
    const CALLS: &str = r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"tool-calls","type":"step-finish","tokens":{"total":20,"input":11,"output":4,"reasoning":0,"cache":{"write":2,"read":3}},"cost":0}}"#;
    const STOP: &str = r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"stop","type":"step-finish","tokens":{"total":20,"input":11,"output":4,"reasoning":0,"cache":{"write":2,"read":3}},"cost":0}}"#;
    const OTHER: &str = r#"{"type":"step_finish","sessionID":"ses_child","part":{"reason":"stop","type":"step-finish","tokens":{"input":999,"output":999,"cache":{"write":0,"read":0}}}}"#;

    #[test]
    fn a_text_turn_maps_the_whole_text_and_finishes_on_the_stop_step() {
        let (ev, all) = run(&[START, TEXT, STOP]);
        assert_eq!(
            ev,
            [HarnessEvent::TextDelta {
                message_id: "msg_1".into(),
                delta: "pong".into(),
                reply_to: None
            }]
        );
        assert!(
            all.iter()
                .all(|m| m.json && m.session.as_deref() == Some("ses_a"))
        );
        assert!(!all[0].finished && all[2].finished);
        assert!(all.iter().all(|m| m.error.is_none()));
    }

    #[test]
    fn a_tool_call_is_started_and_finished_together_and_platform_tools_are_renamed() {
        let (ev, _) = run(&[TOOL, MCP]);
        assert!(
            matches!(&ev[0], HarnessEvent::ToolStarted { call_id, name, summary }
            if call_id == "toolu_1" && name == "bash" && summary == "bash: echo hi")
        );
        assert!(
            matches!(&ev[1], HarnessEvent::ToolFinished { call_id, ok: true, summary: Some(s) }
            if call_id == "toolu_1" && s == "hi")
        );
        assert!(
            matches!(&ev[2], HarnessEvent::ToolStarted { name, .. } if name == "mcp__aurora__ping")
        );
    }

    #[test]
    fn a_failed_tool_is_finished_not_ok() {
        let (ev, _) = run(&[
            r#"{"type":"tool_use","sessionID":"ses_a","part":{"tool":"read","callID":"c","state":{"status":"error","input":{"filePath":"/x"},"error":"not found"}}}"#,
        ]);
        assert!(
            matches!(&ev[1], HarnessEvent::ToolFinished { ok: false, summary: Some(s), .. } if s == "not found")
        );
    }

    #[test]
    fn another_sessions_events_never_end_or_count_in_the_turn() {
        let mut m = StreamMapper::new("m1", None);
        m.map_line(START);
        let o = m.map_line(OTHER);
        assert!(!o.json && !o.finished && o.events.is_empty());
        m.map_line(CALLS);
        let u = m.usage_fallback();
        assert_eq!(u["input_tokens"], 11, "{u}");
        // A resumed turn knows its session before the first event.
        let mut r = StreamMapper::new("m1", Some("ses_old"));
        assert!(!r.map_line(START).json, "an event of another session");
    }

    #[test]
    fn the_fallback_usage_is_one_entry_for_the_default_model_summed_over_the_steps() {
        let mut m = StreamMapper::new("m1", None);
        for l in [START, CALLS, TOOL, STOP] {
            m.map_line(l);
        }
        let u = m.usage_fallback();
        assert_eq!(u["by_model"].as_array().unwrap().len(), 1);
        assert_eq!(u["by_model"][0]["model"], "m1");
        for (k, v) in [
            ("input_tokens", 22),
            ("output_tokens", 8),
            ("cache_read_tokens", 6),
            ("cache_write_tokens", 4),
        ] {
            assert_eq!(u[k], v, "{k}");
            assert_eq!(u["by_model"][0][k], v, "by_model {k}");
        }
    }

    #[test]
    fn the_database_rows_become_per_model_usage_whose_totals_are_the_sum() {
        let rows = r#"[{"model":"a","input":11,"output":4,"cache_read":3,"cache_write":2},
                      {"model":"b","input":5,"output":1,"cache_read":0,"cache_write":0}]"#;
        let u = usage_from_rows(rows).unwrap();
        assert_eq!(u["by_model"].as_array().unwrap().len(), 2);
        assert_eq!(u["input_tokens"], 16);
        assert_eq!(u["output_tokens"], 5);
        assert_eq!(u["cache_read_tokens"], 3);
        assert_eq!(u["cache_write_tokens"], 2);
        for bad in ["", "[]", "{}", "not json", r#"[{"input":1}]"#] {
            assert!(usage_from_rows(bad).is_none(), "{bad}");
        }
        assert_eq!(
            session_from_rows(r#"[{"id":"ses_abc"}]"#).as_deref(),
            Some("ses_abc")
        );
        assert_eq!(session_from_rows(r#"[{"id":"x'; drop"}]"#), None);
        assert_eq!(session_from_rows("[]"), None);
    }

    fn err(e: &str) -> (String, String) {
        classify(serde_json::from_str::<Value>(e).unwrap().get("error"))
    }

    #[test]
    fn a_rate_limit_says_how_long_opencode_retried_and_a_billing_refusal_is_a_quota() {
        // Recorded from 1.19.6: a 429 on every request, after six requests and about 72 s.
        let (code, msg) = err(
            r#"{"type":"error","error":{"name":"APIError","data":{"message":"mock fail-429","statusCode":429,"isRetryable":true,"responseBody":"{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"mock fail-429\"}}"}}}"#,
        );
        assert_eq!(code, "provider_rate_limited", "{msg}");
        assert!(
            msg.contains("mock fail-429") && msg.contains(RETRY_NOTE),
            "{msg}"
        );
        let (code, _) = err(
            r#"{"type":"error","error":{"name":"APIError","data":{"message":"x","statusCode":429,"isRetryable":true,"responseBody":"{\"error\":{\"type\":\"insufficient_quota\",\"code\":\"insufficient_quota\",\"message\":\"spent\"}}"}}}"#,
        );
        assert_eq!(code, "provider_quota");
        let (code, msg) = err(
            r#"{"type":"error","error":{"name":"APIError","data":{"message":"x","statusCode":402,"isRetryable":false,"responseBody":"{\"type\":\"error\",\"error\":{\"type\":\"billing_error\",\"message\":\"mock fail-402\"}}"}}}"#,
        );
        assert_eq!(code, "provider_quota");
        assert!(!msg.contains(RETRY_NOTE), "{msg}");
    }

    #[test]
    fn auth_failures_and_unknown_errors_are_classified() {
        let (code, msg) = err(
            r#"{"type":"error","error":{"name":"APIError","data":{"message":"mock fail-401","statusCode":401,"isRetryable":false,"responseBody":"{\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"mock fail-401\"}}"}}}"#,
        );
        assert_eq!(code, "provider_auth", "{msg}");
        let (code, msg) = err(
            r#"{"type":"error","error":{"name":"UnknownError","data":{"message":"Unexpected server error. Check server logs for details."}}}"#,
        );
        assert_eq!(code, "harness_failed", "{msg}");
        assert!(msg.contains("Unexpected server error"));
        let (code, _) = err(r#"{"type":"error","error":{"name":"X"}}"#);
        assert_eq!(code, "harness_failed");
    }

    #[test]
    fn banner_lines_and_the_overlong_marker_are_handled() {
        let mut m = StreamMapper::new("m", None);
        assert_eq!(
            m.map_line("Performing one time database migration"),
            Mapped::default()
        );
        assert_eq!(m.map_line("{not json"), Mapped::default());
        let o = m.map_line(super::super::launcher::OVERLONG_LINE);
        assert!(
            matches!(o.error, Some(HarnessEvent::Error { ref code, .. }) if code == "harness_failed")
        );
    }
}

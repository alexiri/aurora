//! In-process mock LLM upstream for the **harness conformance suites** of the phase-5 adapters
//! (`docs/18 §Harness adapters`, P5-HARN): Anthropic **Messages** (`POST /v1/messages`), OpenAI
//! **Responses** (`POST /v1/responses`) and OpenAI **Chat** (`POST /v1/chat/completions`), all SSE,
//! one scenario per request `model`:
//!
//! | model          | behaviour                                                                         |
//! |----------------|-----------------------------------------------------------------------------------|
//! | `say` (default)| the text `pong`, then the end of the message with usage                           |
//! | `tool`         | a `bash` call (`{"command":"echo hi"}`); after its result, the text `done`        |
//! | `steer`        | a `bash` call (`sleep 2`), so a steer can land while it runs; then the text `done`|
//! | `mcp`          | a call of the platform tool named by [`HarnessUpstream::start_with_mcp_tool`]     |
//! | `delegate`     | a call of the harness's `delegate` tool (a subagent); after its result, `done`    |
//! | `long`         | the first half of the text, then the stream stalls (abort tests)                  |
//! | `fail-401`     | `401` authentication error                                                        |
//! | `fail-429`     | `429` rate limit                                                                  |
//! | `fail-quota`   | `429` with `insufficient_quota` (OpenAI) / a billing-style rate limit (Anthropic) |
//! | `fail-500`     | `500` server error                                                                |
//! | `fail-402`     | `402` billing error (a harness that retries every 429 and 5xx, OpenCode, ends at once) |
//!
//! Usage is fixed so tests can add it up: input 11, output 4, cache read 3, cache write 2 (the
//! dialect's own fields; OpenAI dialects have no cache write). Every request is recorded with its
//! headers ([`Seen`]), so a test can prove which credential reached which path.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use bytes::Bytes;
use serde_json::{Value, json};

/// One request the upstream saw.
#[derive(Debug, Clone)]
pub struct Seen {
    pub path: String,
    /// Lower-case header names with their values.
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The credential the request carried (`x-api-key` for Messages, the bearer for OpenAI).
    pub fn credential(&self) -> Option<String> {
        self.header("x-api-key").map(str::to_owned).or_else(|| {
            self.header("authorization")
                .map(|a| a.strip_prefix("Bearer ").unwrap_or(a).to_owned())
        })
    }
}

#[derive(Clone)]
pub struct HarnessUpstream {
    seen: Arc<Mutex<Vec<Seen>>>,
    base: String,
    mcp_tool: String,
    /// The harness's shell tool (`bash` for Pi and OpenCode, `shell` for goose).
    shell_tool: String,
    /// The harness's subagent tool (`delegate` for goose, `task` for OpenCode).
    delegate_tool: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Messages,
    Responses,
    Chat,
}

/// What one request asks the model to do next.
enum Step {
    Text(&'static str),
    Tool { name: String, input: Value },
}

fn has_tool_result(body: &Value) -> bool {
    let s = body.to_string();
    s.contains("\"tool_result\"")
        || s.contains("\"role\":\"tool\"")
        || s.contains("function_call_output")
}

fn plan(model: &str, body: &Value, tools: [&str; 3]) -> Step {
    let [mcp_tool, shell_tool, delegate_tool] = tools;
    let done = has_tool_result(body);
    match model {
        "tool" if !done => Step::Tool {
            name: shell_tool.to_owned(),
            input: json!({"command": "echo hi"}),
        },
        "steer" if !done => Step::Tool {
            name: shell_tool.to_owned(),
            input: json!({"command": "sleep 2"}),
        },
        "mcp" if !done => Step::Tool {
            name: mcp_tool.to_owned(),
            input: json!({}),
        },
        // A subagent: the harness's delegate tool, then (once its result is back) the text.
        // The input carries the arguments of every harness's tool (goose `delegate`: `instructions`;
        // OpenCode `task`: `description`, `prompt`, `subagent_type`).
        "delegate" if !done => Step::Tool {
            name: delegate_tool.to_owned(),
            input: json!({
                "instructions": "say pong",
                "description": "say pong",
                "prompt": "say pong",
                "subagent_type": "general",
            }),
        },
        "tool" | "steer" | "mcp" | "delegate" => Step::Text("done"),
        _ => Step::Text("pong"),
    }
}

fn failure(model: &str, dialect: Dialect) -> Option<Response> {
    let (status, kind) = match model {
        "fail-401" => (401, "authentication_error"),
        "fail-429" => (429, "rate_limit_error"),
        "fail-quota" => (429, "insufficient_quota"),
        "fail-500" => (500, "api_error"),
        "fail-402" => (402, "billing_error"),
        _ => return None,
    };
    let body = if dialect == Dialect::Messages {
        json!({"type": "error", "error": {"type": kind, "message": format!("mock {model}")}})
    } else {
        json!({"error": {"message": format!("mock {model}"), "type": kind, "code": kind}})
    };
    Some(
        Response::builder()
            .status(StatusCode::from_u16(status).unwrap())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
}

fn frame(event: Option<&str>, data: &Value) -> Bytes {
    match event {
        Some(e) => Bytes::from(format!("event: {e}\ndata: {data}\n\n")),
        None => Bytes::from(format!("data: {data}\n\n")),
    }
}

/// The frames of one reply. `stall_after` = number of frames sent before the stream stalls.
fn frames(dialect: Dialect, model: &str, step: &Step, n: usize) -> Vec<Bytes> {
    let long = model == "long";
    let (a, b) = if long { ("po", "ng") } else { ("pong", "") };
    match dialect {
        Dialect::Messages => messages_frames(model, step, a, b),
        Dialect::Responses => responses_frames(model, step, a, b, n),
        Dialect::Chat => chat_frames(model, step, a, b),
    }
}

fn messages_frames(model: &str, step: &Step, a: &str, b: &str) -> Vec<Bytes> {
    let mut v = vec![frame(
        Some("message_start"),
        &json!({"type": "message_start", "message": {
            "id": "msg_1", "type": "message", "role": "assistant", "model": model, "content": [],
            "stop_reason": null,
            "usage": {"input_tokens": 11, "output_tokens": 1,
                      "cache_read_input_tokens": 3, "cache_creation_input_tokens": 2}}}),
    )];
    let stop = match step {
        Step::Tool { name, input } => {
            v.push(frame(
                Some("content_block_start"),
                &json!({"type": "content_block_start", "index": 0,
                        "content_block": {"type": "tool_use", "id": "toolu_1", "name": name, "input": {}}}),
            ));
            v.push(frame(
                Some("content_block_delta"),
                &json!({"type": "content_block_delta", "index": 0,
                        "delta": {"type": "input_json_delta", "partial_json": input.to_string()}}),
            ));
            v.push(frame(
                Some("content_block_stop"),
                &json!({"type": "content_block_stop", "index": 0}),
            ));
            "tool_use"
        }
        Step::Text(t) => {
            let (a, b) = if a == "po" { (a, b) } else { (*t, "") };
            v.push(frame(
                Some("content_block_start"),
                &json!({"type": "content_block_start", "index": 0,
                        "content_block": {"type": "text", "text": ""}}),
            ));
            v.push(frame(
                Some("content_block_delta"),
                &json!({"type": "content_block_delta", "index": 0,
                        "delta": {"type": "text_delta", "text": a}}),
            ));
            v.push(Bytes::new()); // stall point of `long`
            if !b.is_empty() {
                v.push(frame(
                    Some("content_block_delta"),
                    &json!({"type": "content_block_delta", "index": 0,
                            "delta": {"type": "text_delta", "text": b}}),
                ));
            }
            v.push(frame(
                Some("content_block_stop"),
                &json!({"type": "content_block_stop", "index": 0}),
            ));
            "end_turn"
        }
    };
    v.push(frame(
        Some("message_delta"),
        &json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 4}}),
    ));
    v.push(frame(
        Some("message_stop"),
        &json!({"type": "message_stop"}),
    ));
    v
}

fn responses_frames(model: &str, step: &Step, a: &str, b: &str, n: usize) -> Vec<Bytes> {
    let id = format!("resp_{n}");
    let base = json!({"id": id, "object": "response", "model": model, "status": "in_progress", "output": []});
    let mut v = vec![frame(
        Some("response.created"),
        &json!({"type": "response.created", "response": base}),
    )];
    let out = match step {
        Step::Tool { name, input } => {
            let item = json!({"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": name,
                              "arguments": input.to_string(), "status": "completed"});
            v.push(frame(
                Some("response.output_item.added"),
                &json!({"type": "response.output_item.added", "output_index": 0,
                        "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": name,
                                 "arguments": "", "status": "in_progress"}}),
            ));
            v.push(frame(
                Some("response.function_call_arguments.done"),
                &json!({"type": "response.function_call_arguments.done", "item_id": "fc_1",
                        "output_index": 0, "arguments": input.to_string()}),
            ));
            v.push(frame(
                Some("response.output_item.done"),
                &json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
            ));
            item
        }
        Step::Text(t) => {
            let (a, b) = if a == "po" { (a, b) } else { (*t, "") };
            let full = format!("{a}{b}");
            let item = json!({"type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
                              "content": [{"type": "output_text", "text": full, "annotations": []}]});
            v.push(frame(
                Some("response.output_item.added"),
                &json!({"type": "response.output_item.added", "output_index": 0,
                        "item": {"type": "message", "id": "msg_1", "role": "assistant",
                                 "status": "in_progress", "content": []}}),
            ));
            v.push(frame(
                Some("response.content_part.added"),
                &json!({"type": "response.content_part.added", "item_id": "msg_1", "output_index": 0,
                        "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}),
            ));
            v.push(frame(
                Some("response.output_text.delta"),
                &json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 0,
                        "content_index": 0, "delta": a}),
            ));
            v.push(Bytes::new()); // stall point of `long`
            if !b.is_empty() {
                v.push(frame(
                    Some("response.output_text.delta"),
                    &json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 0,
                            "content_index": 0, "delta": b}),
                ));
            }
            v.push(frame(
                Some("response.output_text.done"),
                &json!({"type": "response.output_text.done", "item_id": "msg_1", "output_index": 0,
                        "content_index": 0, "text": full}),
            ));
            v.push(frame(
                Some("response.content_part.done"),
                &json!({"type": "response.content_part.done", "item_id": "msg_1", "output_index": 0,
                        "content_index": 0, "part": item["content"][0]}),
            ));
            v.push(frame(
                Some("response.output_item.done"),
                &json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
            ));
            item
        }
    };
    v.push(frame(
        Some("response.completed"),
        &json!({"type": "response.completed", "response": {
            "id": id, "object": "response", "model": model, "status": "completed", "output": [out],
            "usage": {"input_tokens": 11, "input_tokens_details": {"cached_tokens": 3},
                      "output_tokens": 4, "output_tokens_details": {"reasoning_tokens": 0},
                      "total_tokens": 15}}}),
    ));
    v
}

fn chat_frames(model: &str, step: &Step, a: &str, b: &str) -> Vec<Bytes> {
    let chunk = |delta: &Value, fin: Option<&str>| {
        frame(
            None,
            &json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1, "model": model,
                    "choices": [{"index": 0, "delta": delta, "finish_reason": fin}]}),
        )
    };
    let mut v = vec![chunk(&json!({"role": "assistant", "content": ""}), None)];
    match step {
        Step::Tool { name, input } => {
            v.push(chunk(
                &json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                                        "function": {"name": name, "arguments": ""}}]}),
                None,
            ));
            v.push(chunk(
                &json!({"tool_calls": [{"index": 0, "function": {"arguments": input.to_string()}}]}),
                None,
            ));
            v.push(chunk(&json!({}), Some("tool_calls")));
        }
        Step::Text(t) => {
            let (a, b) = if a == "po" { (a, b) } else { (*t, "") };
            v.push(chunk(&json!({"content": a}), None));
            v.push(Bytes::new()); // stall point of `long`
            if !b.is_empty() {
                v.push(chunk(&json!({"content": b}), None));
            }
            v.push(chunk(&json!({}), Some("stop")));
        }
    }
    v.push(frame(
        None,
        &json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1, "model": model,
                "choices": [], "usage": {"prompt_tokens": 11, "completion_tokens": 4, "total_tokens": 15,
                                          "prompt_tokens_details": {"cached_tokens": 3}}}),
    ));
    v.push(Bytes::from("data: [DONE]\n\n"));
    v
}

async fn handle(
    me: HarnessUpstream,
    dialect: Dialect,
    path: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let n = {
        let mut seen = me.seen.lock().unwrap();
        seen.push(Seen {
            path: path.to_owned(),
            headers: headers
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
                .collect(),
            body: json.clone(),
        });
        seen.len()
    };
    let model = json.get("model").and_then(Value::as_str).unwrap_or("say");
    if let Some(r) = failure(model, dialect) {
        return r;
    }
    let step = plan(
        model,
        &json,
        [&me.mcp_tool, &me.shell_tool, &me.delegate_tool],
    );
    let all = frames(dialect, model, &step, n);
    let stall = model == "long" && matches!(step, Step::Text(_));
    let stream = futures::stream::unfold((all.into_iter(), stall), |(mut it, stall)| async move {
        match it.next() {
            // The empty frame marks where `long` stops for a long time.
            Some(b) if b.is_empty() && stall => {
                tokio::time::sleep(Duration::from_secs(120)).await;
                Some((Ok::<_, std::convert::Infallible>(b), (it, stall)))
            }
            Some(b) => Some((Ok(b), (it, stall))),
            None => None,
        }
    });
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

impl HarnessUpstream {
    pub async fn start() -> Self {
        Self::start_with_mcp_tool("aurora_ping").await
    }

    /// The `mcp` scenario calls the platform tool with this name (it depends on the harness:
    /// `mcp__aurora__ping` for Pi, `aurora__ping` for goose, `aurora_ping` for OpenCode).
    pub async fn start_with_mcp_tool(tool: &str) -> Self {
        Self::start_with_tools(tool, "bash").await
    }

    /// [`HarnessUpstream::start_with_mcp_tool`] with the harness's shell tool name too.
    pub async fn start_with_tools(mcp_tool: &str, shell_tool: &str) -> Self {
        Self::start_with_all_tools(mcp_tool, shell_tool, "delegate").await
    }

    /// [`HarnessUpstream::start_with_tools`] with the harness's subagent tool name too.
    pub async fn start_with_all_tools(
        mcp_tool: &str,
        shell_tool: &str,
        delegate_tool: &str,
    ) -> Self {
        let me = Self {
            seen: Arc::default(),
            base: String::new(),
            mcp_tool: mcp_tool.to_owned(),
            shell_tool: shell_tool.to_owned(),
            delegate_tool: delegate_tool.to_owned(),
        };
        let route = |d: Dialect, p: &'static str, me: &Self| {
            let me = me.clone();
            post(move |headers: HeaderMap, body: Bytes| {
                let me = me.clone();
                async move { handle(me, d, p, headers, body).await }
            })
        };
        let app = Router::new()
            .route(
                "/v1/messages",
                route(Dialect::Messages, "/v1/messages", &me),
            )
            .route(
                "/v1/responses",
                route(Dialect::Responses, "/v1/responses", &me),
            )
            .route(
                "/v1/chat/completions",
                route(Dialect::Chat, "/v1/chat/completions", &me),
            )
            // Anything else a harness sends (a model listing, a catalogue): recorded, answered 404,
            // so a test can see a request outside the inference paths.
            .fallback({
                let me = me.clone();
                move |req: axum::extract::Request| {
                    let me = me.clone();
                    async move {
                        me.seen.lock().unwrap().push(Seen {
                            path: req.uri().path().to_owned(),
                            headers: req
                                .headers()
                                .iter()
                                .filter_map(|(k, v)| {
                                    Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned()))
                                })
                                .collect(),
                            body: Value::Null,
                        });
                        Response::builder()
                            .status(404)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(r#"{"error":{"message":"mock: no such path"}}"#))
                            .unwrap()
                    }
                }
            });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { base, ..me }
    }

    /// `http://127.0.0.1:PORT` (no `/v1`).
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Every request received, oldest first.
    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn post_json(base: &str, path: &str, body: Value) -> (u16, String) {
        let r = reqwest::Client::new()
            .post(format!("{base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }

    #[tokio::test]
    async fn each_dialect_answers_say_tool_and_failures_and_records_the_credential() {
        let up = HarnessUpstream::start().await;
        for (path, marker) in [
            ("/v1/messages", "\"text_delta\""),
            ("/v1/responses", "response.output_text.delta"),
            ("/v1/chat/completions", "chat.completion.chunk"),
        ] {
            let (s, t) = post_json(up.base(), path, json!({"model": "say"})).await;
            assert_eq!(
                (s, t.contains(marker), t.contains("pong")),
                (200, true, true)
            );
            let (s, t) = post_json(up.base(), path, json!({"model": "tool"})).await;
            assert!(s == 200 && t.contains("echo hi"), "{path}: {t}");
            let with_result = json!({"model": "tool", "messages": [{"role": "tool"}],
                                     "input": [{"type": "function_call_output"}]});
            let (_, t) = post_json(up.base(), path, with_result).await;
            assert!(t.contains("done") && !t.contains("echo hi"), "{path}: {t}");
            let (s, _) = post_json(up.base(), path, json!({"model": "fail-quota"})).await;
            assert_eq!(s, 429);
        }
        assert_eq!(up.requests().len(), 12);
        assert!(up.requests().iter().all(|r| r.credential().is_none()));
    }
}

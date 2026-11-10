//! In-process mock OpenAI **Responses** upstream (`POST /v1/responses`, SSE) for the harness
//! conformance suites (`docs/18 §Harness adapters`, P4-HAR). The scenario is chosen by the
//! request's `model`:
//!
//! | model              | behaviour                                                              |
//! |--------------------|------------------------------------------------------------------------|
//! | `say` (default)    | one assistant message `pong`, then `response.completed` with usage     |
//! | `tool`             | a `exec_command` call (`echo hi`); after its output, the message `done` |
//! | `long`             | a first text delta, then the stream stalls (abort tests)               |
//! | `fail-401`         | `401` with an OpenAI error body (`invalid_api_key`)                    |
//! | `fail-429`         | `429` `rate_limit_exceeded`                                            |
//! | `fail-quota`       | `429` `insufficient_quota`                                             |
//! | `fail-500`         | `500` `server_error`                                                   |
//!
//! Every request is recorded ([`MockResponses::requests`]) so a test can prove which credential
//! reached the upstream.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use bytes::Bytes;
use serde_json::{Value, json};

/// One request the upstream saw.
#[derive(Debug, Clone)]
pub struct Seen {
    pub path: String,
    pub authorization: Option<String>,
    pub body: Value,
}

#[derive(Clone)]
pub struct MockResponses {
    seen: Arc<Mutex<Vec<Seen>>>,
    base: String,
}

fn sse(event: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

fn error_response(status: u16, kind: &str) -> Response {
    let body = json!({"error": {"message": format!("mock {kind}"), "type": kind, "code": kind}});
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn message_item(text: &str) -> Value {
    json!({"type": "message", "role": "assistant", "id": "msg_1",
           "content": [{"type": "output_text", "text": text}]})
}

fn completed(id: &str) -> Bytes {
    sse(
        "response.completed",
        &json!({"type": "response.completed", "response": {"id": id, "usage": {
            "input_tokens": 5, "input_tokens_details": null,
            "output_tokens": 2, "output_tokens_details": null, "total_tokens": 7}}}),
    )
}

fn say(id: &str, text: &str) -> Vec<Bytes> {
    vec![
        sse(
            "response.created",
            &json!({"type": "response.created", "response": {"id": id}}),
        ),
        sse(
            "response.output_item.added",
            &json!({"type": "response.output_item.added", "output_index": 0,
                    "item": {"type": "message", "role": "assistant", "id": "msg_1", "content": []}}),
        ),
        sse(
            "response.output_text.delta",
            &json!({"type": "response.output_text.delta", "item_id": "msg_1",
                    "output_index": 0, "content_index": 0, "delta": text}),
        ),
        sse(
            "response.output_item.done",
            &json!({"type": "response.output_item.done", "output_index": 0,
                    "item": message_item(text)}),
        ),
        completed(id),
    ]
}

fn tool_call(id: &str) -> Vec<Bytes> {
    let item = json!({"type": "function_call", "id": "fc_1", "call_id": "call_1",
                      "name": "exec_command",
                      "arguments": json!({"cmd": "echo hi"}).to_string()});
    vec![
        sse(
            "response.created",
            &json!({"type": "response.created", "response": {"id": id}}),
        ),
        sse(
            "response.output_item.added",
            &json!({"type": "response.output_item.added", "output_index": 0, "item": item}),
        ),
        sse(
            "response.output_item.done",
            &json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
        ),
        completed(id),
    ]
}

async fn responses(State(me): State<MockResponses>, headers: HeaderMap, body: Bytes) -> Response {
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    me.seen.lock().unwrap().push(Seen {
        path: "/v1/responses".into(),
        authorization: headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        body: json.clone(),
    });
    let n = me.seen.lock().unwrap().len();
    let id = format!("resp_{n}");
    let model = json.get("model").and_then(Value::as_str).unwrap_or("say");
    let has_output = json
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|a| {
            a.iter()
                .any(|i| i.get("type").and_then(Value::as_str) == Some("function_call_output"))
        });
    let events = match model {
        "fail-401" => return error_response(401, "invalid_api_key"),
        "fail-429" => return error_response(429, "rate_limit_exceeded"),
        "fail-quota" => return error_response(429, "insufficient_quota"),
        "fail-500" => return error_response(500, "server_error"),
        "tool" if !has_output => tool_call(&id),
        "tool" => say(&id, "done"),
        "long" => {
            let mut v = say(&id, "start");
            v.truncate(3); // created, item added, one delta: then nothing for a long time
            v
        }
        _ => say(&id, "pong"),
    };
    let stall = model == "long";
    let stream =
        futures::stream::unfold((events.into_iter(), stall), |(mut it, stall)| async move {
            match it.next() {
                Some(b) => Some((Ok::<_, std::convert::Infallible>(b), (it, stall))),
                None if stall => {
                    tokio::time::sleep(Duration::from_secs(120)).await;
                    None
                }
                None => None,
            }
        });
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

impl MockResponses {
    pub async fn start() -> Self {
        let me = Self {
            seen: Arc::default(),
            base: String::new(),
        };
        let app = Router::new()
            .route("/v1/responses", post(responses))
            .with_state(me.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { base, ..me }
    }

    /// `http://127.0.0.1:PORT`; the OpenAI-form base URL is `<base>/v1`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Every request received, oldest first.
    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

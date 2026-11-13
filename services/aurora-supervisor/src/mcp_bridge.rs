//! `aurora-supervisor mcp-bridge --session <id>`: minimal MCP server over stdio (newline-delimited
//! JSON-RPC 2.0) relaying to the supervisor's tools socket (`docs/12 §Platform tools`, S-09).
//!
//! Implemented subset: `initialize`, `notifications/initialized` (and other notifications, ignored),
//! `ping`, `tools/list`, `tools/call`. Protocol version: the client's requested version when it is
//! one of [`SUPPORTED_VERSIONS`], else the latest known ([`LATEST_VERSION`], MCP 2025-11-25).
//! Each request uses its own socket connection so concurrent calls do not block each other.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aurora_protocol::tools::{ToolContent, ToolDescriptor, ToolResult};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::tools::{BridgeRequest, BridgeResponse, Method};

pub const LATEST_VERSION: &str = "2025-11-25";
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const UNAVAILABLE: &str = "platform tools temporarily unavailable";

/// One request/response exchange on a fresh connection.
async fn exchange(socket: &Path, req: &BridgeRequest) -> Result<BridgeResponse, String> {
    let mut s = UnixStream::connect(socket)
        .await
        .map_err(|e| e.to_string())?;
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    s.write_all(&line).await.map_err(|e| e.to_string())?;
    let mut r = BufReader::new(s);
    let mut resp = String::new();
    r.read_line(&mut resp).await.map_err(|e| e.to_string())?;
    serde_json::from_str(&resp).map_err(|e| e.to_string())
}

fn call_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn render(r: ToolResult) -> Value {
    if r.ok {
        let text = r
            .content
            .unwrap_or_default()
            .into_iter()
            .map(|c| match c {
                ToolContent::Text(t) => t.text,
                ToolContent::Json(j) => j.value.to_string(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        call_result(text, false)
    } else {
        let m = r.error.map_or_else(
            || "tool failed".to_owned(),
            |e| format!("{}: {}", e.code.as_str(), e.message),
        );
        call_result(m, true)
    }
}

async fn handle(session: &str, socket: &Path, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?; // notifications have no id
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let err = |code: i64, message: &str| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    Some(match method {
        "initialize" => {
            let want = params.get("protocolVersion").and_then(Value::as_str);
            let version = want
                .filter(|v| SUPPORTED_VERSIONS.contains(v))
                .unwrap_or(LATEST_VERSION);
            ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "aurora", "version": env!("CARGO_PKG_VERSION") },
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => {
            let req = BridgeRequest {
                id: 1,
                session: session.to_owned(),
                method: Method::List,
                tool: None,
                arguments: None,
            };
            let tools: Vec<ToolDescriptor> = match exchange(socket, &req).await {
                Ok(BridgeResponse {
                    result: Some(v), ..
                }) => serde_json::from_value(v).unwrap_or_default(),
                _ => Vec::new(),
            };
            ok(json!({ "tools": tools.into_iter().map(|t| json!({
                "name": t.name,
                "description": t.description.unwrap_or_default(),
                "inputSchema": t.input_schema,
            })).collect::<Vec<_>>() }))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(err(-32602, "name required"));
            };
            let req = BridgeRequest {
                id: 1,
                session: session.to_owned(),
                method: Method::Call,
                tool: Some(name.to_owned()),
                arguments: Some(
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                ),
            };
            ok(match exchange(socket, &req).await {
                Ok(BridgeResponse {
                    result: Some(v), ..
                }) => serde_json::from_value::<ToolResult>(v).map_or_else(
                    |_| call_result("malformed tool result".into(), true),
                    render,
                ),
                Ok(BridgeResponse { error: Some(e), .. }) => {
                    call_result(format!("{}: {}", e.code, e.message), true)
                }
                _ => call_result(UNAVAILABLE.into(), true),
            })
        }
        _ => err(-32601, "method not found"),
    })
}

/// (FIX-114) Longest request line the bridge accepts, and its in-flight cap.
pub const MAX_LINE: usize = 1024 * 1024;
pub const MAX_INFLIGHT: usize = 32;

/// One input line or a refused over-long one.
pub enum Line {
    Ok(String),
    TooLong,
}

/// Cancel-safe line reader bounded to `max` bytes per line: the partial line lives in the
/// caller's `buf`, an over-long line is discarded up to its newline and reported as
/// [`Line::TooLong`]. `Ok(None)` at end of input.
pub async fn next_line_bounded<R: tokio::io::AsyncBufRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
    skipping: &mut bool,
    max: usize,
) -> std::io::Result<Option<Line>> {
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(None);
        }
        match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => {
                if !*skipping {
                    buf.extend_from_slice(&chunk[..i]);
                }
                r.consume(i + 1);
                let over = *skipping || buf.len() > max;
                *skipping = false;
                let line = std::mem::take(buf);
                return Ok(Some(if over {
                    Line::TooLong
                } else {
                    Line::Ok(String::from_utf8_lossy(&line).into_owned())
                }));
            }
            None => {
                let n = chunk.len();
                if !*skipping {
                    buf.extend_from_slice(chunk);
                    if buf.len() > max {
                        buf.clear();
                        *skipping = true;
                    }
                }
                r.consume(n);
            }
        }
    }
}

/// Serves MCP on `input`/`output` until `input` closes.
pub async fn run<R, W>(session: String, socket: PathBuf, input: R, mut output: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let session = Arc::new(session);
    let socket = Arc::new(socket);
    let (tx, mut rx) = mpsc::channel::<Value>(32);
    let mut reader = BufReader::new(input);
    let (mut buf, mut skipping) = (Vec::new(), false);
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            line = next_line_bounded(&mut reader, &mut buf, &mut skipping, MAX_LINE) => {
                let line = match line {
                    Ok(Some(Line::Ok(l))) => l,
                    Ok(Some(Line::TooLong)) => {
                        let _ = tx.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"request too large"}})).await;
                        continue;
                    }
                    _ => break,
                };
                // (FIX-114) At most MAX_INFLIGHT calls in flight per bridge: a harness that
                // floods requests gets an error instead of an unbounded task per line.
                while tasks.try_join_next().is_some() {}
                if tasks.len() >= MAX_INFLIGHT {
                    let _ = tx.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32000,"message":"too many requests in flight"}})).await;
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    let _ = tx.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}})).await;
                    continue;
                };
                let (session, socket, tx) = (session.clone(), socket.clone(), tx.clone());
                tasks.spawn(async move {
                    if let Some(resp) = handle(&session, &socket, &msg).await {
                        let _ = tx.send(resp).await;
                    }
                });
            }
            Some(resp) = rx.recv() => {
                let mut b = resp.to_string().into_bytes();
                b.push(b'\n');
                if output.write_all(&b).await.is_err() || output.flush().await.is_err() { return; }
            }
        }
    }
    // Input closed: let in-flight calls answer before exiting.
    drop(tx);
    while tasks.join_next().await.is_some() {}
    while let Ok(resp) = rx.try_recv() {
        let mut b = resp.to_string().into_bytes();
        b.push(b'\n');
        let _ = output.write_all(&b).await;
    }
    let _ = output.flush().await;
}

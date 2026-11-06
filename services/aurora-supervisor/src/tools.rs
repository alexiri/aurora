//! Tools socket and relay (`docs/12-supervisor-harness.md` §Platform tools, spec fix S-09).
//!
//! The MCP bridge talks newline-delimited JSON over a Unix socket (`/run/aurora/tools.sock`):
//! requests `{id, session, method: "list" | "call", tool?, arguments?}`, responses
//! `{id, result}` or `{id, error: {code, message}}`. `session` is the conversation id. The socket
//! is not a trust boundary: identity comes from the runner (sandbox/boot) and the API (turn check).
//!
//! `list` answers from the synced catalogue (`.aurora/tools.json`). `call` is executed locally for
//! the `process_*` family, otherwise relayed as a `tool.call` frame stamped with the conversation and
//! its current turn, bounded by `TOOL_CALL_TIMEOUT`. After `TOOL_CONSECUTIVE_FAILURES` transport
//! failures in a session the hub answers "platform tools temporarily unavailable" immediately; one
//! probe call per `probe_interval` is let through, and a success closes the breaker.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aurora_core::limits;
use aurora_protocol::supervisor::SupervisorToRunner;
use aurora_protocol::tools::{
    TextContent, ToolCallBody, ToolContent, ToolDescriptor, ToolError, ToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, timeout};
use uuid::Uuid;

use crate::channel::ChannelSender;
use crate::process::{ProcessManager, code};

pub const DEFAULT_SOCKET: &str = "/run/aurora/tools.sock";
/// Serialized results larger than this are truncated (`12 §Path`).
pub const MAX_RESULT_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const TRUNCATION_NOTE: &str = "\n[result truncated: exceeded 64 KiB]";

#[derive(Debug, Clone)]
pub struct ToolsConfig {
    /// `None` disables the socket server.
    pub socket: Option<PathBuf>,
    pub call_timeout: Duration,
    pub max_failures: u32,
    pub probe_interval: Duration,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            socket: None,
            call_timeout: limits::TOOL_CALL_TIMEOUT,
            max_failures: limits::TOOL_CONSECUTIVE_FAILURES,
            probe_interval: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    List,
    Call,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BridgeRequest {
    pub id: u64,
    pub session: String,
    pub method: Method,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BridgeResponse {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeError>,
}

#[derive(Default)]
struct Breaker {
    failures: u32,
    last_probe: Option<Instant>,
}

#[derive(Default)]
struct HubState {
    turns: HashMap<Uuid, Uuid>,
    pending: HashMap<String, oneshot::Sender<ToolResult>>,
    breakers: HashMap<String, Breaker>,
    next_call: u64,
}

pub struct ToolHub {
    cfg: ToolsConfig,
    catalogue: PathBuf,
    out: ChannelSender,
    procs: Arc<ProcessManager>,
    state: Mutex<HubState>,
}

fn fail(call_id: &str, c: &str, message: impl Into<String>) -> ToolResult {
    ToolResult::failure(
        call_id,
        ToolError {
            code: code(c),
            message: message.into(),
        },
    )
}

fn text_ok(call_id: &str, text: String) -> ToolResult {
    ToolResult::success(call_id, vec![ToolContent::Text(TextContent { text })])
}

/// Bounds the serialized result to [`MAX_RESULT_BYTES`], replacing oversized content by a
/// truncated text rendering plus a note.
pub fn bound_result(r: ToolResult) -> ToolResult {
    let size = |r: &ToolResult| serde_json::to_vec(r).map_or(usize::MAX, |v| v.len());
    if size(&r) <= MAX_RESULT_BYTES {
        return r;
    }
    let Some(content) = &r.content else {
        return fail(&r.call_id, "result_too_large", "error too large");
    };
    let mut text = String::new();
    for c in content {
        match c {
            ToolContent::Text(t) => text.push_str(&t.text),
            ToolContent::Json(j) => text.push_str(&j.value.to_string()),
            _ => {}
        }
        text.push('\n');
    }
    // Leave room for the envelope and JSON escaping (worst case 6x for control chars).
    let mut budget = (MAX_RESULT_BYTES - 1024).min(text.len());
    loop {
        while !text.is_char_boundary(budget) {
            budget -= 1;
        }
        let mut t = text[..budget].to_owned();
        t.push_str(TRUNCATION_NOTE);
        let out = text_ok(&r.call_id, t);
        if size(&out) <= MAX_RESULT_BYTES || budget == 0 {
            return out;
        }
        budget = budget * 3 / 4;
    }
}

impl ToolHub {
    pub fn new(
        cfg: ToolsConfig,
        catalogue: PathBuf,
        out: ChannelSender,
        procs: Arc<ProcessManager>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            catalogue,
            out,
            procs,
            state: Mutex::default(),
        })
    }

    fn st(&self) -> std::sync::MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records (or clears) the conversation's current turn for stamping.
    pub fn set_turn(&self, conversation: Uuid, turn: Option<Uuid>) {
        let mut g = self.st();
        match turn {
            Some(t) => g.turns.insert(conversation, t),
            None => g.turns.remove(&conversation),
        };
    }

    /// Delivers a runner `tool.result` to the waiting call.
    pub fn on_result(&self, r: ToolResult) {
        let tx = self.st().pending.remove(&r.call_id);
        if let Some(tx) = tx {
            let _ = tx.send(r);
        }
    }

    pub fn catalogue(&self) -> Vec<ToolDescriptor> {
        let Ok(bytes) = std::fs::read(&self.catalogue) else {
            return Vec::new();
        };
        let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&bytes) else {
            return Vec::new();
        };
        items
            .into_iter()
            .filter_map(|v| serde_json::from_value::<ToolDescriptor>(v).ok())
            .collect()
    }

    /// Breaker gate: `true` when the call may proceed (closed, or an allowed probe).
    fn admit(&self, session: &str) -> bool {
        let mut g = self.st();
        let b = g.breakers.entry(session.to_owned()).or_default();
        if b.failures < self.cfg.max_failures {
            return true;
        }
        let due = b
            .last_probe
            .is_none_or(|t| t.elapsed() >= self.cfg.probe_interval);
        if due {
            b.last_probe = Some(Instant::now());
        }
        due
    }

    fn record(&self, session: &str, transport_ok: bool) {
        let mut g = self.st();
        let b = g.breakers.entry(session.to_owned()).or_default();
        if transport_ok {
            *b = Breaker::default();
        } else {
            b.failures = b.failures.saturating_add(1);
            if b.failures == self.cfg.max_failures {
                b.last_probe = Some(Instant::now());
            }
        }
    }

    /// Executes one tool call and returns a bounded result.
    pub async fn call(&self, session: &str, tool: &str, args: Value) -> ToolResult {
        bound_result(self.call_inner(session, tool, args).await)
    }

    async fn call_inner(&self, session: &str, tool: &str, args: Value) -> ToolResult {
        let Ok(conversation_id) = Uuid::parse_str(session) else {
            return fail("", "bad_session", "session must be a conversation id");
        };
        if !args.is_object() {
            return fail("", "bad_arguments", "arguments must be an object");
        }
        let call_id = {
            let mut g = self.st();
            g.next_call += 1;
            format!("{conversation_id}:{}", g.next_call)
        };
        if !self.catalogue().iter().any(|t| t.name == tool) {
            return fail(
                &call_id,
                "tool_unavailable",
                format!(
                    "tool `{tool}` is not available to this agent (it may have been removed or its prerequisite changed); ask an owner or retry in a new message"
                ),
            );
        }
        if ProcessManager::is_local_tool(tool) {
            return match self.procs.call(tool, &args) {
                Ok(v) => text_ok(&call_id, v.to_string()),
                Err(m) => fail(&call_id, "invalid_request", m),
            };
        }
        if !self.admit(session) {
            return fail(
                &call_id,
                "tools_unavailable",
                "platform tools temporarily unavailable; try again shortly",
            );
        }
        let turn_id = self.st().turns.get(&conversation_id).copied();
        let (tx, rx) = oneshot::channel();
        self.st().pending.insert(call_id.clone(), tx);
        let frame = SupervisorToRunner::ToolCall(ToolCallBody {
            call_id: call_id.clone(),
            conversation_id,
            turn_id,
            tool: tool.to_owned(),
            args,
        });
        if self.out.send(frame).await.is_err() {
            self.st().pending.remove(&call_id);
            self.record(session, false);
            return fail(&call_id, "transport", "platform connection closed");
        }
        match timeout(self.cfg.call_timeout, rx).await {
            Ok(Ok(r)) => {
                self.record(session, true);
                r
            }
            _ => {
                self.st().pending.remove(&call_id);
                self.record(session, false);
                fail(
                    &call_id,
                    "tool_timeout",
                    "the platform did not answer in time; the call may or may not have run",
                )
            }
        }
    }

    pub async fn handle(&self, req: BridgeRequest) -> BridgeResponse {
        let id = req.id;
        let bad = |m: &str| BridgeResponse {
            id,
            result: None,
            error: Some(BridgeError {
                code: "bad_request".into(),
                message: m.into(),
            }),
        };
        let result = match req.method {
            Method::List => serde_json::to_value(self.catalogue()),
            Method::Call => {
                let Some(tool) = req.tool else {
                    return bad("tool required");
                };
                let args = req
                    .arguments
                    .unwrap_or_else(|| Value::Object(Default::default()));
                serde_json::to_value(self.call(&req.session, &tool, args).await)
            }
        };
        match result {
            Ok(v) => BridgeResponse {
                id,
                result: Some(v),
                error: None,
            },
            Err(_) => bad("unserializable result"),
        }
    }

    /// [`ToolHub::serve`] for a spawned task owning its path.
    pub async fn serve_owned(self: Arc<Self>, path: PathBuf) {
        if let Err(e) = self.serve(&path).await {
            tracing::error!(error = %e, "tools socket failed");
        }
    }

    /// Binds the socket (0666: not a trust boundary) and serves until the task is dropped.
    pub async fn serve(self: Arc<Self>, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
        loop {
            let (conn, _) = listener.accept().await?;
            tokio::spawn(self.clone().connection(conn));
        }
    }

    async fn connection(self: Arc<Self>, conn: UnixStream) {
        let (r, mut w) = conn.into_split();
        let mut r = BufReader::new(r);
        let (tx, mut rx) = mpsc::channel::<BridgeResponse>(32);
        let writer = tokio::spawn(async move {
            while let Some(resp) = rx.recv().await {
                let Ok(mut line) = serde_json::to_vec(&resp) else {
                    continue;
                };
                line.push(b'\n');
                if w.write_all(&line).await.is_err() {
                    break;
                }
            }
        });
        loop {
            let mut buf = Vec::new();
            let n = (&mut r)
                .take(MAX_REQUEST_BYTES as u64 + 1)
                .read_until(b'\n', &mut buf)
                .await
                .unwrap_or(0);
            if n == 0 || buf.len() > MAX_REQUEST_BYTES {
                break;
            }
            let Ok(req) = serde_json::from_slice::<BridgeRequest>(&buf) else {
                let _ = tx
                    .send(BridgeResponse {
                        id: 0,
                        result: None,
                        error: Some(BridgeError {
                            code: "bad_request".into(),
                            message: "malformed request".into(),
                        }),
                    })
                    .await;
                continue;
            };
            let hub = self.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(hub.handle(req).await).await;
            });
        }
        drop(tx);
        let _ = writer.await;
    }
}

//! Tools socket and relay (`docs/12-supervisor-harness.md` §Platform tools, spec fix S-09).
//!
//! The MCP bridge talks newline-delimited JSON over a Unix socket (`/run/aurora/tools.sock`):
//! requests `{id, session, method: "list" | "call", tool?, arguments?}`, responses
//! `{id, result}` or `{id, error: {code, message}}`. `session` is the conversation id. The socket
//! is not a trust boundary: identity comes from the runner (sandbox/boot) and the API (turn check).
//!
//! `list` answers with the local `process_*` descriptors merged with the synced remote catalogue
//! (`.aurora/tools.json`, which holds remote tools only). `call` is executed locally for
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
use aurora_protocol::supervisor::{FileResult, SupervisorToRunner};
use aurora_protocol::tools::{
    TextContent, ToolCallBody, ToolContent, ToolDescriptor, ToolError, ToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, timeout};
use uuid::Uuid;

use crate::channel::ChannelSender;
use crate::outbound::{OutboundError, OutboundFile, send_file};
use crate::process::{ProcessManager, code};

pub const DEFAULT_SOCKET: &str = "/run/aurora/tools.sock";
/// Serialized results larger than this are truncated (`12 §Path`).
pub const MAX_RESULT_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Upload ceiling: the runner's API call allows 120 s, plus headroom for queueing.
const FILE_RESULT_TIMEOUT: Duration = Duration::from_secs(150);
/// Name of the supervisor-local file tool (`12 §Platform tools`).
pub const SEND_FILE: &str = "send_file";
/// `file.result`s that arrived before their `send_file` call registered (bounded).
const MAX_EARLY_RESULTS: usize = 32;
const TRUNCATION_NOTE: &str = "\n[result truncated: exceeded 64 KiB]";

#[derive(Debug, Clone)]
pub struct ToolsConfig {
    /// `None` disables the socket server.
    pub socket: Option<PathBuf>,
    pub call_timeout: Duration,
    pub max_failures: u32,
    pub probe_interval: Duration,
    /// Workspace root: `send_file` reads only below it.
    pub home: PathBuf,
    /// How long `send_file` waits for the runner's `file.result` after the last chunk.
    pub file_timeout: Duration,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            socket: None,
            call_timeout: limits::TOOL_CALL_TIMEOUT,
            max_failures: limits::TOOL_CONSECUTIVE_FAILURES,
            probe_interval: Duration::from_secs(5),
            home: PathBuf::from("/workspace"),
            file_timeout: FILE_RESULT_TIMEOUT,
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
    files: HashMap<String, oneshot::Sender<FileResult>>,
    early_files: HashMap<String, FileResult>,
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

fn is_local_tool(name: &str) -> bool {
    name == SEND_FILE || ProcessManager::is_local_tool(name)
}

fn send_file_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: SEND_FILE.to_owned(),
        description: Some(
            "Send a file from the workspace to the current conversation as an attachment; returns its attachment id."
                .to_owned(),
        ),
        input_schema: json!({"type":"object","additionalProperties":false,"required":["path"],"properties":{
            "path":{"type":"string","description":"File inside the workspace (absolute, or relative to it)"},
            "caption":{"type":"string","maxLength":limits::ATTACH_CAPTION_MAX}}}),
    }
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

    /// Delivers a runner `file.result` to the waiting `send_file` call (or parks it briefly if the
    /// call has not registered yet: the result can overtake the return of the upload).
    pub fn on_file_result(&self, r: FileResult) {
        let mut g = self.st();
        match g.files.remove(&r.upload_id) {
            Some(tx) => {
                let _ = tx.send(r);
            }
            None => {
                if g.early_files.len() >= MAX_EARLY_RESULTS {
                    g.early_files.clear();
                }
                g.early_files.insert(r.upload_id.clone(), r);
            }
        }
    }

    /// Remote tools from `.aurora/tools.json` (the API catalogue never holds `process_*`).
    fn remote_catalogue(&self) -> Vec<ToolDescriptor> {
        let Ok(bytes) = std::fs::read(&self.catalogue) else {
            return Vec::new();
        };
        let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&bytes) else {
            return Vec::new();
        };
        items
            .into_iter()
            .filter_map(|v| serde_json::from_value::<ToolDescriptor>(v).ok())
            .filter(|t| !is_local_tool(&t.name))
            .collect()
    }

    /// What `list` answers: the remote catalogue plus the local `process_*` descriptors.
    pub fn catalogue(&self) -> Vec<ToolDescriptor> {
        let mut all = self.remote_catalogue();
        all.extend(ProcessManager::local_descriptors());
        all.push(send_file_descriptor());
        all
    }

    /// `send_file {path, caption?}`: uploads a workspace file to the calling conversation and
    /// answers `{ attachment_id }` once the runner reports the API accepted it. `Err` is shown to
    /// the model.
    async fn send_file(
        &self,
        conversation_id: Uuid,
        turn_id: Option<Uuid>,
        args: &Value,
    ) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or("path required (a file inside the workspace)")?;
        let caption = match args.get("caption") {
            None | Some(Value::Null) => None,
            Some(Value::String(c)) if c.chars().count() <= limits::ATTACH_CAPTION_MAX => {
                Some(c.clone())
            }
            Some(Value::String(_)) => {
                return Err(format!(
                    "caption is longer than {} characters",
                    limits::ATTACH_CAPTION_MAX
                ));
            }
            Some(_) => return Err("caption must be a string".into()),
        };
        let turn_id = turn_id.ok_or("no turn is running in this conversation")?;
        let upload_id = send_file(
            &self.out,
            &self.cfg.home,
            OutboundFile {
                path: PathBuf::from(path),
                conversation_id,
                turn_id,
                caption,
            },
        )
        .await
        .map_err(|e| match e {
            OutboundError::BadPath => {
                "path must be an existing regular file inside the workspace".to_owned()
            }
            OutboundError::Reserved => {
                "files under .aurora/ are platform-managed and cannot be sent".to_owned()
            }
            OutboundError::TooLarge(max) => format!("file is larger than {max} bytes"),
            OutboundError::Io(e) => format!("cannot read the file: {}", e.kind()),
            OutboundError::Closed => "platform connection closed".to_owned(),
        })?;
        let (tx, rx) = oneshot::channel();
        let early = {
            let mut g = self.st();
            let early = g.early_files.remove(&upload_id);
            if early.is_none() {
                g.files.insert(upload_id.clone(), tx);
            }
            early
        };
        let result = match early {
            Some(r) => r,
            None => match timeout(self.cfg.file_timeout, rx).await {
                Ok(Ok(r)) => r,
                _ => {
                    self.st().files.remove(&upload_id);
                    return Err(
                        "the platform did not confirm the upload in time; the file may or may not have been sent"
                            .into(),
                    );
                }
            },
        };
        match (result.ok, result.attachment_id) {
            (true, Some(id)) => Ok(json!({ "attachment_id": id })),
            _ => Err(format!(
                "the platform rejected the file: {}",
                result
                    .error
                    .map_or_else(|| "unknown error".to_owned(), |e| e.message)
            )),
        }
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
        if tool == SEND_FILE {
            let turn_id = self.st().turns.get(&conversation_id).copied();
            return match self.send_file(conversation_id, turn_id, &args).await {
                Ok(v) => text_ok(&call_id, v.to_string()),
                Err(m) => fail(&call_id, "invalid_request", m),
            };
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

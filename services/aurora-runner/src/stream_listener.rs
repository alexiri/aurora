//! The relay-facing stream listener (phase 4, P4-PRV-2, D-147, SEC-82; `16 §W18` leg 1,
//! `16 §W17` runner plane, `11 §Port previews`; P4-TERM terminal leg `16 §W19`, C-617).
//!
//! One WebSocket per upstream connection: `GET /v1/preview-stream` on the stream listener
//! (:4108), `Authorization: Bearer aux_…` (a single-use stream grant) or — P4-TERM —
//! `Bearer auy_…` (a terminal grant). **The relay/API cannot name a sandbox or a target**:
//! the runner redeems the grant at the API (`POST /v1/runner/preview-streams/open`) and uses
//! only the answer's binding, checks it holds that sandbox's live supervisor channel for that
//! boot (`404 not_here` otherwise; the relay re-resolves once), asks the supervisor to open
//! the stream (`stream.open` on W6) or spawn the PTY (`pty.open`, `16 §W19`) and only then
//! completes the WebSocket handshake (101). After that binary messages are raw bytes
//! (≤ `PREVIEW_CHUNK` / `TERMINAL_CHUNK`) in both directions; text messages are ignored on a
//! port leg and interpreted as `pty.resize` on a terminal leg. Credit flow control on the
//! supervisor leg, TCP backpressure on this leg (the runner stops reading the WebSocket when
//! the supervisor-side credit is exhausted). A W6 reconnect or a replaced boot ends every
//! stream and PTY of that channel with 4503; streams are never resumed. An older supervisor
//! ignores `stream.*`/`pty.*` kinds, so the open times out → 4502.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aurora_core::limits::{
    PREVIEW_CHUNK, PREVIEW_CONNECT_TIMEOUT, PREVIEW_STREAM_WINDOW, TERMINAL_AWAKE_GRACE,
    TERMINAL_CHUNK, TERMINAL_STREAM_WINDOW,
};
use aurora_protocol::common::SecretString;
use aurora_protocol::runner::{
    ChannelRef, PreviewStreamClosedRequest, PreviewStreamOpenRequest, PreviewStreamOpenResponse,
    PtyClose, PtyCloseReason, PtyOpen, PtyRefuseReason, PtyResize, StreamCloseReason, StreamOpen,
    StreamRefuseReason,
};
use aurora_protocol::supervisor::{RunnerToSupervisor, SupervisorToRunner};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use uuid::Uuid;

use crate::api::{ApiClient, ApiError};
use crate::channel::ChannelServer;

/// Close codes (`16 §W18` leg 1).
pub const CLOSE_NORMAL: u16 = 1000;
pub const CLOSE_NOT_ALLOWED: u16 = 4403;
pub const CLOSE_NOT_FOUND: u16 = 4404;
pub const CLOSE_LIMIT: u16 = 4409;
pub const CLOSE_REFUSED: u16 = 4502;
pub const CLOSE_CHANNEL_LOST: u16 = 4503;
pub const CLOSE_TIMEOUT: u16 = 4504;

/// Ping interval of an open leg-1 WebSocket (`16 §W18`: ping/pong every 20 s).
const PING_INTERVAL: Duration = Duration::from_secs(20);

/// Idle timeout of an open leg-1 WebSocket (no bytes, no pong): the WS leg of `02 §Limits`.
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// Total lifetime of one stream (`PREVIEW_STREAM_LIFETIME`).
const LIFETIME: Duration = aurora_core::limits::PREVIEW_STREAM_LIFETIME;

/// Per-connection queues (bounded; the credit window bounds actual in-flight bytes).
const DATA_Q: usize = 16;

type Ws = WebSocketStream<TcpStream>;

/// Traffic from the supervisor side to a leg. Terminal frames (`pty.refused`/`pty.close`)
/// never arrive here: the listener removes the route and answers the leg's `close_rx`
/// directly.
enum LegIn {
    Opened { window: u32 },
    Credit(u32),
    Data(Vec<u8>),
}

/// One live leg: how the pump reaches it.
struct RouteEnd {
    /// Frames and data from the supervisor side, tagged with the leg's own stream id.
    to_leg: mpsc::Sender<LegIn>,
    /// Close the leg-1 WebSocket with this code when the supervisor side ends it.
    close_tx: oneshot::Sender<u16>,
    sandbox: Uuid,
    boot: Uuid,
    /// (P4-TERM, C-620) Set on a terminal leg: bytes in either direction stamp this
    /// sandbox's `terminal_active_until` (keep-awake reason `terminal`).
    terminal: bool,
}

/// The listener: owns the routes, accepts connections, enforces the binding.
pub struct StreamListener {
    server: ChannelServer,
    routes: Arc<Mutex<Routes>>,
    /// (P4-TERM, C-620) Last keep-awake stamp per sandbox (rate limit).
    last_stamp: Arc<Mutex<HashMap<Uuid, std::time::Instant>>>,
}

#[derive(Default)]
struct Routes {
    /// `stream_id -> leg ends`. Ids are end-to-end and allocated by this runner.
    legs: HashMap<u32, RouteEnd>,
    next_id: u32,
}

impl StreamListener {
    /// Builds the listener over the channel server.
    pub fn new(server: ChannelServer) -> Arc<Self> {
        Arc::new(Self {
            server,
            routes: Arc::new(Mutex::new(Routes::default())),
            last_stamp: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Routes a supervisor frame to the leg that opened the stream (the channel pump calls
    /// this through `ChannelServer::stream_frame`; `pty.*` rides the same path, W19).
    pub(crate) async fn on_supervisor_frame(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        frame: SupervisorToRunner,
    ) {
        use SupervisorToRunner as M;
        let mut routes = self.routes.lock().await;
        match frame {
            M::StreamOpened(o) => {
                if let Some(end) = routes.legs.get(&o.stream_id)
                    && end.sandbox == sandbox
                    && end.boot == boot
                {
                    let _ = end.to_leg.send(LegIn::Opened { window: o.window }).await;
                }
            }
            M::StreamRefused(r) => {
                if let Some(end) = routes.legs.remove(&r.stream_id) {
                    let code = match r.reason {
                        StreamRefuseReason::NotShared => CLOSE_NOT_FOUND,
                        StreamRefuseReason::Refused => CLOSE_REFUSED,
                        StreamRefuseReason::Timeout => CLOSE_TIMEOUT,
                        StreamRefuseReason::Limit => CLOSE_LIMIT,
                        StreamRefuseReason::Other(_) => CLOSE_REFUSED,
                    };
                    let _ = end.close_tx.send(code);
                }
            }
            M::StreamCredit(c) => {
                if let Some(end) = routes.legs.get(&c.stream_id) {
                    let _ = end.to_leg.send(LegIn::Credit(c.bytes)).await;
                }
            }
            M::StreamClose(c) => {
                if let Some(end) = routes.legs.remove(&c.stream_id) {
                    let code = match c.reason {
                        StreamCloseReason::Eof => CLOSE_NORMAL,
                        StreamCloseReason::Reset | StreamCloseReason::Error => CLOSE_REFUSED,
                        StreamCloseReason::Idle | StreamCloseReason::Lifetime => CLOSE_TIMEOUT,
                        StreamCloseReason::ChannelLost => CLOSE_CHANNEL_LOST,
                        StreamCloseReason::Other(_) => CLOSE_REFUSED,
                    };
                    let _ = end.close_tx.send(code);
                }
            }
            // (P4-TERM, C-619, `16 §W19`) The PTY answers, same shape as the stream ones.
            M::PtyOpened(o) => {
                if let Some(end) = routes.legs.get(&o.pty_id)
                    && end.sandbox == sandbox
                    && end.boot == boot
                {
                    let _ = end.to_leg.send(LegIn::Opened { window: o.window }).await;
                }
            }
            M::PtyRefused(r) => {
                if let Some(end) = routes.legs.remove(&r.pty_id) {
                    let code = match r.reason {
                        PtyRefuseReason::Disabled | PtyRefuseReason::Error => CLOSE_REFUSED,
                        PtyRefuseReason::Limit => CLOSE_LIMIT,
                        PtyRefuseReason::Other(_) => CLOSE_REFUSED,
                    };
                    let _ = end.close_tx.send(code);
                }
            }
            M::PtyCredit(c) => {
                if let Some(end) = routes.legs.get(&c.pty_id) {
                    let _ = end.to_leg.send(LegIn::Credit(c.bytes)).await;
                }
            }
            M::PtyClose(c) => {
                if let Some(end) = routes.legs.remove(&c.pty_id) {
                    let code = match c.reason {
                        PtyCloseReason::Eof => CLOSE_NORMAL,
                        PtyCloseReason::Hangup | PtyCloseReason::Error => CLOSE_REFUSED,
                        PtyCloseReason::Idle | PtyCloseReason::Lifetime => CLOSE_TIMEOUT,
                        PtyCloseReason::ChannelLost => CLOSE_CHANNEL_LOST,
                        PtyCloseReason::Other(_) => CLOSE_REFUSED,
                    };
                    let _ = end.close_tx.send(code);
                }
            }
            // Not stream/pty traffic (the conn pump dispatches the rest); ignore.
            _ => {}
        }
    }

    /// Binary data from a supervisor `[0x01][stream_id BE][payload]`, routed by the channel
    /// connection pump (never copied through an extra queue hop).
    pub(crate) async fn supervisor_data(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        stream_id: u32,
        payload: Vec<u8>,
    ) {
        let routes = self.routes.lock().await;
        if let Some(end) = routes.legs.get(&stream_id)
            && end.sandbox == sandbox
            && end.boot == boot
        {
            let _ = end.to_leg.send(LegIn::Data(payload)).await;
        }
    }

    /// (P4-TERM, W19) Terminal data from a supervisor `[0x02][pty_id BE][payload]`: routed
    /// like the stream data; each chunk also stamps the keep-awake (`C-620`: bytes in either
    /// direction keep the sandbox awake, never a turn, never `last_engaged_at`).
    pub(crate) async fn supervisor_terminal(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        pty_id: u32,
        payload: Vec<u8>,
    ) {
        let terminal = {
            let routes = self.routes.lock().await;
            match routes.legs.get(&pty_id) {
                Some(end) if end.sandbox == sandbox && end.boot == boot => {
                    let _ = end.to_leg.send(LegIn::Data(payload)).await;
                    end.terminal
                }
                _ => false,
            }
        };
        if terminal {
            // Best effort and off the hot path: a keep-awake stamp per data chunk would
            // hammer the API; at most one per grace per sandbox (the grace outlives the gap).
            self.stamp_awake(sandbox).await;
        }
    }

    /// (P4-TERM, C-620) Best-effort keep-awake stamp, rate-limited to one per
    /// [`TERMINAL_AWAKE_GRACE`] per sandbox (the grace already outlives the gap). Never
    /// blocks the leg: the API call is detached, failures are ignored (a missed stamp only
    /// shortens the grace the previous one bought).
    async fn stamp_awake(&self, sandbox: Uuid) {
        if !self.stamp_due(sandbox).await {
            return;
        }
        if let Some(api) = self.server.api() {
            let api = api.clone();
            tokio::spawn(async move { api.terminal_keep_awake(sandbox).await });
        }
    }

    /// Rate-limit gate: `true` at most once per [`TERMINAL_AWAKE_GRACE`] per sandbox.
    async fn stamp_due(&self, sandbox: Uuid) -> bool {
        let mut last = self.last_stamp.lock().await;
        let now = std::time::Instant::now();
        match last.get(&sandbox) {
            Some(t) if now.duration_since(*t) < TERMINAL_AWAKE_GRACE => false,
            _ => {
                last.insert(sandbox, now);
                true
            }
        }
    }

    /// The W6 connection of `(sandbox, boot)` was admitted (`connected`) or dropped: the
    /// channel died, so every stream of that `(sandbox, boot)` ends with 4503
    /// (`16 §W18`: streams are never resumed, on reconnect either).
    pub(crate) async fn channel_epoch(&self, sandbox: Uuid, boot: Uuid, connected: bool) {
        if connected {
            return; // a fresh channel has no streams yet
        }
        // The channel went away: end every stream of that (sandbox, boot).
        let mut routes = self.routes.lock().await;
        let dead: Vec<u32> = routes
            .legs
            .iter()
            .filter(|(_, end)| end.sandbox == sandbox && end.boot == boot)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            if let Some(end) = routes.legs.remove(&id) {
                let _ = end.close_tx.send(CLOSE_CHANNEL_LOST);
            }
        }
    }

    async fn alloc_id(&self) -> u32 {
        let mut routes = self.routes.lock().await;
        routes.next_id = routes.next_id.wrapping_add(1).max(1);
        routes.next_id
    }

    async fn register_route(
        &self,
        id: u32,
        binding: &PreviewStreamOpenResponse,
        terminal: bool,
        to_leg: mpsc::Sender<LegIn>,
    ) -> oneshot::Receiver<u16> {
        let (close_tx, close_rx) = oneshot::channel();
        let mut routes = self.routes.lock().await;
        routes.legs.insert(
            id,
            RouteEnd {
                to_leg,
                close_tx,
                sandbox: binding.sandbox_id,
                boot: binding.boot_id,
                terminal,
            },
        );
        close_rx
    }

    /// Sends one unsequenced `stream.*`/`pty.*` frame to the supervisor of `(sandbox, boot)`.
    async fn send_to_supervisor(
        &self,
        r: &ChannelRef,
        frame: RunnerToSupervisor,
    ) -> Result<(), ()> {
        self.server
            .send_stream_frame(r, frame)
            .await
            .map_err(|_| ())
    }

    /// Sends one binary data chunk to the supervisor of `(sandbox, boot)` as the wire frame
    /// `[tag][id BE][payload]` (`16 §W18` tag `0x01`, `16 §W19` tag `0x02`) — the
    /// supervisor's channel decodes the tag and id and routes the payload to the task.
    async fn send_data(&self, r: &ChannelRef, tag: u8, id: u32, data: &[u8]) -> Result<(), ()> {
        let mut b = Vec::with_capacity(data.len() + 5);
        b.push(tag);
        b.extend_from_slice(&id.to_be_bytes());
        b.extend_from_slice(data);
        self.server.send_stream_data(r, b).await.map_err(|_| ())
    }

    /// The leg ended with `reason`: tell the supervisor and free the route.
    async fn release_stream_as(
        &self,
        id: u32,
        sandbox: Uuid,
        boot: Uuid,
        reason: StreamCloseReason,
        bytes_in: u64,
        bytes_out: u64,
    ) {
        let removed = {
            let mut routes = self.routes.lock().await;
            routes.legs.remove(&id).is_some()
        };
        if !removed {
            return; // the supervisor ended it first; its close is already sent
        }
        let f = RunnerToSupervisor::StreamClose(aurora_protocol::runner::StreamClose {
            stream_id: id,
            reason,
            bytes_in: Some(bytes_in),
            bytes_out: Some(bytes_out),
        });
        let _ = self
            .send_to_supervisor(
                &ChannelRef {
                    sandbox_id: sandbox,
                    boot_id: boot,
                },
                f,
            )
            .await;
    }

    /// Releases the route with `Error` (the supervisor already answered/refused, or the
    /// handshake failed after the open).
    async fn release_stream(&self, id: u32, sandbox: Uuid, boot: Uuid) {
        self.release_stream_as(id, sandbox, boot, StreamCloseReason::Error, 0, 0)
            .await;
    }

    /// (P4-TERM, C-619) The PTY leg ended with `reason`: tell the supervisor `pty.close` and
    /// free the route.
    #[allow(clippy::too_many_arguments)]
    async fn release_pty_as(
        &self,
        id: u32,
        sandbox: Uuid,
        boot: Uuid,
        reason: PtyCloseReason,
        bytes_in: u64,
        bytes_out: u64,
    ) {
        let removed = {
            let mut routes = self.routes.lock().await;
            routes.legs.remove(&id).is_some()
        };
        if !removed {
            return; // the supervisor ended it first; its close is already sent
        }
        let f = RunnerToSupervisor::PtyClose(PtyClose {
            pty_id: id,
            reason,
            bytes_in: Some(bytes_in),
            bytes_out: Some(bytes_out),
        });
        let _ = self
            .send_to_supervisor(
                &ChannelRef {
                    sandbox_id: sandbox,
                    boot_id: boot,
                },
                f,
            )
            .await;
    }

    /// Releases a terminal route with `Error`.
    async fn release_pty(&self, id: u32, sandbox: Uuid, boot: Uuid) {
        self.release_pty_as(id, sandbox, boot, PtyCloseReason::Error, 0, 0)
            .await;
    }
}

// ------------------------------------------------------------------ accept loop

/// Accept loop of the stream listener (:4108). TLS (optional, `runner.stream_tls`) terminates
/// before this: `tls` wraps accepted TCP streams. Waits for the runner to report
/// `previews.enabled` at registration before accepting anything; runs until `stop` turns true.
pub async fn serve(
    listener: TcpListener,
    state: Arc<StreamListener>,
    api: watch::Receiver<Option<Arc<ApiClient>>>,
    tls: Option<tokio_rustls::TlsAcceptor>,
    mut stop: watch::Receiver<bool>,
) {
    // The port opens only when the API reported `previews.enabled` at registration
    // (`15 §Port previews`): before that, nothing is accepted on 4108.
    state.server.previews_wait(&mut stop).await;
    if *stop.borrow() {
        return;
    }
    tracing::info!("previews enabled: stream listener open on 4108");
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((tcp, _)) => {
                    let state = state.clone();
                    let api = api.clone();
                    let tls = tls.clone();
                    tokio::spawn(async move {
                        let tcp = match tls {
                            // The relay is the only client; the operator's chain serves it.
                            Some(a) => match a.accept(tcp).await {
                                Ok(s) => s.into_inner().0,
                                Err(_) => return,
                            },
                            None => tcp,
                        };
                        handle(state, api, tcp).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "stream listener accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            () = stopped(&mut stop) => return,
        }
    }
}

async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

/// One connection: parse the request, redeem the grant, `stream.open`, handshake, copy. The
/// WebSocket handshake happens **after** the supervisor accepted the stream so every failure
/// answers an HTTP status (never a premature 101, `16 §W18`).
async fn handle(
    state: Arc<StreamListener>,
    api: watch::Receiver<Option<Arc<ApiClient>>>,
    tcp: TcpStream,
) {
    let mut tcp = tcp;
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    let grant = loop {
        let n = match tokio::time::timeout(PREVIEW_CONNECT_TIMEOUT, tcp.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return,
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        if let Some(head) = find_header_end(&buf) {
            break bearer_of(head);
        }
        if buf.len() > 16 * 1024 {
            return;
        }
    };
    let Some((grant, request)) = grant else {
        let _ = tcp
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
            )
            .await;
        return;
    };
    // Redeem with the kind the grant's prefix declares (`aux_` → port, `auy_` → terminal,
    // P4-TERM C-617): the API answers the binding from its own tables, the runner never
    // decides.
    let kind = if grant.starts_with("auy_") {
        Some("terminal")
    } else {
        None
    };
    open_and_serve(state, api, tcp, grant, kind, request).await;
}

/// Returns `Some((token, head))` from `Authorization: Bearer <token>` (case-insensitive
/// scheme). `head` is the parsed request, kept for the WebSocket handshake: the caller has
/// consumed the raw bytes from the socket, so `accept_async` (which would read them itself)
/// can never complete — the server side of the handshake is written by hand below.
fn bearer_of(head: &[u8]) -> Option<(String, http::Request<()>)> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let mut builder = http::Request::builder().method(method).uri(target);
    let mut token = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("authorization") {
            let v = value.trim();
            token = v
                .strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
                .map(str::to_owned);
        }
        if let (Ok(n), Ok(v)) = (
            http::HeaderName::try_from(name.trim()),
            http::HeaderValue::try_from(value.trim()),
        ) {
            builder = builder.header(n, v);
        }
    }
    let token = token?;
    Some((token, builder.body(()).ok()?))
}

fn find_header_end(buf: &[u8]) -> Option<&[u8]> {
    let i = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some(&buf[..i])
}

/// Redeems the grant, opens the supervisor stream (port) or PTY (terminal), completes the
/// handshake, copies bytes.
async fn open_and_serve(
    state: Arc<StreamListener>,
    api: watch::Receiver<Option<Arc<ApiClient>>>,
    mut tcp: TcpStream,
    grant: String,
    kind: Option<&str>,
    request: http::Request<()>,
) {
    let Some(api) = api.borrow().clone() else {
        http_close(&mut tcp, 503).await;
        return;
    };
    // ---- 1. redeem the single-use grant (the only source of sandbox/boot/target, `16 §W17`)
    let req = PreviewStreamOpenRequest {
        grant: SecretString::new(grant),
        kind: kind.map(str::to_owned),
    };
    let binding = match api.preview_stream_open(&req).await {
        Ok(r) => r,
        Err(ApiError::Status { status: 404, .. }) => {
            // The grant names a sandbox whose channel this runner does not hold; the relay
            // re-resolves once. Close code 4404 maps to this answer.
            http_close(&mut tcp, CLOSE_NOT_FOUND).await;
            return;
        }
        Err(ApiError::Status {
            status: 403 | 409, ..
        }) => {
            // Grant invalid/expired/used (403) or not current: paused/frozen/off (409).
            http_close(&mut tcp, CLOSE_NOT_ALLOWED).await;
            return;
        }
        Err(_) => {
            http_close(&mut tcp, 503).await;
            return;
        }
    };
    let share_id = binding.share_id;
    // ---- 2. this runner must hold the live supervisor channel of that boot (`404 not_here`)
    let r = ChannelRef {
        sandbox_id: binding.sandbox_id,
        boot_id: binding.boot_id,
    };
    if !state.server.holds_channel(&r) {
        http_close(&mut tcp, CLOSE_NOT_FOUND).await;
        return;
    }
    // ---- 3. ask the supervisor to open the byte stream (5 s round trip), or for a terminal
    // grant the PTY (`P4-TERM`, C-617); then complete the handshake and copy.
    let terminal = binding.port.is_none();
    if let Some(port) = binding.port {
        let Some((stream_id, leg_rx, credit_tx, close_rx)) =
            open_supervisor_stream(&state, &binding, port, &r).await
        else {
            http_close(&mut tcp, CLOSE_REFUSED).await;
            return;
        };
        let Some(ws) =
            complete_handshake(tcp, &request, &state, stream_id, terminal, &binding).await
        else {
            return;
        };
        serve_leg(
            state,
            stream_id,
            binding.sandbox_id,
            binding.boot_id,
            share_id,
            ws,
            leg_rx,
            credit_tx,
            close_rx,
        )
        .await;
    } else {
        // (P4-TERM, C-617, `16 §W19`) terminal leg: `pty.open` beside `stream.open`, tag
        // `0x02` data, keep-awake stamping; the supervisor's PTY posture is `12 §Terminal`.
        let Some((pty_id, leg_rx, credit_tx, close_rx)) =
            open_supervisor_pty(&state, &binding, &r).await
        else {
            http_close(&mut tcp, CLOSE_REFUSED).await;
            return;
        };
        let Some(ws) =
            complete_handshake(tcp, &request, &state, pty_id, terminal, &binding).await
        else {
            return;
        };
        serve_terminal_leg(
            state,
            pty_id,
            binding.sandbox_id,
            binding.boot_id,
            ws,
            leg_rx,
            credit_tx,
            close_rx,
        )
        .await;
    }
}

/// Completes the WebSocket handshake (101) after the supervisor accepted the open. The relay
/// (or the API) is the WebSocket *client* and its upgrade request was already consumed by
/// the bearer read above — `accept_async` would wait for it a second time and hang the
/// leg until the relay's dial timeout (the 504 seen in the kind runs). The server half is
/// therefore written by hand: RFC 6455 101 answer from the pre-read request, then
/// `from_raw_socket` in server role (`16 §W18`: the 101 only after the stream is open).
/// Takes the socket by value (the raw-socket conversion owns it from here on).
async fn complete_handshake(
    mut tcp: TcpStream,
    request: &http::Request<()>,
    state: &Arc<StreamListener>,
    id: u32,
    terminal: bool,
    binding: &PreviewStreamOpenResponse,
) -> Option<Ws> {
    let ws_key = request
        .headers()
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let Some(ws_key) = ws_key else {
        // Not a WebSocket upgrade (no key): a plain HTTP error is all that remains.
        release_route(state, id, binding, terminal).await;
        http_close(&mut tcp, CLOSE_NOT_ALLOWED).await;
        return None;
    };
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(ws_key.as_bytes());
    let answer = format!(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {accept}\r\n\r\n"
    );
    if tcp.write_all(answer.as_bytes()).await.is_err() {
        release_route(state, id, binding, terminal).await;
        return None;
    }
    let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(PREVIEW_CHUNK))
        .max_frame_size(Some(PREVIEW_CHUNK));
    let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
        tcp,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        Some(cfg),
    )
    .await;
    Some(ws)
}

/// Releases a route after the handshake failed (the supervisor already accepted the open):
/// the right release for the leg's kind, so the supervisor learns the PTY/stream is gone.
async fn release_route(
    state: &Arc<StreamListener>,
    id: u32,
    binding: &PreviewStreamOpenResponse,
    terminal: bool,
) {
    if terminal {
        state
            .release_pty(id, binding.sandbox_id, binding.boot_id)
            .await;
    } else {
        state
            .release_stream(id, binding.sandbox_id, binding.boot_id)
            .await;
    }
}

/// Pre-handshake failure form: a plain HTTP status (the close-code table applies after 101).
async fn http_close(tcp: &mut TcpStream, code: u16) {
    let text = match code {
        CLOSE_NOT_ALLOWED => "403 Forbidden",
        CLOSE_NOT_FOUND => "404 Not Found",
        CLOSE_LIMIT => "429 Too Many Requests",
        _ => "503 Service Unavailable",
    };
    let _ = tcp
        .write_all(
            format!("HTTP/1.1 {text}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n").as_bytes(),
        )
        .await;
}

/// Allocates the stream id, registers the route and sends `stream.open` unsequenced on the W6
/// channel, then waits for `stream.opened` / `stream.refused` (5 s; an older supervisor answers
/// nothing → timeout, which maps to `4502`).
async fn open_supervisor_stream(
    state: &Arc<StreamListener>,
    binding: &PreviewStreamOpenResponse,
    port: u16,
    r: &ChannelRef,
) -> Option<(
    u32,
    mpsc::Receiver<LegIn>,
    mpsc::Sender<u32>,
    oneshot::Receiver<u16>,
)> {
    let id = state.alloc_id().await;
    let (leg_tx, leg_rx) = mpsc::channel(DATA_Q);
    let (credit_tx, credit_rx) = mpsc::channel::<u32>(8);
    // Register the route before sending `stream.open` so the answer cannot race us.
    let close_rx = state.register_route(id, binding, false, leg_tx).await;
    let open = StreamOpen {
        stream_id: id,
        port,
        window: PREVIEW_STREAM_WINDOW as u32,
    };
    if state
        .send_to_supervisor(r, RunnerToSupervisor::StreamOpen(open))
        .await
        .is_err()
    {
        state
            .release_stream(id, binding.sandbox_id, binding.boot_id)
            .await;
        return None;
    }
    // Wait for `stream.opened`; the leg consumes everything after. Terminal frames
    // (`stream.refused`) never reach the queue — the listener closes the route directly, which
    // drops this receiver's sender and ends the wait with `None`.
    let mut wait_rx = leg_rx;
    let opened = tokio::time::timeout(PREVIEW_SUPERVISOR_WAIT, async {
        loop {
            match wait_rx.recv().await {
                Some(LegIn::Opened { .. }) => return true,
                None => return false,
                // Data/credit before `opened` is out of order; drop it.
                Some(_) => continue,
            }
        }
    })
    .await;
    drop(credit_rx);
    match opened {
        Ok(true) => Some((id, wait_rx, credit_tx, close_rx)),
        _ => {
            state
                .release_stream(id, binding.sandbox_id, binding.boot_id)
                .await;
            None
        }
    }
}

use aurora_core::limits::PREVIEW_SUPERVISOR_CONNECT_TIMEOUT as PREVIEW_SUPERVISOR_WAIT;
use aurora_core::limits::TERMINAL_PTY_OPEN_TIMEOUT as PTY_OPEN_WAIT;

/// (P4-TERM, C-617, `16 §W19`) Allocates the pty id, registers the route (stamped as a
/// terminal leg for keep-awake) and sends `pty.open` unsequenced on the W6 channel, then
/// waits for `pty.opened` / `pty.refused` ([`PTY_OPEN_WAIT`]; an older supervisor answers
/// nothing → timeout, which maps to `4502`). The PTY seeds at 80×24; the dashboard resizes
/// as soon as the pane is laid out (`pty.resize`).
async fn open_supervisor_pty(
    state: &Arc<StreamListener>,
    binding: &PreviewStreamOpenResponse,
    r: &ChannelRef,
) -> Option<(
    u32,
    mpsc::Receiver<LegIn>,
    mpsc::Sender<u32>,
    oneshot::Receiver<u16>,
)> {
    let id = state.alloc_id().await;
    let (leg_tx, leg_rx) = mpsc::channel(DATA_Q);
    let (credit_tx, credit_rx) = mpsc::channel::<u32>(8);
    // Register the route before sending `pty.open` so the answer cannot race us; the leg is
    // stamped terminal so its bytes drive the keep-awake (`C-620`).
    let close_rx = state.register_route(id, binding, true, leg_tx).await;
    let open = PtyOpen {
        pty_id: id,
        window: TERMINAL_STREAM_WINDOW as u32,
        cols: 80,
        rows: 24,
    };
    if state
        .send_to_supervisor(r, RunnerToSupervisor::PtyOpen(open))
        .await
        .is_err()
    {
        state
            .release_pty(id, binding.sandbox_id, binding.boot_id)
            .await;
        return None;
    }
    // Wait for `pty.opened`; the leg consumes everything after. `pty.refused` never reaches
    // the queue — the listener closes the route directly, which drops this receiver's sender
    // and ends the wait with `None`.
    let mut wait_rx = leg_rx;
    let opened = tokio::time::timeout(PTY_OPEN_WAIT, async {
        loop {
            match wait_rx.recv().await {
                Some(LegIn::Opened { .. }) => return true,
                None => return false,
                // Data/credit before `opened` is out of order; drop it.
                Some(_) => continue,
            }
        }
    })
    .await;
    drop(credit_rx);
    match opened {
        Ok(true) => Some((id, wait_rx, credit_tx, close_rx)),
        _ => {
            state
                .release_pty(id, binding.sandbox_id, binding.boot_id)
                .await;
            None
        }
    }
}

/// After 101 on a terminal leg (`16 §W19`): binary messages are raw PTY bytes both directions
/// (tag `0x02` on the supervisor leg), text messages are `pty.resize` controls from the
/// dashboard, ping/pong every 20 s. Credit flow control mirrors [`serve_leg`] with the
/// terminal limits (`TERMINAL_STREAM_WINDOW` / `TERMINAL_CHUNK`). Every chunk in either
/// direction stamps the keep-awake (reason `terminal`, `C-620`) — never a turn, never an
/// engagement stamp. Exit releases the route with the mapped `pty.close` reason; byte totals
/// go nowhere but the supervisor's informational `pty.close` (never logged, never stored).
#[allow(clippy::too_many_arguments)]
async fn serve_terminal_leg(
    state: Arc<StreamListener>,
    pty_id: u32,
    sandbox: Uuid,
    boot: Uuid,
    ws: Ws,
    mut leg_rx: mpsc::Receiver<LegIn>,
    credit_tx: mpsc::Sender<u32>,
    mut close_rx: oneshot::Receiver<u16>,
) {
    let (mut ws_sink, mut ws_stream) = ws.split();
    let r = ChannelRef {
        sandbox_id: sandbox,
        boot_id: boot,
    };
    // Credit accounting, both directions (`16 §W19` on `16 §W18`): the same shape as the
    // port-stream leg with the terminal limits.
    let mut window = TERMINAL_STREAM_WINDOW;
    let mut to_pty_credit: usize = window;
    let mut from_pty_unacked: usize = 0;
    let mut bytes_to_pty: u64 = 0;
    let mut bytes_from_pty: u64 = 0;
    tokio::pin! {
        let idle = tokio::time::sleep(IDLE_TIMEOUT);
        let lifetime = tokio::time::sleep(LIFETIME);
    }
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let reason = loop {
        tokio::select! {
            // Browser → PTY direction: bytes from the WebSocket go to the supervisor (tag
            // `0x02`). The runner enforces its own send window; over-window or oversized
            // browser data is a violation (4502 after 101).
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if b.len() > TERMINAL_CHUNK {
                            break CLOSE_REFUSED;
                        }
                        if to_pty_credit < b.len() {
                            break CLOSE_REFUSED;
                        }
                        to_pty_credit -= b.len();
                        bytes_from_pty += b.len() as u64;
                        if state.send_data(&r, 0x02, pty_id, &b).await.is_err() {
                            break CLOSE_CHANNEL_LOST;
                        }
                        state.stamp_awake(sandbox).await;
                    }
                    // (P4-TERM, C-619) The one text control the dashboard sends: resize.
                    // `{"kind":"pty.resize","pty_id":…,"cols":…,"rows":…}`. Anything else is
                    // ignored (bytes are never parsed as text, never logged).
                    Some(Ok(Message::Text(t))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if let Ok(resize) = serde_json::from_str::<PtyResize>(&t)
                            && resize.pty_id == pty_id
                        {
                            let _ = state
                                .send_to_supervisor(
                                    &r,
                                    RunnerToSupervisor::PtyResize(PtyResize {
                                        pty_id,
                                        cols: resize.cols,
                                        rows: resize.rows,
                                    }),
                                )
                                .await;
                        }
                    }
                    Some(Ok(Message::Pong(_) | Message::Ping(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break CLOSE_NORMAL,
                }
            }
            // PTY → browser direction: supervisor data out of the leg queue.
            chunk = leg_rx.recv() => {
                match chunk {
                    Some(LegIn::Data(b)) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if to_pty_credit < b.len() {
                            // Over-window data from the supervisor is a protocol violation;
                            // close the leg (the release tells the supervisor `error`).
                            break CLOSE_REFUSED;
                        }
                        to_pty_credit -= b.len();
                        bytes_to_pty += b.len() as u64;
                        from_pty_unacked += b.len();
                        if ws_sink.send(Message::binary(b)).await.is_err() {
                            break CLOSE_NORMAL;
                        }
                        state.stamp_awake(sandbox).await;
                        // Return credit after writing onward, at least every `window / 4`.
                        if from_pty_unacked >= window / 4 {
                            let n = from_pty_unacked as u32;
                            from_pty_unacked = 0;
                            if credit_tx.send(n).await.is_err() {
                                break CLOSE_CHANNEL_LOST;
                            }
                        }
                    }
                    Some(LegIn::Credit(c)) => {
                        to_pty_credit = (to_pty_credit + c as usize).min(window);
                    }
                    Some(LegIn::Opened { window: w }) => {
                        // The supervisor announces its window; never exceeds the runner's own.
                        window = (w as usize).min(TERMINAL_STREAM_WINDOW);
                        to_pty_credit = to_pty_credit.min(window);
                    }
                    // All senders dropped: the route was removed. Either a supervisor frame
                    // (`pty.refused`/`pty.close`) already resolved `close_rx` — that code is
                    // the truth (1000 for `eof`) — or the route vanished another way (4502).
                    // A bare `select!` race must not mask it: the handlers queue the code
                    // before dropping the route's senders.
                    None => match close_rx.try_recv() {
                        Ok(code) => break code,
                        Err(_) => break CLOSE_REFUSED,
                    },
                }
            }
            code = &mut close_rx => {
                let code = code.unwrap_or(CLOSE_CHANNEL_LOST);
                // The supervisor ended the PTY. Any answer bytes it sent before the close are
                // already buffered in `leg_rx` (a bare `select!` race may resolve this arm
                // first): forward them — dropping them truncates the shell's last output. The
                // close follows the queued data.
                while let Ok(chunk) = leg_rx.try_recv() {
                    match chunk {
                        LegIn::Data(b) => {
                            idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                            if ws_sink.send(Message::binary(b)).await.is_err() {
                                break;
                            }
                        }
                        LegIn::Credit(c) => {
                            to_pty_credit = (to_pty_credit + c as usize).min(window);
                        }
                        LegIn::Opened { window: w } => {
                            window = (w as usize).min(TERMINAL_STREAM_WINDOW);
                            to_pty_credit = to_pty_credit.min(window);
                        }
                    }
                }
                break code;
            }
            _ = &mut idle => {
                break CLOSE_TIMEOUT;
            }
            _ = &mut lifetime => {
                break CLOSE_TIMEOUT;
            }
            _ = ping.tick() => {
                if ws_sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break CLOSE_NORMAL;
                }
            }
        }
    };
    // Every leg exit sends the WS close frame before the socket is dropped (the same closing
    // handshake as the port-stream leg).
    if ws_sink
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(reason),
            reason: "".into(),
        })))
        .await
        .is_ok()
    {
        let _ = ws_sink.flush().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(m)) = ws_stream.next().await {
                if matches!(m, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
    }
    // Flush the outstanding credit back so the supervisor's window does not leak.
    if from_pty_unacked > 0 {
        let _ = credit_tx.send(from_pty_unacked as u32).await;
    }
    // Tell the supervisor the PTY ended (or release the route if it ended it first). Byte
    // totals are informational (metrics only) and never logged (SEC-87).
    let close_reason = match reason {
        CLOSE_NORMAL => PtyCloseReason::Eof,
        CLOSE_CHANNEL_LOST => PtyCloseReason::ChannelLost,
        CLOSE_TIMEOUT => PtyCloseReason::Idle,
        // A violation or a peer hang-up: `error` (the supervisor tears its side down).
        _ => PtyCloseReason::Error,
    };
    state
        .release_pty_as(pty_id, sandbox, boot, close_reason, bytes_from_pty, bytes_to_pty)
        .await;
}

/// After 101: binary messages are raw bytes both directions (`16 §W18`), text ignored,
/// ping/pong every 20 s; the supervisor's announced credit gates reads of this socket's
/// relay→app direction (TCP backpressure holds the rest).
#[allow(clippy::too_many_arguments)]
async fn serve_leg(
    state: Arc<StreamListener>,
    stream_id: u32,
    sandbox: Uuid,
    boot: Uuid,
    share_id: Option<Uuid>,
    ws: Ws,
    mut leg_rx: mpsc::Receiver<LegIn>,
    credit_tx: mpsc::Sender<u32>,
    mut close_rx: oneshot::Receiver<u16>,
) {
    let (mut ws_sink, mut ws_stream) = ws.split();
    let r = ChannelRef {
        sandbox_id: sandbox,
        boot_id: boot,
    };
    // Credit accounting, both directions (`16 §W18`):
    // - `to_app`: the runner may send at most the supervisor's announced window of
    //   unacknowledged bytes; the supervisor returns credit after writing to the loopback app.
    // - `from_app`: the runner returns credit to the supervisor only after the bytes were
    //   written onward (to the leg-1 WebSocket), at least every `window / 4`.
    let mut window = PREVIEW_STREAM_WINDOW;
    let mut to_app_credit: usize = window;
    let mut from_app_unacked: usize = 0;
    let mut bytes_to_app: u64 = 0;
    let mut bytes_from_app: u64 = 0;
    tokio::pin! {
        let idle = tokio::time::sleep(IDLE_TIMEOUT);
        let lifetime = tokio::time::sleep(LIFETIME);
    }
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let reason = loop {
        tokio::select! {
            // Relay → app direction: bytes from the WebSocket go to the supervisor. The runner
            // enforces its own send window; over-window relay data is a violation (4403 after
            // 101; here the supervisor's window bounds us, an oversized chunk is 4502).
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        // Reset the idle deadline (tokio ≥1.50 takes the absolute instant the
                        // sleep should fire again, not a restart of its duration).
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if b.len() > PREVIEW_CHUNK {
                            break CLOSE_REFUSED;
                        }
                        if to_app_credit < b.len() {
                            break CLOSE_REFUSED;
                        }
                        to_app_credit -= b.len();
                        bytes_from_app += b.len() as u64;
                        if state.send_data(&r, 0x01, stream_id, &b).await.is_err() {
                            break CLOSE_CHANNEL_LOST;
                        }
                    }

                    // Text is ignored per the spec; pongs and frames keep the connection alive.
                    Some(Ok(Message::Text(_) | Message::Pong(_) | Message::Ping(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break CLOSE_NORMAL,
                }
            }
            // App → relay direction: supervisor data out of the leg queue.
            chunk = leg_rx.recv() => {
                match chunk {
                    Some(LegIn::Data(b)) => {
                        // Same absolute-deadline reset as the relay→app arm above.
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if to_app_credit < b.len() {
                            // Over-window data from the supervisor is a protocol violation;
                            // close the leg (the release tells the supervisor `error`).
                            break CLOSE_REFUSED;
                        }
                        to_app_credit -= b.len();
                        bytes_to_app += b.len() as u64;
                        from_app_unacked += b.len();
                        if ws_sink.send(Message::binary(b)).await.is_err() {
                            break CLOSE_NORMAL;
                        }
                        // Return credit after writing onward, at least every `window / 4`.
                        if from_app_unacked >= window / 4 {
                            let n = from_app_unacked as u32;
                            from_app_unacked = 0;
                            if credit_tx.send(n).await.is_err() {
                                break CLOSE_CHANNEL_LOST;
                            }
                        }
                    }
                    Some(LegIn::Credit(c)) => {
                        to_app_credit = (to_app_credit + c as usize).min(window);
                    }
                    Some(LegIn::Opened { window: w }) => {
                        // The supervisor announces its window; never exceeds the runner's own.
                        window = (w as usize).min(PREVIEW_STREAM_WINDOW);
                        to_app_credit = to_app_credit.min(window);
                    }
                    // All senders dropped: the route was removed. Either a supervisor frame
                    // (`refused`/`close`) already resolved `close_rx` — that code is the
                    // truth (1000 for `eof`) — or the route vanished another way (4502).
                    // A bare `select!` race must not mask it: the handlers queue the code
                    // before dropping the route's senders.
                    None => match close_rx.try_recv() {
                        Ok(code) => break code,
                        Err(_) => break CLOSE_REFUSED,
                    },
                }
            }
            code = &mut close_rx => {
                let code = code.unwrap_or(CLOSE_CHANNEL_LOST);
                // The supervisor ended the stream. Any answer bytes it sent before the
                // close are already buffered in `leg_rx` (the W6 delivers them in order,
                // but a bare `select!` race may resolve this arm first): forward them —
                // dropping them truncates the app's answer (a body-less 200 or a 502
                // "ended before the answer head"). The close follows the queued data.
                while let Ok(chunk) = leg_rx.try_recv() {
                    match chunk {
                        LegIn::Data(b) => {
                            idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                            if ws_sink.send(Message::binary(b)).await.is_err() {
                                break;
                            }
                        }
                        LegIn::Credit(c) => {
                            to_app_credit = (to_app_credit + c as usize).min(window);
                        }
                        LegIn::Opened { window: w } => {
                            window = (w as usize).min(PREVIEW_STREAM_WINDOW);
                            to_app_credit = to_app_credit.min(window);
                        }
                    }
                }
                break code;
            }
            _ = &mut idle => {
                break CLOSE_TIMEOUT;
            }
            _ = &mut lifetime => {
                break CLOSE_TIMEOUT;
            }
            _ = ping.tick() => {
                if ws_sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break CLOSE_NORMAL;
                }
            }
        }
    };
    // The relay maps close codes to HTTP answers (`16 §W18`): every leg exit sends the WS
    // close frame before the socket is dropped — a bare drop is a protocol error for the
    // peer (`ResetWithoutClosingHandshake`), not a close it can interpret. The closing
    // handshake then completes by reading until the peer's close arrives (bounded): a
    // socket dropped with the reply unread resets the connection mid-handshake.
    if ws_sink
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(reason),
            reason: "".into(),
        })))
        .await
        .is_ok()
    {
        let _ = ws_sink.flush().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(m)) = ws_stream.next().await {
                if matches!(m, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
    }
    // Flush the outstanding credit back so the supervisor's window does not leak.
    if from_app_unacked > 0 {
        let _ = credit_tx.send(from_app_unacked as u32).await;
    }
    // Tell the supervisor the leg ended (or release the route if it ended it first).
    let close_reason = match reason {
        CLOSE_NORMAL => StreamCloseReason::Eof,
        CLOSE_CHANNEL_LOST => StreamCloseReason::ChannelLost,
        CLOSE_TIMEOUT => StreamCloseReason::Idle,
        _ => StreamCloseReason::Reset,
    };
    let wire_reason = close_reason.as_str().to_owned();
    state
        .release_stream_as(
            stream_id,
            sandbox,
            boot,
            close_reason,
            bytes_from_app,
            bytes_to_app,
        )
        .await;
    // Advisory accounting at the API (`16 §W17`); never blocks, never logs bytes.
    if let Some(share) = share_id
        && let Some(api) = state.server.api()
    {
        let api_req = PreviewStreamClosedRequest {
            share_id: share,
            bytes_to_app,
            bytes_from_app,
            reason: wire_reason,
        };
        tokio::spawn(async move { api.preview_stream_closed(&api_req).await });
    }
}

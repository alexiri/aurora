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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aurora_core::limits::{
    PREVIEW_CHUNK, PREVIEW_STREAM_WINDOW, SSH_AWAKE_GRACE, SSH_CHANNELS_PER_SANDBOX,
    TERMINAL_AWAKE_GRACE, TERMINAL_CHUNK, TERMINAL_STREAM_WINDOW,
};
use aurora_protocol::common::SecretString;
use aurora_protocol::runner::{
    ChannelRef, ExecRefuseReason, PreviewStreamClosedRequest, PreviewStreamOpenRequest,
    PreviewStreamOpenResponse, PtyClose, PtyCloseReason, PtyCredit, PtyOpen, PtyRefuseReason,
    PtyResize, StreamCloseReason, StreamCredit, StreamOpen, StreamRefuseReason,
};
use aurora_protocol::supervisor::{RunnerToSupervisor, SupervisorToRunner};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot, watch};
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

/// Per-leg queue from the channel pump to the leg task. The supervisor's credit window bounds
/// the in-flight *bytes* (128 KiB), not the number of chunks, so the depth must cover a
/// conforming supervisor that sends small chunks (FIX-060: 128 KiB in 512-byte pieces = 256).
/// The pump never waits on this queue (see [`StreamListener::deliver`]): a full queue means the
/// leg is not draining, and that leg is closed.
const DATA_Q: usize = 256;

/// One write toward the browser may not stall longer than this: a client that stops reading
/// would otherwise hold its leg task (and socket) forever, unable to observe a close.
const WS_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

type Ws = WebSocketStream<TcpStream>;
type WsSink = futures::stream::SplitSink<Ws, Message>;

/// One bounded write to the browser; `Err` on a failed or timed-out (stalled client) write.
async fn ws_send(sink: &mut WsSink, msg: Message) -> Result<(), ()> {
    match tokio::time::timeout(WS_WRITE_TIMEOUT, sink.send(msg)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

/// Traffic from the supervisor side to a leg. Terminal frames (`pty.refused`/`pty.close`)
/// never arrive here: the listener removes the route and answers the leg's `close_rx`
/// directly.
enum LegIn {
    Opened {
        window: u32,
    },
    Credit(u32),
    Data(Vec<u8>),
    /// (P5-FILES-1, `16 §W20`) The exec's stderr (tag `0x03`); files legs only.
    Stderr(Vec<u8>),
    /// (P5-FILES-1) `exec.exit`: the child ended; files legs only. The `pty.close` follows.
    Exit(aurora_protocol::runner::ExecExit),
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
    /// (P5-FILES-1, D-316) Set on an `auf_` files leg: an exec leg of `16 §W21` (never stamps
    /// keep-awake; the supervisor side is `exec.*` + `pty.credit`/`pty.close`).
    files: bool,
    /// (P4-SSH-2, D-239) Set on an `auz_` SSH leg: an exec leg like the files one, whose bytes
    /// stamp keep-awake as kind `ssh` (`16 §W20`).
    ssh: bool,
    /// Sandbox → browser bytes the supervisor has sent and the runner has not yet credited
    /// back (FIX-060: its own counter, separate from the browser → sandbox window). The pump
    /// raises it on arrival, the leg lowers it when it returns credit after writing onward.
    from_sandbox: Arc<AtomicUsize>,
    /// The window the runner announced for that direction (`stream.open`/`pty.open`).
    recv_window: usize,
    /// (P5-REFINE-4, C-1058) The keep-awake heartbeat of a terminal or SSH leg: aborted when
    /// this route is dropped, i.e. on every way the leg ends (release, supervisor close,
    /// channel loss, listener drop).
    _heartbeat: Option<AbortOnDrop>,
}

/// Aborts its task when dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// (P4-TERM, C-620; P4-SSH-2, C-796; P5-REFINE-4, C-1058) The keep-awake stamps of the
/// listener: shared by the byte path and the per-leg heartbeats, so the two never double-post.
#[derive(Clone)]
struct Stamper {
    server: ChannelServer,
    /// Last stamp per (sandbox, ssh): the terminal and SSH columns are separate (C-796).
    last: Arc<Mutex<HashMap<(Uuid, bool), tokio::time::Instant>>>,
    /// Stamps posted (or due while no API client is attached); read by tests.
    posted: Arc<std::sync::atomic::AtomicU64>,
    terminal_grace: Duration,
    ssh_grace: Duration,
}

impl Stamper {
    fn grace(&self, ssh: bool) -> Duration {
        if ssh {
            self.ssh_grace
        } else {
            self.terminal_grace
        }
    }

    /// The heartbeat's period: half the grace, so a quiet session's stamps overlap.
    fn heartbeat_every(&self, ssh: bool) -> Duration {
        self.grace(ssh) / 2
    }

    /// Best-effort stamp when the last one of `(sandbox, ssh)` is at least `min_gap` old. Never
    /// blocks the caller: the API call is detached, failures are ignored (a missed stamp only
    /// shortens the grace the previous one bought).
    async fn stamp(&self, sandbox: Uuid, ssh: bool, min_gap: Duration) {
        if !self.due(sandbox, ssh, min_gap).await {
            return;
        }
        self.posted.fetch_add(1, Ordering::Relaxed);
        if let Some(api) = self.server.api() {
            tokio::spawn(async move { api.keep_awake(sandbox, ssh).await });
        }
    }

    async fn due(&self, sandbox: Uuid, ssh: bool, min_gap: Duration) -> bool {
        let mut last = self.last.lock().await;
        let now = tokio::time::Instant::now();
        match last.get(&(sandbox, ssh)) {
            Some(t) if now.duration_since(*t) < min_gap => false,
            _ => {
                last.insert((sandbox, ssh), now);
                true
            }
        }
    }

    /// (C-1058) Stamps `(sandbox, ssh)` every [`Stamper::heartbeat_every`] for as long as the
    /// returned guard lives, whether or not bytes flow. The first beat is one period after the
    /// open (the session's first bytes stamp at once through the byte path). A beat is skipped
    /// when any stamp of that kind is younger than 9/10 of the period, so the byte path and the
    /// heartbeat share one budget and the gap between two stamps stays under the grace.
    fn heartbeat(&self, sandbox: Uuid, ssh: bool) -> AbortOnDrop {
        let me = self.clone();
        let every = self.heartbeat_every(ssh);
        let min_gap = every.mul_f32(0.9);
        // The clock starts at the open, not at the task's first poll.
        let first = tokio::time::Instant::now() + every;
        AbortOnDrop(tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(first, every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                me.stamp(sandbox, ssh, min_gap).await;
            }
        }))
    }
}

/// What the pump needs to deliver to one leg (cloned out of `routes` under the lock).
struct Target {
    to_leg: mpsc::Sender<LegIn>,
    terminal: bool,
    files: bool,
    ssh: bool,
    from_sandbox: Arc<AtomicUsize>,
    recv_window: usize,
}

/// Which keep-awake column a delivered message's leg stamps (if any).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    No,
    Terminal,
    Ssh,
}

/// The listener: owns the routes, accepts connections, enforces the binding.
pub struct StreamListener {
    server: ChannelServer,
    routes: Arc<Mutex<Routes>>,
    /// (P4-TERM, C-620; C-1058) The keep-awake stamps (byte path and heartbeats).
    stamper: Stamper,
    /// Idle and lifetime limits of a leg (production: [`IDLE_TIMEOUT`], [`LIFETIME`]; tests
    /// build the listener with shortened ones through [`StreamListener::with_limits`]).
    idle: Duration,
    lifetime: Duration,
    bounds: Bounds,
    /// Unauthenticated connections in flight (`Bounds::pre_auth_cap`).
    pre_auth: Arc<Semaphore>,
}

/// Abuse bounds of the listener (FIX-092; `16 §W18`/`§W19`, `02 §Limits`). Production uses
/// [`Bounds::default`]; tests shorten them through [`StreamListener::with_bounds`].
#[derive(Debug, Clone)]
pub struct Bounds {
    /// Connections that have not finished redeeming their grant; beyond it the socket is closed.
    pub pre_auth_cap: usize,
    /// Total time to deliver the request head (not per read: a slow-loris sends a byte a
    /// time).
    pub head_deadline: Duration,
    /// Live port streams per sandbox on this runner (the supervisor's own cap).
    pub max_streams: usize,
    /// Live PTYs per sandbox on this runner.
    pub max_ptys: usize,
    /// (P5-FILES-1) Live `auf_` files legs per sandbox (`FILE_SESSIONS_PER_SANDBOX`, 4409).
    pub max_files: usize,
    /// (P4-SSH-2) Live `auz_` SSH legs per sandbox (`SSH_CHANNELS_PER_SANDBOX`, 4409).
    pub max_ssh: usize,
    /// `pty.resize` from the browser is coalesced to the latest size and sent at most this
    /// often.
    pub resize_flush: Duration,
    /// The keep-awake grace of a terminal / SSH leg ([`TERMINAL_AWAKE_GRACE`],
    /// [`SSH_AWAKE_GRACE`]): bytes stamp at most once per grace, an open leg's heartbeat every
    /// half grace (C-1058). Tests shorten them.
    pub terminal_awake_grace: Duration,
    pub ssh_awake_grace: Duration,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            pre_auth_cap: 256,
            head_deadline: Duration::from_secs(10),
            max_streams: 64,
            max_ptys: 2,
            max_files: aurora_core::limits::FILE_SESSIONS_PER_SANDBOX as usize,
            max_ssh: SSH_CHANNELS_PER_SANDBOX as usize,
            resize_flush: Duration::from_millis(50),
            terminal_awake_grace: TERMINAL_AWAKE_GRACE,
            ssh_awake_grace: SSH_AWAKE_GRACE,
        }
    }
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
        Self::with_limits(server, IDLE_TIMEOUT, LIFETIME)
    }

    /// The listener with explicit leg idle and lifetime limits. Production always uses
    /// [`StreamListener::new`]; the tests of the idle and lifetime closes (`docs/18`) pass
    /// shortened values, so they exercise the very code path that runs in production.
    pub fn with_limits(server: ChannelServer, idle: Duration, lifetime: Duration) -> Arc<Self> {
        Self::with_bounds(server, idle, lifetime, Bounds::default())
    }

    /// [`StreamListener::with_limits`] plus explicit abuse bounds.
    pub fn with_bounds(
        server: ChannelServer,
        idle: Duration,
        lifetime: Duration,
        bounds: Bounds,
    ) -> Arc<Self> {
        Arc::new(Self {
            stamper: Stamper {
                server: server.clone(),
                last: Arc::new(Mutex::new(HashMap::new())),
                posted: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                terminal_grace: bounds.terminal_awake_grace,
                ssh_grace: bounds.ssh_awake_grace,
            },
            server,
            routes: Arc::new(Mutex::new(Routes::default())),
            idle,
            lifetime,
            pre_auth: Arc::new(Semaphore::new(bounds.pre_auth_cap)),
            bounds,
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
        // Every kind that looks up or removes a leg goes through `deliver`/`take_owned`, which
        // bind the leg to the sending `(sandbox, boot)` (FIX-060: ids are sequential and
        // guessable; a supervisor may only touch its own legs) and never hold `routes`
        // across an await on a leg queue.
        match frame {
            M::StreamOpened(o) => {
                self.deliver(
                    o.stream_id,
                    sandbox,
                    boot,
                    LegIn::Opened { window: o.window },
                )
                .await;
            }
            M::StreamRefused(r) => {
                if let Some(end) = self.take_owned(r.stream_id, sandbox, boot).await {
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
                self.deliver(c.stream_id, sandbox, boot, LegIn::Credit(c.bytes))
                    .await;
            }
            M::StreamClose(c) => {
                if let Some(end) = self.take_owned(c.stream_id, sandbox, boot).await {
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
                self.deliver(o.pty_id, sandbox, boot, LegIn::Opened { window: o.window })
                    .await;
            }
            M::PtyRefused(r) => {
                if let Some(end) = self.take_owned(r.pty_id, sandbox, boot).await {
                    let code = match r.reason {
                        PtyRefuseReason::Disabled | PtyRefuseReason::Error => CLOSE_REFUSED,
                        PtyRefuseReason::Limit => CLOSE_LIMIT,
                        PtyRefuseReason::Other(_) => CLOSE_REFUSED,
                    };
                    let _ = end.close_tx.send(code);
                }
            }
            M::PtyCredit(c) => {
                self.deliver(c.pty_id, sandbox, boot, LegIn::Credit(c.bytes))
                    .await;
            }
            M::PtyClose(c) => {
                if let Some(end) = self.take_owned(c.pty_id, sandbox, boot).await {
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
            // (P5-FILES-1, `16 §W20` leg 2) The exec answers, same shape as the PTY ones; they
            // reach only `auf_` legs (an exec id is allocated by a files leg's route).
            M::ExecOpened(o) => {
                self.deliver(o.pty_id, sandbox, boot, LegIn::Opened { window: o.window })
                    .await;
            }
            M::ExecRefused(r) => {
                if let Some(end) = self.take_owned(r.pty_id, sandbox, boot).await {
                    let code = match r.reason {
                        ExecRefuseReason::Limit => CLOSE_LIMIT,
                        _ => CLOSE_REFUSED,
                    };
                    let _ = end.close_tx.send(code);
                }
            }
            M::ExecExit(x) => {
                let id = x.pty_id;
                self.deliver(id, sandbox, boot, LegIn::Exit(x)).await;
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
        self.deliver(stream_id, sandbox, boot, LegIn::Data(payload))
            .await;
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
        match self
            .deliver(pty_id, sandbox, boot, LegIn::Data(payload))
            .await
            .unwrap_or(Keep::No)
        {
            // Best effort and off the hot path: a keep-awake stamp per data chunk would
            // hammer the API; at most one per grace per sandbox (the grace outlives the gap).
            Keep::Terminal => self.stamp_awake(sandbox, false).await,
            Keep::Ssh => self.stamp_awake(sandbox, true).await,
            Keep::No => {}
        }
    }

    /// (P5-FILES-1, `16 §W20`) An exec's stderr from a supervisor `[0x03][pty_id BE][payload]`:
    /// routed like the other data (same window). Never a keep-awake stamp for a files leg; an
    /// SSH leg's stderr is bytes like any other (`16 §W20`).
    pub(crate) async fn supervisor_stderr(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        pty_id: u32,
        payload: Vec<u8>,
    ) {
        if self
            .deliver(pty_id, sandbox, boot, LegIn::Stderr(payload))
            .await
            == Some(Keep::Ssh)
        {
            self.stamp_awake(sandbox, true).await;
        }
    }

    /// (P4-TERM, C-620; P4-SSH-2, C-796) Best-effort keep-awake stamp of the byte path,
    /// rate-limited to one per grace per sandbox and kind (the grace already outlives the gap;
    /// an open leg's heartbeat, C-1058, fills the quiet stretches).
    async fn stamp_awake(&self, sandbox: Uuid, ssh: bool) {
        self.stamper
            .stamp(sandbox, ssh, self.stamper.grace(ssh))
            .await;
    }

    /// Keep-awake stamps this listener has posted (byte path and heartbeats). For tests and
    /// diagnostics.
    pub fn keep_awake_stamps(&self) -> u64 {
        self.stamper.posted.load(Ordering::Relaxed)
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

    /// The leg `id` **if** it belongs to `(sandbox, boot)`: what is needed to deliver to it,
    /// cloned under the lock so the caller sends with the lock released.
    async fn owned_target(&self, id: u32, sandbox: Uuid, boot: Uuid) -> Option<Target> {
        let routes = self.routes.lock().await;
        routes
            .legs
            .get(&id)
            .filter(|end| end.sandbox == sandbox && end.boot == boot)
            .map(|end| Target {
                to_leg: end.to_leg.clone(),
                terminal: end.terminal,
                files: end.files,
                ssh: end.ssh,
                from_sandbox: end.from_sandbox.clone(),
                recv_window: end.recv_window,
            })
    }

    /// Removes and returns leg `id` only when it belongs to `(sandbox, boot)`; another
    /// sandbox's id is left untouched (and unanswered).
    async fn take_owned(&self, id: u32, sandbox: Uuid, boot: Uuid) -> Option<RouteEnd> {
        let mut routes = self.routes.lock().await;
        if routes
            .legs
            .get(&id)
            .is_some_and(|end| end.sandbox == sandbox && end.boot == boot)
        {
            routes.legs.remove(&id)
        } else {
            None
        }
    }

    /// Hands one message to the leg `id` of `(sandbox, boot)` without ever waiting
    /// (FIX-060). Returns which keep-awake column the leg stamps when the message was queued.
    ///
    /// **Design: `try_send`, and close the leg on `Full`** — not "clone the sender and
    /// `send().await` outside the lock". This function runs on the sandbox's channel pump,
    /// which is shared by every stream and terminal of that sandbox: awaiting a full queue
    /// there, lock or no lock, lets one browser tab that stops reading freeze the sandbox's
    /// whole stream plane (and, before this change, with the lock held, the runner's).
    /// A conforming supervisor cannot fill the queue: it is bounded by the credit window,
    /// which the leg only returns after writing onward ([`DATA_Q`] covers a full window of
    /// small chunks). A full queue therefore means the leg is not draining (a stalled
    /// client) or the supervisor ignores credit; closing exactly that leg is correct and
    /// leaves every other leg, and the pump, untouched. Waiting outside the lock instead
    /// would only move the stall from the runner to the sandbox and give a malicious or
    /// stalled peer an unbounded hold on the pump.
    ///
    /// Data is also checked against the window the runner announced for sandbox → browser
    /// (`from_sandbox`, counted on **arrival**, so a supervisor that ignores credit is caught
    /// even while the leg is busy or stalled): exceeding it closes the leg like a full queue.
    async fn deliver(&self, id: u32, sandbox: Uuid, boot: Uuid, msg: LegIn) -> Option<Keep> {
        let t = self.owned_target(id, sandbox, boot).await?;
        let keep = if t.terminal {
            Keep::Terminal
        } else if t.ssh {
            Keep::Ssh
        } else {
            Keep::No
        };
        // An exec leg's close frames are the PTY's (`pty.close`); only terminals and SSH stamp.
        let pty_like = t.terminal || t.files || t.ssh;
        if let LegIn::Data(b) | LegIn::Stderr(b) = &msg {
            let after = t.from_sandbox.fetch_add(b.len(), Ordering::AcqRel) + b.len();
            if after > t.recv_window {
                tracing::warn!(
                    id,
                    "stream data beyond the announced window; closing the leg"
                );
                self.overflow(id, sandbox, boot, pty_like).await;
                return None;
            }
        }
        match t.to_leg.try_send(msg) {
            Ok(()) => Some(keep),
            // The leg task is ending; its own release path frees the route.
            Err(mpsc::error::TrySendError::Closed(_)) => None,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(id, "stream leg queue full; closing the leg");
                self.overflow(id, sandbox, boot, pty_like).await;
                None
            }
        }
    }

    /// Closes one leg whose queue overflowed: route removed, the browser side told
    /// `CLOSE_REFUSED` (a flow-control violation), the supervisor told `error`. Non-blocking:
    /// both notifications are a oneshot send and a synchronous push to the channel queue.
    async fn overflow(&self, id: u32, sandbox: Uuid, boot: Uuid, terminal: bool) {
        let Some(end) = self.take_owned(id, sandbox, boot).await else {
            return;
        };
        let _ = end.close_tx.send(CLOSE_REFUSED);
        let frame = if terminal {
            RunnerToSupervisor::PtyClose(PtyClose {
                pty_id: id,
                reason: PtyCloseReason::Error,
                bytes_in: None,
                bytes_out: None,
            })
        } else {
            RunnerToSupervisor::StreamClose(aurora_protocol::runner::StreamClose {
                stream_id: id,
                reason: StreamCloseReason::Error,
                bytes_in: None,
                bytes_out: None,
            })
        };
        let _ = self
            .send_to_supervisor(
                &ChannelRef {
                    sandbox_id: sandbox,
                    boot_id: boot,
                },
                frame,
            )
            .await;
    }

    async fn alloc_id(&self) -> u32 {
        let mut routes = self.routes.lock().await;
        routes.next_id = routes.next_id.wrapping_add(1).max(1);
        routes.next_id
    }

    #[allow(clippy::too_many_arguments)]
    async fn register_route(
        &self,
        id: u32,
        binding: &PreviewStreamOpenResponse,
        terminal: bool,
        files: bool,
        ssh: bool,
        to_leg: mpsc::Sender<LegIn>,
        from_sandbox: Arc<AtomicUsize>,
    ) -> Option<oneshot::Receiver<u16>> {
        let (close_tx, close_rx) = oneshot::channel();
        let mut routes = self.routes.lock().await;
        // Per-sandbox live-leg cap (FIX-092): the supervisor's own limits, enforced here too so
        // the runner's memory is bounded however the supervisor behaves. Each kind (port,
        // terminal, files, ssh) counts and is capped on its own.
        let live = routes
            .legs
            .values()
            .filter(|e| {
                e.sandbox == binding.sandbox_id
                    && e.terminal == terminal
                    && e.files == files
                    && e.ssh == ssh
            })
            .count();
        let cap = if ssh {
            self.bounds.max_ssh
        } else if files {
            self.bounds.max_files
        } else if terminal {
            self.bounds.max_ptys
        } else {
            self.bounds.max_streams
        };
        if live >= cap {
            return None;
        }
        // (P5-REFINE-4, C-1058, D-416) An open terminal or SSH session keeps the computer awake
        // whether or not bytes flow; the heartbeat lives exactly as long as this route.
        let heartbeat = (terminal || ssh).then(|| self.stamper.heartbeat(binding.sandbox_id, ssh));
        routes.legs.insert(
            id,
            RouteEnd {
                to_leg,
                close_tx,
                sandbox: binding.sandbox_id,
                boot: binding.boot_id,
                terminal,
                files,
                ssh,
                from_sandbox,
                recv_window: if terminal || files || ssh {
                    TERMINAL_STREAM_WINDOW
                } else {
                    PREVIEW_STREAM_WINDOW
                },
                _heartbeat: heartbeat,
            },
        );
        Some(close_rx)
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
                    // Unauthenticated connections are capped (FIX-092): beyond the cap the
                    // socket is closed at once, so idle sockets cannot exhaust descriptors.
                    let Ok(permit) = state.pre_auth.clone().try_acquire_owned() else {
                        tracing::warn!("stream listener pre-auth cap reached; refusing");
                        continue;
                    };
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
                        handle(state, api, tcp, permit).await;
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
    pre_auth: tokio::sync::OwnedSemaphorePermit,
) {
    let mut tcp = tcp;
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    // The whole head under one deadline (a byte every few seconds must not hold the socket).
    let deadline = state.bounds.head_deadline;
    let head = tokio::time::timeout(deadline, async {
        loop {
            let n = match tcp.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            if let Some(head) = find_header_end(&buf) {
                return Some(bearer_of(head));
            }
            if buf.len() > 16 * 1024 {
                return None;
            }
        }
    })
    .await;
    let Ok(Some(grant)) = head else {
        return;
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
    } else if grant.starts_with("auf_") {
        // (P5-FILES-1, D-316, `16 §W21`) a files grant: an `sftp` exec, never a PTY or a port.
        Some("files")
    } else if grant.starts_with("auz_") {
        // (P4-SSH-2, D-239, `16 §W20`) an SSH channel grant: an exec (`shell`/`exec`/`sftp`),
        // never a PTY-terminal or a port. A runner the API did not enable for SSH never even
        // redeems it (the grant is left to expire).
        if !state.server.ssh_enabled() {
            http_close(&mut tcp, CLOSE_NOT_ALLOWED).await;
            return;
        }
        Some("ssh")
    } else {
        None
    };
    open_and_serve(state, api, tcp, grant, kind, request, pre_auth).await;
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
    pre_auth: tokio::sync::OwnedSemaphorePermit,
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
    // The grant was redeemed: the connection is authenticated and leaves the pre-auth cap.
    drop(pre_auth);
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
    if kind == Some("files") || kind == Some("ssh") {
        // (P5-FILES-1, `16 §W21`; P4-SSH-2, `16 §W20`) The exec is opened *after* the 101: the
        // first message names it. A files or SSH grant that comes back with a port is a
        // relay/API fault: refuse.
        if binding.port.is_some() {
            http_close(&mut tcp, CLOSE_NOT_ALLOWED).await;
            return;
        }
        let Some(ws) = complete_handshake(tcp, &request, &state, None, false, &binding).await
        else {
            return;
        };
        serve_exec_leg(state, binding, ws, kind == Some("ssh")).await;
        return;
    }
    if let Some(port) = binding.port {
        let (stream_id, leg_rx, close_rx, from_sandbox) =
            match open_supervisor_stream(&state, &binding, port, &r).await {
                Ok(l) => l,
                Err(code) => {
                    http_close(&mut tcp, code).await;
                    return;
                }
            };
        let Some(ws) =
            complete_handshake(tcp, &request, &state, Some(stream_id), terminal, &binding).await
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
            close_rx,
            from_sandbox,
        )
        .await;
    } else {
        // (P4-TERM, C-617, `16 §W19`) terminal leg: `pty.open` beside `stream.open`, tag
        // `0x02` data, keep-awake stamping; the supervisor's PTY posture is `12 §Terminal`.
        let (pty_id, leg_rx, close_rx, from_sandbox) =
            match open_supervisor_pty(&state, &binding, &r).await {
                Ok(l) => l,
                Err(code) => {
                    http_close(&mut tcp, code).await;
                    return;
                }
            };
        let Some(ws) =
            complete_handshake(tcp, &request, &state, Some(pty_id), terminal, &binding).await
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
            close_rx,
            from_sandbox,
        )
        .await;
    }
}

/// A leg the supervisor accepted: id, its queue, its close signal, and the sandbox → browser
/// unreturned-bytes counter shared with the pump.
type OpenedLeg = (
    u32,
    mpsc::Receiver<LegIn>,
    oneshot::Receiver<u16>,
    Arc<AtomicUsize>,
);

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
    id: Option<u32>,
    terminal: bool,
    binding: &PreviewStreamOpenResponse,
) -> Option<Ws> {
    // `id: None` is an `auf_` files leg (P5-FILES-1): it has no route yet (the exec is opened by
    // its first message) and its binary messages carry a tag byte (`[0x02][≤ 16 KiB]`).
    let release = |state: &Arc<StreamListener>| {
        let state = state.clone();
        let (sandbox, boot) = (binding.sandbox_id, binding.boot_id);
        async move {
            if let Some(id) = id {
                if terminal {
                    state.release_pty(id, sandbox, boot).await;
                } else {
                    state.release_stream(id, sandbox, boot).await;
                }
            }
        }
    };
    let max_message = PREVIEW_CHUNK + usize::from(id.is_none());
    let ws_key = request
        .headers()
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let Some(ws_key) = ws_key else {
        // Not a WebSocket upgrade (no key): a plain HTTP error is all that remains.
        release(state).await;
        http_close(&mut tcp, CLOSE_NOT_ALLOWED).await;
        return None;
    };
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(ws_key.as_bytes());
    let answer = format!(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {accept}\r\n\r\n"
    );
    if tcp.write_all(answer.as_bytes()).await.is_err() {
        release(state).await;
        return None;
    }
    let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(max_message))
        .max_frame_size(Some(max_message));
    let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
        tcp,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        Some(cfg),
    )
    .await;
    Some(ws)
}

/// Pre-handshake failure form: a plain HTTP status (the close-code table applies after 101).
async fn http_close(tcp: &mut TcpStream, code: u16) {
    let (text, body) = match code {
        CLOSE_NOT_ALLOWED => ("403 Forbidden", ""),
        // C-617: the standard error body `not_here` (the relay re-resolves on the status).
        CLOSE_NOT_FOUND => (
            "404 Not Found",
            r#"{"error":{"code":"not_here","message":"This runner does not hold that sandbox's channel."}}"#,
        ),
        CLOSE_LIMIT => ("429 Too Many Requests", ""),
        _ => ("503 Service Unavailable", ""),
    };
    let ctype = if body.is_empty() {
        ""
    } else {
        "content-type: application/json\r\n"
    };
    let _ = tcp
        .write_all(
            format!(
                "HTTP/1.1 {text}\r\nconnection: close\r\n{ctype}content-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
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
) -> Result<OpenedLeg, u16> {
    let id = state.alloc_id().await;
    let (leg_tx, leg_rx) = mpsc::channel(DATA_Q);
    let from_sandbox = Arc::new(AtomicUsize::new(0));
    // Register the route before sending `stream.open` so the answer cannot race us.
    let Some(close_rx) = state
        .register_route(
            id,
            binding,
            false,
            false,
            false,
            leg_tx,
            from_sandbox.clone(),
        )
        .await
    else {
        return Err(CLOSE_LIMIT);
    };
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
        return Err(CLOSE_REFUSED);
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
    match opened {
        Ok(true) => Ok((id, wait_rx, close_rx, from_sandbox)),
        _ => {
            state
                .release_stream(id, binding.sandbox_id, binding.boot_id)
                .await;
            Err(CLOSE_REFUSED)
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
) -> Result<OpenedLeg, u16> {
    let id = state.alloc_id().await;
    let (leg_tx, leg_rx) = mpsc::channel(DATA_Q);
    let from_sandbox = Arc::new(AtomicUsize::new(0));
    // Register the route before sending `pty.open` so the answer cannot race us; the leg is
    // stamped terminal so its bytes drive the keep-awake (`C-620`).
    let Some(close_rx) = state
        .register_route(
            id,
            binding,
            true,
            false,
            false,
            leg_tx,
            from_sandbox.clone(),
        )
        .await
    else {
        return Err(CLOSE_LIMIT);
    };
    let open = PtyOpen {
        pty_id: id,
        window: TERMINAL_STREAM_WINDOW as u32,
        cols: 80,
        rows: 24,
        // (FIX-138) The grant's choice (the API's redemption answer), never the browser's.
        shell: binding.shell.unwrap_or_default(),
        // (C-750 (4)) The models the API chose for the instance's terminal harnesses, unchanged.
        models: binding.models.clone().map(Box::new),
    };
    if state
        .send_to_supervisor(r, RunnerToSupervisor::PtyOpen(open))
        .await
        .is_err()
    {
        state
            .release_pty(id, binding.sandbox_id, binding.boot_id)
            .await;
        return Err(CLOSE_REFUSED);
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
    match opened {
        Ok(true) => Ok((id, wait_rx, close_rx, from_sandbox)),
        _ => {
            state
                .release_pty(id, binding.sandbox_id, binding.boot_id)
                .await;
            Err(CLOSE_REFUSED)
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
    mut close_rx: oneshot::Receiver<u16>,
    from_pty_outstanding: Arc<AtomicUsize>,
) {
    let (mut ws_sink, mut ws_stream) = ws.split();
    let r = ChannelRef {
        sandbox_id: sandbox,
        boot_id: boot,
    };
    // Credit accounting, **one window per direction** (`16 §W19` on `16 §W18`; FIX-060):
    // - browser → PTY: `to_pty_credit`, the supervisor's announced window; browser data
    //   decrements it, `pty.credit` from the supervisor (sent after it wrote to the PTY)
    //   replenishes it. Nothing else touches it.
    // - PTY → browser: the supervisor may have at most `TERMINAL_STREAM_WINDOW` (the window
    //   we announced in `pty.open`) unreturned. `from_pty_outstanding` is shared with the
    //   pump, which raises it on arrival and closes the leg when it exceeds the window
    //   (`StreamListener::deliver`); this leg lowers it when it returns credit after writing
    //   onward to the WebSocket (`from_pty_unacked`, at least every `window / 4`).
    let mut window = TERMINAL_STREAM_WINDOW;
    let mut to_pty_credit: usize = window;
    let mut from_pty_unacked: usize = 0;
    let mut bytes_to_pty: u64 = 0;
    let mut bytes_from_pty: u64 = 0;
    tokio::pin! {
        let idle = tokio::time::sleep(state.idle);
        let lifetime = tokio::time::sleep(state.lifetime);
    }
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pending_resize: Option<(u16, u16)> = None;
    let mut resize_flush = tokio::time::interval(state.bounds.resize_flush);
    resize_flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let reason = loop {
        tokio::select! {
            // The latest coalesced size; a full supervisor queue ends the leg (the supervisor
            // is not draining).
            _ = resize_flush.tick(), if pending_resize.is_some() => {
                if let Some((cols, rows)) = pending_resize.take()
                    && state
                        .send_to_supervisor(
                            &r,
                            RunnerToSupervisor::PtyResize(PtyResize { pty_id, cols, rows }),
                        )
                        .await
                        .is_err()
                {
                    break CLOSE_CHANNEL_LOST;
                }
            }
            // Browser → PTY direction: bytes from the WebSocket go to the supervisor (tag
            // `0x02`). The runner enforces its own send window; over-window or oversized
            // browser data is a violation (4502 after 101).
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
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
                        state.stamp_awake(sandbox, false).await;
                    }
                    // (P4-TERM, C-619) The one text control the dashboard sends: resize.
                    // `{"kind":"pty.resize","pty_id":0,"cols":…,"rows":…}` — the browser
                    // cannot know the allocated `pty_id` (one PTY per leg-1 WebSocket), so
                    // `0` names "this leg's PTY" and the runner stamps its own. Anything
                    // else is ignored (bytes are never parsed as text, never logged).
                    Some(Ok(Message::Text(t))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
                        // Coalesced to the latest size (FIX-092): the flush arm below sends it
                        // at most once per `resize_flush`, so a flood of resizes cannot fill
                        // the supervisor's queue.
                        if let Ok(resize) = serde_json::from_str::<PtyResize>(&t)
                            && resize.pty_id == 0
                        {
                            pending_resize = Some((resize.cols, resize.rows));
                        }
                    }
                    Some(Ok(Message::Pong(_) | Message::Ping(_))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break CLOSE_NORMAL,
                }
            }
            // PTY → browser direction: supervisor data out of the leg queue.
            chunk = leg_rx.recv() => {
                match chunk {
                    Some(LegIn::Data(b)) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
                        bytes_to_pty += b.len() as u64;
                        from_pty_unacked += b.len();
                        if ws_send(&mut ws_sink, Message::binary(b)).await.is_err() {
                            break CLOSE_NORMAL;
                        }
                        state.stamp_awake(sandbox, false).await;
                        // Return credit after writing onward, at least every `window / 4`.
                        if from_pty_unacked >= TERMINAL_STREAM_WINDOW / 4 {
                            let n = from_pty_unacked as u32;
                            from_pty_unacked = 0;
                            from_pty_outstanding.fetch_sub(n as usize, Ordering::AcqRel);
                            let credit = RunnerToSupervisor::PtyCredit(PtyCredit {
                                pty_id,
                                bytes: n,
                            });
                            if state.send_to_supervisor(&r, credit).await.is_err() {
                                break CLOSE_CHANNEL_LOST;
                            }
                        }
                    }
                    // Exec-only traffic never belongs on a port or terminal leg.
                    Some(LegIn::Stderr(_) | LegIn::Exit(_)) => {}
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
                            idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
                            if ws_send(&mut ws_sink, Message::binary(b)).await.is_err() {
                                break;
                            }
                        }
                        LegIn::Stderr(_) | LegIn::Exit(_) => {}
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
                if ws_send(&mut ws_sink, Message::Ping(Vec::new().into())).await.is_err() {
                    break CLOSE_NORMAL;
                }
            }
        }
    };
    // Every leg exit sends the WS close frame before the socket is dropped (the same closing
    // handshake as the port-stream leg).
    if ws_send(
        &mut ws_sink,
        Message::Close(Some(CloseFrame {
            code: CloseCode::from(reason),
            reason: "".into(),
        })),
    )
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
        .release_pty_as(
            pty_id,
            sandbox,
            boot,
            close_reason,
            bytes_from_pty,
            bytes_to_pty,
        )
        .await;
}

/// The one message an `auf_` leg may open with (`16 §W21`): exactly `{"kind":"exec.open",
/// "mode":"sftp"}` (an empty `env` and null `command`/`tty` are tolerated; anything else is not).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesOpen {
    kind: String,
    mode: String,
    #[serde(default)]
    command: Option<serde_json::Value>,
    #[serde(default)]
    tty: Option<serde_json::Value>,
    #[serde(default)]
    env: serde_json::Map<String, serde_json::Value>,
}

/// Whether `text` is the bare `sftp` `exec.open` an `auf_` leg must start with.
fn is_bare_sftp_open(text: &str) -> bool {
    serde_json::from_str::<FilesOpen>(text).is_ok_and(|o| {
        o.kind == "exec.open"
            && o.mode == "sftp"
            && o.command.is_none()
            && o.tty.is_none()
            && o.env.is_empty()
    })
}

/// What an exec leg asked for in its first message.
struct ExecSpec {
    mode: aurora_protocol::runner::ExecMode,
    command: Option<String>,
    tty: Option<aurora_protocol::runner::ExecTty>,
    env: std::collections::BTreeMap<String, String>,
}

/// The first message of an `auz_` leg (`16 §W20`): `{"kind":"exec.open","mode":…,"command"?,
/// "tty"?,"env":{…}}`. Unknown fields are refused.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SshOpen {
    kind: String,
    mode: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    tty: Option<aurora_protocol::runner::ExecTty>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
}

/// Parses and validates an SSH leg's `exec.open` (frame validation: a bad frame closes the leg
/// 4502): the shape per mode (`exec` needs a command without NUL, `shell`/`sftp` take none,
/// `sftp` has no `tty`), `tty` bounds, and the `LANG`/`LC_*` environment allow-list.
fn parse_ssh_open(text: &str) -> Option<ExecSpec> {
    use aurora_core::limits::{SSH_ENV_VARS_MAX, SSH_EXEC_COMMAND_MAX};
    use aurora_protocol::runner::ExecMode;
    let o: SshOpen = serde_json::from_str(text).ok()?;
    if o.kind != "exec.open" {
        return None;
    }
    let mode = match o.mode.as_str() {
        "shell" => ExecMode::Shell,
        "exec" => ExecMode::Exec,
        "sftp" => ExecMode::Sftp,
        _ => return None,
    };
    let shape_ok = match mode {
        ExecMode::Exec => o
            .command
            .as_deref()
            .is_some_and(|c| c.len() <= SSH_EXEC_COMMAND_MAX && !c.contains('\0')),
        ExecMode::Sftp => o.command.is_none() && o.tty.is_none(),
        _ => o.command.is_none(),
    };
    let tty_ok = o.tty.as_ref().is_none_or(|t| {
        (1..=64).contains(&t.term.len())
            && t.term.bytes().all(|b| (0x20..=0x7e).contains(&b))
            && (1..=1000).contains(&t.cols)
            && (1..=1000).contains(&t.rows)
    });
    let env_ok = o.env.len() <= SSH_ENV_VARS_MAX
        && o.env
            .iter()
            .all(|(k, v)| (k == "LANG" || k.starts_with("LC_")) && k.len() <= 64 && v.len() <= 256);
    (shape_ok && tty_ok && env_ok).then_some(ExecSpec {
        mode,
        command: o.command,
        tty: o.tty,
        env: o.env,
    })
}

/// The WebSocket closing handshake of every leg exit: our Close frame, then up to 2 s for the
/// peer's.
async fn close_ws(sink: &mut WsSink, stream: &mut futures::stream::SplitStream<Ws>, code: u16) {
    if ws_send(
        sink,
        Message::Close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: "".into(),
        })),
    )
    .await
    .is_ok()
    {
        let _ = sink.flush().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(m)) = stream.next().await {
                if matches!(m, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
    }
}

/// (P5-FILES-1, D-316, `16 §W21`) An `auf_` files leg after the 101: the API's first message
/// (within 5 s) must be the bare `sftp` `exec.open`; the runner then allocates the `pty_id`,
/// sends leg 2's `exec.open` and answers `{"kind":"exec.opened"}`, or closes 4502 (refused,
/// unsupported, timeout) / 4409 (a cap). Then binary messages are `[0x02][≤ 16 KiB]` both ways
/// (the SFTP stream) and `[0x03][…]` runner → API (the server's stderr), text is `exec.eof`
/// (API → runner) and `exec.exit` (runner → API, then 1000). Any other frame closes the leg
/// 4502 without being forwarded. No keep-awake stamp, ever: the API stamps its own.
///
/// (P4-SSH-2, D-239, `16 §W20`) With `ssh` the same leg serves an `auz_` SSH channel: the first
/// message is the full `exec.open` (`shell`/`exec`/`sftp`, `tty`, the `LANG`/`LC_*` env), at most
/// `SSH_CHANNELS_PER_SANDBOX` legs per sandbox (4409), a `pty.resize` text frame is accepted
/// when the channel has a `tty`, bytes in either direction stamp the SSH keep-awake (at most
/// one per `SSH_AWAKE_GRACE`), and the idle and lifetime limits are the API's answer's.
async fn serve_exec_leg(
    state: Arc<StreamListener>,
    binding: PreviewStreamOpenResponse,
    ws: Ws,
    ssh: bool,
) {
    use aurora_protocol::runner::{ExecEof, ExecMode, ExecOpen};
    let (mut sink, mut stream) = ws.split();
    let (sandbox, boot) = (binding.sandbox_id, binding.boot_id);
    let r = ChannelRef {
        sandbox_id: sandbox,
        boot_id: boot,
    };
    // ---- 1. the first message, within 5 s
    let first = tokio::time::timeout(PTY_OPEN_WAIT, stream.next()).await;
    let spec = match first {
        Err(_) => Err(CLOSE_TIMEOUT),
        Ok(Some(Ok(Message::Text(t)))) if t.len() <= 12 * 1024 => {
            if ssh {
                parse_ssh_open(&t).ok_or(CLOSE_REFUSED)
            } else if is_bare_sftp_open(&t) {
                Ok(ExecSpec {
                    mode: ExecMode::Sftp,
                    command: None,
                    tty: None,
                    env: std::collections::BTreeMap::new(),
                })
            } else {
                Err(CLOSE_REFUSED)
            }
        }
        Ok(Some(Ok(Message::Close(_))) | Some(Err(_)) | None) => Err(CLOSE_NORMAL),
        Ok(_) => Err(CLOSE_REFUSED),
    };
    let spec = match spec {
        Ok(s) => s,
        Err(code) => {
            close_ws(&mut sink, &mut stream, code).await;
            return;
        }
    };
    let has_tty = spec.tty.is_some();
    // ---- 2. register the route (the cap) and ask the supervisor for the exec
    let id = state.alloc_id().await;
    let (leg_tx, mut leg_rx) = mpsc::channel(DATA_Q);
    let from_sandbox = Arc::new(AtomicUsize::new(0));
    let Some(mut close_rx) = state
        .register_route(id, &binding, false, !ssh, ssh, leg_tx, from_sandbox.clone())
        .await
    else {
        close_ws(&mut sink, &mut stream, CLOSE_LIMIT).await;
        return;
    };
    let open = ExecOpen {
        pty_id: id,
        window: TERMINAL_STREAM_WINDOW as u32,
        mode: spec.mode,
        command: spec.command,
        tty: spec.tty,
        env: spec.env,
    };
    if state
        .send_to_supervisor(&r, RunnerToSupervisor::ExecOpen(open))
        .await
        .is_err()
    {
        state.release_pty(id, sandbox, boot).await;
        close_ws(&mut sink, &mut stream, CLOSE_REFUSED).await;
        return;
    }
    // `exec.refused` never reaches the queue: the listener removes the route and answers
    // `close_rx` (4502 / 4409), which ends this wait with `None`.
    let opened = tokio::time::timeout(PTY_OPEN_WAIT, async {
        loop {
            match leg_rx.recv().await {
                Some(LegIn::Opened { .. }) => return true,
                None => return false,
                Some(_) => continue,
            }
        }
    })
    .await;
    match opened {
        Ok(true) => {}
        Ok(false) => {
            let code = close_rx.try_recv().unwrap_or(CLOSE_REFUSED);
            close_ws(&mut sink, &mut stream, code).await;
            return;
        }
        Err(_) => {
            // An older supervisor ignores `exec.*`: the open times out (4502).
            state.release_pty(id, sandbox, boot).await;
            close_ws(&mut sink, &mut stream, CLOSE_REFUSED).await;
            return;
        }
    }
    if ws_send(&mut sink, Message::text(r#"{"kind":"exec.opened"}"#))
        .await
        .is_err()
    {
        state.release_pty(id, sandbox, boot).await;
        return;
    }
    // ---- 3. the copy loop: one credit window per direction, exactly the terminal's
    let window = TERMINAL_STREAM_WINDOW;
    let mut to_sup_credit: usize = window;
    let mut from_sup_unacked: usize = 0;
    let mut bytes_in: u64 = 0;
    let mut bytes_out: u64 = 0;
    // An SSH leg ends with its connection's idle and lifetime limits (the API's answer),
    // never later than the listener's own.
    let (idle_limit, life_limit) = if ssh {
        (
            state
                .idle
                .min(Duration::from_secs(u64::from(binding.idle_secs))),
            state
                .lifetime
                .min(Duration::from_secs(u64::from(binding.max_lifetime_secs))),
        )
    } else {
        (state.idle, state.lifetime)
    };
    tokio::pin! {
        let idle = tokio::time::sleep(idle_limit);
        let lifetime = tokio::time::sleep(life_limit);
    }
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // `pty.resize` (SSH legs with a `tty`), coalesced to the latest size like the terminal's.
    let mut pending_resize: Option<(u16, u16)> = None;
    let mut resize_flush = tokio::time::interval(state.bounds.resize_flush);
    resize_flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Forwards one supervisor → API message; `false` when the socket failed.
    async fn forward(sink: &mut WsSink, msg: LegIn, unacked: &mut usize, out: &mut u64) -> bool {
        let (tag, b) = match msg {
            LegIn::Data(b) => (0x02u8, b),
            LegIn::Stderr(b) => (0x03u8, b),
            LegIn::Exit(x) => {
                let mut v = serde_json::json!({"kind": "exec.exit"});
                if let Some(s) = x.status {
                    v["status"] = s.into();
                }
                if let Some(s) = x.signal {
                    v["signal"] = s.into();
                }
                return ws_send(sink, Message::text(v.to_string())).await.is_ok();
            }
            _ => return true,
        };
        *unacked += b.len();
        *out += b.len() as u64;
        let mut framed = Vec::with_capacity(b.len() + 1);
        framed.push(tag);
        framed.extend_from_slice(&b);
        ws_send(sink, Message::binary(framed)).await.is_ok()
    }
    // (P4-SSH-2) An SSH channel is a bulk path (`scp`, `sftp` uploads): a chunk that does not fit
    // the supervisor's remaining credit is held, the leg stops reading the socket meanwhile (TCP
    // backpressure to the terminator, `16 §W18`) and the chunk is sent when credit returns. The
    // files leg keeps its strict rule: over-window data is a violation.
    let mut held: Option<Vec<u8>> = None;
    let reason = loop {
        tokio::select! {
            _ = resize_flush.tick(), if pending_resize.is_some() => {
                if let Some((cols, rows)) = pending_resize.take()
                    && state
                        .send_to_supervisor(
                            &r,
                            RunnerToSupervisor::PtyResize(PtyResize { pty_id: id, cols, rows }),
                        )
                        .await
                        .is_err()
                {
                    break CLOSE_CHANNEL_LOST;
                }
            }
            msg = stream.next(), if held.is_none() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    idle.as_mut().reset(tokio::time::Instant::now() + idle_limit);
                    // `[0x02][payload ≤ 16 KiB]` only; any other tag is a violation.
                    let Some((&0x02, payload)) = b.split_first() else {
                        break CLOSE_REFUSED;
                    };
                    if payload.len() > TERMINAL_CHUNK {
                        break CLOSE_REFUSED;
                    }
                    if to_sup_credit < payload.len() {
                        if ssh {
                            held = Some(payload.to_vec());
                            continue;
                        }
                        break CLOSE_REFUSED;
                    }
                    if payload.is_empty() {
                        continue;
                    }
                    to_sup_credit -= payload.len();
                    bytes_in += payload.len() as u64;
                    if state.send_data(&r, 0x02, id, payload).await.is_err() {
                        break CLOSE_CHANNEL_LOST;
                    }
                    if ssh {
                        state.stamp_awake(sandbox, true).await;
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    idle.as_mut().reset(tokio::time::Instant::now() + idle_limit);
                    let ctl = (t.len() <= 256)
                        .then(|| serde_json::from_str::<serde_json::Value>(&t).ok())
                        .flatten()
                        .and_then(|v| v.as_object().cloned());
                    // (P4-SSH-2) `{"kind":"pty.resize","cols","rows"}` on an SSH leg with a
                    // `tty`: 1…1000 each, coalesced to the latest. Anything else but `exec.eof`
                    // is a violation.
                    if ssh
                        && has_tty
                        && let Some(o) = &ctl
                        && o.len() == 3
                        && o.get("kind").and_then(|k| k.as_str()) == Some("pty.resize")
                    {
                        let dim = |k: &str| {
                            o.get(k)
                                .and_then(serde_json::Value::as_u64)
                                .filter(|n| (1..=1000).contains(n))
                                .map(|n| n as u16)
                        };
                        let (Some(cols), Some(rows)) = (dim("cols"), dim("rows")) else {
                            break CLOSE_REFUSED;
                        };
                        pending_resize = Some((cols, rows));
                        continue;
                    }
                    let eof = ctl.is_some_and(|o| {
                        o.len() == 1 && o.get("kind").and_then(|k| k.as_str()) == Some("exec.eof")
                    });
                    if !eof {
                        break CLOSE_REFUSED;
                    }
                    if state
                        .send_to_supervisor(&r, RunnerToSupervisor::ExecEof(ExecEof { pty_id: id }))
                        .await
                        .is_err()
                    {
                        break CLOSE_CHANNEL_LOST;
                    }
                }
                Some(Ok(Message::Pong(_) | Message::Ping(_))) => {
                    idle.as_mut().reset(tokio::time::Instant::now() + idle_limit);
                }
                Some(Ok(Message::Frame(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break CLOSE_NORMAL,
            },
            chunk = leg_rx.recv() => match chunk {
                Some(m @ (LegIn::Data(_) | LegIn::Stderr(_) | LegIn::Exit(_))) => {
                    idle.as_mut().reset(tokio::time::Instant::now() + idle_limit);
                    if !forward(&mut sink, m, &mut from_sup_unacked, &mut bytes_out).await {
                        break CLOSE_NORMAL;
                    }
                    // Return credit after writing onward, at least every `window / 4`.
                    if from_sup_unacked >= window / 4 {
                        let n = from_sup_unacked as u32;
                        from_sup_unacked = 0;
                        from_sandbox.fetch_sub(n as usize, Ordering::AcqRel);
                        let credit = RunnerToSupervisor::PtyCredit(PtyCredit { pty_id: id, bytes: n });
                        if state.send_to_supervisor(&r, credit).await.is_err() {
                            break CLOSE_CHANNEL_LOST;
                        }
                    }
                }
                Some(LegIn::Credit(c)) => {
                    to_sup_credit = (to_sup_credit + c as usize).min(window);
                    if let Some(h) = held.take_if(|h| h.len() <= to_sup_credit) {
                        to_sup_credit -= h.len();
                        bytes_in += h.len() as u64;
                        if state.send_data(&r, 0x02, id, &h).await.is_err() {
                            break CLOSE_CHANNEL_LOST;
                        }
                        if ssh {
                            state.stamp_awake(sandbox, true).await;
                        }
                    }
                }
                Some(LegIn::Opened { .. }) => {}
                // The route was removed: the supervisor's `pty.close` already queued the code.
                None => match close_rx.try_recv() {
                    Ok(code) => break code,
                    Err(_) => break CLOSE_REFUSED,
                },
            },
            code = &mut close_rx => {
                let code = code.unwrap_or(CLOSE_CHANNEL_LOST);
                // Output the supervisor sent before ending the exec is already queued; the
                // close follows it (a status read must not be truncated).
                while let Ok(m) = leg_rx.try_recv() {
                    if !forward(&mut sink, m, &mut from_sup_unacked, &mut bytes_out).await {
                        break;
                    }
                }
                break code;
            }
            _ = &mut idle => break CLOSE_TIMEOUT,
            _ = &mut lifetime => break CLOSE_TIMEOUT,
            _ = ping.tick() => {
                if ws_send(&mut sink, Message::Ping(Vec::new().into())).await.is_err() {
                    break CLOSE_NORMAL;
                }
            }
        }
    };
    close_ws(&mut sink, &mut stream, reason).await;
    // Tell the supervisor the exec ended (or release the route if it ended it first). Byte
    // totals are informational and never logged.
    let close_reason = match reason {
        CLOSE_NORMAL => PtyCloseReason::Eof,
        CLOSE_CHANNEL_LOST => PtyCloseReason::ChannelLost,
        CLOSE_TIMEOUT => PtyCloseReason::Idle,
        _ => PtyCloseReason::Error,
    };
    state
        .release_pty_as(id, sandbox, boot, close_reason, bytes_in, bytes_out)
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
    mut close_rx: oneshot::Receiver<u16>,
    from_app_outstanding: Arc<AtomicUsize>,
) {
    let (mut ws_sink, mut ws_stream) = ws.split();
    let r = ChannelRef {
        sandbox_id: sandbox,
        boot_id: boot,
    };
    // Credit accounting, **one window per direction** (`16 §W18`; FIX-060):
    // - `to_app`: the runner may send at most the supervisor's announced window of
    //   unacknowledged bytes; the supervisor returns credit after writing to the loopback app.
    //   Only browser→app data decrements it.
    // - `from_app`: the supervisor may have at most `PREVIEW_STREAM_WINDOW` (what we announced
    //   in `stream.open`) unreturned. `from_app_outstanding` is shared with the pump (raised
    //   on arrival; exceeding the window closes the leg, `StreamListener::deliver`); the
    //   runner returns credit only after the bytes were written onward (to the leg-1
    //   WebSocket, `from_app_unacked`), at least every `window / 4`.
    let mut window = PREVIEW_STREAM_WINDOW;
    let mut to_app_credit: usize = window;
    let mut from_app_unacked: usize = 0;
    let mut bytes_to_app: u64 = 0;
    let mut bytes_from_app: u64 = 0;
    tokio::pin! {
        let idle = tokio::time::sleep(state.idle);
        let lifetime = tokio::time::sleep(state.lifetime);
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
                        idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
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

                    // Text is ignored per the spec. A pong (or ping) is liveness and resets the
                    // idle deadline (`16 §W18`: "a WebSocket's pings count"); keep-awake and
                    // the supervisor's byte accounting stay byte-driven.
                    Some(Ok(Message::Pong(_) | Message::Ping(_))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
                    }
                    Some(Ok(Message::Text(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break CLOSE_NORMAL,
                }
            }
            // App → relay direction: supervisor data out of the leg queue.
            chunk = leg_rx.recv() => {
                match chunk {
                    Some(LegIn::Data(b)) => {
                        // Same absolute-deadline reset as the relay→app arm above.
                        idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
                        bytes_to_app += b.len() as u64;
                        from_app_unacked += b.len();
                        if ws_send(&mut ws_sink, Message::binary(b)).await.is_err() {
                            break CLOSE_NORMAL;
                        }
                        // Return credit after writing onward, at least every `window / 4`.
                        if from_app_unacked >= PREVIEW_STREAM_WINDOW / 4 {
                            let n = from_app_unacked as u32;
                            from_app_unacked = 0;
                            from_app_outstanding.fetch_sub(n as usize, Ordering::AcqRel);
                            let credit = RunnerToSupervisor::StreamCredit(StreamCredit {
                                stream_id,
                                bytes: n,
                            });
                            if state.send_to_supervisor(&r, credit).await.is_err() {
                                break CLOSE_CHANNEL_LOST;
                            }
                        }
                    }
                    // Exec-only traffic never belongs on a port or terminal leg.
                    Some(LegIn::Stderr(_) | LegIn::Exit(_)) => {}
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
                            idle.as_mut().reset(tokio::time::Instant::now() + state.idle);
                            if ws_send(&mut ws_sink, Message::binary(b)).await.is_err() {
                                break;
                            }
                        }
                        LegIn::Stderr(_) | LegIn::Exit(_) => {}
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
                if ws_send(&mut ws_sink, Message::Ping(Vec::new().into())).await.is_err() {
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
    if ws_send(
        &mut ws_sink,
        Message::Close(Some(CloseFrame {
            code: CloseCode::from(reason),
            reason: "".into(),
        })),
    )
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::channel::ChannelSettings;

    fn listener() -> Arc<StreamListener> {
        StreamListener::new(ChannelServer::new(ChannelSettings::new([7; 32])))
    }

    /// Registers a terminal leg with a tiny queue; returns its queue and close signal.
    async fn leg(
        l: &StreamListener,
        id: u32,
        sandbox: Uuid,
        boot: Uuid,
        depth: usize,
    ) -> (mpsc::Receiver<LegIn>, oneshot::Receiver<u16>) {
        let (tx, rx) = mpsc::channel(depth);
        let (close_tx, close_rx) = oneshot::channel();
        l.routes.lock().await.legs.insert(
            id,
            RouteEnd {
                to_leg: tx,
                close_tx,
                sandbox,
                boot,
                terminal: true,
                files: false,
                ssh: false,
                from_sandbox: Arc::new(AtomicUsize::new(0)),
                recv_window: TERMINAL_STREAM_WINDOW,
                _heartbeat: None,
            },
        );
        (rx, close_rx)
    }

    // ------------------------------------------- P5-REFINE-4, C-1058: the open-leg heartbeat

    fn binding(sandbox: Uuid) -> PreviewStreamOpenResponse {
        serde_json::from_value(serde_json::json!({
            "sandbox_id": sandbox, "boot_id": Uuid::new_v4(), "window": 1024,
            "idle_secs": 600, "max_lifetime_secs": 14400
        }))
        .unwrap()
    }

    /// Registers a leg of the given kind through the production path.
    async fn open(l: &StreamListener, id: u32, sandbox: Uuid, kind: (bool, bool, bool)) {
        let (tx, _rx) = mpsc::channel(4);
        let (terminal, files, ssh) = kind;
        l.register_route(
            id,
            &binding(sandbox),
            terminal,
            files,
            ssh,
            tx,
            Arc::new(AtomicUsize::new(0)),
        )
        .await
        .expect("under the cap");
    }

    /// Lets the spawned heartbeat run after a clock advance.
    async fn advance(d: Duration) {
        tokio::time::advance(d).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    const TERMINAL: (bool, bool, bool) = (true, false, false);
    const FILES: (bool, bool, bool) = (false, true, false);
    const SSH: (bool, bool, bool) = (false, false, true);
    const PORT: (bool, bool, bool) = (false, false, false);

    /// A quiet terminal leg stamps every half grace for as long as it is open, and not after.
    #[tokio::test(start_paused = true)]
    async fn an_open_terminal_leg_stamps_every_half_grace_without_bytes() {
        let l = listener();
        let sb = Uuid::new_v4();
        open(&l, 1, sb, TERMINAL).await;
        advance(Duration::from_secs(1)).await;
        assert_eq!(l.keep_awake_stamps(), 0, "the first beat is one period in");
        for n in 1..=4 {
            advance(TERMINAL_AWAKE_GRACE / 2).await;
            assert_eq!(l.keep_awake_stamps(), n, "beat {n}");
        }
        // Every way a leg ends drops its route; the heartbeat goes with it.
        l.release_pty_as(1, sb, Uuid::new_v4(), PtyCloseReason::Eof, 0, 0)
            .await;
        advance(TERMINAL_AWAKE_GRACE * 3).await;
        assert_eq!(l.keep_awake_stamps(), 4, "no stamp after the leg closed");
    }

    /// SSH legs beat on their own (ssh) clock; files and port legs never beat.
    #[tokio::test(start_paused = true)]
    async fn only_terminal_and_ssh_legs_beat() {
        let l = listener();
        let sb = Uuid::new_v4();
        open(&l, 1, sb, FILES).await;
        open(&l, 2, sb, PORT).await;
        advance(TERMINAL_AWAKE_GRACE * 2).await;
        assert_eq!(l.keep_awake_stamps(), 0);
        open(&l, 3, sb, SSH).await;
        advance(SSH_AWAKE_GRACE / 2).await;
        assert_eq!(l.keep_awake_stamps(), 1);
        assert!(l.stamper.last.lock().await.contains_key(&(sb, true)));
        assert!(!l.stamper.last.lock().await.contains_key(&(sb, false)));
        // The channel died: every leg of the boot is dropped, the beat stops.
        l.routes.lock().await.legs.clear();
        advance(SSH_AWAKE_GRACE * 2).await;
        assert_eq!(l.keep_awake_stamps(), 1);
    }

    /// The byte path and the heartbeat share one budget: a byte stamp just before a beat makes
    /// the beat skip, and a beat holds the next byte stamp back for a grace.
    #[tokio::test(start_paused = true)]
    async fn bytes_and_heartbeat_never_double_post() {
        let l = listener();
        let sb = Uuid::new_v4();
        open(&l, 1, sb, TERMINAL).await;
        advance(TERMINAL_AWAKE_GRACE / 2 - Duration::from_secs(1)).await;
        l.stamp_awake(sb, false).await; // bytes at 59 s
        assert_eq!(l.keep_awake_stamps(), 1);
        advance(Duration::from_secs(1)).await; // the beat at 60 s skips
        assert_eq!(l.keep_awake_stamps(), 1);
        l.stamp_awake(sb, false).await; // more bytes: inside the grace
        assert_eq!(l.keep_awake_stamps(), 1);
        advance(TERMINAL_AWAKE_GRACE / 2).await; // the beat at 120 s: 61 s since the last
        assert_eq!(l.keep_awake_stamps(), 2);
        l.stamp_awake(sb, false).await; // bytes right after a beat: held back
        assert_eq!(l.keep_awake_stamps(), 2);
    }

    /// A full leg queue never makes the pump wait (FIX-060: no await on the queue, no lock
    /// held across it): the delivery returns at once, only that leg is closed `4502`, and
    /// every other leg keeps receiving.
    #[tokio::test]
    async fn a_full_queue_closes_that_leg_without_waiting() {
        let l = listener();
        let (sb, boot) = (Uuid::new_v4(), Uuid::new_v4());
        let (_rx_a, mut close_a) = leg(&l, 1, sb, boot, 1).await;
        let (mut rx_b, mut close_b) = leg(&l, 2, sb, boot, 1).await;

        assert_eq!(
            l.deliver(1, sb, boot, LegIn::Data(vec![1])).await,
            Some(Keep::Terminal)
        );
        // The queue (depth 1) is full and nobody drains it: must return, not block.
        let second = tokio::time::timeout(
            Duration::from_secs(2),
            l.deliver(1, sb, boot, LegIn::Data(vec![2])),
        )
        .await
        .expect("the pump waited on a full leg queue");
        assert_eq!(second, None);
        assert_eq!(close_a.try_recv(), Ok(CLOSE_REFUSED));
        assert!(!l.routes.lock().await.legs.contains_key(&1));

        // The neighbour is untouched and still deliverable.
        assert_eq!(
            l.deliver(2, sb, boot, LegIn::Data(vec![3])).await,
            Some(Keep::Terminal)
        );
        assert!(matches!(rx_b.recv().await, Some(LegIn::Data(d)) if d == [3]));
        assert!(close_b.try_recv().is_err());
    }

    /// Frames from a supervisor of another sandbox or boot never find the leg: nothing is
    /// delivered, nothing removed.
    #[tokio::test]
    async fn legs_are_bound_to_their_sandbox_and_boot() {
        let l = listener();
        let (sb, boot) = (Uuid::new_v4(), Uuid::new_v4());
        let (mut rx, mut close) = leg(&l, 1, sb, boot, 4).await;
        for (other_sb, other_boot) in [
            (Uuid::new_v4(), boot),
            (sb, Uuid::new_v4()),
            (Uuid::new_v4(), Uuid::new_v4()),
        ] {
            assert_eq!(
                l.deliver(1, other_sb, other_boot, LegIn::Credit(5)).await,
                None
            );
            assert!(l.take_owned(1, other_sb, other_boot).await.is_none());
        }
        assert!(rx.try_recv().is_err());
        assert!(close.try_recv().is_err());
        assert!(l.routes.lock().await.legs.contains_key(&1));
        assert!(l.take_owned(1, sb, boot).await.is_some());
    }
}

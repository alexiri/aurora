//! Preview byte streams (phase 4, P4-PRV-2, D-147, SEC-82; `16 §W18` leg 2, `12 §Previews`).
//!
//! The supervisor accepts `stream.open { port }` only for a port in its in-memory share list
//! (never the port discovery), never for one of its own listeners, and dials **only**
//! `127.0.0.1:<port>` (then `[::1]:<port>`) with a 5 s connect timeout. Each accepted stream is
//! one task copying loopback bytes over the W6 channel with credit-based flow control: a sender
//! has at most `PREVIEW_STREAM_WINDOW` unacknowledged bytes outstanding per direction and credit
//! is returned only after bytes were written onward. A connection change (epoch bump or reset)
//! ends every stream; streams are never resumed. Round-robin interleaving falls out of one chunk
//! at a time per task through the shared channel queue.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aurora_core::limits::{
    PREVIEW_CHUNK, PREVIEW_STREAM_WINDOW, PREVIEW_STREAMS_PER_SANDBOX,
    PREVIEW_SUPERVISOR_CONNECT_TIMEOUT,
};
use aurora_protocol::runner::{StreamCloseReason, StreamRefuseReason};
use aurora_protocol::supervisor::SupervisorToRunner;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use uuid::Uuid;

use crate::channel::{self, StreamIn, StreamOut, StreamSender};
use crate::supervisor::PreviewShareEntry;

/// The supervisor's own loopback listeners (process manager relay, machine/open proxies). A
/// share can never name one (`aurora-db::preview_shares::RESERVED_PORTS`) and a `stream.open`
/// for one is refused.
const RESERVED_PORTS: [u16; 3] = [3128, 3129, 3130];

/// Queue depth of one stream's runner→loopback data queue. The task drains it into its write
/// buffer at once (it never waits on the app, FIX-091), so it only fills when the whole task is
/// wedged; the pump then closes the stream with `error` instead of waiting (`try_send`). The
/// byte bound is the credit window (`outstanding`), this is the frame-count bound for tiny frames.
pub(crate) const DATA_QUEUE: usize = 4096;

/// Violations within [`VIOLATION_WINDOW`] that close the channel with 4400 (`16 §W18`/§W19).
const VIOLATION_LIMIT: usize = 4;
/// The window violations are counted in (`16 §W18`).
const VIOLATION_WINDOW: Duration = Duration::from_secs(60);
/// Ids remembered as closed per connection before the set is reset (bounds memory).
const CLOSED_IDS_CAP: usize = 4096;

/// Windowed protocol-violation counter (`16 §W18`: four within 60 s close the channel with
/// 4400); reset when the connection changes. Uses tokio's clock (testable with a paused clock).
#[derive(Default)]
pub(crate) struct Violations(VecDeque<tokio::time::Instant>);

impl Violations {
    /// Records one violation; `true` when the limit is reached inside the window.
    pub(crate) fn record(&mut self) -> bool {
        let now = tokio::time::Instant::now();
        while self
            .0
            .front()
            .is_some_and(|t| now.duration_since(*t) >= VIOLATION_WINDOW)
        {
            self.0.pop_front();
        }
        self.0.push_back(now);
        self.0.len() >= VIOLATION_LIMIT
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// Ids this side closed, or was told closed, on the current connection: data for them is a
/// legal close race and is dropped silently (`16 §W18`).
#[derive(Default)]
pub(crate) struct ClosedIds(HashSet<u32>);

impl ClosedIds {
    pub(crate) fn insert(&mut self, id: u32) {
        if self.0.len() >= CLOSED_IDS_CAP {
            self.0.clear();
        }
        self.0.insert(id);
    }

    pub(crate) fn remove(&mut self, id: u32) {
        self.0.remove(&id);
    }

    pub(crate) fn contains(&self, id: u32) -> bool {
        self.0.contains(&id)
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// Credit the runner granted a task: a counter plus a wakeup, so the pump adds credit without
/// ever waiting on the task (a bounded queue of credit frames could fill).
#[derive(Default)]
pub(crate) struct Credit {
    bytes: std::sync::Mutex<usize>,
    notify: tokio::sync::Notify,
    /// Bytes received from the runner and not yet written onward (violation detection). The
    /// pump adds, the task subtracts *before* it returns the credit frame — so a runner that
    /// sends the instant credit arrives never finds the count stale.
    outstanding: std::sync::atomic::AtomicUsize,
}

impl Credit {
    pub(crate) fn outstanding(&self) -> usize {
        self.outstanding.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn reserve(&self, n: usize) {
        self.outstanding
            .fetch_add(n, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn release(&self, n: usize) {
        self.outstanding
            .fetch_sub(n, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn add(&self, n: usize) {
        {
            let mut b = self.bytes.lock().unwrap_or_else(|e| e.into_inner());
            *b = b.saturating_add(n).min(1 << 32);
        }
        self.notify.notify_one();
    }

    /// Takes everything granted since the last call.
    pub(crate) fn take(&self) -> usize {
        std::mem::take(&mut *self.bytes.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub(crate) async fn granted(&self) {
        self.notify.notified().await;
    }
}

/// The binary data wire frame `[0x01][stream_id BE][payload]` (`16 §W18`): the tag and id
/// are how the runner's channel routes the payload to the right stream.
fn frame_data(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(payload.len() + 5);
    b.push(0x01);
    b.extend_from_slice(&id.to_be_bytes());
    b.extend_from_slice(payload);
    b
}

/// The share list the stream engine enforces, `port -> share_id`.
pub type SharedPorts = Arc<HashMap<u16, Uuid>>;

/// Cloneable handle the supervisor keeps: share-list updates and nothing else.
#[derive(Clone)]
pub struct StreamsHandle {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
}

impl StreamsHandle {
    /// A handle whose pump never runs (tests that do not exercise streams).
    pub fn noop() -> Self {
        let (cmd_tx, _) = mpsc::unbounded_channel();
        Self { cmd_tx }
    }
    /// Applies the `previews` home-sync part: the new share list replaces the old; streams on
    /// ports that left the list are closed (`12 §Previews`). An empty list refuses everything.
    pub fn set_shares(&self, shares: &HashMap<Uuid, PreviewShareEntry>) {
        let mut ports: Vec<(u16, Uuid)> = Vec::with_capacity(shares.len());
        for (id, e) in shares {
            if e.port >= 1024 && !RESERVED_PORTS.contains(&e.port) {
                ports.push((e.port, *id));
            }
        }
        let map: SharedPorts = Arc::new(ports.into_iter().collect());
        let _ = self.cmd_tx.send(Cmd::Shares(map));
    }

    /// Forwards a credit frame to a live stream's task (supervisor→runner direction).
    pub async fn credit(&self, id: u32, bytes: u32) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::Credit { id, bytes, tx }).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// The runner closed its side of a stream.
    pub async fn stream_closed(&self, id: u32) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::Closed { id, tx }).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Opens a stream from a sequenced `stream.open` (tests / older runners); the engine
    /// enforces the allow-list and answers. Waits for the pump to act.
    pub async fn stream_open(&self, o: aurora_protocol::runner::StreamOpen) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::Open { open: o, tx }).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Ends every stream (the `shutdown` frame: `12 §Previews`, "everything closes on
    /// shutdown"); the runner is told with `stream.close { channel_lost }`. Returns once the
    /// engine has done it; the caller bounds it with the shutdown grace.
    pub async fn close_all(&self) {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::CloseAll { tx }).is_ok() {
            let _ = rx.await;
        }
    }

    /// Test hook: waits for the pump to process pending commands.
    #[cfg(test)]
    pub async fn settle(&self) {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.send(Cmd::Settle { tx });
        let _ = rx.await;
    }
}

enum Cmd {
    CloseAll {
        tx: oneshot::Sender<()>,
    },
    Shares(SharedPorts),
    Credit {
        id: u32,
        bytes: u32,
        tx: oneshot::Sender<bool>,
    },
    Closed {
        id: u32,
        tx: oneshot::Sender<bool>,
    },
    Open {
        open: aurora_protocol::runner::StreamOpen,
        tx: oneshot::Sender<bool>,
    },
    #[cfg(test)]
    Settle {
        tx: oneshot::Sender<()>,
    },
}

/// Report from a stream task to the pump.
enum TaskMsg {
    /// The task ended: forward `stream.close` and free the id.
    Finished {
        id: u32,
        reason: CloseReason,
        bytes_in: u64,
        bytes_out: u64,
    },
}

struct Live {
    port: u16,
    data_tx: mpsc::Sender<Vec<u8>>,
    credit: Arc<Credit>,
    abort: AbortHandle,
}

/// The pump: owns the stream table, services the channel's stream side-channel.
pub struct Streams {
    out: StreamSender,
    inbound: mpsc::UnboundedReceiver<StreamIn>,
    epoch: watch::Receiver<u64>,
    cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    task_rx: mpsc::UnboundedReceiver<TaskMsg>,
    task_tx: mpsc::UnboundedSender<TaskMsg>,
    shares: SharedPorts,
    live: HashMap<u32, Live>,
    /// Recent unsequenced-frame violations (`16 §W18`): four within 60 s close the channel
    /// with 4400; reset with the connection.
    violations: Violations,
    /// Ids closed on this connection (data for them is dropped, not a violation).
    closed: ClosedIds,
}

/// Why a stream ended (`16 §W18`).
#[derive(Debug, Clone)]
pub enum CloseReason {
    Eof,
    Reset,
    Idle,
    Lifetime,
    ChannelLost,
    Error,
}

impl From<CloseReason> for StreamCloseReason {
    fn from(r: CloseReason) -> Self {
        match r {
            CloseReason::Eof => Self::Eof,
            CloseReason::Reset => Self::Reset,
            CloseReason::Idle => Self::Idle,
            CloseReason::Lifetime => Self::Lifetime,
            CloseReason::ChannelLost => Self::ChannelLost,
            CloseReason::Error => Self::Error,
        }
    }
}

impl Streams {
    /// Spawns the pump on the channel's stream ends; returns the supervisor's handle.
    pub fn spawn(ends: channel::StreamEnds) -> Arc<StreamsHandle> {
        let channel::StreamEnds {
            sender,
            inbound,
            epoch,
        } = ends;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        let pump = Self {
            out: sender,
            inbound,
            epoch,
            cmd_rx,
            task_rx,
            task_tx: task_tx.clone(),
            shares: Arc::new(HashMap::new()),
            live: HashMap::new(),
            violations: Violations::default(),
            closed: ClosedIds::default(),
        };
        tokio::spawn(pump.run());
        Arc::new(StreamsHandle { cmd_tx })
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                msg = self.inbound.recv() => {
                    let Some(msg) = msg else { return };
                    match msg {
                        StreamIn::Frame(f) => self.on_frame(f).await,
                        StreamIn::Data { stream_id, payload } => {
                            self.on_data(stream_id, payload).await;
                        }
                        // A connection change ends every stream (`16 §W18`); also sent after
                        // every (re)connect attempt, so this subsumes the epoch watch.
                        StreamIn::Reset => self.reset(CloseReason::ChannelLost).await,
                    }
                }
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(Cmd::Shares(ports)) => self.apply_shares(ports).await,
                    Some(Cmd::CloseAll { tx }) => {
                        self.close_live(CloseReason::ChannelLost).await;
                        let _ = tx.send(());
                    }
                    Some(Cmd::Credit { id, bytes, tx }) => {
                        let ok = if let Some(l) = self.live.get(&id) {
                            l.credit.add(bytes as usize);
                            true
                        } else {
                            false
                        };
                        let _ = tx.send(ok);
                    }
                    Some(Cmd::Closed { id, tx }) => {
                        let ok = self.live.remove(&id).is_some();
                        if ok {
                            self.closed.insert(id);
                        }
                        let _ = tx.send(ok);
                    }
                    Some(Cmd::Open { open, tx }) => {
                        self.on_open(open).await;
                        let _ = tx.send(true);
                    }
                    #[cfg(test)]
                    Some(Cmd::Settle { tx }) => {
                        let _ = tx.send(());
                    }
                    None => return,
                },
                msg = self.task_rx.recv() => match msg {
                    Some(TaskMsg::Finished { id, reason, bytes_in, bytes_out }) => {
                        // Only the first end of a stream answers (kill may have raced us).
                        if self.live.remove(&id).is_some() {
                            self.closed.insert(id);
                            self.send_close(id, reason, bytes_in, bytes_out).await;
                        }
                    }
                    None => return,
                },
                _ = self.epoch.changed() => {
                    // Belt and braces: the channel also sends `Reset`; a double reset is a no-op.
                    self.reset(CloseReason::ChannelLost).await;
                }
            }
        }
    }

    async fn on_frame(&mut self, f: aurora_protocol::supervisor::RunnerToSupervisor) {
        use aurora_protocol::supervisor::RunnerToSupervisor as M;
        match f {
            M::StreamOpen(o) => self.on_open(o).await,
            M::StreamCredit(c) => {
                if let Some(l) = self.live.get(&c.stream_id) {
                    l.credit.add(c.bytes as usize);
                }
            }
            M::StreamClose(c) => {
                // The runner ended its side: drop ours without another close frame.
                if let Some(l) = self.live.remove(&c.stream_id) {
                    l.abort.abort();
                }
                self.closed.insert(c.stream_id);
            }
            M::Other(u) => {
                tracing::debug!(kind = u.kind.as_str(), "unknown stream frame ignored");
            }
            _ => {}
        }
    }

    /// `stream.open`: allow-list (share list only, never a supervisor listener), per-sandbox
    /// cap, loopback-only dial, then `stream.opened` or `stream.refused`.
    async fn on_open(&mut self, o: aurora_protocol::runner::StreamOpen) {
        if self.live.contains_key(&o.stream_id) {
            // A second open for a live id is a protocol violation (`16 §W18`).
            self.kill(o.stream_id, CloseReason::Error).await;
            self.violate().await;
            return;
        }
        let Some(_share) = self.shares.get(&o.port) else {
            self.refuse(o.stream_id, StreamRefuseReason::NotShared)
                .await;
            return;
        };
        if self.live.len() >= PREVIEW_STREAMS_PER_SANDBOX as usize {
            self.refuse(o.stream_id, StreamRefuseReason::Limit).await;
            return;
        }
        let sock = match dial_loopback(o.port, PREVIEW_SUPERVISOR_CONNECT_TIMEOUT).await {
            Ok(s) => s,
            Err(kind) => {
                let reason = match kind {
                    DialError::Timeout => StreamRefuseReason::Timeout,
                    DialError::Failed => StreamRefuseReason::Refused,
                };
                self.refuse(o.stream_id, reason).await;
                return;
            }
        };
        let opened = SupervisorToRunner::StreamOpened(aurora_protocol::runner::StreamOpened {
            stream_id: o.stream_id,
            window: PREVIEW_STREAM_WINDOW as u32,
        });
        if !self.send_frame(opened).await {
            return;
        }
        // The runner's announced window is our send allowance (direction loopback → runner).
        let (data_tx, data_rx) = mpsc::channel(DATA_QUEUE);
        let credit = Arc::new(Credit::default());
        let task = tokio::spawn(stream_task(
            o.stream_id,
            o.port,
            sock,
            o.window as usize,
            data_rx,
            credit.clone(),
            self.out.clone(),
            self.task_tx.clone(),
        ));
        // A reused id is live again: its old "closed" mark no longer applies.
        self.closed.remove(o.stream_id);
        self.live.insert(
            o.stream_id,
            Live {
                port: o.port,
                data_tx,
                credit,
                abort: task.abort_handle(),
            },
        );
    }

    /// Data from the runner: over-window or unknown-id is a protocol violation (`16 §W18`);
    /// repeated violations close the channel with 4400.
    async fn on_data(&mut self, stream_id: u32, payload: Vec<u8>) {
        let Some(l) = self.live.get_mut(&stream_id) else {
            if self.closed.contains(stream_id) {
                // A close race: this side (or the runner) closed the stream on this
                // connection. Dropped silently, not a violation.
                tracing::debug!(stream_id, "stream data after close dropped");
            } else {
                tracing::warn!(stream_id, "stream data for unknown id");
                self.violate().await;
            }
            return;
        };
        if l.credit.outstanding() + payload.len() > PREVIEW_STREAM_WINDOW {
            tracing::warn!(stream_id, "stream data beyond the credit window");
            self.kill(stream_id, CloseReason::Error).await;
            self.violate().await;
            return;
        }
        l.credit.reserve(payload.len());
        // Never wait on the stream's task (FIX-091): a full queue means it is wedged.
        match l.data_tx.try_send(payload) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(stream_id, "stream queue full; closing the stream");
                self.kill(stream_id, CloseReason::Error).await;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!(stream_id, "stream queue closed; task is ending");
            }
        }
    }

    /// One stream protocol violation; four within 60 s close the channel (`16 §W18`).
    async fn violate(&mut self) {
        if self.violations.record() {
            let _ = self.out.send(StreamOut::Abort).await;
        }
    }

    /// Replaces the share list; streams whose port left it are closed (`12 §Previews`).
    async fn apply_shares(&mut self, ports: SharedPorts) {
        self.shares = ports;
        let gone: Vec<u32> = self
            .live
            .iter()
            .filter(|(_, l)| !self.shares.contains_key(&l.port))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.kill(id, CloseReason::Reset).await;
        }
    }

    /// A connection change (or channel end): every stream ends, none are resumed.
    /// The violation count and the closed-id memory are per connection: both reset.
    async fn reset(&mut self, reason: CloseReason) {
        self.close_live(reason).await;
        self.violations.clear();
        self.closed.clear();
    }

    /// Ends every live stream (connection change or shutdown).
    async fn close_live(&mut self, reason: CloseReason) {
        let ids: Vec<u32> = self.live.keys().copied().collect();
        for id in ids {
            self.kill(id, reason.clone()).await;
        }
    }

    /// Ends one stream: abort the task, free the id, tell the runner.
    async fn kill(&mut self, id: u32, reason: CloseReason) {
        if let Some(l) = self.live.remove(&id) {
            l.abort.abort();
            self.closed.insert(id);
            self.send_close(id, reason, 0, 0).await;
        }
    }

    async fn send_close(&mut self, id: u32, reason: CloseReason, bytes_in: u64, bytes_out: u64) {
        let f = SupervisorToRunner::StreamClose(aurora_protocol::runner::StreamClose {
            stream_id: id,
            reason: reason.into(),
            bytes_in: (bytes_in > 0).then_some(bytes_in),
            bytes_out: (bytes_out > 0).then_some(bytes_out),
        });
        self.send_frame(f).await;
    }

    async fn refuse(&mut self, stream_id: u32, reason: StreamRefuseReason) {
        let f = SupervisorToRunner::StreamRefused(aurora_protocol::runner::StreamRefused {
            stream_id,
            reason,
        });
        self.send_frame(f).await;
    }

    /// One unsequenced `stream.*` text frame; `false` when the channel (or encoding) failed.
    async fn send_frame(&mut self, msg: SupervisorToRunner) -> bool {
        let Some(text) = channel::encode_stream_frame(msg) else {
            tracing::warn!("stream frame dropped (encode)");
            return false;
        };
        self.out.send(StreamOut::Text(text)).await.is_ok()
            && self.out.send(StreamOut::Flush).await.is_ok()
    }
}

/// One stream's copy loop: loopback socket ↔ the W6 channel's binary side-channel. Credit
/// gates the loopback→runner direction (we stop reading the socket at zero credit, which is
/// the backpressure); runner→loopback bytes are window-bounded by the pump.
#[allow(clippy::too_many_arguments)]
async fn stream_task(
    id: u32,
    port: u16,
    sock: TcpStream,
    send_credit: usize,
    mut data_rx: mpsc::Receiver<Vec<u8>>,
    credit_cell: Arc<Credit>,
    out: StreamSender,
    report: mpsc::UnboundedSender<TaskMsg>,
) {
    tracing::debug!(stream_id = id, port, "preview stream open");
    // Runner → app bytes wait here until the app reads them (window-bounded by the pump). The
    // write is one select arm among the others, so a stalled app never stops this task from
    // draining credit and the queue, or from being aborted (FIX-091).
    let mut wbuf: VecDeque<u8> = VecDeque::new();
    let (mut rd, mut wr) = sock.into_split();
    let mut buf = vec![0u8; PREVIEW_CHUNK];
    let mut pending: VecDeque<u8> = VecDeque::new();
    let mut credit = send_credit;
    let mut bytes_in: u64 = 0;
    let mut bytes_out: u64 = 0;
    let reason = loop {
        // Opportunistic drain of leftovers from an over-credit read (credit-gated,
        // non-blocking): keeps one stream from buffering more than the window.
        if credit > 0 && !pending.is_empty() {
            let n = pending.len().min(credit);
            let chunk: Vec<u8> = pending.drain(..n).collect();
            if out
                .send(StreamOut::Binary(frame_data(id, &chunk)))
                .await
                .is_err()
            {
                break CloseReason::ChannelLost;
            }
            credit -= n;
            bytes_out += n as u64;
            continue;
        }
        tokio::select! {
            // Loopback → runner. Only with credit (we stop reading the socket at zero credit:
            // TCP backpressure toward the app, `16 §W18`). The wire frame is
            // `[0x01][stream_id BE][payload]` — the runner's channel decodes it.
            n = rd.read(&mut buf), if credit > 0 => {
                match n {
                    Ok(0) => break CloseReason::Eof,
                    Ok(n) => {
                        let send = n.min(credit);
                        if out.send(StreamOut::Binary(frame_data(id, &buf[..send]))).await.is_err() {
                            break CloseReason::ChannelLost;
                        }
                        credit -= send;
                        bytes_out += send as u64;
                        if send < n {
                            pending.extend(&buf[send..n]);
                        }
                    }
                    Err(_) => break CloseReason::Reset,
                }
            }
            // Runner → loopback; credit returns only after the bytes were written onward.
            chunk = data_rx.recv() => {
                match chunk {
                    Some(chunk) => wbuf.extend(chunk),
                    // The pump dropped the queue (reset path aborts us anyway).
                    None => break CloseReason::ChannelLost,
                }
            }
            n = wr.write(wbuf.as_slices().0), if !wbuf.is_empty() => {
                match n {
                    Ok(0) | Err(_) => break CloseReason::Reset,
                    Ok(n) => {
                        wbuf.drain(..n);
                        bytes_in += n as u64;
                        credit_cell.release(n);
                        let f = SupervisorToRunner::StreamCredit(
                            aurora_protocol::runner::StreamCredit {
                                stream_id: id,
                                bytes: n as u32,
                            },
                        );
                        if let Some(t) = channel::encode_stream_frame(f)
                            && out.send(StreamOut::Text(t)).await.is_err()
                        {
                            break CloseReason::ChannelLost;
                        }
                    }
                }
            }
            () = credit_cell.granted() => {
                credit = (credit + credit_cell.take()).min(PREVIEW_STREAM_WINDOW);
            }
        }
    };
    tracing::debug!(
        stream_id = id,
        ?reason,
        bytes_in,
        bytes_out,
        "preview stream closed"
    );
    let _ = report.send(TaskMsg::Finished {
        id,
        reason,
        bytes_in,
        bytes_out,
    });
}

enum DialError {
    Timeout,
    Failed,
}

/// `127.0.0.1:<port>` first, then `[::1]:<port>`; never any other address, never a proxy,
/// 5 s connect timeout (`16 §W18` allow-list rule 2).
async fn dial_loopback(port: u16, budget: Duration) -> Result<TcpStream, DialError> {
    let targets = [
        SocketAddr::from(([127, 0, 0, 1], port)),
        SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
    ];
    let mut timed_out = false;
    for t in targets {
        match tokio::time::timeout(budget, TcpStream::connect(t)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(_)) => continue,
            Err(_) => timed_out = true,
        }
    }
    if timed_out {
        Err(DialError::Timeout)
    } else {
        Err(DialError::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // (`16 §W18`) The binary data wire frame both decoders (runner channel, supervisor
    // channel) expect: tag 0x01, stream id big-endian, then the payload.
    #[test]
    fn frame_data_is_tag_id_payload() {
        let f = frame_data(0x01020304, b"hello");
        assert_eq!(&f[..5], &[0x01, 0x01, 0x02, 0x03, 0x04]);
        assert_eq!(&f[5..], b"hello");
        assert_eq!(frame_data(7, &[]), vec![0x01, 0, 0, 0, 7]);
    }
}

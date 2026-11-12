//! W6 supervisor channel client (`docs/16-wire-protocols.md` §W6).
//!
//! One long-lived task owns the connection lineage: it dials, sends `hello` (bootstrap token the
//! first time, the stored HMAC session afterwards), numbers outgoing frames, keeps unacked frames
//! (bounded to 4 MiB; back-pressure beyond that), replays them after a reconnect from the runner's
//! `resume_from`, applies each incoming `seq` at most once, acks every second, and pings every
//! 20 s (60 s of silence = dead). `ack` frames are not sequenced (`seq` 0).
//!
//! (P4-PRV-2, `16 §W18`) The channel also carries preview byte streams: `stream.*` text frames
//! are **outside the seq/ack lineage** (`seq: 0, ack: 0`; never buffered for replay, never
//! acknowledged, never blocked by the replay back-pressure) and binary messages carry stream
//! data (`[0x01][stream_id: u32 BE][payload ≤ PREVIEW_CHUNK]`). Stream traffic moves over its
//! own queues ([`StreamSender`] out, [`StreamEnds`] in); the [`StreamEnds::epoch`] watch bumps
//! on every connection (re)establishment and every drop: a change ends every stream.
//! (P4-TERM, `16 §W19`) Terminal PTYs ride the same conventions beside the streams:
//! `pty.*` text frames and binary data `[0x02][pty_id: u32 BE][payload ≤ TERMINAL_CHUNK]`,
//! over [`PtyEnds`] with the same epoch semantics — a change ends every PTY too.

use std::collections::VecDeque;
use std::time::Duration;

use aurora_protocol::common::{Empty, SecretString, Validate};
use aurora_protocol::supervisor::{
    ACK_INTERVAL, CLOSE_SPENT, CLOSE_UNAUTHORIZED, Frame, Hello, HelloBody, MAX_UNACKED_BYTES,
    PING_INTERVAL, RunnerToSupervisor, SILENCE_TIMEOUT, SessionProof, SupervisorToRunner, Welcome,
};
use aurora_protocol::version::Plane;
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval, sleep, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("runner rejected the supervisor (close code {0})")]
    Rejected(u16),
    #[error("protocol violation: {0}")]
    Protocol(&'static str),
}

#[derive(Debug, Clone)]
pub struct ChannelConfig {
    pub url: String,
    pub bootstrap_token: SecretString,
    pub ping_interval: Duration,
    pub silence_timeout: Duration,
    pub ack_interval: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
}

impl ChannelConfig {
    pub fn new(url: impl Into<String>, bootstrap_token: SecretString) -> Self {
        Self {
            url: url.into(),
            bootstrap_token,
            ping_interval: PING_INTERVAL,
            silence_timeout: SILENCE_TIMEOUT,
            ack_interval: ACK_INTERVAL,
            backoff_initial: Duration::from_millis(250),
            backoff_max: Duration::from_secs(5),
        }
    }
}

/// Cloneable handle for sending messages to the runner (sequenced by the channel task).
#[derive(Clone)]
pub struct ChannelSender(mpsc::Sender<SupervisorToRunner>);

#[derive(Debug, thiserror::Error)]
#[error("channel closed")]
pub struct ChannelClosed;

impl ChannelSender {
    pub async fn send(&self, msg: SupervisorToRunner) -> Result<(), ChannelClosed> {
        self.0.send(msg).await.map_err(|_| ChannelClosed)
    }

    /// For tests/wiring that do not need the network task.
    pub fn from_mpsc(tx: mpsc::Sender<SupervisorToRunner>) -> Self {
        Self(tx)
    }
}

impl StreamSender {
    pub async fn send(&self, out: StreamOut) -> Result<(), ChannelClosed> {
        self.0.send(out).await.map_err(|_| ChannelClosed)
    }
}

pub struct Channel {
    pub sender: ChannelSender,
    /// Runner frames, each delivered exactly once and in order (acks are consumed internally).
    pub inbound: mpsc::Receiver<RunnerToSupervisor>,
    /// Ends only on a fatal rejection (4401/4409) or when every sender is dropped.
    pub task: JoinHandle<Result<(), ChannelError>>,
    /// (P4-PRV-2, W18) Preview-stream traffic, outside the seq/ack lineage.
    pub streams: StreamEnds,
    /// (P4-TERM, W19) Terminal PTY traffic, the same conventions beside the streams.
    pub ptys: PtyEnds,
}

pub fn spawn(cfg: ChannelConfig) -> Channel {
    let (out_tx, out_rx) = mpsc::channel(256);
    let (in_tx, in_rx) = mpsc::channel(256);
    let (stream_out_tx, stream_out_rx) = mpsc::channel(1024);
    let (stream_in_tx, stream_in_rx) = mpsc::unbounded_channel();
    let (pty_out_tx, pty_out_rx) = mpsc::channel(1024);
    let (pty_in_tx, pty_in_rx) = mpsc::unbounded_channel();
    let (epoch_tx, epoch_rx) = watch::channel(0u64);
    let (pty_epoch_tx, pty_epoch_rx) = watch::channel(0u64);
    let task = tokio::spawn(run(
        cfg,
        out_rx,
        in_tx,
        stream_out_rx,
        stream_in_tx,
        epoch_tx,
        pty_out_rx,
        pty_in_tx,
        pty_epoch_tx,
    ));
    Channel {
        sender: ChannelSender(out_tx),
        inbound: in_rx,
        task,
        streams: StreamEnds {
            sender: StreamSender(stream_out_tx),
            inbound: stream_in_rx,
            epoch: epoch_rx,
        },
        ptys: PtyEnds {
            sender: StreamSender(pty_out_tx),
            inbound: pty_in_rx,
            epoch: pty_epoch_rx,
        },
    }
}

/// Sends preview-stream frames to the runner (`16 §W18`): unsequenced, never replayed,
/// never back-pressured by the replay buffer. `None` frames flush the socket.
#[derive(Clone)]
pub struct StreamSender(mpsc::Sender<StreamOut>);

/// One unit of stream traffic to the runner.
pub enum StreamOut {
    /// A `stream.*` text frame (already encoded, `seq: 0, ack: 0`).
    Text(String),
    /// Binary data `[0x01][stream_id: u32 BE][payload]`.
    Binary(Vec<u8>),
    /// Flush pending frames now (before an answer that must not wait for Nagle-ish batching).
    Flush,
    /// Repeated stream protocol violations: close the WebSocket with 4400 and reconnect
    /// (`16 §W18`); every stream ends with the reconnect.
    Abort,
}

/// The receiving side of the preview-stream channel, owned by the stream engine.
pub struct StreamEnds {
    pub sender: StreamSender,
    /// Binary data and `stream.*` frames from the runner (validated, unsequenced).
    pub inbound: mpsc::UnboundedReceiver<StreamIn>,
    /// Bumps on every connection established and every drop; a change ends every stream.
    pub epoch: watch::Receiver<u64>,
}

/// The receiving side of the terminal-PTY channel, owned by the PTY engine (P4-TERM, W19).
/// Same shape and epoch semantics as [`StreamEnds`].
pub struct PtyEnds {
    pub sender: StreamSender,
    /// Binary data and `pty.*` frames from the runner (validated, unsequenced).
    pub inbound: mpsc::UnboundedReceiver<PtyIn>,
    /// Bumps on every connection established and every drop; a change ends every PTY.
    pub epoch: watch::Receiver<u64>,
}

/// One validated, unsequenced stream message from the runner.
#[derive(Debug)]
pub enum StreamIn {
    /// A decoded `stream.*` control frame.
    Frame(RunnerToSupervisor),
    /// Binary data; `stream_id` from the header, payload ≤ `PREVIEW_CHUNK`.
    Data { stream_id: u32, payload: Vec<u8> },
    /// The connection dropped (or a new one was admitted): every stream ends.
    Reset,
}

/// One validated, unsequenced terminal message from the runner (P4-TERM, W19).
#[derive(Debug)]
pub enum PtyIn {
    /// A decoded `pty.*` control frame.
    Frame(RunnerToSupervisor),
    /// Binary data; `pty_id` from the header, payload ≤ `TERMINAL_CHUNK`.
    Data { pty_id: u32, payload: Vec<u8> },
    /// The connection dropped (or a new one was admitted): every PTY ends.
    Reset,
}

#[derive(Default)]
struct State {
    session: Option<SessionProof>,
    next_seq: u64,
    unacked: VecDeque<(u64, String)>,
    unacked_bytes: usize,
    /// Highest runner seq applied.
    last_recv: u64,
    /// `last_recv` value last reported to the runner.
    ack_sent: u64,
}

impl State {
    fn on_ack(&mut self, ack: u64) {
        while let Some((seq, s)) = self.unacked.front() {
            if *seq > ack {
                break;
            }
            self.unacked_bytes -= s.len();
            self.unacked.pop_front();
        }
    }
}

enum Outcome {
    Reconnect { was_welcomed: bool },
    Fatal(ChannelError),
    Done,
}

// (P4-TERM) The pty ends ride beside the stream ends; the params mirror `spawn`'s wiring.
#[allow(clippy::too_many_arguments)]
async fn run(
    cfg: ChannelConfig,
    mut out_rx: mpsc::Receiver<SupervisorToRunner>,
    in_tx: mpsc::Sender<RunnerToSupervisor>,
    mut stream_rx: mpsc::Receiver<StreamOut>,
    stream_tx: mpsc::UnboundedSender<StreamIn>,
    epoch_tx: watch::Sender<u64>,
    mut pty_rx: mpsc::Receiver<StreamOut>,
    pty_tx: mpsc::UnboundedSender<PtyIn>,
    pty_epoch_tx: watch::Sender<u64>,
) -> Result<(), ChannelError> {
    let mut st = State {
        next_seq: 1,
        ..State::default()
    };
    let mut delay = cfg.backoff_initial;
    loop {
        let res = connection(
            &cfg,
            &mut st,
            &mut out_rx,
            &in_tx,
            &mut stream_rx,
            &stream_tx,
            &epoch_tx,
            &mut pty_rx,
            &pty_tx,
            &pty_epoch_tx,
        )
        .await;
        // A connection change ends every stream and every PTY (`16 §W18`, `16 §W19`).
        let _ = stream_tx.send(StreamIn::Reset);
        let _ = pty_tx.send(PtyIn::Reset);
        match res {
            Outcome::Done => return Ok(()),
            Outcome::Fatal(e) => return Err(e),
            Outcome::Reconnect { was_welcomed } => {
                if was_welcomed {
                    delay = cfg.backoff_initial;
                }
                tracing::warn!(
                    delay_ms = delay.as_millis() as u64,
                    "channel down; reconnecting"
                );
                sleep(delay).await;
                delay = (delay * 2).min(cfg.backoff_max);
            }
        }
    }
}

fn close_outcome(code: u16, welcomed: bool) -> Outcome {
    if code == CLOSE_UNAUTHORIZED || code == CLOSE_SPENT {
        Outcome::Fatal(ChannelError::Rejected(code))
    } else {
        Outcome::Reconnect {
            was_welcomed: welcomed,
        }
    }
}

fn encode(st: &State, seq: u64, msg: SupervisorToRunner) -> Option<String> {
    let frame = Frame {
        seq,
        ack: st.last_recv,
        msg,
    };
    if let Err(e) = frame.validate() {
        tracing::error!(kind = frame.msg.kind(), error = %e, "dropping invalid outgoing frame");
        return None;
    }
    serde_json::to_string(&frame).ok()
}

#[allow(clippy::too_many_arguments)]
async fn connection(
    cfg: &ChannelConfig,
    st: &mut State,
    out_rx: &mut mpsc::Receiver<SupervisorToRunner>,
    in_tx: &mpsc::Sender<RunnerToSupervisor>,
    stream_rx: &mut mpsc::Receiver<StreamOut>,
    stream_tx: &mpsc::UnboundedSender<StreamIn>,
    epoch_tx: &watch::Sender<u64>,
    pty_rx: &mut mpsc::Receiver<StreamOut>,
    pty_tx: &mpsc::UnboundedSender<PtyIn>,
    pty_epoch_tx: &watch::Sender<u64>,
) -> Outcome {
    let no = Outcome::Reconnect {
        was_welcomed: false,
    };
    let Ok((mut ws, _)) = tokio_tungstenite::connect_async(cfg.url.as_str()).await else {
        return no;
    };

    // ---- handshake
    let body = match &st.session {
        Some(s) => HelloBody {
            protocol: Plane::Supervisor.current(),
            bootstrap_token: None,
            session: Some(s.clone()),
        },
        None => HelloBody {
            protocol: Plane::Supervisor.current(),
            bootstrap_token: Some(cfg.bootstrap_token.clone()),
            session: None,
        },
    };
    let Ok(hello) = serde_json::to_string(&Hello { hello: body }) else {
        return no;
    };
    if ws.send(Message::text(hello)).await.is_err() {
        return no;
    }
    let welcome = loop {
        match timeout(cfg.silence_timeout, ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<Welcome>(t.as_str()) {
                Ok(w) => break w.welcome,
                Err(_) => return Outcome::Fatal(ChannelError::Protocol("bad welcome")),
            },
            Ok(Some(Ok(Message::Close(Some(f))))) => {
                return close_outcome(u16::from(f.code), false);
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            _ => return no,
        }
    };
    st.session = Some(welcome.session);
    st.on_ack(welcome.resume_from);
    // Replay everything the runner has not acked.
    for (_, s) in &st.unacked {
        if ws.send(Message::text(s.clone())).await.is_err() {
            return Outcome::Reconnect { was_welcomed: true };
        }
    }
    let reconnect = || Outcome::Reconnect { was_welcomed: true };
    // Tell the runner what we have so it can replay its side.
    if send_ack(&mut ws, st).await.is_err() {
        return reconnect();
    }

    // ---- steady state
    let mut ack_timer = interval(cfg.ack_interval);
    let mut ping_timer = interval(cfg.ping_interval);
    let mut last_rx = Instant::now();
    // (P4-PRV-2, W18; P4-TERM, W19) Stream/PTY traffic bumps the epoch when the connection
    // is up.
    epoch_tx.send_modify(|e| *e += 1);
    pty_epoch_tx.send_modify(|e| *e += 1);
    loop {
        tokio::select! {
            msg = ws.next() => {
                last_rx = Instant::now();
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        // (P4-PRV-2, W18; P4-TERM, W19) `stream.*`/`pty.*` frames are outside
                        // the seq/ack lineage: they carry `seq: 0, ack: 0`, are never acked
                        // and never replayed. Routing is by the decoded `kind`, not a
                        // substring guess.
                        let unsequenced = serde_json::from_str::<serde_json::Value>(t.as_str())
                            .ok()
                            .and_then(|v| {
                                v.get("kind")
                                    .and_then(|k| k.as_str())
                                    .map(|k| k.starts_with("stream.") || k.starts_with("pty."))
                            })
                            .unwrap_or(false);
                        if unsequenced {
                            let is_pty = serde_json::from_str::<serde_json::Value>(t.as_str())
                                .ok()
                                .and_then(|v| {
                                    v.get("kind").and_then(|k| k.as_str())
                                        .map(|k| k.starts_with("pty."))
                                })
                                .unwrap_or(false);
                            match serde_json::from_str::<Frame<RunnerToSupervisor>>(t.as_str()) {
                                Ok(f) if f.seq == 0 => {
                                    if let Err(e) = f.msg.validate() {
                                        tracing::warn!(error = %e, "invalid unsequenced frame dropped");
                                    } else if is_pty {
                                        if pty_tx.send(PtyIn::Frame(f.msg)).is_err() {
                                            return Outcome::Done;
                                        }
                                    } else if stream_tx.send(StreamIn::Frame(f.msg)).is_err() {
                                        return Outcome::Done;
                                    }
                                }
                                Ok(_) => {
                                    return Outcome::Fatal(ChannelError::Protocol(
                                        "unsequenced frame outside seq 0",
                                    ));
                                }
                                Err(_) => {
                                    return Outcome::Fatal(ChannelError::Protocol(
                                        "bad unsequenced frame",
                                    ));
                                }
                            }
                            continue;
                        }
                        let frame = match serde_json::from_str::<Frame<RunnerToSupervisor>>(t.as_str()) {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::warn!(error = %e, "undecodable frame dropped");
                                continue;
                            }
                        };
                        st.on_ack(frame.ack);
                        if matches!(frame.msg, RunnerToSupervisor::Ack(_)) {
                            continue;
                        }
                        if frame.seq <= st.last_recv {
                            continue; // duplicate (replay)
                        }
                        if frame.seq != st.last_recv + 1 {
                            tracing::warn!(seq = frame.seq, expected = st.last_recv + 1, "sequence gap");
                            return reconnect();
                        }
                        st.last_recv = frame.seq;
                        if let Err(e) = frame.msg.validate() {
                            tracing::warn!(kind = frame.msg.kind(), error = %e, "invalid frame dropped");
                            continue;
                        }
                        if in_tx.send(frame.msg).await.is_err() {
                            return Outcome::Done;
                        }
                    }
                    Some(Ok(Message::Binary(b))) => {
                        // (W18) `[0x01][stream_id: u32 BE][payload ≤ PREVIEW_CHUNK]`;
                        // (P4-TERM, W19) `[0x02][pty_id BE][payload ≤ TERMINAL_CHUNK]`. A bad
                        // header or oversize payload is dropped (a debug line, never content).
                        match decode_binary_data(&b) {
                            Ok(SideData::Stream { id, payload }) => {
                                if stream_tx
                                    .send(StreamIn::Data {
                                        stream_id: id,
                                        payload: payload.to_vec(),
                                    })
                                    .is_err()
                                {
                                    return Outcome::Done;
                                }
                            }
                            Ok(SideData::Pty { id, payload }) => {
                                if pty_tx
                                    .send(PtyIn::Data {
                                        pty_id: id,
                                        payload: payload.to_vec(),
                                    })
                                    .is_err()
                                {
                                    return Outcome::Done;
                                }
                            }
                            Err(why) => {
                                tracing::warn!(why, "bad binary data frame dropped");
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        if ws.flush().await.is_err() { return reconnect(); }
                    }
                    Some(Ok(Message::Close(f))) => {
                        return f.map_or_else(reconnect, |f| close_outcome(u16::from(f.code), true));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return reconnect(),
                }
            }
            msg = out_rx.recv(), if st.unacked_bytes < MAX_UNACKED_BYTES => {
                let Some(msg) = msg else {
                    let _ = ws.close(None).await;
                    return Outcome::Done;
                };
                let seq = st.next_seq;
                let Some(text) = encode(st, seq, msg) else { continue };
                st.next_seq += 1;
                st.ack_sent = st.last_recv;
                st.unacked_bytes += text.len();
                st.unacked.push_back((seq, text.clone()));
                if ws.send(Message::text(text)).await.is_err() { return reconnect(); }
            }
            out = stream_rx.recv() => {
                match out {
                    Some(StreamOut::Text(t)) => {
                        if ws.send(Message::text(t)).await.is_err() { return reconnect(); }
                    }
                    Some(StreamOut::Binary(b)) => {
                        if ws.send(Message::binary(b)).await.is_err() { return reconnect(); }
                    }
                    Some(StreamOut::Flush) => {
                        if ws.flush().await.is_err() { return reconnect(); }
                    }
                    Some(StreamOut::Abort) => {
                        // (W18) Repeated stream violations close the channel with 4400.
                        tracing::warn!("stream protocol violations; closing channel 4400");
                        let _ = ws.close(Some(CloseFrame {
                            code: CloseCode::Library(4400),
                            reason: "stream protocol violation".into(),
                        })).await;
                        return reconnect();
                    }
                    None => {
                        // Every stream sender dropped; keep the control channel alive.
                        continue;
                    }
                }
            }
            out = pty_rx.recv() => {
                // (P4-TERM, W19) Terminal PTY traffic: the same wire shapes, tag `0x02`.
                match out {
                    Some(StreamOut::Text(t)) => {
                        if ws.send(Message::text(t)).await.is_err() { return reconnect(); }
                    }
                    Some(StreamOut::Binary(b)) => {
                        if ws.send(Message::binary(b)).await.is_err() { return reconnect(); }
                    }
                    Some(StreamOut::Flush) => {
                        if ws.flush().await.is_err() { return reconnect(); }
                    }
                    Some(StreamOut::Abort) => {
                        // (W19 on W18) Repeated terminal violations close the channel with 4400.
                        tracing::warn!("pty protocol violations; closing channel 4400");
                        let _ = ws.close(Some(CloseFrame {
                            code: CloseCode::Library(4400),
                            reason: "pty protocol violation".into(),
                        })).await;
                        return reconnect();
                    }
                    None => {
                        // Every PTY sender dropped; keep the control channel alive.
                        continue;
                    }
                }
            }
            _ = ack_timer.tick() => {
                if st.last_recv > st.ack_sent && send_ack(&mut ws, st).await.is_err() {
                    return reconnect();
                }
            }
            _ = ping_timer.tick() => {
                if ws.send(Message::Ping(Vec::new().into())).await.is_err() { return reconnect(); }
            }
            _ = sleep_until(last_rx + cfg.silence_timeout) => {
                tracing::warn!("runner silent; connection considered dead");
                return reconnect();
            }
        }
    }
}

async fn send_ack<S>(ws: &mut S, st: &mut State) -> Result<(), ()>
where
    S: futures::Sink<Message> + Unpin,
{
    let text = encode(st, 0, SupervisorToRunner::Ack(Empty {})).ok_or(())?;
    st.ack_sent = st.last_recv;
    ws.send(Message::text(text)).await.map_err(|_| ())
}

// --------------------------------------------- stream + pty side-channels (W18, W19)

/// One decoded binary side-channel message (`16 §W18` tag `0x01`, `16 §W19` tag `0x02`).
enum SideData<'a> {
    Stream { id: u32, payload: &'a [u8] },
    Pty { id: u32, payload: &'a [u8] },
}

/// Binary data framing: `[tag][id: u32 BE][payload]` — tag `0x01` is stream data
/// (`16 §W18`, payload ≤ `PREVIEW_CHUNK`), tag `0x02` terminal data (`16 §W19`, payload ≤
/// `TERMINAL_CHUNK`), any other tag is unknown.
fn decode_binary_data(b: &[u8]) -> Result<SideData<'_>, &'static str> {
    let Some((&tag, rest)) = b.split_first() else {
        return Err("empty binary data frame");
    };
    if rest.len() < 4 {
        return Err("binary data header too short");
    }
    let id = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
    let payload = &rest[4..];
    match tag {
        0x01 => {
            if payload.len() > aurora_core::limits::PREVIEW_CHUNK {
                return Err("stream data payload too large");
            }
            Ok(SideData::Stream { id, payload })
        }
        0x02 => {
            if payload.len() > aurora_core::limits::TERMINAL_CHUNK {
                return Err("pty data payload too large");
            }
            Ok(SideData::Pty { id, payload })
        }
        _ => Err("unknown binary data tag"),
    }
}

/// Encodes an outgoing unsequenced `stream.*` frame (`seq: 0, ack: 0`).
pub fn encode_stream_frame(msg: SupervisorToRunner) -> Option<String> {
    let frame = Frame {
        seq: 0,
        ack: 0,
        msg,
    };
    if frame.validate().is_err() {
        return None;
    }
    serde_json::to_string(&frame).ok()
}

/// (P4-TERM, W19) Encodes an outgoing unsequenced `pty.*` frame (`seq: 0, ack: 0`); the same
/// frame encoder as [`encode_stream_frame`] — the kinds route by name on the receiving side.
pub fn encode_pty_frame(msg: SupervisorToRunner) -> Option<String> {
    encode_stream_frame(msg)
}

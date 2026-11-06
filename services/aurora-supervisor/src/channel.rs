//! W6 supervisor channel client (`docs/16-wire-protocols.md` §W6).
//!
//! One long-lived task owns the connection lineage: it dials, sends `hello` (bootstrap token the
//! first time, the stored HMAC session afterwards), numbers outgoing frames, keeps unacked frames
//! (bounded to 4 MiB; back-pressure beyond that), replays them after a reconnect from the runner's
//! `resume_from`, applies each incoming `seq` at most once, acks every second, and pings every
//! 20 s (60 s of silence = dead). `ack` frames are not sequenced (`seq` 0).

use std::collections::VecDeque;
use std::time::Duration;

use aurora_protocol::common::{Empty, SecretString, Validate};
use aurora_protocol::supervisor::{
    ACK_INTERVAL, CLOSE_SPENT, CLOSE_UNAUTHORIZED, Frame, Hello, HelloBody, MAX_UNACKED_BYTES,
    PING_INTERVAL, RunnerToSupervisor, SILENCE_TIMEOUT, SessionProof, SupervisorToRunner, Welcome,
};
use aurora_protocol::version::Plane;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval, sleep, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message;

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

pub struct Channel {
    pub sender: ChannelSender,
    /// Runner frames, each delivered exactly once and in order (acks are consumed internally).
    pub inbound: mpsc::Receiver<RunnerToSupervisor>,
    /// Ends only on a fatal rejection (4401/4409) or when every sender is dropped.
    pub task: JoinHandle<Result<(), ChannelError>>,
}

pub fn spawn(cfg: ChannelConfig) -> Channel {
    let (out_tx, out_rx) = mpsc::channel(256);
    let (in_tx, in_rx) = mpsc::channel(256);
    let task = tokio::spawn(run(cfg, out_rx, in_tx));
    Channel {
        sender: ChannelSender(out_tx),
        inbound: in_rx,
        task,
    }
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

async fn run(
    cfg: ChannelConfig,
    mut out_rx: mpsc::Receiver<SupervisorToRunner>,
    in_tx: mpsc::Sender<RunnerToSupervisor>,
) -> Result<(), ChannelError> {
    let mut st = State {
        next_seq: 1,
        ..State::default()
    };
    let mut delay = cfg.backoff_initial;
    loop {
        match connection(&cfg, &mut st, &mut out_rx, &in_tx).await {
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

async fn connection(
    cfg: &ChannelConfig,
    st: &mut State,
    out_rx: &mut mpsc::Receiver<SupervisorToRunner>,
    in_tx: &mpsc::Sender<RunnerToSupervisor>,
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
    loop {
        tokio::select! {
            msg = ws.next() => {
                last_rx = Instant::now();
                match msg {
                    Some(Ok(Message::Text(t))) => {
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

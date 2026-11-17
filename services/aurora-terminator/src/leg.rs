//! Leg 1, terminator → runner (`16 §W20`): one WebSocket per SSH channel to the runner's
//! `:4108` (`Authorization: Bearer auz_…`, the grant the API minted), the first Text message
//! `exec.open`, the runner's `exec.opened`, then `[0x02]` stdin/stdout, `[0x03]` stderr,
//! `exec.eof`, `pty.resize` and `exec.exit`. The runner and the API decide everything about the
//! sandbox; this module only moves bytes. Bytes, commands and grants are never logged.

use std::sync::Arc;
use std::time::Duration;

use aurora_core::limits::TERMINAL_CHUNK;
use aurora_protocol::terminator::ChannelMode;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use russh::ChannelId;
use russh::server::Handle;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::state::Stats;

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OPENED_TIMEOUT: Duration = Duration::from_secs(10);
/// `pty.resize` is coalesced to the latest size and sent at most this often.
const RESIZE_FLUSH: Duration = Duration::from_millis(50);

/// What a channel asked for (`exec.open`).
#[derive(Debug, Clone)]
pub struct LegOpen {
    pub mode: ChannelMode,
    pub command: Option<String>,
    pub tty: Option<(String, u16, u16)>,
    pub env: Vec<(String, String)>,
}

impl LegOpen {
    fn to_json(&self) -> String {
        let mut v = serde_json::json!({"kind": "exec.open", "mode": self.mode.as_str()});
        if let Some(c) = &self.command {
            v["command"] = c.clone().into();
        }
        if let Some((term, cols, rows)) = &self.tty {
            v["tty"] = serde_json::json!({"term": term, "cols": cols, "rows": rows});
        }
        v["env"] = self
            .env
            .iter()
            .map(|(k, x)| (k.clone(), serde_json::Value::from(x.clone())))
            .collect::<serde_json::Map<_, _>>()
            .into();
        v.to_string()
    }
}

/// What the SSH side tells the leg.
#[derive(Debug)]
pub enum ToLeg {
    Data(Bytes),
    Eof,
    Resize(u16, u16),
    /// The channel was closed (or the connection is going away): hang up the leg.
    Close,
}

/// How a leg ended.
#[derive(Debug, PartialEq, Eq)]
pub enum LegEnd {
    /// `exec.exit` then a normal close.
    Exit {
        status: Option<i32>,
        signal: Option<String>,
    },
    /// The leg ended before an exit status (a runner or network failure, a close code ≠ 1000).
    RelayError,
    /// The SSH side closed first.
    ClientClosed,
}

#[derive(Debug)]
pub enum LegError {
    Connect,
    Refused,
}

/// Dials the runner and performs the `exec.open` / `exec.opened` exchange.
pub async fn connect(endpoint: &str, grant: &str, open: &LegOpen) -> Result<Ws, LegError> {
    let mut req = endpoint
        .into_client_request()
        .map_err(|_| LegError::Connect)?;
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {grant}")
            .parse()
            .map_err(|_| LegError::Connect)?,
    );
    let (mut ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| LegError::Connect)?
        .map_err(|_| LegError::Connect)?;
    ws.send(Message::text(open.to_json()))
        .await
        .map_err(|_| LegError::Connect)?;
    let opened = tokio::time::timeout(OPENED_TIMEOUT, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    return serde_json::from_str::<serde_json::Value>(&t)
                        .ok()
                        .is_some_and(|v| v["kind"] == "exec.opened");
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return false,
            }
        }
    })
    .await;
    match opened {
        Ok(true) => Ok(ws),
        _ => Err(LegError::Refused),
    }
}

/// Where the sandbox's bytes go: the SSH channel.
pub struct ChannelSink {
    pub handle: Handle,
    pub channel: ChannelId,
    pub stats: Arc<Stats>,
}

impl ChannelSink {
    async fn stdout(&self, b: Bytes) -> bool {
        self.stats.add_out(b.len());
        self.handle.data(self.channel, b).await.is_ok()
    }
    async fn stderr(&self, b: Bytes) -> bool {
        self.stats.add_out(b.len());
        self.handle.extended_data(self.channel, 1, b).await.is_ok()
    }
}

/// Moves bytes until the leg or the SSH side ends.
pub async fn bridge(mut ws: Ws, mut rx: mpsc::Receiver<ToLeg>, sink: ChannelSink) -> LegEnd {
    let mut exit: Option<(Option<i32>, Option<String>)> = None;
    let mut pending: Option<(u16, u16)> = None;
    let mut flush = tokio::time::interval(RESIZE_FLUSH);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Some(ToLeg::Data(b)) => {
                    sink.stats.add_in(b.len());
                    for chunk in b.chunks(TERMINAL_CHUNK) {
                        let mut f = Vec::with_capacity(chunk.len() + 1);
                        f.push(0x02);
                        f.extend_from_slice(chunk);
                        if ws.send(Message::binary(f)).await.is_err() {
                            return LegEnd::RelayError;
                        }
                    }
                }
                Some(ToLeg::Eof) => {
                    if ws.send(Message::text(r#"{"kind":"exec.eof"}"#)).await.is_err() {
                        return LegEnd::RelayError;
                    }
                }
                Some(ToLeg::Resize(c, r)) => pending = Some((c, r)),
                Some(ToLeg::Close) | None => {
                    let _ = ws.close(None).await;
                    return LegEnd::ClientClosed;
                }
            },
            _ = flush.tick(), if pending.is_some() => {
                if let Some((cols, rows)) = pending.take() {
                    let t = serde_json::json!({"kind": "pty.resize", "cols": cols, "rows": rows});
                    if ws.send(Message::text(t.to_string())).await.is_err() {
                        return LegEnd::RelayError;
                    }
                }
            }
            w = ws.next() => match w {
                Some(Ok(Message::Binary(b))) => {
                    let Some((&tag, payload)) = b.split_first() else {
                        return LegEnd::RelayError;
                    };
                    let payload = Bytes::copy_from_slice(payload);
                    let ok = match tag {
                        0x02 => sink.stdout(payload).await,
                        0x03 => sink.stderr(payload).await,
                        _ => return LegEnd::RelayError,
                    };
                    if !ok {
                        return LegEnd::ClientClosed;
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
                        && v["kind"] == "exec.exit"
                    {
                        exit = Some((
                            v["status"].as_i64().map(|s| s as i32),
                            v["signal"].as_str().map(str::to_owned),
                        ));
                    }
                }
                Some(Ok(Message::Close(c))) => {
                    let normal = c.as_ref().is_none_or(|c| u16::from(c.code) == 1000);
                    return match exit {
                        Some((status, signal)) if normal => LegEnd::Exit { status, signal },
                        _ => LegEnd::RelayError,
                    };
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return LegEnd::RelayError,
            },
        }
    }
}

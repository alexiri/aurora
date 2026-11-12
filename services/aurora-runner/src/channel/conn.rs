//! One supervisor connection: admission, welcome, sequenced frame loop.

use std::sync::Arc;

use aurora_protocol::common::Validate;
use aurora_protocol::runner::ChannelRef;
use aurora_protocol::supervisor::{
    CLOSE_SPENT, CLOSE_UNAUTHORIZED, Frame as WireFrame, Hello, MAX_FRAME, SupervisorToRunner,
    Welcome, WelcomeBody,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::time::{Instant, interval, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::{WebSocketStream, accept_async_with_config};

use super::{ConnMsg, Session, Shared, plane};
use crate::api::ApiError;
use crate::hub::{Frame, HubEvent};

const CLOSE_PROTOCOL: u16 = 1002;
const CLOSE_BAD_DATA: u16 = 1007;
const CLOSE_TRY_LATER: u16 = 1013;

type Ws = WebSocketStream<TcpStream>;

async fn close(ws: &mut Ws, code: u16) {
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: "".into(),
    };
    let _ = ws.send(Message::Close(Some(frame))).await;
}

struct Admitted {
    session: Arc<Session>,
    /// The API was unreachable for `channel/hold`; retry in the background.
    hold_pending: bool,
    /// Held until the connection is attached, so a release cannot slip between hold and attach.
    guard: tokio::sync::OwnedMutexGuard<()>,
}

pub(super) async fn handle(shared: Arc<Shared>, tcp: TcpStream) {
    let cfg = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let Ok(mut ws) = accept_async_with_config(tcp, Some(cfg)).await else {
        return;
    };
    let admitted = match admit(&shared, &mut ws).await {
        Ok(a) => a,
        Err(code) => {
            close(&mut ws, code).await;
            return;
        }
    };
    let Admitted {
        session,
        hold_pending,
        guard,
    } = admitted;
    let attached = session.attach();
    drop(guard);
    // (P4-PRV-2, W18) A fresh channel epoch: streams opened on a previous connection of this
    // (sandbox, boot) are already gone; this just notes the new one.
    if let Some(streams) = shared.streams.get() {
        streams
            .channel_epoch(session.sandbox_id, session.boot_id, true)
            .await;
    }
    let conn_id = attached.conn_id;
    let welcome = Welcome {
        welcome: WelcomeBody {
            session: shared.issue(session.sandbox_id, session.boot_id),
            resume_from: attached.resume_from,
        },
    };
    let sent = match serde_json::to_string(&welcome) {
        Ok(t) => ws.send(Message::text(t)).await.is_ok(),
        Err(_) => false,
    };
    if sent {
        if hold_pending {
            tokio::spawn(retry_hold(shared.clone(), session.clone(), conn_id));
        }
        run(&shared, &session, &mut ws, attached.rx).await;
    }
    if session.detach(conn_id) {
        tracing::info!(sandbox = %session.sandbox_id, "supervisor channel closed");
        // (P4-PRV-2, W18) The channel is gone: every stream of this (sandbox, boot) ends
        // with 4503 (`streams are never resumed`), before the release round-trip.
        if let Some(streams) = shared.streams.get() {
            streams
                .channel_epoch(session.sandbox_id, session.boot_id, false)
                .await;
        }
        if let Some(api) = shared.api() {
            let r = ChannelRef {
                sandbox_id: session.sandbox_id,
                boot_id: session.boot_id,
            };
            let lock = shared.sandbox_lock(session.sandbox_id);
            let session = session.clone();
            tokio::spawn(async move {
                let _g = lock.lock_owned().await;
                // A newer connection was admitted meanwhile: its hold must stand.
                if session.is_connected() {
                    return;
                }
                if let Err(e) = api.channel_release(&r).await {
                    tracing::warn!(error = %e, "channel release failed");
                }
            });
        }
    }
    let _ = ws.close(None).await;
}

/// Authenticate the first message. `Err(code)` is the close code to answer with.
async fn admit(shared: &Shared, ws: &mut Ws) -> Result<Admitted, u16> {
    let first = timeout(shared.settings.hello_timeout, ws.next())
        .await
        .map_err(|_| CLOSE_UNAUTHORIZED)?;
    let Some(Ok(Message::Text(text))) = first else {
        return Err(CLOSE_UNAUTHORIZED);
    };
    let hello: Hello = serde_json::from_str(text.as_str()).map_err(|_| CLOSE_UNAUTHORIZED)?;
    if hello.validate().is_err() || hello.hello.protocol != 1 {
        return Err(CLOSE_UNAUTHORIZED);
    }
    let mut api_rx = shared.api_watch();
    let api = timeout(
        shared.settings.hello_timeout,
        api_rx.wait_for(Option::is_some),
    )
    .await
    .map_err(|_| CLOSE_TRY_LATER)?
    .map_err(|_| CLOSE_TRY_LATER)?
    .clone()
    .ok_or(CLOSE_TRY_LATER)?;

    let (sandbox, boot, from_session) = if let Some(token) = &hello.hello.bootstrap_token {
        match api.claim(token.expose()).await {
            Ok(c) => (c.sandbox_id, c.boot_id, false),
            Err(ApiError::Status { status: 409, .. }) => return Err(CLOSE_SPENT),
            Err(e) if e.retryable() => return Err(CLOSE_TRY_LATER),
            Err(_) => return Err(CLOSE_UNAUTHORIZED),
        }
    } else if let Some(p) = &hello.hello.session {
        if !shared.verify(p) {
            return Err(CLOSE_UNAUTHORIZED);
        }
        (p.sandbox_id, p.boot_id, true)
    } else {
        return Err(CLOSE_UNAUTHORIZED);
    };

    let guard = shared.sandbox_lock(sandbox).lock_owned().await;
    let hold = ChannelRef {
        sandbox_id: sandbox,
        boot_id: boot,
    };
    let hold_pending = match api.channel_hold(&hold).await {
        Ok(()) => false,
        // Boot no longer current / not ours.
        Err(e) if !e.retryable() => return Err(CLOSE_UNAUTHORIZED),
        // API outage: a locally valid session stays connected.
        Err(_) if from_session => true,
        Err(_) => return Err(CLOSE_TRY_LATER),
    };
    // Without the API we cannot tell which of two boots is current: keep the known one.
    let replace = !hold_pending;
    let session = shared
        .session_for(sandbox, boot, replace)
        .ok_or(CLOSE_UNAUTHORIZED)?;
    Ok(Admitted {
        session,
        hold_pending,
        guard,
    })
}

async fn retry_hold(shared: Arc<Shared>, session: Arc<Session>, conn_id: u64) {
    let r = ChannelRef {
        sandbox_id: session.sandbox_id,
        boot_id: session.boot_id,
    };
    while session.is_current(conn_id) {
        tokio::time::sleep(shared.settings.hold_retry).await;
        let Some(api) = shared.api() else { continue };
        match api.channel_hold(&r).await {
            Ok(()) => return,
            Err(e) if e.retryable() => {}
            Err(e) => {
                tracing::warn!(sandbox = %r.sandbox_id, error = %e, "channel hold refused after outage");
                return;
            }
        }
    }
}

async fn run(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    ws: &mut Ws,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ConnMsg>,
) {
    let s = &shared.settings;
    let mut ping = interval(s.ping_interval);
    let mut ack = interval(s.ack_interval);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            msg = ws.next() => {
                last_rx = Instant::now();
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Err(code) = on_text(shared, session, t.as_str()).await {
                            close(ws, code).await;
                            return;
                        }
                    }
                    // (P4-PRV-2, W18) `[0x01][stream_id: u32 BE][payload ≤ PREVIEW_CHUNK]`:
                    // preview-stream data from the supervisor, routed to the listener.
                    // (P4-TERM, W19) `[0x02][pty_id: u32 BE][payload ≤ TERMINAL_CHUNK]`:
                    // terminal data from the supervisor, routed to the same listener.
                    Some(Ok(Message::Binary(b))) => match decode_stream_data(&b) {
                        Ok(DataFrame::Stream(stream_id, payload)) => {
                            shared
                                .stream_data(session.sandbox_id, session.boot_id, stream_id, payload)
                                .await;
                        }
                        Ok(DataFrame::Terminal(pty_id, payload)) => {
                            shared
                                .terminal_data(session.sandbox_id, session.boot_id, pty_id, payload)
                                .await;
                        }
                        Err(why) => {
                            // Not a stream frame (or broken): a 1003 close would end the whole
                            // control channel for one bad preview frame; drop and keep going.
                            tracing::debug!(why, "ignoring non-stream binary message");
                        }
                    },
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                    Some(Ok(_)) => {}
                }
            }
            out = rx.recv() => match out {
                Some(ConnMsg::Text(text)) => {
                    if ws.send(Message::text(text)).await.is_err() {
                        return;
                    }
                }
                Some(ConnMsg::StreamFrame(text)) => {
                    if ws.send(Message::text(text)).await.is_err() {
                        return;
                    }
                }
                Some(ConnMsg::StreamData(data)) => {
                    if ws.send(Message::binary(data)).await.is_err() {
                        return;
                    }
                }
                // Replaced by a newer connection.
                None => {
                    close(ws, 1000).await;
                    return;
                }
            },
            _ = ping.tick() => {
                if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            _ = ack.tick() => {
                if let Some(text) = session.take_ack()
                    && ws.send(Message::text(text)).await.is_err()
                {
                    return;
                }
            }
            () = sleep_until(last_rx + s.silence_timeout) => {
                tracing::info!(sandbox = %session.sandbox_id, "supervisor silent; dropping channel");
                return;
            }
        }
    }
}

#[derive(Deserialize)]
struct Raw {
    seq: u64,
    #[serde(default)]
    ack: u64,
    kind: String,
    #[serde(default)]
    body: Value,
}

/// (P4-PRV-2, W18) Binary data framing: `[0x01][stream_id: u32 BE][payload ≤ PREVIEW_CHUNK]`.
/// (P4-TERM, W19) `[0x02][pty_id: u32 BE][payload ≤ TERMINAL_CHUNK]` rides the same channel
/// beside it; anything else is unknown.
enum DataFrame {
    Stream(u32, Vec<u8>),
    Terminal(u32, Vec<u8>),
}

fn decode_stream_data(b: &[u8]) -> Result<DataFrame, &'static str> {
    let Some((&tag, rest)) = b.split_first() else {
        return Err("empty binary message");
    };
    if tag != 0x01 && tag != 0x02 {
        return Err("unknown binary tag");
    }
    if rest.len() < 4 {
        return Err("stream data header too short");
    }
    let id = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
    let payload = rest[4..].to_vec();
    match tag {
        // Both tags share the chunk bound (TERMINAL_CHUNK == PREVIEW_CHUNK == 16 KiB).
        0x01 => {
            if payload.len() > aurora_core::limits::PREVIEW_CHUNK {
                return Err("stream data payload too large");
            }
            Ok(DataFrame::Stream(id, payload))
        }
        _ => {
            if payload.len() > aurora_core::limits::TERMINAL_CHUNK {
                return Err("terminal data payload too large");
            }
            Ok(DataFrame::Terminal(id, payload))
        }
    }
}

/// Apply one frame at most once. `Err(code)` closes the connection.
async fn on_text(shared: &Arc<Shared>, session: &Arc<Session>, text: &str) -> Result<(), u16> {
    let raw: Raw = serde_json::from_str(text).map_err(|_| CLOSE_BAD_DATA)?;
    // (P4-PRV-2, W18; P4-TERM, W19) Unsequenced `stream.*`/`pty.*` frames bypass the seq/ack
    // lineage entirely: they are never acked, never replayed, and routed straight to the
    // stream listener (which owns both the preview and the terminal legs).
    if raw.seq == 0 && (raw.kind.starts_with("stream.") || raw.kind.starts_with("pty.")) {
        match serde_json::from_str::<WireFrame<SupervisorToRunner>>(text) {
            Ok(f) => match f.validate() {
                Ok(()) => {
                    shared
                        .stream_frame(session.sandbox_id, session.boot_id, f.msg)
                        .await;
                }
                Err(e) => {
                    tracing::warn!(kind = %raw.kind, error = %e, "invalid stream frame dropped");
                }
            },
            Err(e) => {
                tracing::warn!(kind = %raw.kind, error = %e, "undecodable stream frame dropped");
            }
        }
        return Ok(());
    }    session.on_ack(raw.ack);
    if raw.kind == "ack" {
        return Ok(());
    }
    let recv = session.recv_seq();
    if raw.seq <= recv {
        return Ok(()); // replayed; already applied
    }
    // A runner without state for this lineage (restart, other replica) adopts the first seq it
    // sees; the API deduplicates by the deterministic report id (S-07).
    if recv != 0 && raw.seq != recv + 1 {
        return Err(CLOSE_PROTOCOL);
    }
    // A frame that cannot be decoded or fails bounds is dropped, but its seq is consumed.
    match serde_json::from_str::<WireFrame<SupervisorToRunner>>(text) {
        Ok(f) => match f.validate() {
            Ok(()) => dispatch(shared, session, raw.seq, raw.kind, raw.body, f.msg).await,
            Err(e) => {
                tracing::warn!(kind = %raw.kind, error = %e, "invalid supervisor frame dropped");
            }
        },
        Err(e) => {
            tracing::warn!(kind = %raw.kind, error = %e, "undecodable supervisor frame dropped");
        }
    }
    session.set_recv(raw.seq);
    Ok(())
}

async fn dispatch(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    seq: u64,
    kind: String,
    body: Value,
    msg: SupervisorToRunner,
) {
    use SupervisorToRunner as M;
    match msg {
        M::Ack(_) => {}
        M::Other(u) => tracing::warn!(
            kind = u.kind.as_str(),
            "unknown supervisor frame kind dropped"
        ),
        M::ToolCall(call) => plane::spawn_tool_call(shared, session, call),
        M::MemoryWrite(w) => plane::spawn_memory_write(shared, session, w),
        M::FilePart(p) => plane::file_part(session, &p),
        M::FileCommit(c) => plane::spawn_file_commit(shared, session, c),
        _ => {
            let ev = HubEvent {
                sandbox_id: session.sandbox_id,
                boot_id: session.boot_id,
                seq,
                frame: Frame { kind, body },
            };
            let _ = shared.events.send(ev).await;
        }
    }
}

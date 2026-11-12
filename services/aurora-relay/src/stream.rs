//! The stream hop (P4-PRV-2, D-147, SEC-82; `16 §W18` leg 1): one WebSocket per browser
//! connection to the runner's stream listener (`runner_endpoint` from the API's `streams`
//! answer), `Authorization: Bearer aux_…`, no other parameter — the relay cannot name a
//! sandbox or a port.
//!
//! HTTP exchanges ride the same tunnel: the relay serializes the rebuilt request head (and
//! body) into binary frames, the runner writes those bytes to the loopback port and streams
//! the answer back as binary frames (`proxy`). After a `101` the two upgraded pipes are
//! bridged byte-for-byte — the relay never inspects frames after the switch.
//!
//! Open streams are re-validated every heartbeat cycle (`docs/04` step 5): a revoked
//! `(label, session_hash)` pair ends its streams with close code 4403.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::api::StreamsAnswer;
use crate::state::State;

/// Idle timeout of the runner leg (`PREVIEW_WS_IDLE_TIMEOUT`); a stream that sends nothing
/// for this long is closed (the runner's own idle close agrees).
const IDLE: Duration = aurora_core::limits::PREVIEW_WS_IDLE_TIMEOUT;
/// Total lifetime of one stream.
const LIFETIME: Duration = aurora_core::limits::PREVIEW_STREAM_LIFETIME;
/// The runner's connect timeout (`PREVIEW_CONNECT_TIMEOUT`): the dial either completes or
/// the relay answers 504.
const CONNECT: Duration = aurora_core::limits::PREVIEW_CONNECT_TIMEOUT;
/// Largest binary frame sent to the runner (`PREVIEW_CHUNK`; the runner refuses more).
const CHUNK: usize = aurora_core::limits::PREVIEW_CHUNK;
/// Per-stream queue depths (the credit window bounds the in-flight bytes anyway).
const DATA_Q: usize = 16;

/// The identity of one open stream: one browser session of one label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamKey {
    pub label: String,
    pub session_hash: String,
}

/// The browser-side half of one open stream (cloneable; dropping the last copy detaches
/// the pump, which closes the leg).
#[derive(Clone)]
pub struct LiveTx {
    cmd_tx: mpsc::Sender<Vec<u8>>,
    revoked_rx: watch::Receiver<bool>,
}

impl LiveTx {
    /// Sends bytes toward the app, split into leg-sized binary frames (`PREVIEW_CHUNK`).
    /// `false` = the stream is gone (the pump dropped it).
    pub async fn to_app(&self, mut data: &[u8]) -> bool {
        while !data.is_empty() {
            let n = data.len().min(CHUNK);
            let (chunk, rest) = data.split_at(n);
            data = rest;
            if self.cmd_tx.send(chunk.to_vec()).await.is_err() {
                return false;
            }
        }
        true
    }

    /// Whether the validate cycle revoked this stream (`docs/04` step 5).
    pub fn revoked(&self) -> bool {
        *self.revoked_rx.borrow()
    }

    /// Wakes when `revoke` flags this stream (the pumps select on it).
    pub fn watch(&self) -> watch::Receiver<bool> {
        self.revoked_rx.clone()
    }
}

/// One open stream as the exchange layer sees it.
pub struct LiveStream {
    tx: LiveTx,
    from_app_rx: mpsc::Receiver<Bytes>,
}

impl LiveStream {
    /// The cloneable send half (the 101 bridge needs it in two tasks).
    pub fn tx(&self) -> &LiveTx {
        &self.tx
    }

    /// The next chunk from the app (`None` = the stream ended).
    pub async fn from_app(&mut self) -> Option<Bytes> {
        self.from_app_rx.recv().await
    }

    /// Splits the stream into the send half and the receive half (the 101 bridge).
    pub fn into_parts(self) -> (LiveTx, mpsc::Receiver<Bytes>) {
        (self.tx, self.from_app_rx)
    }
}

/// The table of open streams: what `validate` can close and `report` can count.
#[derive(Default)]
pub struct Streams {
    inner: std::sync::Mutex<Table>,
}

#[derive(Default)]
struct Table {
    /// `(label, session_hash) -> open count` plus totals since start.
    open: std::collections::HashMap<StreamKey, u32>,
    /// Every open stream's revocation signal, per key (pumps deregister on exit).
    active: std::collections::HashMap<StreamKey, Vec<Arc<watch::Sender<bool>>>>,
    total: u64,
    bytes_to_app: u64,
    bytes_from_app: u64,
}

impl Streams {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn add(&self, key: &StreamKey) -> (Arc<watch::Sender<bool>>, watch::Receiver<bool>) {
        let (tx, rx) = watch::channel(false);
        let tx = Arc::new(tx);
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        *g.open.entry(key.clone()).or_insert(0) += 1;
        g.total += 1;
        g.active.entry(key.clone()).or_default().push(tx.clone());
        metrics::gauge!("aurora_relay_streams_open").set(g.open.values().sum::<u32>() as f64);
        (tx, rx)
    }

    fn remove(
        &self,
        key: &StreamKey,
        sig: &Arc<watch::Sender<bool>>,
        bytes_to_app: u64,
        bytes_from_app: u64,
    ) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let std::collections::hash_map::Entry::Occupied(mut e) = g.open.entry(key.clone()) {
            *e.get_mut() -= 1;
            if *e.get() == 0 {
                e.remove();
            }
        }
        if let Some(v) = g.active.get_mut(key) {
            v.retain(|s| !Arc::ptr_eq(s, sig));
            if v.is_empty() {
                g.active.remove(key);
            }
        }
        g.bytes_to_app += bytes_to_app;
        g.bytes_from_app += bytes_from_app;
        metrics::gauge!("aurora_relay_streams_open").set(g.open.values().sum::<u32>() as f64);
        metrics::counter!("aurora_relay_stream_bytes_total", "direction" => "to_app")
            .increment(bytes_to_app);
        metrics::counter!("aurora_relay_stream_bytes_total", "direction" => "from_app")
            .increment(bytes_from_app);
    }

    pub fn open_count(&self) -> u32 {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .open
            .values()
            .sum()
    }

    /// The `(label, session_hash)` pairs with at least one open stream (the validate input).
    pub fn live_pairs(&self) -> Vec<(String, String)> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .open
            .keys()
            .map(|k| (k.label.clone(), k.session_hash.clone()))
            .collect()
    }

    /// Closes every stream of the revoked pairs (the validate cycle): the pumps observe the
    /// flag and close their legs with 4403 (`docs/04` step 5).
    pub fn revoke(&self, revoked: &[(String, String)]) {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut n = 0;
        for (k, sigs) in &g.active {
            if !revoked
                .iter()
                .any(|(l, h)| *l == k.label && *h == k.session_hash)
            {
                continue;
            }
            for s in sigs {
                let _ = s.send(true);
                n += 1;
            }
        }
        if n > 0 {
            metrics::counter!("aurora_relay_stream_closed_total", "reason" => "revoked")
                .increment(n);
        }
    }
}

/// Opens the leg-1 WebSocket to the runner (`wss://…/v1/preview-stream`, bearer aux_).
pub type LegWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn dial(_state: &State, sa: &StreamsAnswer) -> Result<LegWs, u16> {
    let mut req = sa
        .runner_endpoint
        .as_str()
        .into_client_request()
        .map_err(|_| 502u16)?;
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", sa.grant).parse().map_err(|_| 502u16)?,
    );
    let started = std::time::Instant::now();
    let endpoint = redact_endpoint(req.uri());
    let r = tokio::time::timeout(CONNECT, tokio_tungstenite::connect_async(req)).await;
    match r {
        // A slow (but successful) dial is worth seeing in metrics.
        Ok(Ok((ws, _))) => {
            metrics::histogram!("aurora_relay_stream_dial_seconds").record(started.elapsed());
            Ok(ws)
        }
        // The endpoint is malformed (the API mints it; ws/wss and a host are required).
        Ok(Err(e)) => {
            tracing::warn!(error = %e, endpoint, "runner stream dial failed");
            metrics::counter!("aurora_relay_stream_dial_errors_total", "kind" => "connect")
                .increment(1);
            Err(502)
        }
        // The dial did not complete within the budget: dropped SYN (NetworkPolicy) or a
        // peer that accepts TCP but never answers the handshake.
        Err(_) => {
            tracing::warn!(endpoint, budget = ?CONNECT, "runner stream dial timed out");
            metrics::counter!("aurora_relay_stream_dial_errors_total", "kind" => "timeout")
                .increment(1);
            Err(504)
        }
    }
}

/// `scheme://host:port` — the endpoint without path/query (nothing secret rides the URL:
/// the grant is the `Authorization` header, but the full endpoint is not for logs either).
fn redact_endpoint(uri: &http::Uri) -> String {
    match (uri.scheme_str(), uri.authority()) {
        (Some(s), Some(a)) => format!("{s}://{a}"),
        _ => "unparseable".to_owned(),
    }
}

/// Splits a freshly dialed leg-1 WebSocket into a [`LiveStream`] and drives its pump:
/// bytes sent through [`LiveTx::to_app`] go to the app, bytes from the app come back
/// through [`LiveStream::from_app`]; revocation (`Streams::revoke`), idle and lifetime
/// close the leg (4403 / 4504), and the leg is closed properly when both halves drop.
pub(crate) fn run_leg(streams: Arc<Streams>, key: StreamKey, ws: LegWs) -> LiveStream {
    let (sig, revoked_rx) = streams.add(&key);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(DATA_Q);
    let (from_app_tx, from_app_rx) = mpsc::channel::<Bytes>(DATA_Q);
    let pump_rx = revoked_rx.clone();
    tokio::spawn(async move {
        let mut pump_rx = pump_rx;
        let (mut sink, mut source) = ws.split();
        let mut bytes_to_app: u64 = 0;
        let mut bytes_from_app: u64 = 0;
        #[expect(unused_assignments)]
        let mut reason = "closed";
        let mut close_code: u16 = 1000;
        tokio::pin! {
            let idle = tokio::time::sleep(IDLE);
            let lifetime = tokio::time::sleep(LIFETIME);
        }
        let mut ping = tokio::time::interval(Duration::from_secs(20));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut revoked = false;
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(b) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE);
                        bytes_to_app += b.len() as u64;
                        if sink.send(Message::binary(b)).await.is_err() {
                            close_code = 1000;
                            reason = "closed";
                            break;
                        }
                    }
                    None => {
                        // All `LiveTx` dropped: the exchange (or bridge) is done.
                        reason = "detached";
                        break;
                    }
                },
                msg = source.next() => match msg {
                    Some(Ok(Message::Binary(b))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE);
                        bytes_from_app += b.len() as u64;
                        if from_app_tx.send(b).await.is_err() {
                            reason = "closed";
                            break;
                        }
                    }
                    Some(Ok(Message::Close(c))) => {
                        close_code = c.as_ref().map(|f| u16::from(f.code)).unwrap_or(1000);
                        reason = if close_code == 4403 { "revoked" } else { "eof" };
                        break;
                    }
                    Some(Ok(Message::Text(_) | Message::Pong(_) | Message::Ping(_) | Message::Frame(_))) => {}
                    Some(Err(_)) | None => {
                        reason = "closed";
                        break;
                    }
                },
                _ = pump_rx.changed() => {
                    if *pump_rx.borrow() {
                        revoked = true;
                        close_code = 4403;
                        reason = "revoked";
                        break;
                    }
                }
                _ = &mut idle => { close_code = 4504; reason = "idle"; break; }
                _ = &mut lifetime => { close_code = 4504; reason = "lifetime"; break; }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        reason = "closed";
                        break;
                    }
                }
            }
        }
        if revoked {
            let _ = sink
                .send(Message::Close(Some(
                    tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: 4403.into(),
                        reason: "revoked".into(),
                    },
                )))
                .await;
        } else {
            let _ = sink.send(Message::Close(None)).await;
        }
        tracing::debug!(
            reason,
            close_code,
            bytes_to_app,
            bytes_from_app,
            "relay: leg pump done"
        );
        drop(from_app_tx);
        streams.remove(&key, &sig, bytes_to_app, bytes_from_app);
        metrics::counter!("aurora_relay_stream_closed_total", "reason" => reason).increment(1);
        let _ = close_code;
    });
    LiveStream {
        tx: LiveTx {
            cmd_tx,
            revoked_rx: revoked_rx.clone(),
        },
        from_app_rx,
    }
}

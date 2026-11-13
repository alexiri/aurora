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
use crate::limits::Timeouts;
use crate::state::State;

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
    upgraded: Arc<Upgraded>,
}

/// Whether the stream switched protocols (a `101`): an upgraded stream idles out after
/// `Timeouts::idle_ws`, a plain HTTP one after `Timeouts::idle_http`.
struct Upgraded {
    flag: std::sync::atomic::AtomicBool,
    wake: tokio::sync::Notify,
}

impl LiveTx {
    /// The app answered `101`: from now on the longer upgraded idle limit applies.
    pub fn mark_upgraded(&self) {
        self.upgraded
            .flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.upgraded.wake.notify_one();
    }

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

/// Relayed bytes are handed to the per-share report counter in batches of at least this many
/// (per stream), and the rest when the stream ends: the table's lock is not taken per chunk.
const BYTES_FLUSH: u64 = 64 * 1024;

/// Which concurrent-stream cap refused a reservation (`FIX-072`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotRefused {
    Share,
    User,
}

impl SlotRefused {
    /// The `aurora_relay_refused_total{reason}` label value (no ids in labels).
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Share => "share_streams",
            Self::User => "user_streams",
        }
    }
}

/// A reserved concurrent-stream slot of one share and one user. RAII: every exit path of the
/// exchange (client abort, upstream error, a finished pump) releases it by dropping it.
pub struct Slot {
    streams: Arc<Streams>,
    share: uuid::Uuid,
    user: String,
}

impl Slot {
    pub fn share(&self) -> uuid::Uuid {
        self.share
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.streams.release(self.share, &self.user);
    }
}

/// The table of open streams: what `validate` can close and `report` can count.
pub struct Streams {
    inner: std::sync::Mutex<Table>,
    per_share_max: u32,
    per_user_max: u32,
    timeouts: Timeouts,
}

#[derive(Default)]
struct Table {
    /// Reserved + open streams per share and per user (the per-replica caps).
    per_share: std::collections::HashMap<uuid::Uuid, u32>,
    per_user: std::collections::HashMap<String, u32>,
    /// Relayed bytes not yet reported to the API (`to_app`, `from_app`) per share.
    pending: std::collections::HashMap<uuid::Uuid, (u64, u64)>,
    /// `(label, session_hash) -> open count` plus totals since start.
    open: std::collections::HashMap<StreamKey, u32>,
    /// Every open stream's revocation signal, per key (pumps deregister on exit).
    active: std::collections::HashMap<StreamKey, Vec<Arc<watch::Sender<bool>>>>,
    total: u64,
    bytes_to_app: u64,
    bytes_from_app: u64,
}

impl Streams {
    /// The spec's caps: `PREVIEW_STREAMS_PER_SHARE` and `PREVIEW_STREAMS_PER_USER`.
    pub fn new() -> Arc<Self> {
        Self::with_limits(
            aurora_core::limits::PREVIEW_STREAMS_PER_SHARE,
            aurora_core::limits::PREVIEW_STREAMS_PER_USER,
        )
    }

    /// Explicit caps (`relay.max_streams_per_user` may lower the per-user one).
    pub fn with_limits(per_share_max: u32, per_user_max: u32) -> Arc<Self> {
        Self::with_timeouts(per_share_max, per_user_max, Timeouts::spec())
    }

    /// Explicit caps and time limits (tests shorten the latter; production never does).
    pub fn with_timeouts(per_share_max: u32, per_user_max: u32, timeouts: Timeouts) -> Arc<Self> {
        Arc::new(Self {
            inner: std::sync::Mutex::new(Table::default()),
            per_share_max,
            per_user_max,
            timeouts,
        })
    }

    /// The stream and header time limits in force.
    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// Reserves one concurrent stream of `share` for `user` (a user id, or the session hash
    /// when the API named none), or says which cap is full.
    pub fn reserve(self: &Arc<Self>, share: uuid::Uuid, user: &str) -> Result<Slot, SlotRefused> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if g.per_share.get(&share).copied().unwrap_or(0) >= self.per_share_max {
            return Err(SlotRefused::Share);
        }
        if g.per_user.get(user).copied().unwrap_or(0) >= self.per_user_max {
            return Err(SlotRefused::User);
        }
        *g.per_share.entry(share).or_insert(0) += 1;
        *g.per_user.entry(user.to_owned()).or_insert(0) += 1;
        Ok(Slot {
            streams: self.clone(),
            share,
            user: user.to_owned(),
        })
    }

    fn release(&self, share: uuid::Uuid, user: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let std::collections::hash_map::Entry::Occupied(mut e) = g.per_share.entry(share) {
            *e.get_mut() = e.get().saturating_sub(1);
            if *e.get() == 0 {
                e.remove();
            }
        }
        if let std::collections::hash_map::Entry::Occupied(mut e) =
            g.per_user.entry(user.to_owned())
        {
            *e.get_mut() = e.get().saturating_sub(1);
            if *e.get() == 0 {
                e.remove();
            }
        }
    }

    /// Streams currently reserved for `share` (tests, metrics).
    pub fn share_count(&self, share: uuid::Uuid) -> u32 {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .per_share
            .get(&share)
            .copied()
            .unwrap_or(0)
    }

    fn add_bytes(&self, share: uuid::Uuid, to_app: u64, from_app: u64) {
        if to_app + from_app == 0 {
            return;
        }
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let e = g.pending.entry(share).or_insert((0, 0));
        e.0 += to_app;
        e.1 += from_app;
    }

    /// Takes the bytes relayed since the last report, per share, as report entries (at most
    /// 500 per call, the API's batch bound; the rest stays pending).
    pub fn drain_report(&self) -> Vec<crate::api::ReportShare> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let ids: Vec<uuid::Uuid> = g.pending.keys().copied().take(500).collect();
        ids.into_iter()
            .filter_map(|id| {
                let (to_app, from_app) = g.pending.remove(&id)?;
                Some(crate::api::ReportShare {
                    share_id: id,
                    streams_open: i32::try_from(g.per_share.get(&id).copied().unwrap_or(0))
                        .unwrap_or(i32::MAX),
                    streams_total: 0,
                    bytes_to_app: to_app,
                    bytes_from_app: from_app,
                    last_request_at: None,
                })
            })
            .collect()
    }

    /// Puts a report whose delivery failed back, so its bytes are counted on the next try.
    pub fn restore_report(&self, shares: &[crate::api::ReportShare]) {
        for s in shares {
            self.add_bytes(s.share_id, s.bytes_to_app, s.bytes_from_app);
        }
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

/// Hands the bytes moved since the last flush to the share's report counter once at least
/// `BYTES_FLUSH` accumulated (or `last`): the quota overshoot per stream is below that.
fn flush(
    streams: &Streams,
    slot: &Slot,
    to_app: &u64,
    from_app: &u64,
    flushed: &mut (u64, u64),
    last: bool,
) {
    let (dt, df) = (to_app - flushed.0, from_app - flushed.1);
    if dt + df >= BYTES_FLUSH || (last && dt + df > 0) {
        streams.add_bytes(slot.share(), dt, df);
        *flushed = (*to_app, *from_app);
    }
}

/// Splits a freshly dialed leg-1 WebSocket into a [`LiveStream`] and drives its pump:
/// bytes sent through [`LiveTx::to_app`] go to the app, bytes from the app come back
/// through [`LiveStream::from_app`]; revocation (`Streams::revoke`), idle and lifetime
/// close the leg (4403 / 4504), and the leg is closed properly when both halves drop.
pub(crate) fn run_leg(streams: Arc<Streams>, key: StreamKey, ws: LegWs, slot: Slot) -> LiveStream {
    let (sig, revoked_rx) = streams.add(&key);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(DATA_Q);
    let (from_app_tx, from_app_rx) = mpsc::channel::<Bytes>(DATA_Q);
    let pump_rx = revoked_rx.clone();
    let upgraded = Arc::new(Upgraded {
        flag: std::sync::atomic::AtomicBool::new(false),
        wake: tokio::sync::Notify::new(),
    });
    let pump_up = upgraded.clone();
    let tm = streams.timeouts;
    tokio::spawn(async move {
        let upgraded = pump_up;
        // The idle limit that applies now (plain HTTP until a `101`).
        let idle_len = || {
            if upgraded.flag.load(std::sync::atomic::Ordering::Relaxed) {
                tm.idle_ws
            } else {
                tm.idle_http
            }
        };
        let mut pump_rx = pump_rx;
        let (mut sink, mut source) = ws.split();
        let mut bytes_to_app: u64 = 0;
        let mut bytes_from_app: u64 = 0;
        // Bytes already handed to the per-share report counter (to_app, from_app).
        let mut flushed: (u64, u64) = (0, 0);
        #[expect(unused_assignments)]
        let mut reason = "closed";
        let mut close_code: u16 = 1000;
        tokio::pin! {
            let idle = tokio::time::sleep(idle_len());
            let lifetime = tokio::time::sleep(tm.lifetime);
        }
        let mut ping = tokio::time::interval(Duration::from_secs(20));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut revoked = false;
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(b) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + idle_len());
                        bytes_to_app += b.len() as u64;
                        flush(&streams, &slot, &bytes_to_app, &bytes_from_app, &mut flushed, false);
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
                        idle.as_mut().reset(tokio::time::Instant::now() + idle_len());
                        bytes_from_app += b.len() as u64;
                        flush(&streams, &slot, &bytes_to_app, &bytes_from_app, &mut flushed, false);
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
                () = upgraded.wake.notified() => {
                    // The `101` arrived: the stream now idles on the upgraded limit.
                    idle.as_mut().reset(tokio::time::Instant::now() + idle_len());
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
        flush(
            &streams,
            &slot,
            &bytes_to_app,
            &bytes_from_app,
            &mut flushed,
            true,
        );
        streams.remove(&key, &sig, bytes_to_app, bytes_from_app);
        drop(slot);
        metrics::counter!("aurora_relay_stream_closed_total", "reason" => reason).increment(1);
        let _ = close_code;
    });
    LiveStream {
        tx: LiveTx {
            cmd_tx,
            revoked_rx: revoked_rx.clone(),
            upgraded,
        },
        from_app_rx,
    }
}

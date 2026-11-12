//! Supervisor control-channel server (W6, `docs/16 §W6`, `docs/11 §The control channel`).
//!
//! One WebSocket listener (port 4105). Admission by bootstrap token (API claim) or HMAC
//! session; sequenced frames with per-(sandbox, boot) replay state that survives reconnects
//! within the process. Identity (`sandbox_id`/`boot_id`) always comes from the authenticated
//! channel, never from a frame.

mod conn;
mod plane;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::common::SecretString;
use aurora_protocol::runner::ChannelRef;
use aurora_protocol::supervisor::{
    ACK_INTERVAL, MAX_FRAME, MAX_UNACKED_BYTES, PING_INTERVAL, RunnerToSupervisor, SILENCE_TIMEOUT,
    SessionProof,
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, watch};
use uuid::Uuid;

use crate::api::ApiClient;
use crate::hub::{ChannelHub, Frame, HubError, HubEvent};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct ChannelSettings {
    pub session_key: [u8; 32],
    pub hello_timeout: Duration,
    pub ping_interval: Duration,
    pub silence_timeout: Duration,
    pub ack_interval: Duration,
    /// Interval between `channel/hold` retries while the API is unreachable.
    pub hold_retry: Duration,
    pub max_unacked_bytes: usize,
}

impl ChannelSettings {
    pub fn new(session_key: [u8; 32]) -> Self {
        Self {
            session_key,
            hello_timeout: Duration::from_secs(10),
            ping_interval: PING_INTERVAL,
            silence_timeout: SILENCE_TIMEOUT,
            ack_interval: ACK_INTERVAL,
            hold_retry: Duration::from_secs(5),
            max_unacked_bytes: MAX_UNACKED_BYTES,
        }
    }
}

/// A partially received outbound file.
#[derive(Default)]
pub(crate) struct Upload {
    pub next: u32,
    pub buf: Vec<u8>,
    pub failed: Option<&'static str>,
}

struct Conn {
    id: u64,
    tx: mpsc::UnboundedSender<ConnMsg>,
}

/// One message on the current W6 connection: a sequenced replay-buffered text frame, or
/// (P4-PRV-2, W18) unsequenced preview-stream traffic.
pub(crate) enum ConnMsg {
    Text(String),
    /// `stream.*` frame with `seq: 0` (never buffered, never acked).
    StreamFrame(String),
    /// Binary data `[0x01][stream_id BE][payload]`.
    StreamData(Vec<u8>),
}

#[derive(Default)]
pub(crate) struct State {
    /// Last supervisor seq applied (our `resume_from`).
    recv_seq: u64,
    send_seq: u64,
    /// Highest `recv_seq` already acknowledged on the wire.
    ack_sent: u64,
    unacked: VecDeque<(u64, String)>,
    unacked_bytes: usize,
    conn: Option<Conn>,
    next_conn: u64,
    pub uploads: HashMap<String, Upload>,
}

/// Replay/sequence state of one boot's channel lineage.
pub(crate) struct Session {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    max_unacked: usize,
    st: Mutex<State>,
}

pub(crate) struct Attached {
    pub conn_id: u64,
    pub resume_from: u64,
    pub rx: mpsc::UnboundedReceiver<ConnMsg>,
}

impl Session {
    fn new(sandbox_id: Uuid, boot_id: Uuid, max_unacked: usize) -> Self {
        Self {
            sandbox_id,
            boot_id,
            max_unacked,
            st: Mutex::default(),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.st
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.lock().conn.is_some()
    }

    pub(crate) fn recv_seq(&self) -> u64 {
        self.lock().recv_seq
    }

    pub(crate) fn set_recv(&self, seq: u64) {
        self.lock().recv_seq = seq;
    }

    pub(crate) fn on_ack(&self, ack: u64) {
        let mut st = self.lock();
        while let Some((seq, text)) = st.unacked.front() {
            if *seq > ack {
                break;
            }
            let n = text.len();
            st.unacked_bytes -= n;
            st.unacked.pop_front();
        }
    }

    /// Queue a sequenced frame. `need_conn` frames fail while disconnected; others wait
    /// in the replay buffer for the next connection.
    pub(crate) fn push(&self, kind: &str, body: Value, need_conn: bool) -> Result<(), HubError> {
        let mut st = self.lock();
        if need_conn && st.conn.is_none() {
            return Err(HubError::NotConnected);
        }
        let seq = st.send_seq + 1;
        let text =
            json!({ "seq": seq, "ack": st.recv_seq, "kind": kind, "body": body }).to_string();
        if text.len() > MAX_FRAME {
            return Err(HubError::TooLarge);
        }
        if st.unacked_bytes + text.len() > self.max_unacked {
            return Err(HubError::Backlog);
        }
        st.send_seq = seq;
        st.ack_sent = st.recv_seq;
        st.unacked_bytes += text.len();
        st.unacked.push_back((seq, text.clone()));
        if let Some(c) = &st.conn {
            let _ = c.tx.send(ConnMsg::Text(text));
        }
        Ok(())
    }

    /// (P4-PRV-2, W18) One unsequenced `stream.*` frame: `seq: 0, ack: 0`, never buffered for
    /// replay, never acked, never back-pressured by the replay buffer. Fails while
    /// disconnected (streams die with the channel anyway).
    pub(crate) fn push_stream_frame(&self, frame_text: String) -> Result<(), HubError> {
        let st = self.lock();
        match &st.conn {
            Some(c) => {
                c.tx.send(ConnMsg::StreamFrame(frame_text))
                    .map_err(|_| HubError::NotConnected)
            }
            None => Err(HubError::NotConnected),
        }
    }

    /// (P4-PRV-2, W18) One binary data message to the supervisor.
    pub(crate) fn push_stream_data(&self, data: Vec<u8>) -> Result<(), HubError> {
        let st = self.lock();
        match &st.conn {
            Some(c) => {
                c.tx.send(ConnMsg::StreamData(data))
                    .map_err(|_| HubError::NotConnected)
            }
            None => Err(HubError::NotConnected),
        }
    }

    /// Unsequenced `ack` frame when something is unacknowledged.
    pub(crate) fn take_ack(&self) -> Option<String> {
        let mut st = self.lock();
        if st.recv_seq <= st.ack_sent {
            return None;
        }
        st.ack_sent = st.recv_seq;
        Some(json!({ "seq": 0, "ack": st.recv_seq, "kind": "ack", "body": {} }).to_string())
    }

    /// Make a new connection current (dropping any previous one) and queue the replay.
    pub(crate) fn attach(&self) -> Attached {
        let mut st = self.lock();
        let (tx, rx) = mpsc::unbounded_channel();
        for (_, text) in &st.unacked {
            let _ = tx.send(ConnMsg::Text(text.clone()));
        }
        st.next_conn += 1;
        let conn_id = st.next_conn;
        st.conn = Some(Conn { id: conn_id, tx });
        Attached {
            conn_id,
            resume_from: st.recv_seq,
            rx,
        }
    }

    /// True if `conn_id` was still the current connection.
    pub(crate) fn detach(&self, conn_id: u64) -> bool {
        let mut st = self.lock();
        if st.conn.as_ref().is_some_and(|c| c.id == conn_id) {
            st.conn = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn is_current(&self, conn_id: u64) -> bool {
        self.lock().conn.as_ref().is_some_and(|c| c.id == conn_id)
    }
}

pub(crate) struct Shared {
    pub settings: ChannelSettings,
    api: watch::Sender<Option<Arc<ApiClient>>>,
    sessions: Mutex<HashMap<Uuid, Arc<Session>>>,
    /// Serializes hold+attach and release per sandbox.
    locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    pub events: mpsc::Sender<HubEvent>,
    events_rx: Mutex<Option<mpsc::Receiver<HubEvent>>>,
    /// Bounds concurrent plane calls (tool/memory/upload).
    pub plane: Semaphore,
    /// (P4-PRV-2, W18) The relay-facing stream listener, when opened; supervisors' stream
    /// traffic is routed here.
    pub streams: streams_cell::StreamsCell,
    /// (P4-PRV-2, W18) Set by the runner when the API reported `previews.enabled` at
    /// registration: the stream listener's port opens only then.
    previews_on: watch::Sender<bool>,
}

/// Late-bound [`StreamListener`] reference (the listener needs the `ChannelServer`, the
/// server needs the listener; a plain lock avoids a dependency cycle). Set once at startup,
/// before the runner loop opens the port.
pub(crate) mod streams_cell {
    use std::sync::{Arc, Mutex};

    pub(crate) struct StreamsCell(
        Mutex<Option<Arc<super::super::stream_listener::StreamListener>>>,
    );

    impl StreamsCell {
        pub(crate) fn empty() -> Self {
            Self(Mutex::new(None))
        }
        pub(crate) fn set(&self, l: Arc<super::super::stream_listener::StreamListener>) {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(l);
        }
        pub(crate) fn get(&self) -> Option<Arc<super::super::stream_listener::StreamListener>> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }
}

impl Shared {
    fn sessions(&self) -> MutexGuard<'_, HashMap<Uuid, Arc<Session>>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn sandbox_lock(&self, sandbox: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(sandbox)
            .or_default()
            .clone()
    }

    pub(crate) fn api(&self) -> Option<Arc<ApiClient>> {
        self.api.borrow().clone()
    }

    pub(crate) fn api_watch(&self) -> watch::Receiver<Option<Arc<ApiClient>>> {
        self.api.subscribe()
    }

    /// (P4-PRV-2, W18) Routes one unsequenced `stream.*` frame to the stream listener.
    pub(crate) async fn stream_frame(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        frame: aurora_protocol::supervisor::SupervisorToRunner,
    ) {
        if let Some(l) = self.streams.get() {
            l.on_supervisor_frame(sandbox, boot, frame).await;
        }
    }

    /// (P4-PRV-2, W18) Routes one binary data message to the stream listener.
    pub(crate) async fn stream_data(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        stream_id: u32,
        payload: Vec<u8>,
    ) {
        if let Some(l) = self.streams.get() {
            l.supervisor_data(sandbox, boot, stream_id, payload).await;
        }
    }

    /// (P4-TERM, W19) Routes one terminal data message (tag `0x02`) to the stream listener.
    pub(crate) async fn terminal_data(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        pty_id: u32,
        payload: Vec<u8>,
    ) {
        if let Some(l) = self.streams.get() {
            l.supervisor_terminal(sandbox, boot, pty_id, payload).await;
        }
    }

    /// Session for `(sandbox, boot)`. A different boot of the same sandbox replaces the old
    /// state when `replace` is set; otherwise `None` (boot currency cannot be established).
    pub(crate) fn session_for(
        &self,
        sandbox: Uuid,
        boot: Uuid,
        replace: bool,
    ) -> Option<Arc<Session>> {
        let mut map = self.sessions();
        match map.get(&sandbox) {
            Some(s) if s.boot_id == boot => Some(s.clone()),
            Some(_) if !replace => None,
            _ => {
                let s = Arc::new(Session::new(sandbox, boot, self.settings.max_unacked_bytes));
                map.insert(sandbox, s.clone());
                Some(s)
            }
        }
    }

    fn mac(&self, sandbox: Uuid, boot: Uuid, issued_at: i64) -> HmacSha256 {
        // Any key length is valid for HMAC; a failure here is impossible.
        let mut m = HmacSha256::new_from_slice(&self.settings.session_key)
            .unwrap_or_else(|_| unreachable!("hmac accepts any key length"));
        m.update(sandbox.as_bytes());
        m.update(boot.as_bytes());
        m.update(&issued_at.to_be_bytes());
        m
    }

    /// `HMAC-SHA256(key, sandbox_id ‖ boot_id ‖ issued_at)`, hex encoded.
    pub(crate) fn issue(&self, sandbox: Uuid, boot: Uuid) -> SessionProof {
        let issued_at = chrono::Utc::now().timestamp();
        let mac = hex::encode(self.mac(sandbox, boot, issued_at).finalize().into_bytes());
        SessionProof {
            sandbox_id: sandbox,
            boot_id: boot,
            issued_at,
            mac: SecretString::new(mac),
        }
    }

    pub(crate) fn verify(&self, p: &SessionProof) -> bool {
        hex::decode(p.mac.expose()).is_ok_and(|raw| {
            self.mac(p.sandbox_id, p.boot_id, p.issued_at)
                .verify_slice(&raw)
                .is_ok()
        })
    }
}

/// The W6 server and the runner's [`ChannelHub`].
#[derive(Clone)]
pub struct ChannelServer(Arc<Shared>);

impl ChannelServer {
    /// (P4-PRV-2, W18) The API client when attached (`attach_api`); the stream listener uses it
    /// for the advisory `preview-streams/closed` accounting call.
    pub fn api(&self) -> Option<Arc<ApiClient>> {
        self.0.api()
    }

    /// (P4-PRV-2, W18) Installs the stream listener; the channel pump routes supervisors'
    /// stream traffic to it. Called once at startup.
    pub fn set_streams(&self, l: Arc<crate::stream_listener::StreamListener>) {
        self.0.streams.set(l);
    }

    /// (P4-PRV-2, W18) The installed stream listener, if any.
    pub fn streams(&self) -> Option<Arc<crate::stream_listener::StreamListener>> {
        self.0.streams.get()
    }

    /// (P4-PRV-2, W18) Watch of the API client: the stream listener's serve loop ends when
    /// the runner's API client is dropped (shutdown).
    pub fn api_watch(&self) -> watch::Receiver<Option<Arc<ApiClient>>> {
        self.0.api_watch()
    }

    /// (P4-PRV-2, W18) Flip when the API reported `previews.enabled` at registration; the
    /// stream listener's port opens only then. Never un-flipped (a running process keeps the
    /// listener; the next registration re-decides).
    pub fn set_previews_enabled(&self, on: bool) {
        let _ = self.0.previews_on.send_replace(on);
    }

    /// (P4-PRV-2, W18) Waits until `set_previews_enabled(true)` (or `stop`), then returns.
    /// Used by the stream listener's serve loop to keep the port dark otherwise.
    pub async fn previews_wait(&self, stop: &mut watch::Receiver<bool>) {
        let mut on = self.0.previews_on.subscribe();
        let mut stop = stop.clone();
        tokio::select! {
            _ = on.wait_for(|v| *v) => {}
            _ = stop.wait_for(|v| *v) => {}
        }
    }

    pub fn new(settings: ChannelSettings) -> Self {
        let (events, rx) = mpsc::channel(256);
        let (api, _) = watch::channel(None);
        let (previews_on, _) = watch::channel(false);
        Self(Arc::new(Shared {
            settings,
            api,
            sessions: Mutex::default(),
            locks: Mutex::default(),
            events,
            events_rx: Mutex::new(Some(rx)),
            plane: Semaphore::new(64),
            streams: streams_cell::StreamsCell::empty(),
            previews_on,
        }))
    }

    /// Accept supervisor connections until `stop` turns true.
    pub async fn serve(&self, listener: TcpListener, mut stop: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((tcp, _)) => {
                        let shared = self.0.clone();
                        tokio::spawn(conn::handle(shared, tcp));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "channel accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                () = stopped(&mut stop) => return,
            }
        }
    }

    /// (P4-PRV-2, W17) True when this runner holds the live supervisor channel of that
    /// `(sandbox, boot)` — the stream listener's `404 not_here` check.
    pub fn holds_channel(&self, r: &ChannelRef) -> bool {
        self.0
            .sessions()
            .get(&r.sandbox_id)
            .is_some_and(|s| s.boot_id == r.boot_id && s.is_connected())
    }

    /// (P4-PRV-2, W18) Sends one unsequenced `stream.*` frame on that boot's channel.
    pub async fn send_stream_frame(
        &self,
        r: &ChannelRef,
        frame: RunnerToSupervisor,
    ) -> Result<(), HubError> {
        let text = encode_stream_frame(frame).ok_or(HubError::TooLarge)?;
        self.0
            .sessions()
            .get(&r.sandbox_id)
            .ok_or(HubError::NotConnected)?
            .push_stream_frame(text)
    }

    /// (P4-PRV-2, W18) Sends one binary data message on that boot's channel.
    pub async fn send_stream_data(&self, r: &ChannelRef, data: Vec<u8>) -> Result<(), HubError> {
        self.0
            .sessions()
            .get(&r.sandbox_id)
            .ok_or(HubError::NotConnected)?
            .push_stream_data(data)
    }

    /// (P4-TERM, W19) Sends one binary terminal data message (tag `0x02`) on that boot's
    /// channel — the same queues as the stream data.
    pub async fn send_terminal_data(&self, r: &ChannelRef, data: Vec<u8>) -> Result<(), HubError> {
        self.send_stream_data(r, data).await
    }
}

/// Encodes an unsequenced `stream.*` frame (`seq: 0, ack: 0`, `16 §W18`).
fn encode_stream_frame(msg: RunnerToSupervisor) -> Option<String> {
    use aurora_protocol::common::Validate as _;
    use aurora_protocol::supervisor::Frame as WireFrame;
    let frame = WireFrame {
        seq: 0,
        ack: 0,
        msg,
    };
    if frame.validate().is_err() {
        return None;
    }
    serde_json::to_string(&frame).ok()
}

#[async_trait]
impl ChannelHub for ChannelServer {
    fn is_connected(&self, sandbox_id: Uuid) -> bool {
        self.0
            .sessions()
            .get(&sandbox_id)
            .is_some_and(|s| s.is_connected())
    }

    async fn send(&self, sandbox_id: Uuid, frame: Frame) -> Result<(), HubError> {
        let session = self.0.sessions().get(&sandbox_id).cloned();
        session
            .ok_or(HubError::NotConnected)?
            .push(&frame.kind, frame.body, true)
    }

    fn take_events(&self) -> Option<mpsc::Receiver<HubEvent>> {
        self.0.events_rx.lock().ok().and_then(|mut r| r.take())
    }

    fn attach_api(&self, api: Arc<ApiClient>) {
        self.0.api.send_replace(Some(api));
    }

    fn set_previews_enabled(&self, on: bool) {
        ChannelServer::set_previews_enabled(self, on);
    }
}

async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

//! Control-channel server seam. The WebSocket server is RUN-002; this task wires the trait.

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

/// A W6 frame body without the sequencing envelope (the hub owns seq/ack/replay).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub kind: String,
    pub body: Value,
}

/// A frame received from a supervisor, identity taken from the authenticated channel.
#[derive(Debug, Clone, PartialEq)]
pub struct HubEvent {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    /// Supervisor-side sequence number (feeds the deterministic report id, S-07).
    pub seq: u64,
    pub frame: Frame,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum HubError {
    #[error("sandbox not connected")]
    NotConnected,
}

#[async_trait]
pub trait ChannelHub: Send + Sync {
    fn is_connected(&self, sandbox_id: Uuid) -> bool;
    /// Queue a frame towards the sandbox's supervisor.
    async fn send(&self, sandbox_id: Uuid, frame: Frame) -> Result<(), HubError>;
    /// Stream of supervisor frames; may be taken once.
    fn take_events(&self) -> Option<mpsc::Receiver<HubEvent>>;
}

/// Hub with no connections (used until RUN-002 lands).
#[derive(Debug, Default)]
pub struct NullHub;

#[async_trait]
impl ChannelHub for NullHub {
    fn is_connected(&self, _: Uuid) -> bool {
        false
    }
    async fn send(&self, _: Uuid, _: Frame) -> Result<(), HubError> {
        Err(HubError::NotConnected)
    }
    fn take_events(&self) -> Option<mpsc::Receiver<HubEvent>> {
        None
    }
}

/// Test stub: scripted connections, recorded sends, injectable events.
pub struct StubHub {
    connected: Mutex<HashSet<Uuid>>,
    sent: Mutex<Vec<(Uuid, Frame)>>,
    tx: mpsc::Sender<HubEvent>,
    rx: Mutex<Option<mpsc::Receiver<HubEvent>>>,
}

impl Default for StubHub {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            connected: Mutex::default(),
            sent: Mutex::default(),
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }
}

impl StubHub {
    pub fn set_connected(&self, sandbox_id: Uuid, connected: bool) {
        if let Ok(mut c) = self.connected.lock() {
            if connected {
                c.insert(sandbox_id);
            } else {
                c.remove(&sandbox_id);
            }
        }
    }

    pub fn sent(&self) -> Vec<(Uuid, Frame)> {
        self.sent.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Inject a supervisor frame.
    pub async fn emit(&self, event: HubEvent) {
        let _ = self.tx.send(event).await;
    }
}

#[async_trait]
impl ChannelHub for StubHub {
    fn is_connected(&self, sandbox_id: Uuid) -> bool {
        self.connected
            .lock()
            .map(|c| c.contains(&sandbox_id))
            .unwrap_or(false)
    }

    async fn send(&self, sandbox_id: Uuid, frame: Frame) -> Result<(), HubError> {
        if !self.is_connected(sandbox_id) {
            return Err(HubError::NotConnected);
        }
        if let Ok(mut s) = self.sent.lock() {
            s.push((sandbox_id, frame));
        }
        Ok(())
    }

    fn take_events(&self) -> Option<mpsc::Receiver<HubEvent>> {
        self.rx.lock().ok().and_then(|mut r| r.take())
    }
}

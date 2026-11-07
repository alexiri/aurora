//! Postgres LISTEN/NOTIFY event bus (docs/05 §Event bus).
//!
//! Notifications are hints: after a (re)connect the [`Listener`] yields
//! [`BusEvent::Resync`] and the consumer must re-read state from the database.

use std::collections::VecDeque;
use std::time::Duration;

use sqlx::postgres::PgListener;
use sqlx::{PgExecutor, PgPool};

/// Maximum NOTIFY payload size we send (Postgres hard limit is 8000 bytes).
pub const MAX_PAYLOAD_BYTES: usize = 7000;

const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// The bus channels defined in docs/05.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Conversation,
    Delta,
    Work,
    Ops,
    Inbox,
    OpResult,
    Approval,
    GatewayInvalidate,
    Registry,
    SecretOrigin,
    AuthRevoked,
}

impl Channel {
    pub const ALL: [Channel; 11] = [
        Channel::Conversation,
        Channel::Delta,
        Channel::Work,
        Channel::Ops,
        Channel::Inbox,
        Channel::OpResult,
        Channel::Approval,
        Channel::GatewayInvalidate,
        Channel::Registry,
        Channel::SecretOrigin,
        Channel::AuthRevoked,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Channel::Conversation => "aurora_conversation",
            Channel::Delta => "aurora_delta",
            Channel::Work => "aurora_work",
            Channel::Ops => "aurora_ops",
            Channel::Inbox => "aurora_inbox",
            Channel::OpResult => "aurora_op_result",
            Channel::Approval => "aurora_approval",
            Channel::GatewayInvalidate => "aurora_gateway_invalidate",
            Channel::Registry => "aurora_registry",
            Channel::SecretOrigin => "aurora_secret_origin",
            Channel::AuthRevoked => "aurora_auth_revoked",
        }
    }

    pub fn parse(name: &str) -> Option<Channel> {
        Self::ALL.into_iter().find(|c| c.as_str() == name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("notify payload is {size} bytes, limit is {max}")]
    PayloadTooLarge { size: usize, max: usize },
    #[error("bus database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// `SELECT pg_notify(channel, payload)`. Inside a transaction the notification
/// is delivered on commit, so producers should call this with their tx.
pub async fn notify<'e, E: PgExecutor<'e>>(
    executor: E,
    channel: Channel,
    payload: &serde_json::Value,
) -> Result<(), BusError> {
    let text = payload.to_string();
    if text.len() > MAX_PAYLOAD_BYTES {
        return Err(BusError::PayloadTooLarge {
            size: text.len(),
            max: MAX_PAYLOAD_BYTES,
        });
    }
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(channel.as_str())
        .bind(text)
        .execute(executor)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum BusEvent {
    Notification {
        channel: Channel,
        payload: serde_json::Value,
    },
    /// The listener (re)connected; notifications may have been missed.
    Resync,
}

/// Dedicated LISTEN connection with reconnect and capped backoff.
pub struct Listener {
    inner: PgListener,
    pending: VecDeque<BusEvent>,
    backoff: Duration,
    needs_resync: bool,
}

impl Listener {
    /// Connect and LISTEN on `channels`. The first event yielded is `Resync`.
    pub async fn connect(pool: &PgPool, channels: &[Channel]) -> Result<Self, BusError> {
        let mut inner = PgListener::connect_with(pool).await?;
        inner
            .listen_all(channels.iter().map(|c| c.as_str()))
            .await?;
        Ok(Self {
            inner,
            pending: VecDeque::from([BusEvent::Resync]),
            backoff: BACKOFF_START,
            needs_resync: false,
        })
    }

    /// Next event. Never returns an error: connection failures are retried
    /// with capped exponential backoff and surface as `Resync` once healthy.
    /// Cancel-safe.
    pub async fn next(&mut self) -> BusEvent {
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return ev;
            }
            match self.inner.try_recv().await {
                Ok(Some(n)) => {
                    self.backoff = BACKOFF_START;
                    if std::mem::take(&mut self.needs_resync) {
                        self.pending.push_back(BusEvent::Resync);
                    }
                    let Some(channel) = Channel::parse(n.channel()) else {
                        tracing::warn!(
                            channel = n.channel(),
                            "notification on unknown channel skipped"
                        );
                        continue;
                    };
                    match serde_json::from_str(n.payload()) {
                        Ok(payload) => self
                            .pending
                            .push_back(BusEvent::Notification { channel, payload }),
                        Err(e) => tracing::warn!(
                            channel = channel.as_str(),
                            error = %e,
                            "malformed notification payload skipped"
                        ),
                    }
                }
                // Connection was lost and sqlx re-established + re-LISTENed.
                Ok(None) => {
                    self.backoff = BACKOFF_START;
                    self.needs_resync = false;
                    return BusEvent::Resync;
                }
                Err(e) => {
                    tracing::warn!(error = %e, backoff_ms = self.backoff.as_millis() as u64, "bus listener error; retrying");
                    self.needs_resync = true;
                    tokio::time::sleep(self.backoff).await;
                    self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The delta bus seam (docs/05 §The delta bus seam, phase 1, RFE-016, D-48).
// ---------------------------------------------------------------------------------------------

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use uuid::Uuid;

/// What a delta carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeltaKind {
    Text,
    Thinking,
    Activity,
    /// (phase 2, RFE-010) `data` is the seq of a transcript event whose payload was redacted.
    Redacted,
}

impl DeltaKind {
    /// The wire name (`type` of the `aurora_delta` payload and of the SSE event).
    pub const fn as_str(self) -> &'static str {
        match self {
            DeltaKind::Text => "text.delta",
            DeltaKind::Thinking => "thinking.delta",
            DeltaKind::Activity => "activity",
            DeltaKind::Redacted => "event.redacted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "text.delta" => Some(DeltaKind::Text),
            "thinking.delta" => Some(DeltaKind::Thinking),
            "activity" => Some(DeltaKind::Activity),
            "event.redacted" => Some(DeltaKind::Redacted),
            _ => None,
        }
    }
}

/// One coalesced ephemeral delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub conversation_id: Uuid,
    pub turn_id: Uuid,
    pub kind: DeltaKind,
    pub message_id: Option<String>,
    pub reply_to: Option<String>,
    pub data: String,
}

impl Delta {
    /// The `aurora_delta` payload: `{"c","t","type","d"[,"message_id"][,"reply_to"]}`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "c": self.conversation_id,
            "t": self.turn_id,
            "type": self.kind.as_str(),
            "d": self.data,
        });
        if let Some(m) = &self.message_id {
            v["message_id"] = serde_json::Value::String(m.clone());
        }
        if let Some(r) = &self.reply_to {
            v["reply_to"] = serde_json::Value::String(r.clone());
        }
        v
    }

    /// Inverse of [`Delta::to_json`]; `None` for a malformed payload.
    pub fn from_json(p: &serde_json::Value) -> Option<Self> {
        let uuid = |k: &str| p.get(k)?.as_str()?.parse::<Uuid>().ok();
        let opt = |k: &str| p.get(k).and_then(|m| m.as_str()).map(str::to_owned);
        Some(Self {
            conversation_id: uuid("c")?,
            turn_id: uuid("t")?,
            kind: DeltaKind::parse(p.get("type")?.as_str()?)?,
            message_id: opt("message_id"),
            reply_to: opt("reply_to"),
            data: p.get("d")?.as_str()?.to_owned(),
        })
    }
}

/// Fan-out of ephemeral streaming deltas to every API replica.
///
/// Contract: at-most-once, loss allowed (a lost delta is repaired by the durable `text`
/// event); per-`(conversation, turn)` order preserved; JSON-encoded payload at most
/// [`MAX_PAYLOAD_BYTES`]; coalescing happens above the trait; no durability, no ack.
#[async_trait]
pub trait DeltaBus: Send + Sync + 'static {
    /// Publish one coalesced delta. Best effort: callers log the error and carry on.
    async fn publish(&self, delta: Delta) -> Result<(), BusError>;

    /// One stream of every delta from every replica, including this one. The bus replays
    /// nothing: a new subscription (first, or after the previous stream ended because of a
    /// loss) obliges the consumer to resynchronize from the database.
    async fn subscribe(&self) -> Result<BoxStream<'static, Delta>, BusError>;
}

/// Registry id of the Postgres NOTIFY implementation (the default).
pub const DELTA_BUS_NOTIFY: &str = "notify";

/// The phase 1 implementation: the `aurora_delta` NOTIFY channel.
pub struct NotifyDeltaBus {
    pool: PgPool,
}

impl NotifyDeltaBus {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DeltaBus for NotifyDeltaBus {
    async fn publish(&self, delta: Delta) -> Result<(), BusError> {
        notify(&self.pool, Channel::Delta, &delta.to_json()).await
    }

    /// The LISTEN is active when this returns. The stream ends when the connection was lost
    /// and re-established (notifications may have been missed): subscribe again and resync.
    async fn subscribe(&self) -> Result<BoxStream<'static, Delta>, BusError> {
        let listener = Listener::connect(&self.pool, &[Channel::Delta]).await?;
        let s = futures::stream::unfold((listener, true), |(mut l, mut first_resync)| async move {
            loop {
                match l.next().await {
                    BusEvent::Resync if first_resync => first_resync = false,
                    BusEvent::Resync => return None,
                    BusEvent::Notification { payload, .. } => {
                        if let Some(d) = Delta::from_json(&payload) {
                            return Some((d, (l, first_resync)));
                        }
                    }
                }
            }
        });
        Ok(s.boxed())
    }
}

/// Resolve a `events.delta_bus` id; an unknown id is an error naming the key.
pub fn delta_bus_by_id(id: &str, pool: &PgPool) -> Result<std::sync::Arc<dyn DeltaBus>, String> {
    match id {
        DELTA_BUS_NOTIFY => Ok(std::sync::Arc::new(NotifyDeltaBus::new(pool.clone()))),
        other => Err(format!(
            "events.delta_bus: unknown delta bus `{other}` (known: {DELTA_BUS_NOTIFY})"
        )),
    }
}

/// An in-process `DeltaBus` (single replica): the reference implementation for tests and
/// conformance runs. Slow subscribers lose deltas (allowed by the contract).
pub struct InMemoryDeltaBus {
    tx: tokio::sync::broadcast::Sender<Delta>,
}

impl Default for InMemoryDeltaBus {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryDeltaBus {
    pub fn new() -> Self {
        Self {
            tx: tokio::sync::broadcast::channel(1024).0,
        }
    }
}

#[async_trait]
impl DeltaBus for InMemoryDeltaBus {
    async fn publish(&self, delta: Delta) -> Result<(), BusError> {
        let size = delta.to_json().to_string().len();
        if size > MAX_PAYLOAD_BYTES {
            return Err(BusError::PayloadTooLarge {
                size,
                max: MAX_PAYLOAD_BYTES,
            });
        }
        // No subscriber is not an error: deltas are ephemeral.
        let _ = self.tx.send(delta);
        Ok(())
    }

    async fn subscribe(&self) -> Result<BoxStream<'static, Delta>, BusError> {
        let rx = self.tx.subscribe();
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(d) => return Some((d, rx)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .boxed())
    }
}

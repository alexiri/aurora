//! One shared `aurora_conversation` LISTEN connection and one [`DeltaBus`] subscription per API
//! process (the default `notify` implementation is the `aurora_delta` channel), fanned out to SSE
//! streams; plus the producer-side delta coalescer (>= 100 ms or >= 2 000 chars per turn).

use aurora_core::ApiError;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Delta, DeltaBus, DeltaKind, Listener, NotifyDeltaBus};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{OnceCell, broadcast};
use uuid::Uuid;

/// Default coalescing window (`events.delta_coalesce_ms`, `docs/15 §Configuration`).
pub const DELTA_FLUSH_AFTER: Duration = Duration::from_millis(100);
pub const DELTA_FLUSH_CHARS: usize = 2_000;
/// Keeps each NOTIFY payload far below the 7 000 byte bus limit even for multibyte text.
const DELTA_CHUNK_CHARS: usize = 1_000;

#[derive(Debug, Clone)]
pub enum BusMsg {
    /// A durable event was committed.
    Seq { conversation: Uuid, seq: i64 },
    Delta {
        conversation: Uuid,
        turn: Uuid,
        /// `text.delta` | `thinking.delta` | `activity`
        kind: String,
        message_id: Option<String>,
        reply_to: Option<String>,
        text: String,
    },
    /// Notifications may have been missed (listener reconnected, or receiver lagged).
    Resync,
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct Key {
    conversation: Uuid,
    turn: Uuid,
    kind: String,
    message_id: Option<String>,
    reply_to: Option<String>,
}

struct Pending {
    text: String,
    chars: usize,
    last_flush: Instant,
    timer: bool,
}

pub struct ConversationBus {
    tx: OnceCell<broadcast::Sender<BusMsg>>,
    /// Ephemeral delta fan-out (RFE-016); `notify` unless injected.
    delta_bus: OnceLock<Arc<dyn DeltaBus>>,
    pending: Mutex<HashMap<Key, Pending>>,
    /// Coalescing window (`events.delta_coalesce_ms`).
    window: Duration,
}

impl Default for ConversationBus {
    fn default() -> Self {
        Self::with_window(DELTA_FLUSH_AFTER)
    }
}

impl ConversationBus {
    /// The bus selected by `events.delta_bus` (`notify` is the only implementation; the id is
    /// validated at startup) with `events.delta_coalesce_ms` as the coalescing window.
    pub fn from_config(
        events: &aurora_config::EventsSection,
        pool: &PgPool,
    ) -> Result<Self, String> {
        let bus = aurora_db::bus::delta_bus_by_id(&events.delta_bus, pool)?;
        Ok(Self::with_delta_bus_and_window(
            bus,
            Duration::from_millis(u64::from(events.delta_coalesce_ms)),
        ))
    }

    /// A `notify` bus with an explicit coalescing window.
    pub fn with_window(window: Duration) -> Self {
        Self {
            tx: OnceCell::new(),
            delta_bus: OnceLock::new(),
            pending: Mutex::new(HashMap::new()),
            window,
        }
    }

    /// A bus whose deltas flow through `delta_bus` instead of the default NOTIFY channel.
    pub fn with_delta_bus(delta_bus: Arc<dyn DeltaBus>) -> Self {
        Self::with_delta_bus_and_window(delta_bus, DELTA_FLUSH_AFTER)
    }

    /// As [`Self::with_delta_bus`] with an explicit coalescing window.
    pub fn with_delta_bus_and_window(delta_bus: Arc<dyn DeltaBus>, window: Duration) -> Self {
        let b = Self::with_window(window);
        let _ = b.delta_bus.set(delta_bus);
        b
    }

    fn delta_bus(&self, pool: &PgPool) -> Arc<dyn DeltaBus> {
        self.delta_bus
            .get_or_init(|| Arc::new(NotifyDeltaBus::new(pool.clone())))
            .clone()
    }

    /// Subscribe; the LISTEN is active when this returns, so later NOTIFYs are seen.
    pub async fn subscribe(&self, pool: &PgPool) -> Result<broadcast::Receiver<BusMsg>, ApiError> {
        let tx = self
            .tx
            .get_or_try_init(|| async {
                let mut listener = Listener::connect(pool, &[Channel::Conversation])
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "conversation bus connect failed");
                        ApiError::unavailable("bus_unavailable", "Event bus unavailable.")
                    })?;
                let delta_bus = self.delta_bus(pool);
                let first = delta_bus.subscribe().await.map_err(|e| {
                    tracing::error!(error = %e, "delta bus subscribe failed");
                    ApiError::unavailable("bus_unavailable", "Event bus unavailable.")
                })?;
                let (tx, _) = broadcast::channel(1024);
                tokio::spawn(forward_deltas(delta_bus, first, tx.clone()));
                let out = tx.clone();
                tokio::spawn(async move {
                    loop {
                        let msg = match listener.next().await {
                            BusEvent::Resync => BusMsg::Resync,
                            BusEvent::Notification { channel, payload } => {
                                match parse(channel, &payload) {
                                    Some(m) => m,
                                    None => continue,
                                }
                            }
                        };
                        let _ = out.send(msg);
                    }
                });
                Ok::<_, ApiError>(tx)
            })
            .await?;
        Ok(tx.subscribe())
    }

    /// (RFE-010) Tell open streams that the payload of event `seq` was replaced.
    pub async fn publish_redacted(&self, pool: &PgPool, conversation: Uuid, seq: i64) {
        send(
            &*self.delta_bus(pool),
            conversation,
            Uuid::nil(),
            "event.redacted",
            (None, None),
            &seq.to_string(),
        )
        .await;
    }

    /// Publish an ephemeral delta (never stored). `text.delta`/`thinking.delta` are coalesced per
    /// `(conversation, turn, kind, message)`. Emitted when 100 ms passed since the last emit or
    /// 2 000 chars are buffered, otherwise held and flushed by a short timer. Activity lines
    /// are discrete and sent immediately.
    pub async fn publish_delta(
        self: &Arc<Self>,
        pool: &PgPool,
        conversation: Uuid,
        turn: Uuid,
        kind: &str,
        (message_id, reply_to): (Option<&str>, Option<&str>),
        text: &str,
    ) {
        if kind == "activity" {
            send(
                &*self.delta_bus(pool),
                conversation,
                turn,
                kind,
                (None, None),
                text,
            )
            .await;
            return;
        }
        let key = Key {
            conversation,
            turn,
            kind: kind.to_owned(),
            message_id: message_id.map(str::to_owned),
            reply_to: reply_to.map(str::to_owned),
        };
        let (emit, arm) = {
            let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            if map.len() > 1024 {
                // Drop idle buffers of long-finished turns.
                map.retain(|_, p| {
                    p.timer
                        || !p.text.is_empty()
                        || p.last_flush.elapsed() < Duration::from_secs(30)
                });
            }
            let p = map.entry(key.clone()).or_insert_with(|| Pending {
                text: String::new(),
                chars: 0,
                // A fresh buffer may be flushed immediately (nothing was sent within 100 ms).
                last_flush: Instant::now()
                    .checked_sub(self.window)
                    .unwrap_or_else(Instant::now),
                timer: false,
            });
            p.text.push_str(text);
            p.chars += text.chars().count();
            if p.chars >= DELTA_FLUSH_CHARS || p.last_flush.elapsed() >= self.window {
                p.last_flush = Instant::now();
                p.chars = 0;
                (Some(std::mem::take(&mut p.text)), false)
            } else if !p.timer {
                p.timer = true;
                (None, true)
            } else {
                (None, false)
            }
        };
        if let Some(t) = emit {
            send(
                &*self.delta_bus(pool),
                conversation,
                turn,
                kind,
                (message_id, reply_to),
                &t,
            )
            .await;
        }
        if arm {
            let bus = Arc::clone(self);
            let pool = pool.clone();
            let window = self.window;
            tokio::spawn(async move {
                tokio::time::sleep(window).await;
                bus.flush_key(&pool, &key).await;
            });
        }
    }

    async fn flush_key(&self, pool: &PgPool, key: &Key) {
        let text = {
            let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            match map.get_mut(key) {
                Some(p) => {
                    p.timer = false;
                    p.chars = 0;
                    p.last_flush = Instant::now();
                    std::mem::take(&mut p.text)
                }
                None => String::new(),
            }
        };
        if !text.is_empty() {
            send(
                &*self.delta_bus(pool),
                key.conversation,
                key.turn,
                &key.kind,
                (key.message_id.as_deref(), key.reply_to.as_deref()),
                &text,
            )
            .await;
        }
    }

    /// Flush and forget everything buffered for `turn` (call before its durable `text` /
    /// `turn.finished` event so no delta trails the final answer).
    pub async fn flush_turn(&self, pool: &PgPool, conversation: Uuid, turn: Uuid) {
        let keys: Vec<Key> = {
            let map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            map.keys()
                .filter(|k| k.conversation == conversation && k.turn == turn)
                .cloned()
                .collect()
        };
        for k in keys {
            self.flush_key(pool, &k).await;
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&k);
        }
    }
}

async fn send(
    bus: &dyn DeltaBus,
    conversation: Uuid,
    turn: Uuid,
    kind: &str,
    (message_id, reply_to): (Option<&str>, Option<&str>),
    text: &str,
) {
    let Some(kind) = DeltaKind::parse(kind) else {
        tracing::warn!(kind, "delta of unknown kind dropped");
        return;
    };
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(DELTA_CHUNK_CHARS) {
        let delta = Delta {
            conversation_id: conversation,
            turn_id: turn,
            kind,
            message_id: message_id.map(str::to_owned),
            reply_to: reply_to.map(str::to_owned),
            data: chunk.iter().collect(),
        };
        if let Err(e) = bus.publish(delta).await {
            tracing::warn!(error = %e, "delta publish failed");
        }
    }
}

fn to_msg(d: Delta) -> BusMsg {
    BusMsg::Delta {
        conversation: d.conversation_id,
        turn: d.turn_id,
        kind: d.kind.as_str().to_owned(),
        message_id: d.message_id,
        reply_to: d.reply_to,
        text: d.data,
    }
}

/// Pump `stream` into `out`; when it ends (loss), resubscribe with capped backoff and tell
/// consumers to resynchronize (the bus replays nothing). Stops when no receiver can ever exist.
async fn forward_deltas(
    bus: Arc<dyn DeltaBus>,
    mut stream: futures::stream::BoxStream<'static, Delta>,
    out: broadcast::Sender<BusMsg>,
) {
    let mut backoff = Duration::from_millis(250);
    loop {
        while let Some(d) = stream.next().await {
            let _ = out.send(to_msg(d));
        }
        loop {
            tokio::time::sleep(backoff).await;
            match bus.subscribe().await {
                Ok(s) => {
                    stream = s;
                    backoff = Duration::from_millis(250);
                    let _ = out.send(BusMsg::Resync);
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "delta bus resubscribe failed; retrying");
                    backoff = (backoff * 2).min(Duration::from_secs(10));
                }
            }
        }
    }
}

fn parse(channel: Channel, p: &serde_json::Value) -> Option<BusMsg> {
    let uuid = |k: &str| p.get(k)?.as_str()?.parse::<Uuid>().ok();
    match channel {
        Channel::Conversation => Some(BusMsg::Seq {
            conversation: uuid("c")?,
            seq: p.get("s")?.as_i64()?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_db::bus::InMemoryDeltaBus;
    use sqlx::postgres::PgPoolOptions;

    /// The pool is never touched when an in-memory `DeltaBus` is injected.
    fn unused_pool() -> PgPool {
        PgPoolOptions::new()
            .connect_lazy("postgres://unused@127.0.0.1:1/unused")
            .unwrap()
    }

    #[tokio::test]
    async fn deltas_flow_through_an_injected_bus_coalesced_and_ordered() {
        let mem = Arc::new(InMemoryDeltaBus::new());
        let mut sub = mem.subscribe().await.unwrap();
        let bus = Arc::new(ConversationBus::with_delta_bus(mem.clone()));
        let pool = unused_pool();
        let (c, t) = (Uuid::now_v7(), Uuid::now_v7());
        // First delta flushes at once; the next two are held and coalesced by flush_turn.
        bus.publish_delta(&pool, c, t, "text.delta", (Some("m1"), None), "a")
            .await;
        bus.publish_delta(&pool, c, t, "text.delta", (Some("m1"), None), "b")
            .await;
        bus.publish_delta(&pool, c, t, "text.delta", (Some("m1"), None), "c")
            .await;
        bus.publish_delta(&pool, c, t, "activity", (None, None), "running ls")
            .await;
        bus.flush_turn(&pool, c, t).await;
        let mut got = Vec::new();
        while let Ok(Some(d)) = tokio::time::timeout(Duration::from_millis(200), sub.next()).await {
            got.push(d);
        }
        let text: Vec<&str> = got
            .iter()
            .filter(|d| d.kind == DeltaKind::Text)
            .map(|d| d.data.as_str())
            .collect();
        assert_eq!(text, ["a", "bc"]);
        assert!(got.iter().all(|d| d.conversation_id == c && d.turn_id == t));
        assert!(
            got.iter()
                .any(|d| d.kind == DeltaKind::Activity && d.data == "running ls")
        );
    }

    /// A non-default window is honoured: with 300 ms, a delta sent 150 ms after the first is
    /// still held (the default 100 ms would have emitted it), and the timer flushes it at 300 ms.
    #[tokio::test]
    async fn a_non_default_coalesce_window_is_honoured() {
        let mem = Arc::new(InMemoryDeltaBus::new());
        let mut sub = mem.subscribe().await.unwrap();
        let events = aurora_config::EventsSection {
            delta_coalesce_ms: 300,
            ..Default::default()
        };
        let pool = unused_pool();
        let configured = ConversationBus::from_config(&events, &pool).unwrap();
        assert_eq!(configured.window, Duration::from_millis(300));
        let bus = Arc::new(ConversationBus::with_delta_bus_and_window(
            mem.clone(),
            configured.window,
        ));
        let (c, t) = (Uuid::now_v7(), Uuid::now_v7());
        let p = |s: &'static str| {
            let (bus, pool) = (bus.clone(), pool.clone());
            async move {
                bus.publish_delta(&pool, c, t, "text.delta", (Some("m1"), None), s)
                    .await
            }
        };
        p("a").await; // flushed at once
        tokio::time::sleep(Duration::from_millis(150)).await;
        p("b").await; // within the window: held
        p("c").await;
        let mut got = Vec::new();
        // Nothing but "a" until the window (300 ms after "a") elapses.
        while let Ok(Some(d)) = tokio::time::timeout(Duration::from_millis(50), sub.next()).await {
            got.push(d.data);
        }
        assert_eq!(got, ["a"], "held within the 300 ms window");
        tokio::time::sleep(Duration::from_millis(350)).await;
        while let Ok(Some(d)) = tokio::time::timeout(Duration::from_millis(50), sub.next()).await {
            got.push(d.data);
        }
        assert_eq!(got, ["a", "bc"], "flushed by the timer after the window");
    }

    #[tokio::test]
    async fn from_config_rejects_an_unknown_bus_id() {
        let events = aurora_config::EventsSection {
            delta_bus: "kafka".into(),
            ..Default::default()
        };
        let e = ConversationBus::from_config(&events, &unused_pool())
            .err()
            .unwrap();
        assert!(e.contains("events.delta_bus"), "{e}");
    }

    #[tokio::test]
    async fn large_deltas_are_chunked_below_the_payload_limit() {
        let mem = Arc::new(InMemoryDeltaBus::new());
        let mut sub = mem.subscribe().await.unwrap();
        let bus = Arc::new(ConversationBus::with_delta_bus(mem));
        let big = "é".repeat(2_500);
        bus.publish_delta(
            &unused_pool(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            "thinking.delta",
            (None, None),
            &big,
        )
        .await;
        let mut total = 0;
        while let Ok(Some(d)) = tokio::time::timeout(Duration::from_millis(200), sub.next()).await {
            assert!(d.to_json().to_string().len() <= aurora_db::bus::MAX_PAYLOAD_BYTES);
            total += d.data.chars().count();
        }
        assert_eq!(total, 2_500);
    }

    #[tokio::test]
    async fn forwarder_maps_published_deltas_to_bus_messages() {
        let mem = Arc::new(InMemoryDeltaBus::new());
        let first = mem.subscribe().await.unwrap();
        let (tx, mut rx) = broadcast::channel(16);
        tokio::spawn(forward_deltas(mem.clone(), first, tx));
        let d = Delta {
            conversation_id: Uuid::now_v7(),
            turn_id: Uuid::now_v7(),
            kind: DeltaKind::Thinking,
            message_id: None,
            reply_to: None,
            data: "hm".into(),
        };
        mem.publish(d.clone()).await.unwrap();
        match rx.recv().await.unwrap() {
            BusMsg::Delta {
                kind, text, turn, ..
            } => {
                assert_eq!(
                    (kind.as_str(), text.as_str(), turn),
                    ("thinking.delta", "hm", d.turn_id)
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

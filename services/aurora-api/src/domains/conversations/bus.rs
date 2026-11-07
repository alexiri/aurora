//! One shared `aurora_conversation` + `aurora_delta` LISTEN connection per API process, fanned out
//! to SSE streams; plus the producer-side delta coalescer (>= 100 ms or >= 2 000 chars per turn).

use aurora_core::ApiError;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use aurora_db::repo::transcript;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OnceCell, broadcast};
use uuid::Uuid;

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

#[derive(Default)]
pub struct ConversationBus {
    tx: OnceCell<broadcast::Sender<BusMsg>>,
    pending: Mutex<HashMap<Key, Pending>>,
}

impl ConversationBus {
    /// Subscribe; the LISTEN is active when this returns, so later NOTIFYs are seen.
    pub async fn subscribe(&self, pool: &PgPool) -> Result<broadcast::Receiver<BusMsg>, ApiError> {
        let tx = self
            .tx
            .get_or_try_init(|| async {
                let mut listener =
                    Listener::connect(pool, &[Channel::Conversation, Channel::Delta])
                        .await
                        .map_err(|e| {
                            tracing::error!(error = %e, "conversation bus connect failed");
                            ApiError::unavailable("bus_unavailable", "Event bus unavailable.")
                        })?;
                let (tx, _) = broadcast::channel(1024);
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
            send(pool, conversation, turn, kind, (None, None), text).await;
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
                last_flush: Instant::now() - DELTA_FLUSH_AFTER,
                timer: false,
            });
            p.text.push_str(text);
            p.chars += text.chars().count();
            if p.chars >= DELTA_FLUSH_CHARS || p.last_flush.elapsed() >= DELTA_FLUSH_AFTER {
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
            send(pool, conversation, turn, kind, (message_id, reply_to), &t).await;
        }
        if arm {
            let bus = Arc::clone(self);
            let pool = pool.clone();
            tokio::spawn(async move {
                tokio::time::sleep(DELTA_FLUSH_AFTER).await;
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
                pool,
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
    pool: &PgPool,
    conversation: Uuid,
    turn: Uuid,
    kind: &str,
    (message_id, reply_to): (Option<&str>, Option<&str>),
    text: &str,
) {
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(DELTA_CHUNK_CHARS) {
        let d: String = chunk.iter().collect();
        let mut v = serde_json::json!({"c": conversation, "t": turn, "type": kind, "d": d});
        if let Some(m) = message_id {
            v["message_id"] = serde_json::Value::String(m.to_owned());
        }
        if let Some(r) = reply_to {
            v["reply_to"] = serde_json::Value::String(r.to_owned());
        }
        if let Err(e) = transcript::notify_delta(pool, &v).await {
            tracing::warn!(error = %e, "delta notify failed");
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
        Channel::Delta => Some(BusMsg::Delta {
            conversation: uuid("c")?,
            turn: uuid("t")?,
            kind: p.get("type")?.as_str()?.to_owned(),
            message_id: p
                .get("message_id")
                .and_then(|m| m.as_str())
                .map(str::to_owned),
            reply_to: p
                .get("reply_to")
                .and_then(|m| m.as_str())
                .map(str::to_owned),
            text: p.get("d")?.as_str()?.to_owned(),
        }),
        _ => None,
    }
}

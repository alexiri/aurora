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

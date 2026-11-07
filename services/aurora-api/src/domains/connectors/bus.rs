//! One shared LISTEN connection per API process for the connector plane channels
//! (`aurora_ops`, `aurora_op_result`, `aurora_inbox`; docs/05 §Event bus), fanned out to claim
//! long-polls, correlated-call waiters and the inbox processor. Started lazily on first
//! subscribe. Notifications are hints: every consumer re-reads the database.

use aurora_core::ApiError;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use tokio::sync::{OnceCell, broadcast};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum Msg {
    /// New or re-queued ops of this connector.
    Ops {
        connector: String,
    },
    OpResult {
        op_id: Uuid,
    },
    Inbox,
    /// Notifications may have been missed (listener reconnected, or receiver lagged).
    Resync,
}

#[derive(Default)]
pub struct ConnectorBus {
    tx: OnceCell<broadcast::Sender<Msg>>,
}

impl ConnectorBus {
    /// Subscribe; the LISTEN is active when this returns, so later NOTIFYs are seen.
    pub async fn subscribe(&self, pool: &PgPool) -> Result<broadcast::Receiver<Msg>, ApiError> {
        let tx = self
            .tx
            .get_or_try_init(|| async {
                let mut listener =
                    Listener::connect(pool, &[Channel::Ops, Channel::OpResult, Channel::Inbox])
                        .await
                        .map_err(|e| {
                            tracing::error!(error = %e, "connector bus connect failed");
                            ApiError::unavailable("bus_unavailable", "Event bus unavailable.")
                        })?;
                let (tx, _) = broadcast::channel(512);
                let out = tx.clone();
                tokio::spawn(async move {
                    loop {
                        let msg = match listener.next().await {
                            BusEvent::Resync => Msg::Resync,
                            BusEvent::Notification { channel, payload } => match channel {
                                Channel::Ops => {
                                    match payload.get("connector").and_then(|v| v.as_str()) {
                                        Some(c) => Msg::Ops {
                                            connector: c.to_owned(),
                                        },
                                        None => continue,
                                    }
                                }
                                Channel::OpResult => {
                                    match payload
                                        .get("op_id")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| s.parse().ok())
                                    {
                                        Some(op_id) => Msg::OpResult { op_id },
                                        None => continue,
                                    }
                                }
                                Channel::Inbox => Msg::Inbox,
                                _ => continue,
                            },
                        };
                        // No receivers is fine.
                        let _ = out.send(msg);
                    }
                });
                Ok::<_, ApiError>(tx)
            })
            .await?;
        Ok(tx.subscribe())
    }
}

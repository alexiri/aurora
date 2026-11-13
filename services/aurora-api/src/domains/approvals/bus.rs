//! One shared `aurora_approval` LISTEN connection per API process, fanned out
//! to SSE streams and long-polls. Started lazily on first subscribe.

use aurora_core::ApiError;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use tokio::sync::{OnceCell, broadcast};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum BusMsg {
    Event {
        id: Uuid,
        workspace_id: Uuid,
        /// `created` | `decided` | `expired` | `abandoned`
        event: String,
    },
    /// Notifications may have been missed (listener reconnected, or receiver lagged).
    Resync,
}

#[derive(Default)]
pub struct ApprovalBus {
    tx: OnceCell<broadcast::Sender<BusMsg>>,
}

impl ApprovalBus {
    /// Subscribe; the LISTEN is active when this returns, so later NOTIFYs are seen.
    pub async fn subscribe(&self, pool: &PgPool) -> Result<broadcast::Receiver<BusMsg>, ApiError> {
        let tx = self
            .tx
            .get_or_try_init(|| async {
                let mut listener = Listener::connect(pool, &[Channel::Approval])
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "approval bus connect failed");
                        ApiError::unavailable("bus_unavailable", "Event bus unavailable.")
                    })?;
                let (tx, _) = broadcast::channel(256);
                let out = tx.clone();
                tokio::spawn(async move {
                    loop {
                        let msg = match listener.next().await {
                            BusEvent::Resync => BusMsg::Resync,
                            BusEvent::Notification { payload, .. } => {
                                let id = payload
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok());
                                let ws = payload
                                    .get("workspace_id")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok());
                                let ev = payload.get("event").and_then(|v| v.as_str());
                                match (id, ws, ev) {
                                    (Some(id), Some(workspace_id), Some(ev)) => BusMsg::Event {
                                        id,
                                        workspace_id,
                                        event: ev.to_owned(),
                                    },
                                    _ => continue,
                                }
                            }
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

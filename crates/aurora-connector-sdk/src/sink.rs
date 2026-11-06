//! Event posting: batching, retry until acked, ack-after-accept.

use std::sync::Arc;
use std::time::Duration;

use aurora_protocol::connector::{ErrorCode, EventsRequest, InboundEvent, MAX_EVENTS_PER_BATCH};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};

use crate::client::CoreClient;

/// What the core decided about one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventAck {
    Accepted,
    Duplicate,
    Rejected(ErrorCode),
}

impl EventAck {
    /// The event is durably in the core inbox: the platform may be acked.
    pub fn is_persisted(&self) -> bool {
        matches!(self, EventAck::Accepted | EventAck::Duplicate)
    }
}

#[derive(Debug, Error)]
#[error("event poster is shut down")]
pub struct SinkClosed;

struct Pending {
    event: InboundEvent,
    ack: oneshot::Sender<EventAck>,
    attempts: u32,
}

/// Handle given to integrations to submit inbound events.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<Pending>,
}

impl EventSink {
    /// Submits an event and resolves once the core answered. Acknowledge the
    /// platform only after an [`EventAck::is_persisted`] answer.
    pub async fn post(&self, event: InboundEvent) -> Result<EventAck, SinkClosed> {
        let (ack, rx) = oneshot::channel();
        self.tx
            .send(Pending {
                event,
                ack,
                attempts: 0,
            })
            .await
            .map_err(|_| SinkClosed)?;
        rx.await.map_err(|_| SinkClosed)
    }
}

/// Events unanswered by the core this many times are rejected `unavailable`.
const MAX_UNANSWERED: u32 = 10;
const LINGER: Duration = Duration::from_millis(20);

/// Spawns the batching poster. Retries a batch (same event ids) with backoff
/// until the core answers; 4xx rejections reject the whole batch.
pub fn spawn_event_poster(
    client: Arc<CoreClient>,
    mut shutdown: watch::Receiver<bool>,
) -> EventSink {
    let (tx, mut rx) = mpsc::channel::<Pending>(1024);
    tokio::spawn(async move {
        let mut backlog: Vec<Pending> = Vec::new();
        let mut failures = 0u32;
        loop {
            if backlog.is_empty() {
                tokio::select! {
                    first = rx.recv() => match first {
                        Some(p) => backlog.push(p),
                        None => return,
                    },
                    _ = shutdown.wait_for(|s| *s) => return,
                }
                tokio::time::sleep(LINGER).await;
            }
            while backlog.len() < MAX_EVENTS_PER_BATCH {
                match rx.try_recv() {
                    Ok(p) => backlog.push(p),
                    Err(_) => break,
                }
            }
            let batch: Vec<Pending> = std::mem::take(&mut backlog);
            let req = EventsRequest {
                events: batch.iter().map(|p| p.event.clone()).collect(),
            };
            match client.post_events(&req).await {
                Ok(resp) => {
                    failures = 0;
                    for mut p in batch {
                        let id = &p.event.event_id;
                        let ack = if resp.accepted.contains(id) {
                            Some(EventAck::Accepted)
                        } else if resp.duplicates.contains(id) {
                            Some(EventAck::Duplicate)
                        } else {
                            resp.rejected
                                .iter()
                                .find(|r| &r.event_id == id)
                                .map(|r| EventAck::Rejected(r.code.clone()))
                        };
                        match ack {
                            Some(a) => {
                                let _ = p.ack.send(a);
                            }
                            None => {
                                p.attempts += 1;
                                if p.attempts >= MAX_UNANSWERED {
                                    let _ = p.ack.send(EventAck::Rejected(ErrorCode::Unavailable));
                                } else {
                                    backlog.push(p);
                                }
                            }
                        }
                    }
                }
                Err(e) if e.is_retryable() => {
                    tracing::warn!(error = %e, "event post failed; retrying batch");
                    backlog = batch;
                    let delay = client.retry_policy().delay(failures);
                    failures = failures.saturating_add(1);
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = shutdown.wait_for(|s| *s) => return,
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "event post rejected by core");
                    for p in batch {
                        let _ = p.ack.send(EventAck::Rejected(ErrorCode::Invalid));
                    }
                }
            }
        }
    });
    EventSink { tx }
}

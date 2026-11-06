//! Approval holds (docs/06 §Approval holds, docs/07 §Approvals (gateway holds)).
//!
//! A request whose verdict is `needs_approval` is stored as a `pending` row and
//! parked here until the API writes a decision (first decision wins), the
//! deadline passes, the client goes away or the gateway stops. The row is the
//! source of truth; NOTIFY events only shorten the wait (2 s poll fallback).

use crate::auth::RequestIdentity;
use crate::hygiene::AuroraHeaders;
use crate::invalidation::{Hub, stopped};
use aurora_db::PgPool;
use aurora_db::bus::{Channel, notify};
use aurora_db::repo::gateway::{self as repo, NewApproval};
use aurora_summary::ApprovalSummary;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldOutcome {
    Approved,
    Denied(Uuid),
    Expired(Uuid),
    /// The gateway is stopping (or the hold was abandoned by a sweeper).
    Restarting,
    /// The approval store could not record or read the hold (fail closed).
    StoreUnavailable,
}

pub struct HoldRequest<'a> {
    pub identity: &'a RequestIdentity,
    pub method: &'a str,
    pub host: &'a str,
    /// Redacted, without query.
    pub path: &'a str,
    pub rule: Option<(Uuid, &'a str)>,
    pub batch: &'a AuroraHeaders,
    pub summary: &'a ApprovalSummary,
}

pub struct Holds {
    pool: PgPool,
    hub: Arc<Hub>,
    instance_id: Uuid,
    timeout: Duration,
    poll: Duration,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
}

/// Marks the hold `abandoned` if the request future is dropped (client gone).
struct AbandonOnDrop {
    pool: PgPool,
    id: Uuid,
    armed: bool,
}

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (pool, id) = (self.pool.clone(), self.id);
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if let Err(e) = repo::abandon_approval(&pool, id).await {
                    tracing::warn!(error = %e, approval_id = %id, "abandoning approval failed");
                }
            });
        }
    }
}

fn clip(s: &str) -> String {
    s.chars()
        .take(aurora_core::limits::BATCH_HEADER_MAX)
        .collect()
}

impl Holds {
    pub fn new(
        pool: PgPool,
        hub: Arc<Hub>,
        instance_id: Uuid,
        timeout: Duration,
        shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Self {
        Self {
            pool,
            hub,
            instance_id,
            timeout,
            poll: POLL_INTERVAL,
            shutdown,
        }
    }

    /// Overrides the poll interval (tests).
    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    async fn emit(&self, id: Uuid, workspace_id: Uuid, event: &str) {
        let payload = json!({ "id": id, "workspace_id": workspace_id, "event": event });
        if let Err(e) = notify(&self.pool, Channel::Approval, &payload).await {
            tracing::warn!(error = %e, approval_id = %id, event, "approval notify failed");
        }
    }

    /// Map a non-pending status to an outcome.
    fn outcome(id: Uuid, status: &str) -> Option<HoldOutcome> {
        match status {
            "approved" => Some(HoldOutcome::Approved),
            "denied" => Some(HoldOutcome::Denied(id)),
            "expired" => Some(HoldOutcome::Expired(id)),
            "abandoned" => Some(HoldOutcome::Restarting),
            _ => None,
        }
    }

    /// Records the hold and waits for its resolution.
    pub async fn hold(&self, r: HoldRequest<'_>) -> HoldOutcome {
        let id = Uuid::now_v7();
        let ws = r.identity.workspace_id;
        // Register before inserting so a fast decision cannot be missed.
        let waiter = self.hub.approval_waiter(id);
        let summary = match serde_json::to_value(r.summary) {
            Ok(v) => v,
            Err(_) => return HoldOutcome::StoreUnavailable,
        };
        let row = NewApproval {
            id,
            org_id: r.identity.org_id,
            workspace_id: ws,
            agent_id: r.identity.agent_id,
            conversation_id: r.identity.conversation_id,
            turn_id: r.identity.turn_id,
            gateway_instance_id: self.instance_id,
            method: r.method.to_owned(),
            host: r.host.to_owned(),
            path: r.path.to_owned(),
            connection_id: None,
            rule_logical_id: r.rule.map(|x| x.0),
            rule_name: r.rule.map(|x| x.1.to_owned()),
            summary,
            batch_id: r.batch.batch_id.as_deref().map(clip),
            batch_label: r.batch.batch_label.as_deref().map(clip),
            batch_total: r.batch.batch_total.and_then(|n| i32::try_from(n).ok()),
            timeout_secs: self.timeout.as_secs_f64(),
        };
        if let Err(e) = repo::insert_approval(&self.pool, &row).await {
            tracing::error!(error = %e, "approval insert failed");
            return HoldOutcome::StoreUnavailable;
        }
        let mut guard = AbandonOnDrop {
            pool: self.pool.clone(),
            id,
            armed: true,
        };
        self.emit(id, ws, "created").await;

        let deadline = Instant::now() + self.timeout;
        let mut sd = self.shutdown.clone();
        let stop = async {
            match sd.as_mut() {
                Some(rx) => stopped(rx).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(stop);
        let out = loop {
            tokio::select! {
                () = &mut stop => {
                    if let Err(e) = repo::abandon_approval(&self.pool, id).await {
                        tracing::warn!(error = %e, approval_id = %id, "abandoning approval failed");
                    }
                    break HoldOutcome::Restarting;
                }
                () = tokio::time::sleep_until(deadline) => {
                    match repo::expire_approval(&self.pool, id).await {
                        Ok(true) => {
                            self.emit(id, ws, "expired").await;
                            break HoldOutcome::Expired(id);
                        }
                        // Lost the race to a decision (or another expirer): honour it.
                        Ok(false) => {
                            let st = repo::get_approval_status(&self.pool, id).await;
                            break match st {
                                Ok(Some(s)) => Self::outcome(id, &s).unwrap_or(HoldOutcome::Expired(id)),
                                _ => HoldOutcome::Expired(id),
                            };
                        }
                        // Fail closed; the API sweeper expires the row.
                        Err(e) => {
                            tracing::warn!(error = %e, approval_id = %id, "expiring approval failed");
                            break HoldOutcome::Expired(id);
                        }
                    }
                }
                () = waiter.wait() => {}
                () = tokio::time::sleep(self.poll) => {}
            }
            match repo::get_approval_status(&self.pool, id).await {
                Ok(Some(s)) => {
                    if let Some(o) = Self::outcome(id, &s) {
                        break o;
                    }
                }
                Ok(None) => break HoldOutcome::StoreUnavailable,
                Err(e) => tracing::warn!(error = %e, approval_id = %id, "approval poll failed"),
            }
        };
        guard.armed = false;
        out
    }

    /// Safety net at shutdown: abandon anything still pending for this instance.
    pub async fn abandon_all(&self) {
        if let Err(e) = repo::abandon_approvals(&self.pool, self.instance_id).await {
            tracing::warn!(error = %e, "abandoning approvals at shutdown failed");
        }
    }
}

//! Self-contained `presence.deprovision` ops (phase 3, RFE-024, D-67, SEC-58; docs/13 §4.2,
//! C-360..C-364).
//!
//! A delete that removes dedicated presences (agent, workspace, org, integration, detach)
//! works in three steps:
//!
//! 1. [`prepare`] reads the dedicated presences about to disappear and seals, per presence, a
//!    copy of the integration's credentials under the **op's own context** (purpose
//!    `channel_op_deprovision`, owner = the op id). A seal (or open) failure because the store
//!    is unavailable refuses the delete with 503 `secret_store_unavailable`; nothing is deleted.
//! 2. [`Prepared::enqueue`] inserts the ops in the delete's transaction, before the cascade.
//!    The ops carry `integration_id = NULL`, so neither the integration's nor the org's
//!    deletion removes them, and any live replica of the connector may claim them.
//! 3. [`Prepared::committed`] / [`Prepared::rollback`] discard the sealed copies that were not
//!    used (best effort).
//!
//! The stored payload holds the internal keys `credentials_ref` (the sealed copy) and `org_id`
//! (the seal context and the failure audit's scope); the claim strips both. The claim opens the
//! copy and returns it as `Operation.credentials`; at `done` / `failed` / `dropped` (or in
//! [`sweep`]) the copy is discarded and `credentials_ref` set to `null`.
//!
//! Decisions:
//! * the integration's credentials open under their own context; when they are unreadable for
//!   a reason other than an unavailable store (corrupt or foreign), no op is possible: the
//!   delete proceeds (it must stay possible) and the failure is recorded like any other failed
//!   deprovision (`error_code: credentials_unreadable`);
//! * a presence without a `platform_user_ref` has no bot yet: nothing to deprovision;
//! * a target that appeared between [`prepare`] and the transaction makes the delete answer
//!   409 `conflict` (retry); one that vanished only has its sealed copy discarded.

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::connectors::integration_seal_ctx;
use crate::domains::connectors::ops::{self, NewOp};
use aurora_core::limits::DEPROVISION_RETRY_WINDOW;
use aurora_core::{ApiError, Secret};
use aurora_db::DbError;
use aurora_db::repo::channel_ops;
use aurora_db::repo::channel_presences::{self as repo, DeprovisionTarget};
use aurora_protocol::connector::DeprovisionReason;
use aurora_secrets::{Purpose, SealContext, SecretError, SecretRef, ValueSource};
use serde_json::{Value, json};
use sqlx::PgConnection;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use uuid::Uuid;

/// Period of the expiry / pruning job ([`spawn`]).
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// What is being deleted.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    Agent(Uuid),
    Workspace(Uuid),
    Org(Uuid),
    Integration(Uuid),
    /// A single presence (detach).
    Presence(Uuid),
}

impl Scope {
    fn reason(self) -> DeprovisionReason {
        match self {
            Self::Agent(_) => DeprovisionReason::AgentDeleted,
            Self::Workspace(_) => DeprovisionReason::WorkspaceDeleted,
            Self::Org(_) => DeprovisionReason::OrgDeleted,
            Self::Integration(_) | Self::Presence(_) => DeprovisionReason::Detached,
        }
    }

    async fn targets(self, conn: &mut PgConnection) -> Result<Vec<DeprovisionTarget>, DbError> {
        let all = match self {
            Self::Agent(id) => repo::dedicated_for_agent(&mut *conn, id).await?,
            Self::Workspace(id) => repo::dedicated_for_workspace(&mut *conn, id).await?,
            Self::Org(id) => repo::dedicated_for_org(&mut *conn, id).await?,
            Self::Integration(id) => repo::dedicated_for_integration(&mut *conn, id).await?,
            Self::Presence(id) => repo::dedicated_for_presence(&mut *conn, id).await?,
        };
        // No bot account yet (provisioning never finished): nothing to remove.
        Ok(all
            .into_iter()
            .filter(|t| t.platform_user_ref.is_some())
            .collect())
    }
}

/// Seal context of an op's credential copy.
pub fn op_seal_ctx(op_id: Uuid, org_id: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::ChannelOpDeprovision,
        owner_id: op_id,
        org_id,
        workspace_id: None,
    }
}

fn store_unavailable() -> ApiError {
    ApiError::unavailable(
        "secret_store_unavailable",
        "The secret store is unavailable; nothing was deleted. Try again shortly.",
    )
}

struct Item {
    op_id: Uuid,
    target: DeprovisionTarget,
    sealed: SecretRef,
}

/// Sealed credential copies for the dedicated presences about to be deleted.
pub struct Prepared {
    scope: Scope,
    items: Vec<Item>,
    /// Targets whose integration credentials cannot be read: no op can act.
    unreadable: Vec<DeprovisionTarget>,
    used: HashSet<Uuid>,
}

/// Seal the credential copies for `scope` (step 1); 503 `secret_store_unavailable` when the
/// store cannot be used.
pub async fn prepare(s: &AppState, scope: Scope) -> Result<Prepared, ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let targets = scope.targets(&mut conn).await?;
    drop(conn);
    let mut prepared = Prepared {
        scope,
        items: Vec::new(),
        unreadable: Vec::new(),
        used: HashSet::new(),
    };
    // One decryption per integration.
    let mut creds: HashMap<Uuid, Option<Vec<u8>>> = HashMap::new();
    for t in targets {
        let plain = match creds.get(&t.integration_id) {
            Some(p) => p.clone(),
            None => {
                let opened = open_integration(s, &t).await;
                let plain = match opened {
                    Ok(p) => p,
                    Err(e) => {
                        prepared.rollback(s).await;
                        return Err(e);
                    }
                };
                creds.insert(t.integration_id, plain.clone());
                plain
            }
        };
        let Some(plain) = plain else {
            prepared.unreadable.push(t);
            continue;
        };
        let op_id = Uuid::now_v7();
        match s
            .vault
            .seal(&op_seal_ctx(op_id, t.org_id), Secret::from(plain))
            .await
        {
            Ok(sealed) => prepared.items.push(Item {
                op_id,
                target: t,
                sealed,
            }),
            Err(e) => {
                tracing::error!(error = %e, "deprovision credential copy not sealed");
                prepared.rollback(s).await;
                return Err(store_unavailable());
            }
        }
    }
    Ok(prepared)
}

/// The integration's credentials as JSON bytes. `Ok(None)`: unreadable for good.
async fn open_integration(
    s: &AppState,
    t: &DeprovisionTarget,
) -> Result<Option<Vec<u8>>, ApiError> {
    let Ok(sealed) = t.integration_credentials_ref.parse::<SecretRef>() else {
        return Ok(None);
    };
    match s
        .vault
        .open(&integration_seal_ctx(t.integration_id, t.org_id), &sealed)
        .await
    {
        Ok(plain) => Ok(Some(plain.expose().clone())),
        Err(SecretError::StoreUnavailable | SecretError::Internal) => Err(store_unavailable()),
        Err(_) => Ok(None),
    }
}

impl Prepared {
    /// Step 2: insert the ops in the delete's transaction, before the cascade. A target that
    /// was not prepared (it appeared meanwhile) answers 409 so the caller retries.
    pub async fn enqueue(&mut self, tx: &mut PgConnection) -> Result<(), ApiError> {
        let current = self.scope.targets(&mut *tx).await?;
        for t in &current {
            let item = self
                .items
                .iter()
                .find(|i| i.target.presence_id == t.presence_id);
            let Some(item) = item else {
                if self
                    .unreadable
                    .iter()
                    .any(|u| u.presence_id == t.presence_id)
                {
                    continue;
                }
                return Err(ApiError::conflict(
                    "conflict",
                    "Channel presences changed while deleting; try again.",
                ));
            };
            let payload = json!({
                "presence_id": t.presence_id,
                "reason": self.scope.reason().as_str(),
                "platform_user_ref": t.platform_user_ref,
                "handle": t.handle,
                "tenant_ref": t.tenant_ref,
                "tenant_label": t.tenant_label,
                "config": t.config,
                // Internal: stripped at claim.
                "credentials_ref": item.sealed.to_column(),
                "org_id": t.org_id,
            });
            let id = ops::enqueue_with_id(
                tx,
                item.op_id,
                NewOp {
                    kind: "presence.deprovision".to_owned(),
                    connector_id: Some(t.connector_id.clone()),
                    integration_id: None,
                    presence_id: Some(t.presence_id),
                    payload,
                    dedupe_key: Some(format!("presence.deprovision:{}", t.presence_id)),
                },
            )
            .await?;
            if id == item.op_id {
                self.used.insert(item.op_id);
            }
        }
        Ok(())
    }

    /// The delete committed: drop the copies no op uses, record the unreadable ones.
    pub async fn committed(self, s: &AppState) {
        for i in &self.items {
            if !self.used.contains(&i.op_id) {
                discard_ref(s, i.op_id, i.target.org_id, &i.sealed).await;
            }
        }
        for t in &self.unreadable {
            record_failure(
                s,
                t.org_id,
                t.presence_id,
                &t.connector_id,
                &t.tenant_label,
                t.platform_user_ref.as_deref(),
                t.handle.as_deref(),
                "credentials_unreadable",
            );
        }
    }

    /// The delete did not commit: discard every sealed copy (best effort).
    pub async fn rollback(&self, s: &AppState) {
        for i in &self.items {
            discard_ref(s, i.op_id, i.target.org_id, &i.sealed).await;
        }
    }
}

/// Delete the stored copy behind `sealed` (best effort; never logs the value).
async fn discard_ref(s: &AppState, op_id: Uuid, org_id: Uuid, sealed: &SecretRef) {
    if let Err(e) = s
        .vault
        .discard(&op_seal_ctx(op_id, org_id), sealed, ValueSource::Managed)
        .await
    {
        tracing::warn!(error = %e, op_id = %op_id, "deprovision credential copy not discarded");
    }
}

/// Discard the sealed copy of a finished op and null `credentials_ref`. When the discard fails
/// the reference stays, so [`sweep`] retries it.
pub async fn discard_op_copy(s: &AppState, op_id: Uuid, org_id: Option<Uuid>, sealed: &str) {
    let (Some(org), Ok(r)) = (org_id, sealed.parse::<SecretRef>()) else {
        // Not a reference we can act on: drop it so it is not kept.
        if let Err(e) = channel_ops::clear_credentials_ref(&s.pool, op_id).await {
            tracing::warn!(error = %e, op_id = %op_id, "deprovision credential reference not cleared");
        }
        return;
    };
    match s
        .vault
        .discard(&op_seal_ctx(op_id, org), &r, ValueSource::Managed)
        .await
    {
        Ok(()) | Err(SecretError::NotFound) => {
            if let Err(e) = channel_ops::clear_credentials_ref(&s.pool, op_id).await {
                tracing::warn!(error = %e, op_id = %op_id, "deprovision credential reference not cleared");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, op_id = %op_id, "deprovision credential copy not discarded");
        }
    }
}

/// A deprovision that did not happen (C-364): best-effort audit row and the counter. Never
/// carries credentials.
#[allow(clippy::too_many_arguments)]
pub fn record_failure(
    s: &AppState,
    org_id: Uuid,
    presence_id: Uuid,
    connector_id: &str,
    tenant_label: &str,
    platform_user_ref: Option<&str>,
    handle: Option<&str>,
    error_code: &str,
) {
    metrics::counter!("aurora_channel_deprovision_failed_total").increment(1);
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::system("channel-deprovision"),
            "presence.deprovision_failed",
            Outcome::Failure,
        )
        .org(org_id)
        .target("presence", presence_id)
        .metadata(json!({
            "connector_id": connector_id,
            "tenant_label": tenant_label,
            "platform_user_ref": platform_user_ref,
            "handle": handle,
            "error_code": error_code,
        })),
    );
}

/// [`record_failure`] from an op's stored payload.
pub fn record_op_failure(s: &AppState, op: &channel_ops::OpRow, error_code: &str) {
    let str_of = |k: &str| op.payload.get(k).and_then(Value::as_str);
    let (Some(org), Some(presence)) = (
        op.payload
            .get("org_id")
            .and_then(Value::as_str)
            .and_then(|v| v.parse().ok()),
        op.presence_id,
    ) else {
        tracing::warn!(op_id = %op.id, "failed deprovision without an org; audit row skipped");
        metrics::counter!("aurora_channel_deprovision_failed_total").increment(1);
        return;
    };
    record_failure(
        s,
        org,
        presence,
        &op.connector_id,
        str_of("tenant_label").unwrap_or_default(),
        str_of("platform_user_ref"),
        str_of("handle"),
        error_code,
    );
}

/// One expiry / pruning pass (idempotent, safe on every replica): deprovision ops still open
/// after [`DEPROVISION_RETRY_WINDOW`] become `failed`; every finished op's sealed copy that is
/// still referenced is discarded. Returns `(expired, pruned)`.
pub async fn sweep(s: &AppState) -> Result<(usize, usize), ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let expired =
        channel_ops::expire_deprovision(&mut tx, DEPROVISION_RETRY_WINDOW.as_secs_f64()).await?;
    for e in &expired {
        ops::notify_op_result(&mut tx, e.id).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    for e in &expired {
        let str_of = |k: &str| e.payload.get(k).and_then(Value::as_str);
        match (
            e.org_id(),
            str_of("presence_id").and_then(|v| v.parse().ok()),
        ) {
            (Some(org), Some(presence)) => record_failure(
                s,
                org,
                presence,
                &e.connector_id,
                str_of("tenant_label").unwrap_or_default(),
                str_of("platform_user_ref"),
                str_of("handle"),
                "unavailable",
            ),
            _ => {
                metrics::counter!("aurora_channel_deprovision_failed_total").increment(1);
            }
        }
    }
    let mut pruned = 0;
    for e in channel_ops::finished_with_copy(&s.pool, 200).await? {
        if let Some(r) = e.credentials_ref() {
            discard_op_copy(s, e.id, e.org_id(), r).await;
            pruned += 1;
        }
    }
    Ok((expired.len(), pruned))
}

/// Run [`sweep`] every [`SWEEP_EVERY`] on this replica.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match sweep(&state).await {
                Ok((e, p)) if e + p > 0 => {
                    tracing::info!(expired = e, pruned = p, "presence deprovision sweep");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "presence deprovision sweep failed"),
            }
        }
    });
}

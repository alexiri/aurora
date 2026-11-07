//! Reconcile and orphan reaping (`11 §Reconcile and orphan reaping`).
//!
//! All API calls happen before anything is destroyed, so any API error aborts the
//! pass with zero destruction.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use aurora_config::OrphanReap;
use aurora_protocol::runner::{DesiredState, StrayObject, StraysRequest};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::api::{ApiClient, ApiError};
use crate::backend::{Backend, BackendError, ManagedObject, ObjectKind};
use crate::executor::Executor;

#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Backend(#[from] BackendError),
}

pub struct ReconcileCtx<'a> {
    pub runner_id: Uuid,
    /// First 16 hex of `sha256(token)`.
    pub installation: &'a str,
    pub mode: OrphanReap,
    pub grace: Duration,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileOutcome {
    pub destroyed: Vec<String>,
    /// Objects that `log_only` mode left alone.
    pub would_destroy: Vec<String>,
    pub strays: usize,
}

pub async fn reconcile_once(
    api: &ApiClient,
    backend: &dyn Backend,
    exec: &Executor,
    ctx: &ReconcileCtx<'_>,
) -> Result<ReconcileOutcome, ReconcileError> {
    let runner = ctx.runner_id.to_string();
    let objects: Vec<ManagedObject> = backend
        .list_managed()
        .await?
        .into_iter()
        // Objects of other installations are never touched.
        .filter(|o| o.installation.as_deref() == Some(ctx.installation))
        .collect();
    let expected: HashMap<Uuid, _> = api
        .list_sandboxes()
        .await?
        .into_iter()
        .map(|a| (a.sandbox_id, a))
        .collect();

    let mut doomed: Vec<(ManagedObject, &'static str)> = Vec::new();
    let mut strays: Vec<StrayObject> = Vec::new();
    let mut foreign: Vec<ManagedObject> = Vec::new();

    for o in objects {
        if o.runner.as_deref() != Some(runner.as_str()) {
            foreign.push(o);
            continue;
        }
        if o.sandbox_id.is_some_and(|s| exec.is_busy(s)) {
            continue;
        }
        match o.sandbox_id.and_then(|s| expected.get(&s)) {
            None => doomed.push((o, "sandbox unknown to control plane")),
            Some(a) if a.desired == DesiredState::Deleted => doomed.push((o, "sandbox deleted")),
            Some(a) if o.kind == ObjectKind::Container => match (a.boot_id, o.boot_id) {
                (Some(want), Some(have)) if want != have => doomed.push((o, "stale boot")),
                (None, _) if o.running => strays.push(StrayObject {
                    sandbox_id: o.sandbox_id,
                    boot_id: o.boot_id,
                    container_ref: o.reference.clone(),
                    created_at: o.created_at.to_rfc3339(),
                }),
                _ => {}
            },
            Some(_) => {}
        }
    }

    // Installation sweep: other runner labels (or none).
    let ids: Vec<Uuid> = foreign
        .iter()
        .filter_map(|o| o.sandbox_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let known: HashSet<Uuid> = if ids.is_empty() {
        HashSet::new()
    } else {
        api.check_sandboxes(ids).await?.into_iter().collect()
    };
    for o in foreign {
        if !o.sandbox_id.is_some_and(|s| known.contains(&s)) {
            doomed.push((o, "orphan of this installation"));
        }
    }

    let grace = chrono::Duration::from_std(ctx.grace).unwrap_or(chrono::Duration::MAX);
    let mut out = ReconcileOutcome {
        strays: strays.len(),
        ..Default::default()
    };
    for (o, why) in doomed {
        if ctx.now.signed_duration_since(o.created_at) < grace {
            continue;
        }
        match ctx.mode {
            OrphanReap::LogOnly => {
                tracing::warn!(object = %o.reference, why, "orphan found (log_only; not destroyed)");
                out.would_destroy.push(o.reference);
            }
            OrphanReap::Reap => match backend.destroy(&o).await {
                Ok(()) => {
                    tracing::info!(object = %o.reference, why, "orphan destroyed");
                    out.destroyed.push(o.reference);
                }
                Err(e) => {
                    tracing::error!(object = %o.reference, error = %e, "orphan destroy failed")
                }
            },
        }
    }
    if !strays.is_empty()
        && let Err(e) = api.strays(&StraysRequest { objects: strays }).await
    {
        tracing::warn!(error = %e, "stray report failed");
    }
    Ok(out)
}

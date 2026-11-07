//! register / config / ops loops.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aurora_core::limits;
use aurora_protocol::common::Validate;
use aurora_protocol::connector::*;
use thiserror::Error;
use tokio::sync::{Semaphore, watch};
use uuid::Uuid;

use crate::client::{ClientError, ConfigPoll, CoreClient, RetryPolicy};
use crate::connector::{Connector, IntegrationHandle};
use crate::sink::{EventSink, spawn_event_poster};

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("core rejected the connector credentials: {0}")]
    Unauthorized(ClientError),
    #[error("client: {0}")]
    Client(#[from] ClientError),
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// API origin, e.g. `https://aurora.example`.
    pub core_url: String,
    /// Anchor token `aun_...`.
    pub token: String,
    /// Detached manifest signature sent with `register`.
    pub manifest_signature: String,
    pub version: String,
    pub replica_hint: Option<String>,
    pub retry: RetryPolicy,
    /// Lease of a claimed op; execution is abandoned (no result sent) when it lapses.
    pub op_lease: Duration,
    /// Max ops claimed per call.
    pub claim_max: u32,
    /// Max ops executing concurrently.
    pub op_concurrency: usize,
    /// Overrides the advertised poll intervals (tests).
    pub config_interval: Option<Duration>,
    pub ops_wait_secs: Option<u32>,
}

impl RuntimeConfig {
    pub fn new(core_url: &str, token: &str) -> Self {
        Self {
            core_url: core_url.to_owned(),
            token: token.to_owned(),
            manifest_signature: String::new(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            replica_hint: None,
            retry: RetryPolicy::default(),
            op_lease: limits::OP_LEASE,
            claim_max: 10,
            op_concurrency: 16,
            config_interval: None,
            ops_wait_secs: None,
        }
    }
}

async fn sleep_or_shutdown(d: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => false,
        _ = shutdown.wait_for(|s| *s) => true,
    }
}

/// Runs the connector until `shutdown` turns true or the token is rejected.
pub async fn run(
    connector: Arc<dyn Connector>,
    cfg: RuntimeConfig,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    let client = Arc::new(CoreClient::new(&cfg.core_url, &cfg.token, cfg.retry)?);
    let req = RegisterRequest {
        manifest: connector.manifest(),
        signature: cfg.manifest_signature.clone(),
        version: cfg.version.clone(),
        replica_hint: cfg.replica_hint.clone(),
    };
    let mut n = 0;
    let reg = loop {
        match client.register(&req).await {
            Ok(r) => break r,
            Err(e) if e.is_auth() => return Err(RuntimeError::Unauthorized(e)),
            Err(e) if e.is_retryable() => {
                tracing::warn!(error = %e, "register failed; retrying");
                if sleep_or_shutdown(cfg.retry.delay(n), &mut shutdown).await {
                    return Ok(());
                }
                n += 1;
            }
            Err(e) => return Err(e.into()),
        }
    };
    tracing::info!(status = reg.status.as_str(), replica = %reg.replica_id, "registered");

    connector.attach_core_client(client.clone());
    let sink = spawn_event_poster(client.clone(), shutdown.clone());
    let ops_wait = cfg
        .ops_wait_secs
        .unwrap_or(reg.poll.ops_wait_secs.min(MAX_WAIT_SECS));
    let ops_task = tokio::spawn(ops_loop(
        connector.clone(),
        client.clone(),
        cfg.clone(),
        ops_wait,
        shutdown.clone(),
    ));

    let interval = cfg
        .config_interval
        .unwrap_or_else(|| Duration::from_secs(u64::from(reg.poll.config_secs.max(1))));
    let result = config_loop(&*connector, &client, sink, interval, shutdown.clone()).await;

    ops_task.abort();
    if let Err(e) = client.retire().await {
        tracing::warn!(error = %e, "retire failed");
    }
    result
}

struct Owned {
    handle: IntegrationHandle,
    integration: Integration,
    presences: Vec<Presence>,
}

async fn config_loop(
    connector: &dyn Connector,
    client: &CoreClient,
    sink: EventSink,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    let mut owned: HashMap<Uuid, Owned> = HashMap::new();
    let mut etag: Option<String> = None;
    let result = loop {
        match client.config(etag.as_deref()).await {
            Ok(ConfigPoll::NotModified) => {}
            Ok(ConfigPoll::Changed(c)) => {
                let complete = apply_config(connector, &sink, &mut owned, *c.clone()).await;
                // An incomplete apply is retried on the next poll.
                etag = complete.then(|| c.etag.clone());
            }
            Err(e) if e.is_auth() => break Err(RuntimeError::Unauthorized(e)),
            Err(e) => tracing::warn!(error = %e, "config poll failed"),
        }
        if sleep_or_shutdown(interval, &mut shutdown).await {
            break Ok(());
        }
    };
    for o in owned.values() {
        o.handle.close();
    }
    result
}

/// Returns true when every owned integration is in the desired state.
async fn apply_config(
    connector: &dyn Connector,
    sink: &EventSink,
    owned: &mut HashMap<Uuid, Owned>,
    c: ConfigResponse,
) -> bool {
    let mut complete = true;
    let mut by_integration: HashMap<Uuid, Vec<Presence>> = HashMap::new();
    for p in c.presences {
        by_integration.entry(p.integration_id).or_default().push(p);
    }
    owned.retain(|id, o| {
        let keep = c.integrations.iter().any(|i| i.id == *id);
        if !keep {
            o.handle.close();
        }
        keep
    });
    for integration in c.integrations {
        let presences = by_integration.remove(&integration.id).unwrap_or_default();
        match owned.get_mut(&integration.id) {
            Some(o) => {
                if o.integration != integration || o.presences != presences {
                    match connector
                        .update_integration(&o.handle, integration.clone(), presences.clone())
                        .await
                    {
                        Ok(()) => {
                            o.integration = integration;
                            o.presences = presences;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "update_integration failed");
                            complete = false;
                        }
                    }
                }
            }
            None => {
                match connector
                    .open_integration(integration.clone(), presences.clone(), sink.clone())
                    .await
                {
                    Ok(handle) => {
                        owned.insert(
                            integration.id,
                            Owned {
                                handle,
                                integration,
                                presences,
                            },
                        );
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "open_integration failed");
                        complete = false;
                    }
                }
            }
        }
    }
    complete
}

async fn ops_loop(
    connector: Arc<dyn Connector>,
    client: Arc<CoreClient>,
    cfg: RuntimeConfig,
    wait_secs: u32,
    mut shutdown: watch::Receiver<bool>,
) {
    let permits = Arc::new(Semaphore::new(cfg.op_concurrency.max(1)));
    let mut failures = 0u32;
    loop {
        if *shutdown.borrow() {
            return;
        }
        // Wait for at least one free slot so a claimed op never queues past its lease.
        let Ok(first) = permits.clone().acquire_owned().await else {
            return;
        };
        let free = permits.available_permits() as u32 + 1;
        let max = free.min(cfg.claim_max).clamp(1, MAX_CLAIM);
        let claim_req = ClaimRequest { max, wait_secs };
        let claimed = tokio::select! {
            r = client.claim_ops(&claim_req) => r,
            _ = shutdown.wait_for(|s| *s) => return,
        };
        let ops = match claimed {
            Ok(r) => {
                failures = 0;
                r.ops
            }
            Err(e) => {
                // A connector that registered while its manifest awaits approval is not
                // active yet: keep polling until an admin approves it.
                let pending = matches!(
                    &e,
                    ClientError::Status { code, .. } if code == "connector_not_active"
                );
                if pending {
                    tracing::info!("connector not approved yet; waiting");
                    drop(first);
                    if sleep_or_shutdown(Duration::from_secs(1), &mut shutdown).await {
                        return;
                    }
                    continue;
                }
                if e.is_auth() {
                    tracing::error!(error = %e, "ops claim unauthorized; stopping ops loop");
                    return;
                }
                tracing::warn!(error = %e, "ops claim failed");
                let d = cfg.retry.delay(failures);
                failures = failures.saturating_add(1);
                drop(first);
                if sleep_or_shutdown(d, &mut shutdown).await {
                    return;
                }
                continue;
            }
        };
        if ops.is_empty() {
            drop(first);
            if wait_secs == 0 && sleep_or_shutdown(Duration::from_millis(250), &mut shutdown).await
            {
                return;
            }
            continue;
        }
        let mut first = Some(first);
        for op in ops {
            let permit = match first.take() {
                Some(p) => p,
                None => match permits.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => return,
                },
            };
            let connector = connector.clone();
            let client = client.clone();
            let lease = cfg.op_lease;
            tokio::spawn(async move {
                let _permit = permit;
                if let Some(result) = execute_one(&*connector, op, lease).await
                    && let Err(e) = client
                        .post_results(&ResultsRequest {
                            results: vec![result],
                        })
                        .await
                {
                    tracing::error!(error = %e, "posting op result failed; the lease will lapse");
                }
            });
        }
    }
}

/// Executes one op. Unknown kinds answer `unsupported`; an op still running
/// when its lease lapses is abandoned (`None`): the core re-claims it with
/// `maybe_sent = true`, so a late result must not be reported.
pub async fn execute_one(
    connector: &dyn Connector,
    op: Operation,
    lease: Duration,
) -> Option<OpResult> {
    let op_id = op.op_id;
    if matches!(op.kind, OpKind::Other(_)) {
        tracing::warn!(
            kind = op.kind.as_str(),
            "unknown op kind; answering unsupported"
        );
        return Some(OpResult::err(
            op_id,
            OpError::new(ErrorCode::Unsupported, "unknown operation kind"),
        ));
    }
    match tokio::time::timeout(lease, connector.execute(op)).await {
        Ok(mut r) => {
            r.op_id = op_id;
            if let Err(e) = r.validate() {
                return Some(OpResult::err(
                    op_id,
                    OpError::new(ErrorCode::Invalid, e.to_string()),
                ));
            }
            Some(r)
        }
        Err(_) => {
            tracing::warn!(%op_id, "op exceeded its lease; abandoning");
            None
        }
    }
}

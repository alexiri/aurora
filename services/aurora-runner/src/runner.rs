//! Runner loop: register, poll, dispatch, report, reconcile, retire.

use std::sync::Arc;
use std::time::Duration;

use aurora_config::{OrphanReap, RunnerConfig};
use aurora_protocol::runner::{
    Capacity, MAX_WAIT_SECS, RegisterRequest, ReportBody, WorkRequest, frame_report_id,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::api::{ApiClient, ApiError, RetryPolicy};
use crate::backend::Backend;
use crate::executor::{Executor, ExecutorSettings};
use crate::hub::{ChannelHub, HubEvent};
use crate::reconcile::{ReconcileCtx, reconcile_once};
use crate::reporter::{self, ReportSink, report};

/// Empty polls returning faster than this are paced (guards against a hot loop).
const MIN_EMPTY_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("registration rejected: {0}")]
    Auth(ApiError),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("shutdown requested before registration")]
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct RunnerSettings {
    pub api_url: String,
    pub token: String,
    pub version: String,
    pub advertised_host: String,
    pub orphan_reap: OrphanReap,
    pub retry: RetryPolicy,
    pub executor: ExecutorSettings,
    pub drain_timeout: Duration,
    pub min_empty_poll: Duration,
}

impl RunnerSettings {
    pub fn from_config(cfg: &RunnerConfig) -> Self {
        Self {
            api_url: cfg.api_url.clone(),
            token: cfg.token.expose().clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            advertised_host: cfg
                .runner
                .advertised_host
                .clone()
                .unwrap_or_else(|| "aurora-runner".into()),
            orphan_reap: cfg.orphan_reap,
            retry: RetryPolicy::default(),
            executor: ExecutorSettings {
                max_concurrency: cfg.runner.max_sandboxes.max(1) as usize,
                ..ExecutorSettings::default()
            },
            drain_timeout: Duration::from_secs(30),
            min_empty_poll: MIN_EMPTY_POLL,
        }
    }
}

/// First 16 hex of `sha256(token)`; labels every infrastructure object.
pub fn installation_fingerprint(token: &str) -> String {
    let d = Sha256::digest(token.as_bytes());
    hex::encode(d)[..16].to_owned()
}

pub struct Runner {
    settings: RunnerSettings,
    backend: Arc<dyn Backend>,
    hub: Arc<dyn ChannelHub>,
}

impl Runner {
    pub fn new(
        settings: RunnerSettings,
        backend: Arc<dyn Backend>,
        hub: Arc<dyn ChannelHub>,
    ) -> Self {
        Self {
            settings,
            backend,
            hub,
        }
    }

    /// Runs until `shutdown` turns true, then drains, flushes reports and retires.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> Result<(), RunnerError> {
        let s = &self.settings;
        let api = Arc::new(ApiClient::new(&s.api_url, &s.token, s.retry)?);
        let caps = self.backend.capabilities();
        let max = caps.max_sandboxes;
        let reg = RegisterRequest {
            version: s.version.clone(),
            backend: self.backend.id().to_owned(),
            capabilities: caps,
            advertised_host: s.advertised_host.clone(),
        };
        let registered = loop {
            match api.register(&reg).await {
                Ok(r) => break r,
                Err(e) if e.is_auth() => return Err(RunnerError::Auth(e)),
                Err(e) => {
                    tracing::warn!(error = %e, "registration failed; retrying");
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(2)) => {}
                        _ = shutdown.wait_for(|v| *v) => return Err(RunnerError::Shutdown),
                    }
                }
            }
        };
        tracing::info!(runner_id = %registered.runner_id, replica_id = %registered.replica_id, "registered");
        self.hub.attach_api(api.clone());
        let cfg = registered.config;
        let fingerprint = installation_fingerprint(&s.token);

        let (sink, reporter_task) = reporter::spawn(api.clone(), shutdown.clone());
        let exec = Executor::new(
            self.backend.clone(),
            self.hub.clone(),
            sink.clone(),
            s.executor.clone(),
        );
        if let Ok(objs) = self.backend.list_managed().await {
            let rid = registered.runner_id.to_string();
            exec.seed_running(
                objs.iter()
                    .filter(|o| {
                        o.running
                            && o.runner.as_deref() == Some(rid.as_str())
                            && o.installation.as_deref() == Some(fingerprint.as_str())
                    })
                    .filter_map(|o| o.sandbox_id),
            );
        }

        let translator = self.hub.take_events().map(|mut rx| {
            let sink = sink.clone();
            tokio::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    translate(&sink, ev);
                }
            })
        });

        let reconciler = {
            let (api, backend, exec) = (api.clone(), self.backend.clone(), exec.clone());
            let (mode, runner_id, fp) = (s.orphan_reap, registered.runner_id, fingerprint.clone());
            let every = Duration::from_secs(u64::from(cfg.reconcile_interval_secs.max(1)));
            let grace = Duration::from_secs(u64::from(cfg.orphan_grace_secs));
            let mut stop = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = tokio::time::sleep(every) => {}
                        _ = stop.wait_for(|v| *v) => return,
                    }
                    if !api.is_reconciler() {
                        continue;
                    }
                    let ctx = ReconcileCtx {
                        runner_id,
                        installation: &fp,
                        mode,
                        grace,
                        now: Utc::now(),
                    };
                    if let Err(e) = reconcile_once(&api, backend.as_ref(), &exec, &ctx).await {
                        tracing::warn!(error = %e, "reconcile aborted");
                    }
                }
            })
        };

        let wait_secs = cfg.poll_wait_secs.min(MAX_WAIT_SECS);
        let mut failures = 0u32;
        while !*shutdown.borrow() {
            let req = WorkRequest {
                wait_secs,
                capacity: Capacity {
                    running: exec.running_count(),
                    max,
                },
            };
            let started = tokio::time::Instant::now();
            let polled = tokio::select! {
                r = api.work(&req) => r,
                _ = shutdown.wait_for(|v| *v) => break,
            };
            match polled {
                Ok(resp) => {
                    failures = 0;
                    let empty = resp.items.is_empty();
                    for item in resp.items {
                        exec.submit(item);
                    }
                    if empty && started.elapsed() < s.min_empty_poll {
                        tokio::time::sleep(s.min_empty_poll).await;
                    }
                }
                Err(e) if e.is_auth() => {
                    tracing::error!(error = %e, "work poll rejected");
                    tokio::time::sleep(s.retry.max).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "work poll failed");
                    let d = s.retry.delay(failures);
                    failures = failures.saturating_add(1);
                    tokio::select! {
                        () = tokio::time::sleep(d) => {}
                        _ = shutdown.wait_for(|v| *v) => break,
                    }
                }
            }
        }

        // Clean shutdown: stop reconcile/translation, finish steps, flush, retire.
        reconciler.abort();
        if !exec.drain(s.drain_timeout).await {
            tracing::warn!("executor did not drain before the deadline");
        }
        if let Some(t) = translator {
            t.abort();
        }
        sink.finish();
        let _ = reporter_task.await;
        if let Err(e) = api.retire().await {
            tracing::warn!(error = %e, "retire failed");
        }
        Ok(())
    }
}

/// Supervisor frame -> report (1:1; identity from the channel, id per S-07).
pub fn translate(sink: &ReportSink, ev: HubEvent) {
    let HubEvent {
        sandbox_id,
        boot_id,
        seq,
        frame,
    } = ev;
    let mut body = match frame.body {
        serde_json::Value::Object(m) => m,
        serde_json::Value::Null => serde_json::Map::new(),
        _ => {
            tracing::warn!(kind = %frame.kind, "frame body is not an object; dropped");
            return;
        }
    };
    let kind = match frame.kind.as_str() {
        "ready" => "supervisor.ready".to_owned(),
        // Single-event frame becomes a one-event batch.
        "turn.event" => {
            let (Some(turn_id), Some(n), Some(event)) = (
                body.remove("turn_id"),
                body.remove("n"),
                body.remove("event"),
            ) else {
                tracing::warn!("malformed turn.event frame; dropped");
                return;
            };
            body.insert("turn_id".into(), turn_id);
            body.insert(
                "events".into(),
                serde_json::json!([{ "n": n, "event": event }]),
            );
            "turn.events".to_owned()
        }
        // Not W5 reports: tool/memory/file frames are plane calls handled by the hub (RUN-002).
        "tool.call" | "memory.write" | "file.part" | "file.commit" | "ack" => return,
        other => other.to_owned(),
    };
    body.insert("kind".into(), serde_json::Value::String(kind.clone()));
    match serde_json::from_value::<ReportBody>(serde_json::Value::Object(body)) {
        Ok(ReportBody::Other(_)) => tracing::warn!(%kind, "unknown supervisor frame kind dropped"),
        Ok(b) => sink.send(report(
            frame_report_id(boot_id, seq),
            sandbox_id,
            boot_id,
            b,
        )),
        Err(e) => tracing::warn!(%kind, error = %e, "undecodable supervisor frame dropped"),
    }
}

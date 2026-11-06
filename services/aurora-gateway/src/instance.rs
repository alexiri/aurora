//! `gateway_instances` bookkeeping (docs/03 §gateway_instances).

use crate::invalidation::stopped;
use aurora_db::PgPool;
use aurora_db::repo::gateway as repo;
use std::time::Duration;
use uuid::Uuid;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Register the row (retrying until the DB is reachable), heartbeat every
/// `interval`, and mark `stopped_at` when `shutdown` flips to true.
pub async fn run(
    pool: PgPool,
    id: Uuid,
    interval: Duration,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let host = hostname();
    loop {
        match repo::register_instance(&pool, id, &host, env!("CARGO_PKG_VERSION")).await {
            Ok(()) => break,
            Err(e) => {
                tracing::warn!(error = %e, "gateway instance registration failed; retrying");
                tokio::select! {
                    _ = stopped(&mut shutdown) => return,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
        }
    }
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = stopped(&mut shutdown) => break,
            _ = tick.tick() => {
                if let Err(e) = repo::heartbeat_instance(&pool, id).await {
                    tracing::warn!(error = %e, "gateway heartbeat failed");
                }
            }
        }
    }
    if let Err(e) = repo::stop_instance(&pool, id).await {
        tracing::warn!(error = %e, "gateway instance stop failed");
    }
}

//! Web Push background jobs (C-833, C-834): the first-start key, the daily key destruction and
//! subscription pruning. A no-op while `push.enabled` is off: no key is generated, nothing is sent.
use crate::app::AppState;
use std::time::Duration;

const DAILY: Duration = Duration::from_secs(86_400);

/// Daily: retired keys past `PUSH_VAPID_RETIRE` (or unreferenced) are destroyed with their sealed
/// secret, and subscriptions of revoked or expired sessions are pruned (D-266(c)).
pub async fn daily(state: &AppState) {
    match super::vapid::destroy_expired(state).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(keys = n, "retired Web Push keys destroyed"),
        Err(e) => tracing::warn!(error = %e.message(), "Web Push key destruction failed"),
    }
    match aurora_db::repo::push::prune_dead_sessions(&state.pool).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            subscriptions = n,
            "Web Push subscriptions of dead sessions pruned"
        ),
        Err(e) => tracing::warn!(error = %e, "Web Push subscription pruning failed"),
    }
}

pub fn spawn(state: AppState) {
    if !state.push.enabled {
        return;
    }
    super::deliver::spawn(state.clone());
    tokio::spawn(async move {
        if let Err(e) = super::ensure_ready(&state).await {
            tracing::warn!(error = %e.message(), "Web Push key could not be prepared");
        }
        let mut tick = tokio::time::interval(DAILY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            daily(&state).await;
        }
    });
}

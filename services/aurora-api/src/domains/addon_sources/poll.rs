//! The addon source poll job (C-598, D-250): claims due sources under their lease and
//! runs the shared check path with `trigger = "scheduled"`. Like the library refresh, it
//! is safe on every replica at once.

use std::time::Duration;

use crate::app::AppState;
use crate::audit::AuditActor;

const TICK: Duration = Duration::from_secs(60);

/// Starts the poll loop of this replica.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = super::poll_once(&state).await {
                tracing::warn!(error = %e.message(), "addon source poll failed");
            }
        }
    });
}

/// The `AuditActor` the scheduled path audits with.
#[allow(dead_code)]
pub(crate) fn actor() -> AuditActor {
    AuditActor::system("addon-source-poll")
}

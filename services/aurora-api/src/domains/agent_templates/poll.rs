//! The template source poll job (C-1107, C-598's shape): claims due sources under their lease
//! (`FOR UPDATE SKIP LOCKED`, at most [`CHECK_CONCURRENCY`] per replica, ±10 % jitter, a
//! failure backs off 1 h) and runs the shared check path with `trigger = "scheduled"`. The
//! loop skeleton is the shared `polling::spawn_loop`.

use crate::app::AppState;
use crate::audit::AuditActor;
use aurora_core::{ApiError, limits::TEMPLATE_LEASE};
use aurora_db::repo::agent_templates as repo;

use super::check::{CHECK_CONCURRENCY, check_source, fail};

/// Starts the poll loop of this replica.
pub fn spawn(state: AppState) {
    crate::domains::polling::spawn_loop("template source poll", state, |s| async move {
        poll_once(&s).await
    });
}

/// The scheduled poll: claim due sources under their lease and check them. Returns how many
/// sources this tick checked.
pub async fn poll_once(state: &AppState) -> Result<usize, ApiError> {
    let owner = crate::domains::polling::lease_owner();
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let claimed = repo::claim_due(
        &mut c,
        owner,
        CHECK_CONCURRENCY as i64,
        TEMPLATE_LEASE.as_secs_f64(),
    )
    .await?;
    drop(c);
    let n = claimed.len();
    if n == 0 {
        return Ok(0);
    }
    let work = async {
        for source in claimed {
            let actor = AuditActor::system("template-source-poll");
            if let Err(e) = check_source(state, &source, &actor).await {
                tracing::warn!(source = %source.id, error = %e.message(), "template source check failed");
                // The check died without writing its outcome: write the failed outcome
                // (which releases the lease and applies the failure backoff), so a source
                // that keeps erroring is not re-claimed on every tick.
                if let Err(w) = fail(state, &source, e.message()).await {
                    tracing::error!(
                        source = %source.id,
                        error = %w.message(),
                        "writing the failed check outcome"
                    );
                }
            }
        }
    };
    tracing::Instrument::instrument(
        work,
        aurora_core::telemetry::job_span("template source poll"),
    )
    .await;
    Ok(n)
}

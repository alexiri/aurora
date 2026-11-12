//! The web terminal (phase 4, P4-TERM, D-161, SEC-87, C-616..C-625; docs/16 §W19 on
//! docs/16 §W18): the dashboard upgrades a same-origin WebSocket on the API
//! (`GET /v1/agents/{agent}/terminal`, session cookie only), the API mints the single-use
//! `auy_` grant (hashed in `terminal_stream_grants`, C-618), redeems it at the runner's
//! existing :4108 listener with `kind: "terminal"` (C-617) and proxies the leg-1 WebSocket
//! itself. The grant value never reaches the browser; terminal bytes are never logged,
//! stored or request-log-rowed anywhere (byte totals only, SEC-87).
pub mod service;

mod routes;

pub use routes::router;

use serde::Deserialize;
use uuid::Uuid;

/// `POST /v1/runner/terminal/keep-awake` (the runner's stamp; C-620). Accounting only.
#[derive(Debug, Deserialize)]
pub struct KeepAwakeRequest {
    pub sandbox_id: Uuid,
}

/// The runner-plane keep-awake stamp: refresh `terminal_active_until` by
/// `TERMINAL_AWAKE_GRACE` (the cap clock lives in the repo predicate). Any hosted replica may
/// stamp; a sandbox this runner does not own is a no-op (the UPDATE is id-scoped).
pub async fn keep_awake(
    s: &crate::app::AppState,
    runner: &aurora_db::repo::runners::RunnerRow,
    req: KeepAwakeRequest,
) -> Result<(), aurora_core::ApiError> {
    use aurora_db::DbError;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    // Scope the stamp to a sandbox this runner (replica) actually owns: a stale or foreign id
    // updates nothing.
    let owned: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sandboxes WHERE id = $1 AND runner_id = $2 AND state = 'running'",
    )
    .bind(req.sandbox_id)
    .bind(runner.id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if owned.is_some() {
        aurora_db::repo::sandboxes::keep_terminal_awake(
            &mut *conn,
            req.sandbox_id,
            aurora_core::limits::TERMINAL_AWAKE_GRACE.as_secs() as i64,
        )
        .await?;
    }
    Ok(())
}

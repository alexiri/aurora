//! Preview-stream grants on the runner plane (phase 4, P4-PRV-2, D-147, SEC-82; `16 §W17`).
//! The runner redeems one single-use `aux_` grant at `POST /v1/runner/preview-streams/open`
//! and uses **only** the answer's binding (never anything the relay sent). The advisory
//! `closed` call is accounting only (bytes are never logged).
use crate::app::AppState;
use crate::domains::previews::service as previews;
use crate::domains::terminal::service as terminal;
use aurora_core::limits::{
    PREVIEW_STREAM_LIFETIME, PREVIEW_STREAM_WINDOW, PREVIEW_WS_IDLE_TIMEOUT,
};
use aurora_core::{ApiError, token};
use aurora_db::DbError;
use aurora_db::repo::preview_relay as relay_repo;
use aurora_db::repo::preview_shares as shares_repo;
use aurora_db::repo::runners;
use aurora_protocol::runner::{
    PreviewStreamClosedRequest, PreviewStreamOpenRequest, PreviewStreamOpenResponse,
};
use uuid::Uuid;

fn grant_invalid() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "grant_invalid",
        "This stream grant is not valid.",
    )
}

fn not_current() -> ApiError {
    ApiError::conflict(
        "not_current",
        "The boot is no longer current or the share is not active.",
    )
}

/// The stream endpoint of a runner replica (`ws[s]://<advertised_host>:4108/v1/preview-stream`
/// — the relay dials it; the scheme follows the runner's registered `stream_tls` capability:
/// TLS is optional (`runner.stream_tls`, default off in-cluster, `16 §W18`), so a plaintext
/// listener must be dialled with plain `ws://`). Verification against `relay.runner_ca_file`
/// is the relay's own TLS job when TLS is on.
pub fn runner_endpoint_of(advertised_host: &str, tls: bool) -> String {
    let host = if advertised_host.contains(':') {
        advertised_host.to_owned()
    } else {
        format!("{advertised_host}:4108")
    };
    let scheme = if tls { "wss" } else { "ws" };
    format!("{scheme}://{host}/v1/preview-stream")
}

/// `POST /v1/runner/preview-streams/open`: consume the grant (single use, ≤ 30 s) and return
/// its binding. 403 `grant_invalid`; 409 `not_current` (boot not current, share paused, agent
/// frozen, previews off, or the answering runner does not hold the channel). A terminal grant
/// (`kind: "terminal"`, P4-TERM C-617) redeems from `terminal_stream_grants`: the answer
/// carries no port and no share, only the sandbox/boot binding and the terminal window.
pub async fn open(
    s: &AppState,
    runner: &runners::RunnerRow,
    req: PreviewStreamOpenRequest,
) -> Result<PreviewStreamOpenResponse, ApiError> {
    // (P4-TERM, C-617) The kind rides the grant prefix at the runner (`aux_` → port,
    // `auy_` → terminal); an explicit `kind: "terminal"` from an older-API-shaped client
    // without the prefix is refused below (its grant cannot be a terminal grant).
    if matches!(req.kind.as_deref(), Some("terminal")) {
        return terminal::open(s, runner, req).await;
    }
    previews::ensure_enabled(s)?;
    let row = relay_repo::consume_stream_grant(&s.pool, &token::hash(req.grant.expose()))
        .await?
        .ok_or_else(grant_invalid)?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    // The boot must still be current and the sandbox running on a runner of this
    // installation (`16 §W17`: a stale boot or a stopped sandbox is 409).
    let cur: Option<(Uuid, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT sb.id, sb.boot_id, sb.channel_replica_id FROM sandboxes sb \
         WHERE sb.id = $1 AND sb.state = 'running'",
    )
    .bind(row.sandbox_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((sandbox_id, boot_id, channel_replica)) = cur else {
        return Err(not_current());
    };
    if boot_id != Some(row.boot_id) {
        return Err(not_current());
    }
    // The answering runner must hold the channel (the one that redeemed it; a replica that
    // lost the channel cannot serve the stream).
    if channel_replica.is_none() {
        return Err(not_current());
    }
    // The share must still be active and its port the granted one (defense in depth: the
    // binding was minted from a resolve seconds ago).
    let share = shares_repo::get(&mut *conn, row.share_id)
        .await?
        .ok_or_else(not_current)?;
    if share.state != "active" || share.port != row.port {
        return Err(not_current());
    }
    crate::domains::freeze::service::ensure_agent_active(&mut conn, share.agent_id).await?;
    let _ = runner; // any replica of the installation may answer; the channel check fenced it
    Ok(PreviewStreamOpenResponse {
        sandbox_id,
        boot_id: row.boot_id,
        port: Some(u16::try_from(row.port).map_err(|_| not_current())?),
        share_id: Some(row.share_id),
        window: PREVIEW_STREAM_WINDOW as u32,
        idle_secs: PREVIEW_WS_IDLE_TIMEOUT.as_secs() as u32,
        max_lifetime_secs: PREVIEW_STREAM_LIFETIME.as_secs() as u32,
    })
}

/// `POST /v1/runner/preview-streams/closed`: advisory accounting; refreshes
/// `preview_shares.last_accessed_at` and the awake stamp. Never fails a stream.
pub async fn closed(
    s: &AppState,
    _runner: &runners::RunnerRow,
    req: PreviewStreamClosedRequest,
) -> Result<(), ApiError> {
    if let Some(row) = shares_repo::get(&s.pool, req.share_id).await? {
        let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
        shares_repo::touch_accessed(&mut conn, row.id).await?;
        relay_repo::keep_preview_awake(&mut *conn, row.agent_id).await?;
    }
    Ok(())
}

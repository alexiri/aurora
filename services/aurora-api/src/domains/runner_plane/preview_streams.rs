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

/// Whether `advertised_host` is a host name or an IP literal with an optional port, and
/// nothing else (`11`, `16 §W17`): the relay dials `ws[s]://<advertised_host>…` with a live
/// stream grant in `Authorization`, so anything that is not a plain authority — a scheme, a
/// path, userinfo (`user@evil`), a query, whitespace — would let a runner credential steer
/// preview grants to a host of its choosing. Forms: `name`, `name:port`, `1.2.3.4[:port]`,
/// `[v6][:port]` (a bare IPv6 address is refused: its colons are ambiguous with the port).
pub fn advertised_host_ok(h: &str) -> bool {
    if h.is_empty() || h.len() > 255 {
        return false;
    }
    let (host, port) = if let Some(rest) = h.strip_prefix('[') {
        let Some((v6, tail)) = rest.split_once(']') else {
            return false;
        };
        if v6.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        match tail {
            "" => return true,
            // Anything after `]` must be `:port` (`[v6]x` is not a host).
            t => match t.strip_prefix(':') {
                Some(p) => (None, Some(p)),
                None => return false,
            },
        }
    } else {
        match h.split_once(':') {
            Some((host, port)) => (Some(host), Some(port)),
            None => (Some(h), None),
        }
    };
    if let Some(p) = port
        && (p.is_empty()
            || p.len() > 5
            || !p.bytes().all(|b| b.is_ascii_digit())
            || p == "0"
            || p.parse::<u32>().is_ok_and(|n| n > 65_535))
    {
        return false;
    }
    let Some(host) = host else {
        return true; // `[v6]:port`, port checked above
    };
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    // A DNS name: labels of letters, digits and inner hyphens (underscore tolerated: cluster
    // service names), separated by single dots, 1..=63 characters each.
    !host.is_empty()
        && host.split('.').all(|l| {
            (1..=63).contains(&l.len())
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
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
/// `preview_shares.last_accessed_at` and the awake stamp. Never fails a stream. (FIX-071) Only
/// the runner that holds the share's agent's running sandbox may refresh them: a report from any
/// other runner is ignored (`Ok`, nothing written), so one runner cannot keep another's agents
/// awake or skew their share stamps.
pub async fn closed(
    s: &AppState,
    runner: &runners::RunnerRow,
    req: PreviewStreamClosedRequest,
) -> Result<(), ApiError> {
    if let Some(row) = shares_repo::get(&s.pool, req.share_id).await? {
        let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
        let held: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sandboxes \
             WHERE agent_id = $1 AND state = 'running' AND runner_id = $2)",
        )
        .bind(row.agent_id)
        .bind(runner.id)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        if !held {
            tracing::debug!(
                runner_id = %runner.id,
                share_id = %req.share_id,
                "preview stream closed report from a runner not holding the sandbox; ignored"
            );
            return Ok(());
        }
        shares_repo::touch_accessed(&mut conn, row.id).await?;
        relay_repo::keep_preview_awake(&mut *conn, row.agent_id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::advertised_host_ok;

    #[test]
    fn advertised_host_is_a_name_or_ip_with_an_optional_port() {
        for ok in [
            "runner-1",
            "runner-1.internal",
            "runner-1.ns.svc.cluster.local:4108",
            "10.0.0.7",
            "10.0.0.7:4108",
            "[fd00::1]",
            "[fd00::1]:4108",
            "a_b.c",
        ] {
            assert!(advertised_host_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "evil.example/x",
            "user@evil.example",
            "evil.example?x=1",
            "evil.example#f",
            "ws://evil.example",
            "evil example",
            "evil.example\t",
            "evil.example:",
            "evil.example:0",
            "evil.example:99999",
            "evil.example:80:81",
            "fd00::1",
            "[fd00::1",
            "[not-an-ip]",
            "[fd00::1]x",
            "-lead.example",
            "trail-.example",
            "a..b",
            ".a",
            "evil.example\\x",
            "1.2.3.4/32",
        ] {
            assert!(!advertised_host_ok(bad), "{bad:?}");
        }
        assert!(!advertised_host_ok(&"a".repeat(256)));
    }
}

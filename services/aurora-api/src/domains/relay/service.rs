//! Relay-plane service (phase 4, P4-PRV-2, D-146, D-147, SEC-81, SEC-85; docs/04 §Preview
//! access, `16 §W17`). Authority lives here; `preview_relay` only stores and reads.
//!
//! Denials: `deny` reasons are the **uniform** `not_found` for every "no such preview or no
//! access" cause (the existence of a share is never revealed to a caller who cannot use its
//! workspace); with a valid session of the share the specific reasons (`paused` 403,
//! `frozen` 423, `wake_limited` 429, `unreachable` 502) may be spoken.
use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::previews::service as previews;
use crate::domains::runner_plane::preview_streams::runner_endpoint_of;
use aurora_core::limits::{
    PREVIEW_GRANT_TTL, PREVIEW_SESSION_IDLE, PREVIEW_SESSION_TTL, PREVIEW_STREAM_GRANT_TTL,
    PREVIEW_WAKE_WAIT,
};
use aurora_core::{ApiError, token};
use aurora_crypto::Purpose;
use aurora_db::DbError;
use aurora_db::authz;
use aurora_db::repo::preview_relay as relay_repo;
use aurora_db::repo::preview_shares as shares_repo;
use aurora_db::repo::{agents, sessions};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

/// Seal AAD: binds a grant to this flow so seals are never interchangeable.
const GRANT_AAD: &[u8] = b"preview-grant";

#[derive(Debug, Serialize, Deserialize)]
pub struct PreviewGrant {
    pub jti: Uuid,
    pub share_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub nonce: String,
    /// Unix seconds.
    pub exp: i64,
}

fn not_found() -> ApiError {
    ApiError::not_found("Preview not found.")
}

fn grant_invalid() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_REQUEST,
        "grant_invalid",
        "This preview grant is not valid.",
    )
}

fn not_running() -> ApiError {
    ApiError::conflict("not_running", "The sandbox of this preview is not running.")
}

/// `session_hash` must be 64 hex chars (the relay hashes the cookie token itself).
fn parse_session_hash(raw: &str) -> Result<[u8; 32], ApiError> {
    fn hex_byte(h: &[u8]) -> Option<u8> {
        let hi = (h[0] as char).to_digit(16)? as u8;
        let lo = (h[1] as char).to_digit(16)? as u8;
        Some(hi * 16 + lo)
    }
    let b = raw.as_bytes();
    if b.len() != 64 {
        return Err(not_found());
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = hex_byte(&b[i * 2..i * 2 + 2]).ok_or_else(not_found)?;
    }
    Ok(out)
}

fn validate_label(label: &str) -> Result<(), ApiError> {
    // `p` + `PREVIEW_LABEL_CHARS` base32-lowercase characters (docs/05 §Port previews).
    if label.len() == 1 + aurora_core::limits::PREVIEW_LABEL_CHARS
        && label.starts_with('p')
        && label[1..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        Ok(())
    } else {
        Err(not_found())
    }
}

/// The share row of a label (or `not_found`; the label shape itself is not an oracle).
async fn share_of(conn: &mut PgConnection, label: &str) -> Result<shares_repo::ShareRow, ApiError> {
    shares_repo::get_by_label(&mut *conn, label)
        .await?
        .ok_or_else(not_found)
}

/// The live preview session of a token hash plus its share; `None` when the session row is
/// dead (expired/idle) **or bound to a different share than `label`** (a preview session is
/// only valid on its own share's hostname, D-145/SEC-80), `not_found` when the share is
/// gone (cascaded).
async fn session_of(
    conn: &mut PgConnection,
    hash: &[u8; 32],
    label: &str,
) -> Result<Option<(relay_repo::PreviewSessionRow, shares_repo::ShareRow)>, ApiError> {
    let idle = i64::try_from(PREVIEW_SESSION_IDLE.as_secs()).unwrap_or(3600);
    let Some(s) = relay_repo::find_live_preview_session(&mut *conn, hash, idle).await? else {
        return Ok(None);
    };
    let share = shares_repo::get(&mut *conn, s.share_id)
        .await?
        .ok_or_else(not_found)?;
    if share.label != label {
        return Ok(None);
    }
    Ok(Some((s, share)))
}

/// The whole access law of one resolve (`docs/04` step 4): share `active`, agent not frozen,
/// org previews on, previews on at the instance, user still an active member with `can_use`.
/// Failing anything (except the caller-visible states) is `not_found`.
async fn access_of(
    conn: &mut PgConnection,
    share: &shares_repo::ShareRow,
    user: Uuid,
) -> Result<(), ApiError> {
    if share.state != "active" {
        return Err(not_found());
    }
    if agents::get(&mut *conn, share.agent_id).await?.is_none() {
        return Err(not_found());
    }
    if !authz::can_use(&mut *conn, user, share.workspace_id).await? {
        return Err(not_found());
    }
    // The org switch (docs/18: "org switch off → next resolve denies").
    let on: bool = sqlx::query_scalar(
        "SELECT coalesce((settings->'previews_enabled')::bool, false) FROM organizations WHERE id = $1",
    )
    .bind(share.org_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if !on {
        return Err(not_found());
    }
    // A frozen agent ends access (423 to someone who may see it; folded into `not_found`
    // here — the relay renders its uniform page either way).
    crate::domains::freeze::service::ensure_agent_active(conn, share.agent_id).await?;
    Ok(())
}

// ---------------------------------------------------------------- register

/// `AURORA_BOOTSTRAP_RELAY_TOKEN`: upsert the relay anchor token's hash. Idempotent.
pub async fn bootstrap_relay_token(s: &AppState) -> Result<(), DbError> {
    if let Some(t) = &s.config.bootstrap_relay_token {
        let t = t.expose();
        relay_repo::upsert_relay_token(&s.pool, &token::hash(t)).await?;
    }
    Ok(())
}

pub async fn register(
    s: &AppState,
    token_id: Uuid,
    req: RelayRegisterRequest,
) -> Result<RelayRegisterResponse, ApiError> {
    let version = req.version.trim();
    if version.is_empty() || version.len() > 64 || version.chars().any(char::is_control) {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![aurora_core::FieldError::new(
                "/version",
                "must be 1..64 characters",
            )],
        ));
    }
    let replica = Uuid::now_v7();
    relay_repo::upsert_relay_replica(&s.pool, replica, token_id, version).await?;
    let domain = s.config.previews.domain.clone().unwrap_or_default();
    Ok(RelayRegisterResponse {
        replica_id: replica,
        protocol: 1,
        config: RelayConfig {
            previews_domain: domain,
            external_url: s.config.external_url.clone(),
            auth_cache_secs: aurora_core::limits::PREVIEW_AUTH_CACHE.as_secs() as u32,
            wake_wait_secs: PREVIEW_WAKE_WAIT.as_secs() as u32,
        },
    })
}

// ---------------------------------------------------------------- resolve

/// The derived route of a share's sandbox (the `route` object of `resolve`).
#[allow(clippy::type_complexity)]
async fn route_of(
    conn: &mut PgConnection,
    share: &shares_repo::ShareRow,
) -> Result<RelayRoute, ApiError> {
    let row: Option<(
        Option<String>,
        Option<Uuid>,
        Option<Uuid>,
        Option<Vec<i32>>,
        Option<String>,
        Option<serde_json::Value>,
    )> = sqlx::query_as(
        "SELECT sb.state, sb.boot_id, sb.channel_replica_id, sb.listening_ports, r.advertised_host, \
             rn.capabilities \
             FROM sandboxes sb \
             LEFT JOIN runner_replicas r ON r.id = sb.channel_replica_id \
             LEFT JOIN runners rn ON rn.id = r.runner_id \
             WHERE sb.agent_id = $1 AND sb.state IS NOT NULL",
    )
    .bind(share.agent_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((state, boot_id, channel_replica, ports, advertised, caps)) = row else {
        return Ok(RelayRoute {
            sandbox: "unavailable".into(),
            listening: false,
            start: "none".into(),
            runner_endpoint: None,
            retry_after_secs: None,
        });
    };
    let listening = ports.is_some_and(|p| p.contains(&share.port));
    let sandbox_state = match state.as_deref() {
        Some("running") if boot_id.is_some() => "running",
        Some("starting") | Some("stopping") => "waking",
        _ => "stopped",
    };
    let runner_endpoint = match (sandbox_state, channel_replica, advertised) {
        ("running", Some(_), Some(host)) if boot_id.is_some() => {
            // The dial scheme follows the runner's registered `stream_tls` capability
            // (`16 §W18`: TLS optional, default off in-cluster).
            let tls = caps
                .as_ref()
                .and_then(|c| c.get("stream_tls"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Some(runner_endpoint_of(&host, tls))
        }
        _ => None,
    };
    Ok(RelayRoute {
        sandbox: sandbox_state.to_owned(),
        listening,
        start: "none".into(),
        runner_endpoint,
        retry_after_secs: None,
    })
}

pub async fn resolve(
    s: &AppState,
    req: RelayResolveRequest,
) -> Result<RelayResolveResponse, ApiError> {
    previews::ensure_enabled(s)?;
    validate_label(&req.label)?;
    let wait = req
        .wait_secs
        .unwrap_or(0)
        .min(PREVIEW_WAKE_WAIT.as_secs() as u32);
    let deadline = Utc::now() + Duration::seconds(i64::from(wait));
    loop {
        let answer = resolve_once(s, &req).await?;
        // Long-poll only while waking and not yet listening: the relay's wake-wait page.
        let again = answer.decision == "allow"
            && !answer.route.as_ref().is_some_and(|r| r.listening)
            && wait > 0;
        if !again || Utc::now() >= deadline {
            return Ok(answer);
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

async fn resolve_once(
    s: &AppState,
    req: &RelayResolveRequest,
) -> Result<RelayResolveResponse, ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    if share_of(&mut conn, &req.label).await.is_err() {
        return Ok(deny("not_found"));
    }
    // The session decides between `login` (no live credential: the flow restarts silently)
    // and `deny` (uniform `not_found` otherwise).
    let hash = match &req.session_hash {
        Some(h) => match parse_session_hash(h) {
            Ok(h) => h,
            Err(_) => return Ok(login()),
        },
        None => {
            return Ok(login());
        }
    };
    // `session_of` only yields a session bound to this request's label (a session minted
    // for another share is answered like a dead one: the flow restarts for this share).
    let Some((session, share)) = session_of(&mut conn, &hash, &req.label).await? else {
        return Ok(login());
    };
    if access_of(&mut conn, &share, session.user_id).await.is_err() {
        // A live preview session whose holder lost access (paused/deleted share, frozen
        // agent, lost `can_use`, org switch off): the uniform `not_found` deny (docs/18
        // "next resolve denies"). A dead session never reaches here — `session_of` already
        // filtered it (that is the silent `login` restart).
        return Ok(deny("not_found"));
    }
    relay_repo::touch_preview_session(&mut *conn, session.id).await?;
    let mut route = route_of(&mut conn, &share).await?;
    // A stopped sandbox: set the wake demand (`11 §Port previews`) — once per window; the
    // share's own wake limiter answers 429 `wake_limited` beyond it.
    if route.sandbox == "stopped" {
        match demand_wake(&mut conn, &share).await {
            Ok(()) => route.start = "starting".into(),
            Err(limited) => {
                route.start = "limited".into();
                route.retry_after_secs = Some(60);
                let _ = limited;
            }
        }
    }
    shares_repo::touch_accessed(&mut conn, share.id).await?;
    relay_repo::keep_preview_awake(&mut *conn, share.agent_id).await?;
    Ok(RelayResolveResponse {
        decision: "allow".into(),
        reason: None,
        share: Some(RelayShareRef {
            id: share.id,
            port: share.port,
        }),
        route: Some(route),
    })
}

fn deny(reason: &str) -> RelayResolveResponse {
    RelayResolveResponse {
        decision: "deny".into(),
        reason: Some(reason.to_owned()),
        share: None,
        route: None,
    }
}

fn login() -> RelayResolveResponse {
    RelayResolveResponse {
        decision: "login".into(),
        reason: None,
        share: None,
        route: None,
    }
}

/// The wake demand of a stopped sandbox: record both stamps (wake + start request), notify
/// due work, honour the share's wake limiter (`true` = limited).
async fn demand_wake(conn: &mut PgConnection, share: &shares_repo::ShareRow) -> Result<(), bool> {
    if share.start_command.is_none() {
        // Nothing to start; the relay shows its own page. Not an error.
        return Ok(());
    }
    if !shares_repo::record_wake(&mut *conn, share.id, 3600.0)
        .await
        .map_err(|_| false)?
    {
        return Err(true);
    }
    sqlx::query(
        "UPDATE sandboxes SET preview_wake_at = now(), preview_start_requested_at = now(), \
              updated_at = now() \
          WHERE agent_id = $1 AND state IS NOT NULL",
    )
    .bind(share.agent_id)
    .execute(&mut *conn)
    .await
    .map_err(|_| false)?;
    crate::domains::conversations::service::notify_work(conn)
        .await
        .map_err(|_| false)?;
    Ok(())
}

// ---------------------------------------------------------------- authorize (dashboard side)

/// The sealed grant of `GET /v1/previews/authorize` (`docs/04` step 2).
pub fn seal_grant(s: &AppState, g: &PreviewGrant) -> Option<String> {
    s.derived
        .seal(
            Purpose::PreviewGrant,
            GRANT_AAD,
            &serde_json::to_vec(g).ok()?,
        )
        .ok()
}

pub fn open_grant(s: &AppState, sealed: &str) -> Option<PreviewGrant> {
    let plain = s
        .derived
        .open(Purpose::PreviewGrant, GRANT_AAD, sealed)
        .ok()?;
    let g: PreviewGrant = serde_json::from_slice(plain.expose()).ok()?;
    (g.exp >= Utc::now().timestamp()).then_some(g)
}

/// `GET /v1/previews/authorize?host=<label>&state=<nonce>`: session-only route on the
/// dashboard origin. Checks (uniform 404 page for every failure): label names a share,
/// previews on (instance + org), `can_use(workspace)`, share `active`. A frozen agent
/// answers the 423 page. Mints the sealed grant and redirects to the share's own
/// `/_aurora/grant?g=…` URL (constructed from the share's label, never from the request).
pub async fn authorize(
    s: &AppState,
    caller: &crate::http::auth::Caller,
    host: Option<String>,
    state_nonce: Option<String>,
) -> Result<(String, Option<String>), ApiError> {
    previews::ensure_enabled(s)?;
    let label = host.as_deref().map(str::trim).unwrap_or("");
    let nonce = state_nonce.as_deref().map(str::trim).unwrap_or("");
    if nonce.is_empty() || nonce.len() > 128 {
        return Err(not_found());
    }
    validate_label(label)?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let share = share_of(&mut conn, label).await?;
    // Org switch (404 page, same as every other failure of this route).
    let on: bool = sqlx::query_scalar(
        "SELECT coalesce((settings->'previews_enabled')::bool, false) FROM organizations WHERE id = $1",
    )
    .bind(share.org_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if !on {
        return Err(not_found());
    }
    if share.state != "active" {
        return Err(not_found());
    }
    let _ = caller.workspace(s, share.workspace_id).await.map_err(|e| {
        if e.status() == axum::http::StatusCode::FORBIDDEN {
            e
        } else {
            not_found()
        }
    })?;
    let session_id = match caller.principal {
        crate::http::auth::Principal::Session { id } => id,
        _ => return Err(not_found()),
    };
    let g = PreviewGrant {
        jti: Uuid::now_v7(),
        share_id: share.id,
        user_id: caller.user_id,
        session_id,
        nonce: nonce.to_owned(),
        exp: Utc::now().timestamp() + i64::try_from(PREVIEW_GRANT_TTL.as_secs()).unwrap_or(60),
    };
    let Some(sealed) = seal_grant(s, &g) else {
        return Err(ApiError::internal("grant seal failed"));
    };
    let domain = s.config.previews.domain.clone().unwrap_or_default();
    // The seal is `<key-id>.<URL_SAFE_NO_PAD base64>`: already query-safe (`-`, `_`, no
    // padding), so it goes into the `g` parameter verbatim (`open_grant` re-assembles it).
    Ok((
        format!(
            "https://{}.{}/_aurora/grant?g={}",
            share.label, domain, sealed
        ),
        None,
    ))
}

// ---------------------------------------------------------------- grants/redeem

/// `POST /v1/relay/grants/redeem`: verify seal/expiry/label/nonce/single-use/session/user,
/// create the `preview_sessions` row, return the `auv_` token once.
pub async fn redeem(
    s: &AppState,
    req: RelayRedeemRequest,
) -> Result<RelayRedeemResponse, ApiError> {
    previews::ensure_enabled(s)?;
    validate_label(&req.label)?;
    if req.nonce.is_empty() || req.nonce.len() > 128 {
        return Err(grant_invalid());
    }
    let Some(g) = open_grant(s, &req.grant) else {
        return Err(grant_invalid());
    };
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let share = shares_repo::get(&mut *conn, g.share_id)
        .await?
        .filter(|sh| sh.label == req.label)
        .ok_or_else(grant_invalid)?;
    if g.nonce != req.nonce {
        return Err(grant_invalid());
    }
    // Single use (`jti` insert-on-conflict; a replayed grant fails).
    let until = Utc::now() + Duration::seconds(60);
    if !relay_repo::claim_grant_use(&mut *conn, g.jti, until).await? {
        return Err(grant_invalid());
    }
    // The dashboard session must still be live and the user must still pass the access law.
    let live = sessions::is_live(&s.pool, g.session_id, s.config.auth.session_idle_secs).await?;
    if !live {
        return Err(grant_invalid());
    }
    access_of(&mut conn, &share, g.user_id).await?;
    // Mint the preview credential.
    let token = token::generate(token::TokenPrefix::PreviewSession)
        .expose()
        .to_owned();
    let hash = token::hash(&token);
    let expires_at = (Utc::now()
        + Duration::seconds(i64::try_from(PREVIEW_SESSION_TTL.as_secs()).unwrap_or(8 * 3600)))
    .min(
        sessions::expires_of(&s.pool, g.session_id)
            .await?
            .unwrap_or(
                Utc::now()
                    + Duration::seconds(
                        i64::try_from(PREVIEW_SESSION_TTL.as_secs()).unwrap_or(8 * 3600),
                    ),
            ),
    );
    let display_prefix: String = token.chars().take(12).collect();
    relay_repo::insert_preview_session(
        &mut *conn,
        Uuid::now_v7(),
        &hash,
        &display_prefix,
        share.id,
        g.user_id,
        g.session_id,
        expires_at,
    )
    .await?;
    // Best-effort audit row (`docs/04` §Preview access; not security-critical). The actor is
    // the person who authorized the preview (their dashboard session).
    let ev = AuditEvent::new(
        AuditActor {
            kind: "user",
            id: Some(g.user_id),
            label: String::new(),
            source: "dashboard",
            ip: None,
        },
        "preview.session.create",
        Outcome::Success,
    )
    .target("preview_share", share.id)
    .metadata(serde_json::json!({
        "label": share.label,
        "session_prefix": display_prefix,
    }));
    audit::record(s, ev);
    Ok(RelayRedeemResponse {
        token,
        expires_at,
        idle_secs: PREVIEW_SESSION_IDLE.as_secs() as u32,
    })
}

// ---------------------------------------------------------------- streams

/// `POST /v1/relay/streams`: mint one single-use `aux_` stream grant bound to
/// (share, sandbox, boot, port) after the same checks as `resolve`; 409 `not_running`.
pub async fn streams(
    s: &AppState,
    req: RelayStreamsRequest,
) -> Result<RelayStreamsResponse, ApiError> {
    previews::ensure_enabled(s)?;
    validate_label(&req.label)?;
    let hash = parse_session_hash(&req.session_hash)?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let Some((session, share)) = session_of(&mut conn, &hash, &req.label).await? else {
        return Err(not_found());
    };
    access_of(&mut conn, &share, session.user_id).await?;
    relay_repo::touch_preview_session(&mut *conn, session.id).await?;
    let route = route_of(&mut conn, &share).await?;
    if route.sandbox != "running" || route.runner_endpoint.is_none() {
        return Err(not_running());
    }
    // The boot is current and a runner holds its channel.
    let (sandbox_id, boot, host, tls) =
        sqlx::query_as::<_, (Uuid, Uuid, String, Option<serde_json::Value>)>(
            "SELECT sb.id, sb.boot_id, r.advertised_host, rn.capabilities->'stream_tls' \
         FROM sandboxes sb \
         JOIN runner_replicas r ON r.id = sb.channel_replica_id \
         JOIN runners rn ON rn.id = r.runner_id \
         WHERE sb.agent_id = $1 AND sb.state = 'running' AND sb.boot_id IS NOT NULL",
        )
        .bind(share.agent_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| not_running())?;
    let token = token::generate(token::TokenPrefix::PreviewStream)
        .expose()
        .to_owned();
    let expires_at = Utc::now()
        + Duration::seconds(i64::try_from(PREVIEW_STREAM_GRANT_TTL.as_secs()).unwrap_or(30));
    relay_repo::insert_stream_grant(
        &mut *conn,
        &token::hash(&token),
        share.id,
        sandbox_id,
        boot,
        share.port,
        &hash,
        expires_at,
    )
    .await?;
    // The dial scheme follows the runner's registered `stream_tls` capability (`16 §W18`).
    let stream_tls = tls
        .as_ref()
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok(RelayStreamsResponse {
        grant: token,
        expires_in: PREVIEW_STREAM_GRANT_TTL.as_secs() as u32,
        runner_endpoint: runner_endpoint_of(&host, stream_tls),
    })
}

// ---------------------------------------------------------------- validate

/// `POST /v1/relay/validate`: the entries that no longer pass (the relay closes their
/// streams with 4403). Bounded batch (≤ 500).
pub async fn validate(
    s: &AppState,
    req: RelayValidateRequest,
) -> Result<RelayValidateResponse, ApiError> {
    previews::ensure_enabled(s)?;
    if req.sessions.len() > 500 {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![aurora_core::FieldError::new(
                "/sessions",
                "at most 500 entries",
            )],
        ));
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let mut revoked = Vec::new();
    for e in req.sessions {
        let entry = |r: &RelayValidateEntry| RelayValidateEntry {
            label: r.label.clone(),
            session_hash: r.session_hash.clone(),
        };
        let Ok(hash) = parse_session_hash(&e.session_hash) else {
            revoked.push(entry(&e));
            continue;
        };
        match session_of(&mut conn, &hash, &e.label).await {
            Ok(Some((session, share))) => {
                match access_of(&mut conn, &share, session.user_id).await {
                    Ok(()) => {
                        relay_repo::touch_preview_session(&mut *conn, session.id).await?;
                    }
                    Err(_) => revoked.push(entry(&e)),
                }
            }
            _ => revoked.push(entry(&e)),
        }
    }
    Ok(RelayValidateResponse { revoked })
}

// ---------------------------------------------------------------- report

/// `POST /v1/relay/report`: the heartbeat (every 15 s). Refreshes `relay_replicas.last_seen_at`,
/// `preview_shares.last_accessed_at` and `sandboxes.preview_active_until`. Byte totals are
/// metrics only (never logged, never stored per row).
pub async fn report(
    s: &AppState,
    ctx: &crate::domains::relay::auth::RelayCtx,
    req: RelayReportRequest,
) -> Result<RelayOkResponse, ApiError> {
    if req.shares.len() > 500 {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![aurora_core::FieldError::new(
                "/shares",
                "at most 500 entries",
            )],
        ));
    }
    let total_streams: i32 = req.shares.iter().map(|sh| sh.streams_open).sum();
    relay_repo::touch_relay_replica(&s.pool, ctx.replica, total_streams).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    for sh in &req.shares {
        if let Some(row) = shares_repo::get(&mut *conn, sh.share_id).await? {
            shares_repo::touch_accessed(&mut conn, row.id).await?;
            relay_repo::keep_preview_awake(&mut *conn, row.agent_id).await?;
        }
        // Byte accounting is a counter, not a row (docs/16: bytes are never logged).
        if sh.bytes_to_app + sh.bytes_from_app > 0 {
            metrics::counter!(
                "aurora_preview_stream_bytes_total",
                "direction" => "mixed"
            )
            .increment(sh.bytes_to_app.saturating_add(sh.bytes_from_app));
        }
    }
    Ok(RelayOkResponse { ok: true })
}

/// Admin → Overview "Previews" line (docs/05 line 791): relay replicas seen in the last
/// 15 minutes, with their versions and open-stream gauges.
pub async fn list_replicas(s: &AppState) -> Result<Vec<RelayReplicaInfo>, ApiError> {
    let rows = relay_repo::list_relay_replicas(&s.pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| RelayReplicaInfo {
            id: r.id,
            version: r.version,
            streams_open: r.streams_open,
            last_seen_at: r.last_seen_at,
        })
        .collect())
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RelayReplicaInfo {
    pub id: Uuid,
    pub version: String,
    pub streams_open: i32,
    pub last_seen_at: DateTime<Utc>,
}

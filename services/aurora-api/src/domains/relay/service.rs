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
    PREVIEW_GRANT_TTL, PREVIEW_LISTEN_WAIT, PREVIEW_SESSION_IDLE, PREVIEW_SESSION_TTL,
    PREVIEW_START_ATTEMPTS, PREVIEW_STREAM_GRANT_TTL, PREVIEW_WAKE_WAIT, PREVIEW_WAKE_WINDOW,
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

/// Why the access law refuses someone. `NotFound` is the uniform "no such preview or no
/// access"; `Paused` and `Frozen` are only ever spoken to a person who still passes every
/// other check (a live session whose holder may use the workspace), so they reveal nothing.
#[derive(Debug)]
enum Denial {
    NotFound,
    Paused,
    /// The 423 the freeze service words (level included).
    Frozen(ApiError),
}

/// The paused-share refusal (403) for the routes that answer with an HTTP status.
fn paused_error() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "preview_paused",
        "This preview share is paused.",
    )
}

/// The share-state half of the law: `paused` and a frozen agent. The caller has already
/// established that the person may use the workspace.
async fn state_denial(
    conn: &mut PgConnection,
    share: &shares_repo::ShareRow,
) -> Result<Option<Denial>, ApiError> {
    match share.state.as_str() {
        "active" => {}
        "paused" => return Ok(Some(Denial::Paused)),
        _ => return Ok(Some(Denial::NotFound)),
    }
    match crate::domains::freeze::service::ensure_agent_active(conn, share.agent_id).await {
        Ok(()) => Ok(None),
        Err(e) if e.status() == axum::http::StatusCode::LOCKED => Ok(Some(Denial::Frozen(e))),
        Err(e) => Err(e),
    }
}

/// The org switch (docs/18: "org switch off → next resolve denies").
async fn org_previews_on(conn: &mut PgConnection, org: Uuid) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT coalesce((settings->'previews_enabled')::bool, false) FROM organizations WHERE id = $1",
    )
    .bind(org)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?)
}

/// The whole access law of one resolve (`docs/04` step 4): agent exists, user still an active
/// member with `can_use`, previews on in the org (and at the instance), share `active`, agent
/// not frozen. The first three failing is `NotFound` for anyone; `paused` and `frozen` are
/// reported separately so a person who passes the first three can be told. A database error
/// is an `Err` — never a denial (FIX-089: an outage must not revoke anyone).
async fn access_verdict(
    conn: &mut PgConnection,
    share: &shares_repo::ShareRow,
    user: Uuid,
) -> Result<Option<Denial>, ApiError> {
    if agents::get(&mut *conn, share.agent_id).await?.is_none() {
        return Ok(Some(Denial::NotFound));
    }
    if !authz::can_use(&mut *conn, user, share.workspace_id).await? {
        return Ok(Some(Denial::NotFound));
    }
    if !org_previews_on(conn, share.org_id).await? {
        return Ok(Some(Denial::NotFound));
    }
    state_denial(conn, share).await
}

/// [`access_verdict`] as an HTTP answer: 404 / 403 `preview_paused` / 423 `frozen`.
async fn access_of(
    conn: &mut PgConnection,
    share: &shares_repo::ShareRow,
    user: Uuid,
) -> Result<(), ApiError> {
    match access_verdict(conn, share, user).await? {
        None => Ok(()),
        Some(Denial::NotFound) => Err(not_found()),
        Some(Denial::Paused) => Err(paused_error()),
        Some(Denial::Frozen(e)) => Err(e),
    }
}

// ---------------------------------------------------------------- register

/// `AURORA_BOOTSTRAP_RELAY_TOKEN`: upsert the relay anchor token's hash and make it the only
/// one: every other `relay_tokens` row (and, by cascade, its replicas) is deleted, so changing
/// the secret ends the old token's access (rotation, FIX-090). Idempotent.
pub async fn bootstrap_relay_token(s: &AppState) -> Result<(), DbError> {
    if let Some(t) = &s.config.bootstrap_relay_token {
        let t = t.expose();
        let mut tx = s.pool.begin().await?;
        let keep = relay_repo::upsert_relay_token(&mut *tx, &token::hash(t)).await?;
        let gone = relay_repo::delete_other_relay_tokens(&mut *tx, keep).await?;
        tx.commit().await?;
        if gone > 0 {
            tracing::info!(
                removed = gone,
                "relay anchor token rotated: older tokens removed"
            );
        }
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
            exit_code: None,
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
        exit_code: None,
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
    // The wake demand is recorded at most once per request, on the first pass: a long-poll
    // that watches for the port must not spend the share's hourly wake budget (FIX-089).
    let mut first = true;
    loop {
        let answer = resolve_once(s, &req, first).await?;
        first = false;
        // Long-poll only while a wake or a start is under way and the port is not yet
        // listening: the relay's wake-wait page. A sandbox that runs with nothing starting
        // (no start command, an exited or budget-limited command) answers at once.
        let again = answer.decision == "allow"
            && wait > 0
            && answer.route.as_ref().is_some_and(|r| {
                !r.listening
                    && match r.sandbox.as_str() {
                        "running" => r.start == "starting",
                        "waking" | "stopped" => true,
                        _ => false,
                    }
            });
        if !again || Utc::now() >= deadline {
            return Ok(answer);
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// What the start flow of one resolve decided.
enum Flow {
    Go,
    /// The hourly wake budget is spent: `deny` `wake_limited`.
    WakeLimited(u32),
}

/// Fills `route.start` (and `exit_code`/`retry_after_secs`) and sets the demands a request
/// makes (`docs/11 §Port previews`). `demand` is true only on the first pass of a resolve.
///
/// * parked sandbox: the wake demand (once per window, hourly budget; over it `WakeLimited`);
/// * running sandbox, share with a start command, port not listening: `starting` while a
///   start request is outstanding or the launched command has not yet exited; `exited` (with
///   the code) when the last launch ended, which also re-arms a start; `limited` when
///   `PREVIEW_START_ATTEMPTS` launches already happened in `PREVIEW_START_WINDOW`.
async fn start_flow(
    conn: &mut PgConnection,
    share: &shares_repo::ShareRow,
    route: &mut RelayRoute,
    demand: bool,
) -> Result<Flow, ApiError> {
    match route.sandbox.as_str() {
        "stopped" => {
            if demand {
                match shares_repo::demand_wake(
                    conn,
                    share.id,
                    share.agent_id,
                    share.start_command.is_some(),
                    PREVIEW_WAKE_WINDOW.as_secs_f64(),
                )
                .await?
                {
                    shares_repo::WakeOutcome::Limited { retry_after_secs } => {
                        return Ok(Flow::WakeLimited(retry_after_secs));
                    }
                    shares_repo::WakeOutcome::Recorded => {
                        crate::domains::conversations::service::notify_work(conn).await?;
                    }
                    shares_repo::WakeOutcome::Outstanding => {}
                }
            }
            route.start = "starting".into();
        }
        "running" if !route.listening && share.start_command.is_some() => {
            let window = shares_repo::START_WINDOW_SECS;
            let launches = shares_repo::attempts_within(&share.start_attempts, window);
            let last_launch = launches.iter().max().copied();
            let (_, requested_at) = shares_repo::sandbox_stamps(conn, share.agent_id).await?;
            let now = Utc::now();
            let wake_wait = Duration::seconds(PREVIEW_WAKE_WAIT.as_secs() as i64);
            let request_fresh = requested_at.is_some_and(|t| now - t < wake_wait);
            // The last launch ended after the latest request: it ran and exited.
            let ended = share
                .last_start_at
                .filter(|_| share.last_start_exit_code.is_some());
            let exited = ended.is_some_and(|e| {
                last_launch.is_some_and(|l| e >= l) && requested_at.is_none_or(|r| e > r)
            });
            // Launched, not yet ended, not yet listening: still coming up.
            let coming_up =
                last_launch.is_some_and(|l| now - l < wake_wait && ended.is_none_or(|e| e < l));
            let exhausted = launches.len() >= PREVIEW_START_ATTEMPTS as usize;
            if exited {
                route.exit_code = share.last_start_exit_code;
            }
            if (request_fresh && !exited) || coming_up {
                route.start = "starting".into();
            } else if exhausted {
                route.start = "limited".into();
                route.retry_after_secs = Some(shares_repo::secs_until_slot(&launches, window));
            } else {
                // No launch under way (never started, or exited): this request asks for one.
                // `exited` is still reported to this visitor; the next sees `starting`.
                if demand {
                    shares_repo::stamp_start_requested(conn, share.agent_id).await?;
                    crate::domains::conversations::service::notify_work(conn).await?;
                }
                route.start = if exited { "exited" } else { "starting" }.into();
            }
        }
        _ => {}
    }
    Ok(Flow::Go)
}

async fn resolve_once(
    s: &AppState,
    req: &RelayResolveRequest,
    first: bool,
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
    // A live preview session whose holder lost access (deleted share, lost `can_use`, org
    // switch off) is the uniform `not_found` deny (docs/18 "next resolve denies"); a holder
    // who still passes those checks is told `paused` or `frozen` (`16 §W17`). A dead session
    // never reaches here — `session_of` already filtered it (the silent `login` restart).
    match access_verdict(&mut conn, &share, session.user_id).await? {
        None => {}
        Some(Denial::NotFound) => return Ok(deny("not_found")),
        Some(Denial::Paused) => return Ok(deny("paused")),
        Some(Denial::Frozen(_)) => return Ok(deny("frozen")),
    }
    // The daily byte quota (`PREVIEW_BYTES_PER_SHARE_DAY`, shared across relay replicas): a
    // holder of a live, authorized session may be told (429 `preview_quota`); nobody else
    // ever reaches this line. No new stream opens, the sandbox is not woken for it.
    if super::quota::exceeded(s, share.id).await? {
        return Ok(deny("preview_quota"));
    }
    relay_repo::touch_preview_session(&mut *conn, session.id).await?;
    let mut route = route_of(&mut conn, &share).await?;
    // The wake and start demands of this request (`11 §Port previews`); over the share's
    // hourly wake budget the answer is a deny the relay renders as its 429 page.
    if let Flow::WakeLimited(retry) = start_flow(&mut conn, &share, &mut route, first).await? {
        return Ok(RelayResolveResponse {
            retry_after_secs: Some(retry),
            ..deny("wake_limited")
        });
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
        user: Some(session.user_id),
        retry_after_secs: None,
    })
}

fn deny(reason: &str) -> RelayResolveResponse {
    RelayResolveResponse {
        decision: "deny".into(),
        reason: Some(reason.to_owned()),
        share: None,
        route: None,
        user: None,
        retry_after_secs: None,
    }
}

fn login() -> RelayResolveResponse {
    RelayResolveResponse {
        decision: "login".into(),
        reason: None,
        share: None,
        route: None,
        user: None,
        retry_after_secs: None,
    }
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
    if !org_previews_on(&mut conn, share.org_id).await? {
        return Err(not_found());
    }
    let _ = caller.workspace(s, share.workspace_id).await.map_err(|e| {
        if e.status() == axum::http::StatusCode::FORBIDDEN {
            e
        } else {
            not_found()
        }
    })?;
    // The same access law as `resolve` runs before anything is minted (FIX-089): a paused
    // share answers 403, a frozen agent the 423 page, anything else the uniform 404 — and
    // only a person who may use the workspace is told which (the checks above come first).
    match state_denial(&mut conn, &share).await? {
        None => {}
        Some(Denial::NotFound) => return Err(not_found()),
        Some(Denial::Paused) => return Err(paused_error()),
        Some(Denial::Frozen(e)) => return Err(e),
    }
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
    streams_waiting(s, req, PREVIEW_LISTEN_WAIT).await
}

/// [`streams`] with the listen wait as a parameter (production: `PREVIEW_LISTEN_WAIT`; the
/// tests shorten it through this one code path).
pub async fn streams_waiting(
    s: &AppState,
    req: RelayStreamsRequest,
    listen_wait: std::time::Duration,
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
    let mut route = route_of(&mut conn, &share).await?;
    // A running sandbox whose port is not listening yet: wait up to `PREVIEW_LISTEN_WAIT` for
    // it to appear in the current boot's listening set before dialling (`docs/11`); then
    // `not_listening`, which the relay renders as the 502 "nothing is listening" page. The
    // pooled connection is not held across the wait.
    if route.sandbox == "running" && route.runner_endpoint.is_some() && !route.listening {
        drop(conn);
        let deadline = tokio::time::Instant::now() + listen_wait;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            tokio::time::sleep((deadline - now).min(std::time::Duration::from_millis(250))).await;
            let mut c = s.pool.acquire().await.map_err(DbError::from)?;
            route = route_of(&mut c, &share).await?;
            if route.listening || route.sandbox != "running" {
                break;
            }
        }
        conn = s.pool.acquire().await.map_err(DbError::from)?;
    }
    if route.sandbox != "running" || route.runner_endpoint.is_none() {
        return Err(not_running());
    }
    if !route.listening {
        return Err(ApiError::conflict(
            "not_listening",
            "Nothing is listening on this preview's port.",
        ));
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
        // `revoked` holds only decided denials (a dead session, a deleted share, an access
        // verdict). A database error is the API's own failure: it answers 5xx, the relay
        // keeps its streams and asks again next cycle (FIX-089).
        match session_of(&mut conn, &hash, &e.label).await {
            Ok(Some((session, share))) => {
                match access_verdict(&mut conn, &share, session.user_id).await? {
                    None => {
                        relay_repo::touch_preview_session(&mut *conn, session.id).await?;
                    }
                    Some(_) => revoked.push(entry(&e)),
                }
            }
            Ok(None) => revoked.push(entry(&e)),
            Err(err) if err.status() == axum::http::StatusCode::NOT_FOUND => {
                revoked.push(entry(&e));
            }
            Err(err) => return Err(err),
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
    let mut bytes: Vec<(Uuid, u64)> = Vec::new();
    for sh in &req.shares {
        let moved = sh.bytes_to_app.saturating_add(sh.bytes_from_app);
        if let Some(row) = shares_repo::get(&mut *conn, sh.share_id).await? {
            shares_repo::touch_accessed(&mut conn, row.id).await?;
            relay_repo::keep_preview_awake(&mut *conn, row.agent_id).await?;
            // Only real shares are counted (a relay cannot create counter keys at will).
            if moved > 0 {
                bytes.push((row.id, moved));
            }
        }
        // Byte accounting is a metric and a quota counter, not a row (docs/16: bytes are
        // never logged).
        if moved > 0 {
            metrics::counter!(
                "aurora_preview_stream_bytes_total",
                "direction" => "mixed"
            )
            .increment(moved);
        }
    }
    // The shared daily counter of each share (`PREVIEW_BYTES_PER_SHARE_DAY`): one
    // all-or-nothing add. Under a `closed` class with the store down this answers 503 and the
    // relay re-sends the same bytes with its next heartbeat (see `quota`).
    super::quota::add(s, &bytes).await?;
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

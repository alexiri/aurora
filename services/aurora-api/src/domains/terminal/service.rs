//! The terminal WebSocket upgrade (`GET /v1/agents/{agent}/terminal`, C-616), the grant mint
//! (C-618) and the leg-1 proxy to the runner (C-617). Session cookie only, same-origin,
//! `can_use` of the agent's workspace — the preview share authority, nobody wider. The API
//! mints and redeems the single-use `auy_` grant; the browser never sees it. Terminal bytes
//! are proxied opaquely and never logged, stored or request-log-rowed (SEC-87).
use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as Ws2Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service::load_agent;
use crate::http::auth::Caller;
use aurora_core::limits::{
    TERMINAL_CHUNK, TERMINAL_GRANT_TTL, TERMINAL_IDLE_TIMEOUT, TERMINAL_MAX_LIFETIME,
    TERMINAL_STREAM_WINDOW, TERMINAL_WAKES_PER_AGENT_HOUR,
};
use aurora_core::{ApiError, token};
use aurora_db::DbError;
use aurora_db::repo::agents::{self as agents_repo, AgentRow};
use aurora_db::repo::runners as runners_repo;
use aurora_db::repo::terminal_grants as grants_repo;
use aurora_protocol::runner::PreviewStreamOpenRequest;
use aurora_protocol::runner::PreviewStreamOpenResponse;

/// Close codes of leg 1 (`16 §W18`/`§W19`, C-617). `4403 not allowed` originates here only from
/// the periodic access re-check of an open leg (admission refuses before the upgrade); `4409`
/// from the live-session caps; the proxy passes the runner's codes through.
const CLOSE_NORMAL: u16 = 1000;
const CLOSE_NOT_ALLOWED: u16 = 4403;
const CLOSE_NOT_FOUND: u16 = 4404;
const CLOSE_LIMIT: u16 = 4409;
const CLOSE_REFUSED: u16 = 4502;
const CLOSE_CHANNEL_LOST: u16 = 4503;
const CLOSE_TIMEOUT: u16 = 4504;

pub(crate) fn terminal_disabled() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::NOT_FOUND,
        "terminal_disabled",
        "This instance has no web terminal.",
    )
}

fn terminal_not_supported() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "terminal_not_supported",
        "Web terminals exist only on hosted agents.",
    )
}

/// How often an open terminal re-checks that its holder still has access (C-617 `4403`
/// "access lost": a removed member, a frozen agent, a revoked dashboard session).
const ACCESS_RECHECK: Duration = Duration::from_secs(10);

/// A browser text message is a `pty.resize` control — a few dozen bytes; longer is refused
/// `4502` and never forwarded (C-617).
const TEXT_MAX: usize = 256;
/// One write toward the browser or the runner may not stall longer than this.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Ping interval toward the browser (`16 §W18`: ping/pong every 20 s).
const PING_INTERVAL: Duration = Duration::from_secs(20);

/// Live terminal sessions terminated on **this** API replica, per user and per agent
/// (C-617 `4409`; `TERMINALS_PER_USER` / `TERMINALS_PER_AGENT` are per replica — with N
/// replicas the effective ceiling is N×, `15 §Web terminal operations`). A slot is a
/// [`TerminalSlot`]: taken before the grant is minted, released by `Drop` on every exit path
/// (normal close, dial failure, error, task panic or cancellation).
#[derive(Default)]
struct Live {
    per_user: std::collections::HashMap<Uuid, u32>,
    per_agent: std::collections::HashMap<Uuid, u32>,
}

fn live() -> &'static std::sync::Mutex<Live> {
    static LIVE: std::sync::OnceLock<std::sync::Mutex<Live>> = std::sync::OnceLock::new();
    LIVE.get_or_init(Default::default)
}

/// One live terminal's cap slots; releases both counters on drop.
pub(crate) struct TerminalSlot {
    user: Uuid,
    agent: Uuid,
}

impl TerminalSlot {
    /// `None` when the user or the agent is at its cap (the caller closes `4409`).
    pub(crate) fn acquire(user: Uuid, agent: Uuid) -> Option<Self> {
        let mut g = live().lock().unwrap_or_else(|e| e.into_inner());
        let u = g.per_user.get(&user).copied().unwrap_or(0);
        let a = g.per_agent.get(&agent).copied().unwrap_or(0);
        if u >= aurora_core::limits::TERMINALS_PER_USER
            || a >= aurora_core::limits::TERMINALS_PER_AGENT
        {
            return None;
        }
        g.per_user.insert(user, u + 1);
        g.per_agent.insert(agent, a + 1);
        Some(Self { user, agent })
    }
}

impl Drop for TerminalSlot {
    fn drop(&mut self) {
        let mut guard = live().lock().unwrap_or_else(|e| e.into_inner());
        let g = &mut *guard;
        for (map, key) in [(&mut g.per_user, self.user), (&mut g.per_agent, self.agent)] {
            if let Some(n) = map.get_mut(&key) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    map.remove(&key);
                }
            }
        }
    }
}

fn too_many_sessions() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "too_many_sessions",
        "Too many open terminal sessions.",
    )
}

/// The admission gate of C-616/SEC-87, in order: feature on (404), hosted agent (422),
/// `can_use` (404/403), not frozen (423). API keys, agent tokens and anonymous callers never
/// get this far (the `Caller` extractor refuses them with 401 before the handler runs).
async fn admit(s: &AppState, c: &Caller, agent: Uuid) -> Result<AgentRow, ApiError> {
    // Off → every surface 404 `terminal_disabled` (C-616; the route itself answers before this
    // when the feature is off, but re-check: the flag may flip between route registration and
    // now on a hot config reload).
    if !s.config.terminal.enabled {
        return Err(terminal_disabled());
    }
    let a = load_agent(s, c, agent).await?;
    // Hosted only: a BYO agent has no sandbox to open a shell in (422).
    if a.kind != "hosted" {
        return Err(terminal_not_supported());
    }
    // `can_use` of the agent's workspace (the preview-share authority).
    c.workspace(s, a.workspace_id).await?;
    // Frozen agents show the freeze banner and no terminal (423).
    crate::domains::freeze::service::ensure_agent_active_pool(s, a.id).await?;
    Ok(a)
}

/// `GET /v1/agents/{agent}/terminal`: the WebSocket upgrade (C-616). Answers 202
/// `{waking: true}` for a parked sandbox (the open was the wake demand; the dashboard retries
/// on ready) or long-polls ≤ `TERMINAL_WAKE_WAIT` when the client asked to wait.
pub async fn upgrade(
    s: AppState,
    c: Caller,
    agent: Uuid,
    ws: WebSocketUpgrade,
    wait: bool,
) -> Result<axum::response::Response, ApiError> {
    let a = admit(&s, &c, agent).await?;
    let Some((sandbox_id, state, boot_id)) = load_sandbox(&s, a.id).await? else {
        // Unprovisioned agent: waking it is meaningless without a start request; the dashboard
        // starts the agent explicitly. 404 as a pre-upgrade answer.
        return Err(ApiError::not_found("Agent has no sandbox."));
    };
    let mut boot_id = boot_id;
    if !is_live(&state) {
        // Parked → wake demand (C-621): rate-limited, never a turn. Frozen never gets here
        // (admit refused it).
        demand_wake(&s, &a, sandbox_id).await?;
        if !wait {
            return Err(waking());
        }
        // C-616: long-poll ≤ `TERMINAL_WAKE_WAIT` for the wake, then upgrade; the 202 stays
        // the answer when it did not come up in time. No connection is held while waiting.
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(s.config.terminal.wake_wait_secs.max(1));
        loop {
            tokio::time::sleep(WAKE_POLL).await;
            match load_sandbox(&s, a.id).await? {
                // The redemption and the runner's dial need `running` (not `starting`).
                Some((_, st, boot)) if st == "running" => {
                    boot_id = boot;
                    break;
                }
                Some(_) if tokio::time::Instant::now() < deadline => {}
                _ => return Err(waking()),
            }
        }
        // The wake may have recreated the holder's session or frozen the agent meanwhile.
        admit(&s, &c, agent).await?;
    }
    let boot = boot_id.ok_or_else(|| ApiError::not_found("Agent has no sandbox."))?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    // ---- caps over LIVE sessions (C-617 `4409`): the slot is held for the life of the leg
    // (moved into `proxy_leg`) and released by `Drop` on every exit, including a failed dial.
    // Over the cap the upgrade completes and the socket closes `4409` (no grant is minted).
    let Some(slot) = TerminalSlot::acquire(c.user_id, a.id) else {
        drop(conn);
        return Ok(ws.on_upgrade(|mut socket| async move {
            let _ = socket
                .send(WsMessage::Close(Some(axum::extract::ws::CloseFrame {
                    code: CLOSE_LIMIT,
                    reason: "".into(),
                })))
                .await;
        }));
    };
    // ---- mint the single-use `auy_` grant (C-618) and answer the upgrade by proxying.
    let grant = token::generate(token::TokenPrefix::TerminalStream);
    let grant_hash = hex(&token::hash(grant.expose()));
    let id = Uuid::now_v7();
    let expires_at = chrono::Utc::now()
        + chrono::Duration::seconds(i64::try_from(TERMINAL_GRANT_TTL.as_secs()).unwrap_or(30));
    grants_repo::insert_terminal_grant(
        &mut *conn,
        id,
        a.id,
        sandbox_id,
        boot,
        c.user_id,
        &grant_hash,
        expires_at,
    )
    .await?;
    drop(conn);
    // Best-effort audit `agent.terminal.open` (C-618): metadata is the agent/workspace only —
    // never bytes, never the grant value.
    crate::audit::record(
        &s,
        AuditEvent::new(
            AuditActor::from_caller(&c),
            "agent.terminal.open",
            Outcome::Success,
        )
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("agent", a.id)
        .metadata(serde_json::json!({ "agent": a.id, "workspace": a.workspace_id })),
    );
    // ---- proxy: dial the runner's :4108 with the grant as the bearer, then bridge.
    let endpoint = dial_endpoint(&s, sandbox_id).await?;
    let upstream = dial_runner(&endpoint.url, grant.expose()).await;
    let upstream = match upstream {
        Ok(u) => u,
        // A stale boot between mint and dial: the grant is consumed already (single-use); the
        // browser retries with a fresh mint.
        Err(code) => return Err(proxy_error(code)),
    };
    let recheck = Recheck {
        s: s.clone(),
        c: c.clone(),
        agent: a.id,
        org: a.org_id,
        workspace: a.workspace_id,
    };
    Ok(ws.on_upgrade(move |socket| proxy_leg(socket, upstream, endpoint.redacted, slot, recheck)))
}

/// The agent's sandbox row: `(id, state, boot_id)`.
type SandboxRow = (Uuid, String, Option<Uuid>);

async fn load_sandbox(s: &AppState, agent: Uuid) -> Result<Option<SandboxRow>, ApiError> {
    Ok(
        sqlx::query_as("SELECT id, state, boot_id FROM sandboxes WHERE agent_id = $1")
            .bind(agent)
            .fetch_optional(&s.pool)
            .await
            .map_err(DbError::from)?,
    )
}

fn is_live(state: &str) -> bool {
    matches!(state, "starting" | "running")
}

/// How often the `?wait=true` long-poll looks at the sandbox again.
const WAKE_POLL: Duration = Duration::from_millis(250);

fn waking() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::ACCEPTED,
        "waking",
        "The sandbox is waking; retry in a moment.",
    )
}

// ---------------------------------------------------------------- grant redemption (C-617)

fn grant_invalid() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "grant_invalid",
        "This stream grant is not valid.",
    )
}

fn not_current() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::CONFLICT,
        "not_current",
        "The sandbox's terminal is not current.",
    )
}

/// The runner-plane redemption of a terminal grant (`POST
/// /v1/runner/preview-streams/open` with `kind: "terminal"`, C-617): consume the single-use
/// `auy_` grant (CAS by hash, ≤ `TERMINAL_GRANT_TTL`), then validate the binding is still
/// live: the boot current, the sandbox running, the terminal on, the agent hosted and not
/// frozen. The answer carries only the binding (`16 §W19`): no port, no share, the terminal
/// window and the leg timeouts. The answering runner is any live replica of the installation
/// — the channel check at the runner (`not_here`) fences it to the one holding the channel.
pub(crate) async fn open(
    s: &AppState,
    runner: &runners_repo::RunnerRow,
    req: PreviewStreamOpenRequest,
) -> Result<PreviewStreamOpenResponse, ApiError> {
    // Off → the feature does not exist on this instance (404 `terminal_disabled` shape, but a
    // runner-plane route: 403 `grant_invalid` — no terminal grant could have been minted).
    if !s.config.terminal.enabled {
        return Err(grant_invalid());
    }
    let hash = hex(&token::hash(req.grant.expose()));
    let row = grants_repo::consume_terminal_grant(&s.pool, &hash)
        .await?
        .ok_or_else(grant_invalid)?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    // The boot must still be current and the sandbox running (`16 §W17` posture).
    let cur: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT sb.boot_id, sb.channel_replica_id FROM sandboxes sb \
         WHERE sb.id = $1 AND sb.state = 'running'",
    )
    .bind(row.sandbox_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((Some(boot_id), Some(_channel_replica))) = cur else {
        return Err(not_current());
    };
    if boot_id != row.boot_id {
        return Err(not_current());
    }
    // The agent must still be hosted and not frozen (defense in depth: admission checked at
    // the upgrade; the grant may be redeemed up to its TTL later).
    let a = agents_repo::get(&s.pool, row.agent_id)
        .await?
        .ok_or_else(not_current)?;
    if a.kind != "hosted" {
        return Err(not_current());
    }
    crate::domains::freeze::service::ensure_agent_active(&mut conn, row.agent_id).await?;
    // Any live replica of the installation may answer the redemption; the runner's own
    // `not_here` check fences the leg to the replica holding the sandbox's channel.
    let _ = runner;
    Ok(PreviewStreamOpenResponse {
        sandbox_id: row.sandbox_id,
        boot_id: row.boot_id,
        port: None,
        share_id: None,
        window: TERMINAL_STREAM_WINDOW as u32,
        idle_secs: TERMINAL_IDLE_TIMEOUT.as_secs() as u32,
        max_lifetime_secs: TERMINAL_MAX_LIFETIME.as_secs() as u32,
    })
}

fn proxy_error(code: u16) -> ApiError {
    // Post-mint failures map to the close codes (C-617): the dashboard reads the message code.
    match code {
        CLOSE_NOT_FOUND => ApiError::not_found("The sandbox's terminal is not here."),
        CLOSE_LIMIT => too_many_sessions(),
        CLOSE_TIMEOUT | CLOSE_CHANNEL_LOST | CLOSE_REFUSED => ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "terminal_upstream",
            "The sandbox's terminal could not be opened.",
        ),
        _ => ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "terminal_upstream",
            "The sandbox's terminal could not be opened.",
        ),
    }
}

/// The wake demand (C-621): set `terminal_wake_at` (due work for `TERMINAL_WAKE_WINDOW`),
/// rate-limited to `TERMINAL_WAKES_PER_AGENT_HOUR` per agent (429 beyond it).
async fn demand_wake(s: &AppState, a: &AgentRow, sandbox: Uuid) -> Result<(), ApiError> {
    s.rate_limiter
        .check_quota(
            "terminal_wake",
            &a.id.to_string(),
            crate::http::ratelimit::Quota {
                capacity: TERMINAL_WAKES_PER_AGENT_HOUR,
                per: Duration::from_secs(3600),
            },
        )
        .map_err(|_| too_many_sessions())?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    aurora_db::repo::sandboxes::set_terminal_wake(&mut *conn, sandbox).await?;
    // Notify the due-work engine on this replica; other replicas poll anyway.
    crate::domains::runner_plane::service::notify_work(s, "terminal_wake").await;
    Ok(())
}

/// The dial endpoint of the sandbox's runner replica (`ws[s]://…:4108/v1/preview-stream`),
/// from the registered advertised host + `stream_tls` capability (C-617: the runner never
/// learns a port; the endpoint is the same listener the relay dials).
async fn dial_endpoint(s: &AppState, sandbox: Uuid) -> Result<Endpoint, ApiError> {
    let row: Option<(Option<Uuid>, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT boot_id, channel_replica_id, runner_id FROM sandboxes \
         WHERE id = $1 AND state = 'running'",
    )
    .bind(sandbox)
    .fetch_optional(&s.pool)
    .await
    .map_err(DbError::from)?;
    let Some((Some(_boot), Some(channel_replica), Some(runner))) = row else {
        return Err(proxy_error(CLOSE_NOT_FOUND));
    };
    // The endpoint of the replica **holding the channel** (the leg must land on it, C-617);
    // a sandbox without a channel replica is not current (checked again by the caller).
    let host = runners_repo::replica_host(&s.pool, channel_replica)
        .await?
        .ok_or_else(|| proxy_error(CLOSE_NOT_FOUND))?;
    let r = runners_repo::get(&s.pool, runner)
        .await?
        .ok_or_else(|| proxy_error(CLOSE_NOT_FOUND))?;
    let tls = crate::domains::runner_plane::work::caps_of(&r)
        .map(|c| c.stream_tls)
        .unwrap_or(false);
    Ok(Endpoint {
        url: crate::domains::runner_plane::preview_streams::runner_endpoint_of(&host, tls),
        redacted: format!("runner:{runner}"),
    })
}

struct Endpoint {
    url: String,
    redacted: String,
}

/// Dials the runner's stream listener with the `auy_` grant as the bearer (C-617 leg 1).
/// TLS is the runner's own certificate job (same posture as the relay's dial); the endpoint
/// scheme already encodes it. Never logs the grant.
async fn dial_runner(
    endpoint: &str,
    grant: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    u16,
> {
    let mut req = endpoint.into_client_request().map_err(|_| CLOSE_REFUSED)?;
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {grant}")
            .parse()
            .map_err(|_| CLOSE_REFUSED)?,
    );
    let r = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(req),
    )
    .await;
    match r {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(_)) => Err(CLOSE_REFUSED),
        Err(_) => Err(CLOSE_TIMEOUT),
    }
}

/// What an open leg needs to re-check its holder's access.
struct Recheck {
    s: AppState,
    c: Caller,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
}

impl Recheck {
    /// `false` only on a definite refusal (not an infrastructure error: a flapping DB must
    /// not kill a shell): the dashboard session is no longer live, or the admission law
    /// (`admit`: feature on, hosted, `can_use`, not frozen) now refuses.
    async fn still_allowed(&self) -> bool {
        if let crate::http::auth::Principal::Session { id } = self.c.principal {
            // Revoked / expired / disabled user. Not the idle clock: a shell in active use
            // makes no other API calls, and its own idle timeout is the runner's.
            let live: Result<Option<bool>, _> = sqlx::query_scalar(
                "SELECT (s.revoked_at IS NULL AND s.expires_at > now() AND u.disabled_at IS NULL) \
                 FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id = $1",
            )
            .bind(id)
            .fetch_optional(&self.s.pool)
            .await
            .map(Option::flatten);
            match live {
                Ok(Some(true)) | Err(_) => {}
                Ok(Some(false) | None) => return false,
            }
        }
        match admit(&self.s, &self.c, self.agent).await {
            Ok(_) => true,
            // A 4xx is a definite refusal (404 lost access/gone, 423 frozen, 422 not hosted);
            // anything else (5xx) is infrastructure: keep the shell.
            Err(e) => !e.status().is_client_error(),
        }
    }
}

/// The leg-1 bridge (C-617): browser ⟷ runner, byte-transparent. Binary messages are PTY
/// bytes (≤ `TERMINAL_CHUNK`) both ways; browser text messages are `pty.resize` controls
/// passed through as text; runner text messages (`pty.*` answers) are not for the browser and
/// are dropped. Bytes are never parsed, never logged; only the close code carries meaning.
async fn proxy_leg(
    browser: WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    endpoint: String,
    _slot: TerminalSlot,
    recheck: Recheck,
) {
    let (mut b_sink, mut b_stream) = browser.split();
    let (mut u_sink, mut u_stream) = upstream.split();
    let mut tick =
        tokio::time::interval_at(tokio::time::Instant::now() + ACCESS_RECHECK, ACCESS_RECHECK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Liveness (`16 §W18`: idle 600 s, "a WebSocket's pings count"): the API pings the browser
    // and any frame from it — a pong included — resets the idle clock, so a user reading
    // output for ten minutes stays connected while a vanished tab is cut at the limit. The
    // runner's own pings are answered by the upstream socket itself.
    let mut ping =
        tokio::time::interval_at(tokio::time::Instant::now() + PING_INTERVAL, PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let idle = tokio::time::sleep(TERMINAL_IDLE_TIMEOUT);
    tokio::pin!(idle);
    let started = std::time::Instant::now();
    let reason = loop {
        tokio::select! {
            () = &mut idle => break CLOSE_TIMEOUT,
            _ = ping.tick() => {
                if !bounded_send(&mut b_sink, WsMessage::Ping(Vec::new().into()), WRITE_TIMEOUT).await {
                    break CLOSE_NORMAL;
                }
            }
            // Access re-check (C-617 `4403`): the holder must still pass the admission law.
            _ = tick.tick() => {
                if !recheck.still_allowed().await {
                    let _ = bounded_send(&mut u_sink, Ws2Message::Close(None), WRITE_TIMEOUT).await;
                    break CLOSE_NOT_ALLOWED;
                }
            }
            // Browser → runner.
            msg = b_stream.next() => {
                if msg.as_ref().is_some_and(Result::is_ok) {
                    idle.as_mut().reset(tokio::time::Instant::now() + TERMINAL_IDLE_TIMEOUT);
                }
                match msg {
                    Some(Ok(WsMessage::Binary(b))) => {
                        if b.len() > TERMINAL_CHUNK {
                            break CLOSE_REFUSED;
                        }
                        if !bounded_send(&mut u_sink, Ws2Message::Binary(b), WRITE_TIMEOUT).await {
                            break CLOSE_CHANNEL_LOST;
                        }
                    }
                    // `pty.resize` controls ride as text (a few dozen bytes); a longer text
                    // message is a violation and is never forwarded (C-617).
                    Some(Ok(WsMessage::Text(t))) => {
                        if t.len() > TEXT_MAX {
                            break CLOSE_REFUSED;
                        }
                        if !bounded_send(&mut u_sink, Ws2Message::Text(t.as_str().into()), WRITE_TIMEOUT).await {
                            break CLOSE_CHANNEL_LOST;
                        }
                    }
                    // A browser ping is answered by axum itself; the idle clock was reset above.
                    Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => {}
                    // The browser ended the leg: pass the close frame's reason upstream (the
                    // runner releases the route; the code the browser chose is final).
                    Some(Ok(WsMessage::Close(code))) => {
                        let _ = u_sink.send(Ws2Message::Close(code.map(|f| {
                            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(f.code),
                                reason: f.reason.as_str().into(),
                            }
                        }))).await;
                        break CLOSE_NORMAL;
                    }
                    Some(Err(_)) | None => break CLOSE_NORMAL,
                }
            }
            // Runner → browser.
            msg = u_stream.next() => {
                match msg {
                    Some(Ok(Ws2Message::Binary(b))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + TERMINAL_IDLE_TIMEOUT);
                        // A tab that stops reading must not hold the slot: the write is
                        // bounded and a timeout ends the leg.
                        if !bounded_send(&mut b_sink, WsMessage::Binary(b), WRITE_TIMEOUT).await {
                            break CLOSE_NORMAL;
                        }
                    }
                    // The runner's `stream.opened`-style text frames are not for the browser;
                    // the leg-1 contract after 101 is bytes + close codes only. Drop them.
                    Some(Ok(Ws2Message::Text(_) | Ws2Message::Frame(_))) => {}
                    // The upstream socket answers the runner's pings itself.
                    Some(Ok(Ws2Message::Ping(_) | Ws2Message::Pong(_))) => {}
                    Some(Ok(Ws2Message::Close(frame))) => {
                        let code: u16 = frame
                            .as_ref()
                            .map(|f| u16::from(f.code))
                            .unwrap_or(CLOSE_NORMAL);
                        let _ = bounded_send(&mut b_sink, WsMessage::Close(Some(axum::extract::ws::CloseFrame {
                            code,
                            reason: frame
                                .as_ref()
                                .map(|f| f.reason.as_str())
                                .unwrap_or_default()
                                .into(),
                        })), WRITE_TIMEOUT)
                        .await;
                        break code;
                    }
                    Some(Err(_)) | None => break CLOSE_CHANNEL_LOST,
                }
            }
        }
    };
    // Complete the closing handshake on the browser side with the upstream's code (C-617's
    // close-code table is the browser's only signal; 1000 when the browser ended it).
    let code = if reason == CLOSE_NORMAL {
        CLOSE_NORMAL
    } else {
        reason
    };
    let _ = bounded_send(
        &mut b_sink,
        WsMessage::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: "".into(),
        })),
        WRITE_TIMEOUT,
    )
    .await;
    // Byte totals ride the runner's `pty.close` informational fields only; nothing is logged
    // here (the endpoint string is host:port only, no grant, no bytes).
    tracing::debug!(endpoint, code, "terminal leg ended");
    // (D-178, C-618) Best-effort `agent.terminal.close`: the close code and the session's
    // duration — never bytes, never content (SEC-87).
    crate::audit::record(
        &recheck.s,
        AuditEvent::new(
            AuditActor::from_caller(&recheck.c),
            "agent.terminal.close",
            Outcome::Success,
        )
        .org(recheck.org)
        .workspace(recheck.workspace)
        .target("agent", recheck.agent)
        .metadata(serde_json::json!({
            "agent": recheck.agent,
            "workspace": recheck.workspace,
            "close_code": code,
            "duration_secs": started.elapsed().as_secs(),
        })),
    );
}

/// One write under a deadline: `false` when it failed or stalled (a client that stopped reading
/// must not hold the leg, and with it a [`TerminalSlot`], forever).
async fn bounded_send<S, M>(sink: &mut S, msg: M, within: Duration) -> bool
where
    S: futures::Sink<M> + Unpin,
{
    matches!(
        tokio::time::timeout(within, sink.send(msg)).await,
        Ok(Ok(()))
    )
}

/// Lower-case hex of a SHA-256 (the `grant_hash` column is text, like the previews plane's).
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink whose writes never complete: a browser tab that stopped reading.
    struct Stalled;
    impl futures::Sink<u8> for Stalled {
        type Error = ();
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), ()>> {
            std::task::Poll::Pending
        }
        fn start_send(self: std::pin::Pin<&mut Self>, _: u8) -> Result<(), ()> {
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), ()>> {
            std::task::Poll::Pending
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), ()>> {
            std::task::Poll::Pending
        }
    }

    /// FIX-092: a write toward a peer that never reads ends at the deadline (`false`), so the
    /// leg returns and its slot is released by `Drop`.
    #[tokio::test]
    async fn a_stalled_write_ends_at_the_deadline() {
        let mut sink = Stalled;
        assert!(!bounded_send(&mut sink, 1u8, Duration::from_millis(50)).await);
        let (user, agent) = (Uuid::now_v7(), Uuid::now_v7());
        let slot = TerminalSlot::acquire(user, agent).unwrap();
        drop(slot);
        assert!(TerminalSlot::acquire(user, agent).is_some(), "slot freed");
    }
}

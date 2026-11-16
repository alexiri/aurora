//! SSH sessions: the API decides every session and can always end it (phase 5, P4-SSH-2, D-239,
//! SEC-94; C-793, C-794). The terminator presents a certificate; the API re-verifies it against
//! its own CA keys, checks the key and key-id binding, the access law (`can_ssh`) and the global
//! caps in one transaction, and from then on every channel grant, every heartbeat and every
//! sweep re-runs the law. A session closes exactly once (the guarded `open -> closed` update
//! decides who writes the one `agent.ssh.close` audit row). Certificates, command lines and
//! channel bytes are never logged or stored; the audit rows carry names, counts and totals.
use super::dto::{SshSession, SshSessionAgent, SshSessionKey, SshSessionList, SshSessionUser};
use super::service::ssh_disabled;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::Caller;
use aurora_core::limits::{
    SSH_CHANNEL_POLL_MAX, SSH_CHANNELS_PER_SESSION, SSH_GRANT_TTL, SSH_HEARTBEAT, SSH_MAX_LIFETIME,
    SSH_SESSION_LEASE,
};
use aurora_core::{ApiError, token};
use aurora_db::DbError;
use aurora_db::authz::{self, SshAccess};
use aurora_db::repo::ssh_sessions::{self as repo, SshSessionRow};
use aurora_db::repo::{agents, ssh as keys_repo};
use aurora_protocol::terminator as wire;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::json;
use ssh_key::{Certificate, HashAlg, PublicKey, certificate::CertType};
use std::time::Duration;
use uuid::Uuid;

/// The close reasons the audit and the table accept; anything else a terminator sends is stored
/// as `relay_error` (an open vocabulary on the wire, a closed one in the data).
const CLOSE_REASONS: [&str; 14] = [
    "client_disconnect",
    "idle_timeout",
    "max_duration",
    "access_revoked",
    "key_deleted",
    "frozen",
    "ssh_disabled",
    "user_disconnect",
    "owner_disconnect",
    "agent_deleted",
    "lease_expired",
    "wake_timeout",
    "terminator_shutdown",
    "relay_error",
];

fn lease_secs() -> i64 {
    SSH_SESSION_LEASE.as_secs() as i64
}

/// How the terminator sees a refused session open: one hint-free body for every cause.
pub enum OpenError {
    Refused(wire::SessionRefusal),
    Api(ApiError),
}

impl From<ApiError> for OpenError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}
impl From<DbError> for OpenError {
    fn from(e: DbError) -> Self {
        Self::Api(e.into())
    }
}

impl IntoResponse for OpenError {
    fn into_response(self) -> Response {
        match self {
            Self::Refused(reason) => (
                StatusCode::FORBIDDEN,
                axum::Json(wire::SessionRefusedBody { reason }),
            )
                .into_response(),
            Self::Api(e) => e.into_response(),
        }
    }
}

// ------------------------------------------------------------------------- register/config

/// The CA public keys a terminator may pre-verify with: the active CA and the retired ones.
fn ca_public_keys(s: &AppState) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(signer) = &s.ssh_signer
        && let Ok(line) = signer.public_key().to_openssh()
    {
        v.push(line.trim().to_owned());
    }
    v.extend(s.config.ssh.previous_public_keys.iter().cloned());
    v
}

fn ca_fingerprints(s: &AppState) -> Vec<ssh_key::Fingerprint> {
    ca_public_keys(s)
        .iter()
        .filter_map(|l| PublicKey::from_openssh(l).ok())
        .map(|k| k.fingerprint(HashAlg::Sha256))
        .collect()
}

pub fn terminator_config(s: &AppState) -> wire::TerminatorConfig {
    let c = &s.config.ssh;
    wire::TerminatorConfig {
        ca_public_keys: ca_public_keys(s),
        heartbeat_secs: SSH_HEARTBEAT.as_secs() as u32,
        lease_secs: SSH_SESSION_LEASE.as_secs() as u32,
        wake_wait_secs: c.wake_wait_secs as u32,
        idle_timeout_secs: c.idle_timeout_secs as u32,
        max_lifetime_secs: SSH_MAX_LIFETIME.as_secs() as u32,
        channels_per_session: SSH_CHANNELS_PER_SESSION,
    }
}

pub async fn register(
    s: &AppState,
    req: wire::RegisterRequest,
) -> Result<wire::RegisterResponse, ApiError> {
    let key = PublicKey::from_openssh(req.host_public_key.trim())
        .ok()
        .filter(|k| k.algorithm() == ssh_key::Algorithm::Ed25519)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_host_key",
                "host_public_key must be one ssh-ed25519 public key.",
            )
        })?;
    let mut bare = key.clone();
    bare.set_comment("");
    let canonical = bare
        .to_openssh()
        .map_err(|_| ApiError::internal("The host key could not be encoded."))?
        .trim()
        .to_owned();
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let id = Uuid::now_v7();
    repo::insert_replica(&s.pool, id, &req.version, &canonical, &fingerprint).await?;
    Ok(wire::RegisterResponse {
        replica_id: id,
        protocol: wire::TERMINATOR_PROTOCOL,
        config: terminator_config(s),
    })
}

// --------------------------------------------------------------------------- session open

/// What the certificate's key id (`aurora:u=…:k=…:a=…:w=…`, C-791) names.
struct KeyIdParts {
    user: Uuid,
    key: Uuid,
    agent: Uuid,
    workspace: Uuid,
}

fn parse_key_id(id: &str) -> Option<KeyIdParts> {
    let mut parts = id.split(':');
    if parts.next()? != "aurora" {
        return None;
    }
    let mut get = |tag: &str| -> Option<Uuid> {
        Uuid::parse_str(parts.next()?.strip_prefix(tag)?.strip_prefix('=')?).ok()
    };
    let p = KeyIdParts {
        user: get("u")?,
        key: get("k")?,
        agent: get("a")?,
        workspace: get("w")?,
    };
    parts.next().is_none().then_some(p)
}

/// Re-verify the certificate on the API's own terms. `None` for every defect: the caller
/// answers one hint-free refusal.
fn verify_certificate(
    s: &AppState,
    line: &str,
    username: &str,
) -> Option<(Certificate, KeyIdParts)> {
    let cert = Certificate::from_openssh(line.trim()).ok()?;
    let cas = ca_fingerprints(s);
    if cas.is_empty() {
        return None;
    }
    // Signature, validity window and CA only; everything else below.
    cert.validate_at(Utc::now().timestamp().max(0) as u64, cas.iter())
        .ok()?;
    if cert.cert_type() != CertType::User
        || cert.valid_principals().len() != 1
        || cert.valid_principals()[0] != username
        || !cert.critical_options().is_empty()
    {
        return None;
    }
    let parts = parse_key_id(cert.key_id())?;
    (parts.agent.to_string() == username).then_some((cert, parts))
}

/// `POST /v1/terminator/sessions`.
pub async fn open_session(
    s: &AppState,
    replica: Uuid,
    req: wire::SessionOpenRequest,
) -> Result<wire::SessionOpenResponse, OpenError> {
    use wire::SessionRefusal::{Denied, Limit};
    if !s.config.ssh.enabled {
        return Err(ssh_disabled().into());
    }
    let (cert, kid) = verify_certificate(s, req.certificate.expose(), &req.username)
        .ok_or(OpenError::Refused(Denied))?;
    let fingerprint = PublicKey::from(cert.public_key().clone())
        .fingerprint(HashAlg::Sha256)
        .to_string();
    let serial = i64::try_from(cert.serial()).unwrap_or(i64::MAX);

    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    // The certified key must still be registered to the key id's person, with this fingerprint.
    let key = keys_repo::find_key_by_fingerprint(&mut *tx, kid.user, &fingerprint)
        .await?
        .filter(|k| k.id == kid.key)
        .ok_or(OpenError::Refused(Denied))?;
    let SshAccess::Allowed {
        workspace_id,
        org_id,
    } = authz::can_ssh(&mut tx, true, kid.user, kid.agent).await?
    else {
        return Err(OpenError::Refused(Denied));
    };
    if workspace_id != kid.workspace {
        return Err(OpenError::Refused(Denied));
    }
    // Caps over open, lease-current rows, global; the rows are locked so concurrent opens over
    // different replicas serialise here.
    let per_user = i64::from(s.config.ssh.sessions_per_user);
    let per_agent = i64::from(s.config.ssh.sessions_per_agent);
    if repo::lock_count_open_for_user(&mut tx, kid.user).await? >= per_user
        || repo::lock_count_open_for_agent(&mut tx, kid.agent).await? >= per_agent
    {
        return Err(OpenError::Refused(Limit));
    }
    // The sandbox: a parked one gets the wake demand within the shared `terminal_wake` budget.
    let agent = agents::get(&s.pool, kid.agent)
        .await?
        .ok_or(OpenError::Refused(Denied))?;
    let sandbox = sandbox_of(s, kid.agent).await?;
    let hint = match &sandbox {
        Some((_, st, _)) if st == "running" => wire::SandboxHint::Running,
        Some((_, st, _)) if st == "starting" => wire::SandboxHint::Waking,
        Some((id, _, _)) => {
            if !crate::domains::terminal::service::wake_demand(s, &agent, *id).await? {
                return Err(OpenError::Refused(Limit));
            }
            wire::SandboxHint::Waking
        }
        None => wire::SandboxHint::Stopped,
    };
    let address = req
        .client_address
        .as_deref()
        .and_then(|a| a.parse::<std::net::IpAddr>().ok())
        .map(crate::domains::cli_login::service::coarse_addr);
    let id = Uuid::now_v7();
    let row = repo::insert_session(
        &mut *tx,
        id,
        kid.user,
        key.id,
        &fingerprint,
        serial,
        org_id,
        workspace_id,
        kid.agent,
        replica,
        address.as_deref(),
        lease_secs(),
    )
    .await?;
    keys_repo::touch_key(&mut *tx, key.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::system("ssh-terminator").with_source("system"),
            "agent.ssh.open",
            Outcome::Success,
        )
        .org(org_id)
        .workspace(workspace_id)
        .target("agent", kid.agent)
        .metadata(json!({
            "agent": kid.agent,
            "workspace": workspace_id,
            "user": kid.user,
            "fingerprint": fingerprint,
            "serial": cert.serial().to_string(),
            "client_address": address,
            "replica": replica,
            "session": id,
        })),
    );
    Ok(wire::SessionOpenResponse {
        session_id: id,
        agent_id: kid.agent,
        sandbox: hint,
        lease_secs: SSH_SESSION_LEASE.as_secs() as u32,
        expires_at: row.lease_until.to_rfc3339(),
    })
}

/// The agent's sandbox row: `(id, state, boot_id)`.
async fn sandbox_of(
    s: &AppState,
    agent: Uuid,
) -> Result<Option<(Uuid, String, Option<Uuid>)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, state, boot_id FROM sandboxes WHERE agent_id = $1")
            .bind(agent)
            .fetch_optional(&s.pool)
            .await?,
    )
}

// ------------------------------------------------------------------------------- closing

/// Write the one `agent.ssh.close` row for a transition this caller won.
fn audit_close(s: &AppState, row: &SshSessionRow, by_mode: Option<&wire::ChannelsByMode>) {
    let secs = row
        .closed_at
        .map(|c| (c - row.created_at).num_seconds().max(0))
        .unwrap_or(0);
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::system("ssh-terminator").with_source("system"),
            "agent.ssh.close",
            Outcome::Success,
        )
        .org(row.org_id)
        .workspace(row.workspace_id)
        .target("agent", row.agent_id)
        .metadata(json!({
            "agent": row.agent_id,
            "workspace": row.workspace_id,
            "user": row.user_id,
            "session": row.id,
            "reason": row.close_reason,
            "duration_secs": secs,
            "channels_total": row.channels_total,
            "channels_by_mode": by_mode.map(|m| json!({"shell": m.shell, "exec": m.exec, "sftp": m.sftp})),
            "bytes_in": row.bytes_in,
            "bytes_out": row.bytes_out,
        })),
    );
}

/// The guarded transition plus its audit row; `true` when this caller won it.
async fn close_with(
    s: &AppState,
    id: Uuid,
    reason: &str,
    channels_total: i32,
    bytes_in: i64,
    bytes_out: i64,
    by_mode: Option<&wire::ChannelsByMode>,
) -> Result<bool, ApiError> {
    match repo::close_session(&s.pool, id, reason, channels_total, bytes_in, bytes_out).await? {
        Some(row) => {
            audit_close(s, &row, by_mode);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// `POST /v1/terminator/sessions/{id}/close`: idempotent, the first close wins.
pub async fn close_session(
    s: &AppState,
    replica: Uuid,
    id: Uuid,
    req: wire::SessionCloseRequest,
) -> Result<wire::OkBody, ApiError> {
    // Only the replica that holds the session may close it; anything else is a no-op `ok`.
    if let Some(row) = repo::get_session(&s.pool, id).await?
        && row.terminator_replica_id == replica
    {
        let reason = if CLOSE_REASONS.contains(&req.reason.as_str()) {
            req.reason.as_str()
        } else {
            "relay_error"
        };
        close_with(
            s,
            id,
            reason,
            i32::try_from(req.channels_total).unwrap_or(i32::MAX),
            i64::try_from(req.bytes_in).unwrap_or(i64::MAX),
            i64::try_from(req.bytes_out).unwrap_or(i64::MAX),
            Some(&req.channels_by_mode),
        )
        .await?;
    }
    Ok(wire::OkBody { ok: true })
}

/// The reason the access law refuses a session, or `None` while it holds.
async fn revocation_reason(
    s: &AppState,
    row: &SshSessionRow,
) -> Result<Option<&'static str>, ApiError> {
    if row.ssh_key_id.is_none() {
        return Ok(Some("key_deleted"));
    }
    if !s.config.ssh.enabled {
        return Ok(Some("ssh_disabled"));
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let access = authz::can_ssh(&mut conn, true, row.user_id, row.agent_id).await?;
    Ok(match access {
        SshAccess::Allowed { .. } => None,
        SshAccess::Disabled => Some("ssh_disabled"),
        SshAccess::Frozen(_) => Some("frozen"),
        SshAccess::Denied | SshAccess::NotHosted => Some("access_revoked"),
    })
}

/// `POST /v1/terminator/heartbeat`: renew every kept session's lease and re-run the access law;
/// a revoked, unknown, foreign or lease-expired session is closed here (the keep-awake drops
/// even when a hostile terminator keeps heartbeating) and comes back in `revoked`.
pub async fn heartbeat(
    s: &AppState,
    replica: Uuid,
    req: wire::HeartbeatRequest,
) -> Result<wire::HeartbeatResponse, ApiError> {
    let ids: Vec<Uuid> = req.sessions.iter().map(|b| b.id).collect();
    let rows = repo::get_sessions(&s.pool, &ids).await?;
    let mut revoked = Vec::new();
    let mut kept = 0i32;
    for beat in &req.sessions {
        let Some(row) = rows.iter().find(|r| r.id == beat.id) else {
            revoked.push(wire::Revoked {
                id: beat.id,
                reason: "unknown".into(),
            });
            continue;
        };
        if row.terminator_replica_id != replica {
            revoked.push(wire::Revoked {
                id: beat.id,
                reason: "unknown".into(),
            });
            continue;
        }
        if row.state != "open" {
            revoked.push(wire::Revoked {
                id: beat.id,
                reason: row.close_reason.clone().unwrap_or_else(|| "unknown".into()),
            });
            continue;
        }
        let reason: Option<&'static str> = if row.lease_until < Utc::now() {
            Some("lease_expired")
        } else if let Some(r) = row
            .revoke_reason
            .as_deref()
            .filter(|_| row.revoke_requested_at.is_some())
        {
            Some(if r == "owner_disconnect" {
                "owner_disconnect"
            } else {
                "user_disconnect"
            })
        } else {
            revocation_reason(s, row).await?
        };
        if let Some(reason) = reason {
            close_with(
                s,
                row.id,
                reason,
                i32::try_from(beat.channels_total).unwrap_or(i32::MAX),
                i64::try_from(beat.bytes_in).unwrap_or(i64::MAX),
                i64::try_from(beat.bytes_out).unwrap_or(i64::MAX),
                None,
            )
            .await?;
            revoked.push(wire::Revoked {
                id: row.id,
                reason: reason.into(),
            });
            continue;
        }
        let at = beat
            .last_activity_at
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
            // Never a time in the future (a hostile terminator keeps no session "active").
            .map(|t| t.min(Utc::now()));
        if repo::renew_session(
            &s.pool,
            row.id,
            replica,
            lease_secs(),
            at,
            i32::try_from(beat.channels_total).unwrap_or(i32::MAX),
            i64::try_from(beat.bytes_in).unwrap_or(i64::MAX),
            i64::try_from(beat.bytes_out).unwrap_or(i64::MAX),
        )
        .await?
        {
            kept += 1;
        } else {
            revoked.push(wire::Revoked {
                id: row.id,
                reason: "unknown".into(),
            });
        }
    }
    repo::touch_replica(&s.pool, replica, kept).await?;
    Ok(wire::HeartbeatResponse {
        revoked,
        config: terminator_config(s),
    })
}

/// One tick of the stale-session sweep (every API replica, every 30 s, `SKIP LOCKED`): close
/// the open rows whose lease passed through the same guarded transition.
pub async fn sweep(s: &AppState) -> Result<usize, ApiError> {
    let rows = repo::sweep_expired(&s.pool, 200).await?;
    for r in &rows {
        audit_close(s, r, None);
    }
    Ok(rows.len())
}

/// A person or an owner ends a session (`DELETE /v1/ssh-sessions/{id}`, C-793): the row is
/// marked and the terminator learns at its next heartbeat. 404 for anyone else.
pub async fn disconnect(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let not_found = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "ssh_session_not_found",
            "SSH session not found.",
        )
    };
    let row = repo::get_session(&s.pool, id)
        .await?
        .filter(|r| r.state == "open")
        .ok_or_else(not_found)?;
    let own = row.user_id == c.user_id;
    let owner = !own
        && c.workspace(s, row.workspace_id)
            .await
            .is_ok_and(|w| w.can_own());
    if !own && !owner {
        return Err(not_found());
    }
    let reason = if own {
        "user_disconnect"
    } else {
        "owner_disconnect"
    };
    repo::request_revoke(&s.pool, id, reason).await?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "ssh_session.disconnect",
            Outcome::Success,
        )
        .org(row.org_id)
        .workspace(row.workspace_id)
        .target("agent", row.agent_id)
        .metadata(
            json!({"session": id, "agent": row.agent_id, "by": if own { "user" } else { "owner" }}),
        ),
    );
    Ok(())
}

// ------------------------------------------------------------------------------- channels

fn session_closed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "session_closed",
        "The SSH session is closed.",
    )
}

/// `POST /v1/terminator/sessions/{id}/channels`: a single-use `auz_` grant for one channel, only
/// for an open, lease-current session that still passes the access law.
pub async fn channel_grant(
    s: &AppState,
    replica: Uuid,
    id: Uuid,
    req: wire::ChannelGrantRequest,
) -> Result<wire::ChannelGrantResponse, ApiError> {
    let row = repo::get_session(&s.pool, id)
        .await?
        .filter(|r| r.terminator_replica_id == replica && r.state == "open")
        .ok_or_else(session_closed)?;
    if row.lease_until < Utc::now() {
        close_with(s, id, "lease_expired", 0, 0, 0, None).await?;
        return Err(session_closed());
    }
    if let Some(reason) = revocation_reason(s, &row).await? {
        close_with(s, id, reason, 0, 0, 0, None).await?;
        return Err(session_closed());
    }
    let live_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ssh_stream_grants \
          WHERE session_id = $1 AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(id)
    .fetch_one(&s.pool)
    .await
    .map_err(DbError::from)?;
    if live_grants >= i64::from(SSH_CHANNELS_PER_SESSION) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "ssh_channel_limit",
            "Too many channels are opening at once.",
        )
        .with_retry_after(1));
    }
    let agent = agents::get(&s.pool, row.agent_id)
        .await?
        .ok_or_else(session_closed)?;
    let wait = Duration::from_secs(u64::from(req.wait_secs.unwrap_or(0))).min(SSH_CHANNEL_POLL_MAX);
    let deadline = tokio::time::Instant::now() + wait;
    let mut woke = false;
    let (sandbox, boot) = loop {
        match sandbox_of(s, row.agent_id).await? {
            Some((sb, st, Some(boot))) if st == "running" => break (sb, boot),
            Some((sb, st, _)) if !woke && st != "starting" => {
                if !crate::domains::terminal::service::wake_demand(s, &agent, sb).await? {
                    return Err(ApiError::new(
                        StatusCode::TOO_MANY_REQUESTS,
                        "wake_limited",
                        "Too many wake-ups for this agent this hour.",
                    )
                    .with_retry_after(60));
                }
                woke = true;
            }
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "not_running",
                "The sandbox is not running yet.",
            )
            .with_details(json!({"retry_after_secs": 2})));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let endpoint = crate::domains::terminal::service::dial_endpoint(s, sandbox).await?;
    let grant = token::generate(token::TokenPrefix::SshStream);
    let hash = hex::encode(token::hash(grant.expose()));
    repo::insert_grant(
        &s.pool,
        Uuid::now_v7(),
        id,
        row.agent_id,
        sandbox,
        boot,
        &hash,
        req.mode.as_str(),
        SSH_GRANT_TTL.as_secs() as i64,
    )
    .await?;
    Ok(wire::ChannelGrantResponse {
        grant: aurora_protocol::common::SecretString::new(grant.expose().to_owned()),
        expires_in: SSH_GRANT_TTL.as_secs() as u32,
        runner_endpoint: endpoint.url,
    })
}

// ------------------------------------------------------------- redemption (runner plane)

/// `POST /v1/runner/preview-streams/open` with `kind: "ssh"` (C-796, `16 §W20`): consume the
/// single-use `auz_` grant (compare-and-set by hash) while its session is open and
/// lease-current, and re-validate the binding: SSH on, boot current, sandbox running, agent
/// hosted and not frozen. A grant of any other prefix is never in this table: 403.
pub(crate) async fn redeem(
    s: &AppState,
    req: aurora_protocol::runner::PreviewStreamOpenRequest,
) -> Result<aurora_protocol::runner::PreviewStreamOpenResponse, ApiError> {
    let invalid = || {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "grant_invalid",
            "This stream grant is not valid.",
        )
    };
    let not_current = || {
        ApiError::new(
            StatusCode::CONFLICT,
            "not_current",
            "The sandbox's SSH channel is not current.",
        )
    };
    if !req
        .grant
        .expose()
        .starts_with(token::TokenPrefix::SshStream.as_str())
    {
        return Err(invalid());
    }
    let hash = hex::encode(token::hash(req.grant.expose()));
    let g = repo::consume_grant(&s.pool, &hash)
        .await?
        .ok_or_else(invalid)?;
    if !s.config.ssh.enabled {
        return Err(not_current());
    }
    let cur: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT sb.boot_id, sb.channel_replica_id FROM sandboxes sb \
          WHERE sb.id = $1 AND sb.state = 'running'",
    )
    .bind(g.sandbox_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(DbError::from)?;
    let Some((Some(boot_id), Some(_))) = cur else {
        return Err(not_current());
    };
    if boot_id != g.boot_id {
        return Err(not_current());
    }
    let a = agents::get(&s.pool, g.agent_id)
        .await?
        .ok_or_else(not_current)?;
    if a.kind != "hosted" {
        return Err(not_current());
    }
    crate::domains::freeze::service::ensure_agent_active_pool(s, a.id)
        .await
        .map_err(|_| not_current())?;
    Ok(aurora_protocol::runner::PreviewStreamOpenResponse {
        sandbox_id: g.sandbox_id,
        boot_id: g.boot_id,
        port: None,
        share_id: None,
        window: aurora_core::limits::TERMINAL_STREAM_WINDOW as u32,
        idle_secs: s.config.ssh.idle_timeout_secs as u32,
        max_lifetime_secs: SSH_MAX_LIFETIME.as_secs() as u32,
        shell: None,
        models: None,
    })
}

// ---------------------------------------------------------------------------- C-793 lists

fn to_dto(r: repo::SessionListRow, with_user: bool) -> SshSession {
    SshSession {
        id: r.id,
        agent: SshSessionAgent {
            id: r.agent_id,
            name: r.agent_name,
        },
        workspace_id: r.workspace_id,
        user: with_user.then(|| SshSessionUser {
            id: r.user_id,
            label: r.user_label.unwrap_or_default(),
        }),
        key: SshSessionKey {
            id: r.ssh_key_id,
            name: r.key_name,
            fingerprint: r.key_fingerprint,
        },
        client_address: r.client_address,
        opened_at: r.created_at,
        last_activity_at: r.last_activity_at,
        channels_open: 0,
        channels_total: u32::try_from(r.channels_total).unwrap_or(0),
    }
}

/// `GET /v1/me/ssh-sessions`: the caller's open sessions.
pub async fn list_mine(s: &AppState, c: &Caller) -> Result<SshSessionList, ApiError> {
    let rows = repo::list_open_for_user(&s.pool, c.user_id).await?;
    Ok(SshSessionList {
        sessions: rows.into_iter().map(|r| to_dto(r, false)).collect(),
    })
}

/// `GET /v1/agents/{agent}/ssh-sessions`: `can_own` sees all (with `user`), `can_use` only their
/// own, anyone else gets 404.
pub async fn list_for_agent(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
) -> Result<SshSessionList, ApiError> {
    let a = agents::get(&s.pool, agent)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let w = c.workspace(s, a.workspace_id).await?;
    let owner = w.can_own();
    let rows = repo::list_open_for_agent(&s.pool, agent, (!owner).then_some(c.user_id)).await?;
    Ok(SshSessionList {
        sessions: rows.into_iter().map(|r| to_dto(r, owner)).collect(),
    })
}

/// `GET /v1/admin/terminators` (C-793).
pub async fn admin_terminators(s: &AppState) -> Result<super::dto::TerminatorList, ApiError> {
    let rows = repo::list_replicas(&s.pool, lease_secs()).await?;
    Ok(super::dto::TerminatorList {
        terminators: rows
            .into_iter()
            .map(|r| super::dto::TerminatorInfo {
                id: r.id,
                version: r.version,
                host_key_fingerprint: r.host_key_fingerprint,
                sessions_open: u32::try_from(r.sessions_open).unwrap_or(0),
                last_seen_at: r.last_seen_at,
                live: r.live,
            })
            .collect(),
    })
}

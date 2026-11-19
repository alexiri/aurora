//! SSH keys, the certificate mint and connection facts (P4-SSH-1, D-239, C-790..C-792).
//! Access is `can_ssh` (`aurora-db::authz`), the web terminal's rule; the mint additionally needs
//! a registered key and a free slot in the exact per-(person, agent, hour) ledger.
use super::dto::*;
use super::keys::{self, known_hosts_lines};
use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::http::auth::{Caller, KeyKind, Principal};
use aurora_core::ApiError;
use aurora_core::limits::{SSH_KEYS_PER_USER, SSH_MINTS_PER_USER_AGENT_HOUR};
use aurora_db::DbError;
use aurora_db::authz::{self, SshAccess};
use aurora_db::repo::{agents, ssh as repo};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;
use ssh_key::certificate::{Builder, CertType};
use uuid::Uuid;

/// 404 `ssh_disabled`: SSH access is off on this instance.
pub(crate) fn ssh_disabled() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "ssh_disabled",
        "SSH access is not enabled on this instance.",
    )
}

fn key_404() -> ApiError {
    ApiError::not_found("SSH key not found.")
}

/// Seconds a certificate is backdated (clock skew between the API and the terminators).
const BACKDATE_SECS: i64 = 60;

// ---- keys (C-790) -------------------------------------------------------------------------

pub async fn list_keys(s: &AppState, c: &Caller) -> Result<SshKeyList, ApiError> {
    let rows = repo::list_keys(&s.pool, c.user_id).await?;
    Ok(SshKeyList {
        keys: rows.into_iter().map(Into::into).collect(),
    })
}

pub async fn create_key(
    s: &AppState,
    c: &Caller,
    req: CreateSshKeyRequest,
) -> Result<SshKey, ApiError> {
    let parsed = keys::parse_public_key(&req.public_key)?;
    let name = keys::key_name(req.name.as_deref(), &parsed.comment)?;
    let mut tx = audit::begin(s).await?;
    // One critical section per person: the cap check and the insert.
    let n = repo::lock_and_count_keys(&mut tx, c.user_id).await?;
    if n >= SSH_KEYS_PER_USER as i64 {
        return Err(ApiError::conflict(
            "ssh_key_limit",
            format!("You can register at most {SSH_KEYS_PER_USER} SSH keys. Delete one first."),
        ));
    }
    if repo::find_key_by_fingerprint(&mut *tx, c.user_id, &parsed.fingerprint)
        .await?
        .is_some()
    {
        return Err(exists());
    }
    let id = Uuid::now_v7();
    let row = match repo::insert_key(
        &mut *tx,
        id,
        c.user_id,
        &name,
        &parsed.canonical,
        &parsed.fingerprint,
    )
    .await
    {
        Ok(r) => r,
        Err(DbError::UniqueViolation { .. }) => return Err(exists()),
        Err(e) => return Err(e.into()),
    };
    // Name and fingerprint only: never the key line (docs/04 §Audit).
    let ev = AuditEvent::by(c, "ssh_key.create")
        .target("ssh_key", row.id)
        .metadata(json!({"name": row.name, "fingerprint": row.fingerprint}));
    audit::commit_critical(tx, &ev).await?;
    Ok(row.into())
}

fn exists() -> ApiError {
    ApiError::conflict("ssh_key_exists", "You have already registered this key.")
}

pub async fn rename_key(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    name: &str,
) -> Result<SshKey, ApiError> {
    let clean = aurora_core::names::clean(name, 80)
        .map_err(|e| ApiError::invalid_field("Invalid request.", "/name", e.to_string()))?;
    repo::rename_key(&s.pool, c.user_id, id, &clean)
        .await?
        .map(Into::into)
        .ok_or_else(key_404)
}

pub async fn delete_key(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let mut tx = audit::begin(s).await?;
    let row = repo::delete_key(&mut *tx, c.user_id, id)
        .await?
        .ok_or_else(key_404)?;
    let ev = AuditEvent::by(c, "ssh_key.delete")
        .target("ssh_key", row.id)
        .metadata(json!({"name": row.name, "fingerprint": row.fingerprint}));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}

// ---- connection facts (C-792) --------------------------------------------------------------

/// The live terminator replicas (`last_seen_at` within `SSH_SESSION_LEASE`) and their distinct
/// host keys (P4-SSH-2, C-792).
pub(crate) async fn terminator_facts(s: &AppState) -> Result<(u32, Vec<HostKey>), ApiError> {
    let (live, keys) = aurora_db::repo::ssh_sessions::live_host_keys(
        &s.pool,
        aurora_core::limits::SSH_SESSION_LEASE.as_secs() as i64,
    )
    .await?;
    Ok((
        u32::try_from(live).unwrap_or(u32::MAX),
        keys.into_iter()
            .map(|(public_key, fingerprint)| HostKey {
                public_key,
                fingerprint,
            })
            .collect(),
    ))
}

/// `known_hosts` lines for the instance's public address and the live host keys.
pub(crate) fn known_hosts(s: &AppState, host_keys: &[HostKey]) -> Vec<String> {
    let keys: Vec<String> = host_keys.iter().map(|k| k.public_key.clone()).collect();
    known_hosts_lines(&s.config.ssh.public_host, s.config.ssh.public_port, &keys)
}

pub async fn facts(s: &AppState) -> Result<SshFacts, ApiError> {
    let (live, host_keys) = terminator_facts(s).await?;
    Ok(SshFacts {
        host: s.config.ssh.public_host.clone(),
        port: s.config.ssh.public_port,
        cert_ttl_secs: s.config.ssh.cert_ttl_secs,
        terminators_live: live,
        known_hosts: known_hosts(s, &host_keys),
        host_keys,
    })
}

// ---- certificate mint (C-791) --------------------------------------------------------------

/// Map the access predicate's refusals to the API's answers (the terminal's, `04 §SSH access`).
pub(crate) fn access_error(a: SshAccess) -> Option<ApiError> {
    match a {
        SshAccess::Allowed { .. } => None,
        SshAccess::Disabled => Some(ssh_disabled()),
        SshAccess::Denied => Some(ApiError::not_found("Agent not found.")),
        SshAccess::NotHosted => Some(ApiError::unprocessable(
            "ssh_not_supported",
            "SSH access exists only for hosted agents.",
        )),
        SshAccess::Frozen(level) => Some(crate::domains::freeze::service::frozen_error(level)),
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], ApiError> {
    let mut b = [0u8; N];
    aws_lc_rs::rand::fill(&mut b)
        .map_err(|_| ApiError::internal("The system random generator failed."))?;
    Ok(b)
}

fn to_utc(secs: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(i64::try_from(secs).unwrap_or(i64::MAX), 0).unwrap_or_default()
}

pub async fn mint_certificate(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    req: MintCertificateRequest,
) -> Result<SshCertificate, ApiError> {
    // Service keys and agent credentials never mint (C-791); the extractor already refused
    // agent tokens.
    if matches!(
        c.principal,
        Principal::ApiKey {
            kind: KeyKind::Service,
            ..
        } | Principal::Channel { .. }
    ) {
        return Err(ApiError::forbidden(
            "Certificates are minted for a person, by a session or the person's own API key.",
        ));
    }
    let signer = s.ssh_signer.as_ref().ok_or_else(ssh_disabled)?;
    let parsed = keys::parse_public_key(&req.public_key)?;
    // Key confinement and the access law (unknown or invisible agent: 404).
    let a = agents::get(&s.pool, agent)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    c.workspace(s, a.workspace_id).await?;

    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let access = authz::can_ssh(&mut tx, s.config.ssh.enabled, c.user_id, agent).await?;
    if let Some(e) = access_error(access) {
        return Err(e);
    }
    let SshAccess::Allowed {
        workspace_id,
        org_id,
    } = access
    else {
        return Err(ApiError::internal("unreachable"));
    };
    let key = repo::find_key_by_fingerprint(&mut *tx, c.user_id, &parsed.fingerprint)
        .await?
        .ok_or_else(|| {
            ApiError::unprocessable(
                "ssh_key_not_registered",
                "Register this key under Account › SSH keys first.",
            )
        })?;
    // The exact ledger, in this transaction: no row back means the hour's limit is used up.
    if repo::bump_mint_counter(
        &mut *tx,
        c.user_id,
        agent,
        SSH_MINTS_PER_USER_AGENT_HOUR as i32,
    )
    .await?
    .is_none()
    {
        let retry = repo::seconds_to_hour_end(&mut *tx).await?;
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "ssh_mint_rate_limited",
            "Too many SSH certificates for this agent this hour. Try again later.",
        )
        .with_retry_after(retry.max(1) as u64));
    }

    let now = Utc::now().timestamp();
    let valid_after = (now - BACKDATE_SECS).max(0) as u64;
    let valid_before = (now + s.config.ssh.cert_ttl_secs as i64).max(0) as u64;
    let serial = loop {
        let n = u64::from_be_bytes(random_bytes::<8>()?);
        if n != 0 {
            break n;
        }
    };
    let key_id = format!(
        "aurora:u={}:k={}:a={}:w={}",
        c.user_id, key.id, agent, workspace_id
    );
    let subject = ssh_key::PublicKey::from_openssh(&key.public_key)
        .map_err(|_| ApiError::internal("A stored SSH key does not parse."))?;
    let built = (|| -> Result<Builder, ssh_key::Error> {
        let mut b = Builder::new(
            random_bytes::<32>().map_err(|_| ssh_key::Error::CertificateValidation)?,
            subject.key_data().clone(),
            valid_after,
            valid_before,
        )?;
        b.serial(serial)?;
        b.cert_type(CertType::User)?;
        b.key_id(key_id.clone())?;
        b.valid_principal(agent.to_string())?;
        // No critical options; `permit-pty` is the only extension (a PTY, no forwarding).
        b.extension("permit-pty", "")?;
        b.comment(format!("aurora:{agent}"))?;
        Ok(b)
    })()
    .map_err(|_| ApiError::internal("The SSH certificate could not be built."))?;
    let cert = signer.sign(built).await?;
    let line = cert
        .to_openssh()
        .map_err(|_| ApiError::internal("The SSH certificate could not be encoded."))?;

    repo::touch_key(&mut *tx, key.id).await?;
    tx.commit().await.map_err(DbError::from)?;

    let valid_before_dt = to_utc(cert.valid_before());
    audit::record(
        s,
        AuditEvent::by(c, "agent.ssh_certificate.mint")
            .org(org_id)
            .workspace(workspace_id)
            .target("agent", agent)
            .metadata(json!({
                "agent": agent,
                "workspace": workspace_id,
                "fingerprint": key.fingerprint,
                "serial": cert.serial().to_string(),
                "valid_before": valid_before_dt,
            })),
    );

    let (_, host_keys) = terminator_facts(s).await?;
    Ok(SshCertificate {
        certificate: line.trim().to_owned(),
        serial: cert.serial().to_string(),
        principal: agent.to_string(),
        key_id,
        valid_after: to_utc(cert.valid_after()),
        valid_before: valid_before_dt,
        username: agent.to_string(),
        host: s.config.ssh.public_host.clone(),
        port: s.config.ssh.public_port,
        known_hosts: known_hosts(s, &host_keys),
    })
}

use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::Caller;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::agents::{self, AgentPatch, AgentRow, NewAgent};
use aurora_db::repo::credentials;
use aurora_db::{DbError, conflict_code};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const CA_PATH: &str = "/aurora/ca.pem";
const EFFORTS: [&str; 4] = ["low", "medium", "high", "max"];
const PROACTIVE: [&str; 3] = ["ask", "allow", "off"];

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid agent.", vec![FieldError::new(path, msg)])
}

fn name_ok(n: &str) -> Result<String, ApiError> {
    let t = n.trim();
    if !(1..=80).contains(&t.chars().count()) || t.chars().any(char::is_control) {
        return Err(bad(
            "/name",
            "must be 1-80 characters without control characters",
        ));
    }
    Ok(t.to_owned())
}

fn brief_ok(b: &str) -> Result<(), ApiError> {
    if b.chars().count() > 20000 {
        return Err(bad("/brief", "must be at most 20000 characters"));
    }
    Ok(())
}

fn effort_ok(e: &str) -> Result<(), ApiError> {
    if EFFORTS.contains(&e) {
        Ok(())
    } else {
        Err(bad("/effort", "must be low, medium, high or max"))
    }
}

fn proactive_ok(e: &str) -> Result<(), ApiError> {
    if PROACTIVE.contains(&e) {
        Ok(())
    } else {
        Err(bad("/proactive_messaging", "must be ask, allow or off"))
    }
}

pub fn valid_handle(h: &str) -> bool {
    let b = h.as_bytes();
    (1..=40).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Slugify a name into a valid handle base.
pub fn derive_handle(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_matches('-').to_owned();
    if !out.starts_with(|c: char| c.is_ascii_lowercase()) {
        out = format!("agent-{out}");
    }
    out.truncate(36);
    out.trim_end_matches('-').to_owned()
}

pub fn proxy_url_with(state: &AppState, token: &str) -> String {
    let base = &state.config.advertised_proxy_url;
    match base.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://x:{token}@{rest}"),
        None => format!("x:{token}@{base}"),
    }
}

pub async fn read_ca(state: &AppState) -> Result<String, ApiError> {
    let path = std::path::Path::new(&state.config.gateway_ca_dir).join("ca.pem");
    tokio::fs::read_to_string(path).await.map_err(|e| {
        tracing::warn!(error = %e, "gateway CA unreadable");
        ApiError::unavailable("gateway_ca_unavailable", "The gateway CA is not available.")
    })
}

/// Bootstrap document; `token` embeds the agent token, otherwise `<AGENT_TOKEN>`. For an agent
/// that requires context credentials (RFE-025) the proxy URL carries `<CONTEXT_TOKEN>` instead:
/// the agent token is only for minting `auc_` credentials (`aurora run` does that).
pub async fn container_config(
    state: &AppState,
    token: Option<&str>,
    byo: Option<&aurora_llm::governance::EffectiveByo>,
) -> Result<ContainerConfig, ApiError> {
    let ca_pem = read_ca(state).await?;
    let need_context = byo.is_some_and(|b| b.require_context);
    let proxy_url = proxy_url_with(
        state,
        if need_context {
            "<CONTEXT_TOKEN>"
        } else {
            token.unwrap_or("<AGENT_TOKEN>")
        },
    );
    let mut env = BTreeMap::new();
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        env.insert(k.to_owned(), proxy_url.clone());
    }
    env.insert("NO_PROXY".into(), "localhost,127.0.0.1".into());
    let operator_cert = state.config.gateway_proxy_operator_cert;
    if !operator_cert {
        // (phase 1, RFE-002) git trusts the HTTPS proxy leg separately from the origin leg.
        env.insert("GIT_CONFIG_COUNT".into(), "1".into());
        env.insert("GIT_CONFIG_KEY_0".into(), "http.proxySSLCAInfo".into());
        env.insert("GIT_CONFIG_VALUE_0".into(), CA_PATH.to_owned());
    }
    for k in [
        "SSL_CERT_FILE",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "GIT_SSL_CAINFO",
    ] {
        env.insert(k.to_owned(), CA_PATH.to_owned());
    }
    Ok(ContainerConfig {
        proxy_url,
        env,
        ca_pem,
        ca_path: CA_PATH.into(),
        proxy_ca_path: CA_PATH.into(),
        proxy_tls: operator_cert.then(|| "operator_certificate".to_owned()),
        // Placeholders only; filled once model selection exists.
        llm_env: BTreeMap::new(),
        // Credential stubs arrive with M3 apps (08 §9).
        stubs: Vec::new(),
        byo: byo.map(|b| ContainerByo {
            posture: b.posture.as_str().to_owned(),
            source_cidrs: b.source_strings(),
            require_context: b.require_context,
        }),
        notes: if need_context {
            vec![
                "This agent must use a context credential for requests that need a credential: replace <CONTEXT_TOKEN> in proxy_url with one minted by POST /v1/agent/context-credentials, or run commands through `aurora run -- <command>`, which does that for you.".to_owned(),
            ]
        } else {
            Vec::new()
        },
    })
}

pub struct Created {
    pub agent: AgentRow,
    pub token: Option<String>,
    /// (RFE-013) `expires_at` of the minted token (`None` = no maximum age applies).
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn create(
    state: &AppState,
    c: &Caller,
    auth: &aurora_db::authz::WorkspaceAuth,
    org_id: Uuid,
    ws: Uuid,
    req: CreateAgentRequest,
) -> Result<Created, ApiError> {
    let name = name_ok(&req.name)?;
    if req.kind != "byo" && req.kind != "hosted" {
        return Err(bad("/kind", "must be byo or hosted"));
    }
    if req.kind == "hosted" && !state.config.hosted.enabled {
        return Err(ApiError::unprocessable(
            "hosted_disabled",
            "Hosted agents are disabled on this instance.",
        ));
    }
    if req.kind == "byo" {
        // (phase 2, RFE-025) creation is governed by the effective BYO posture.
        let org = super::byo::org_byo(state, org_id).await?;
        let wsrow = aurora_db::repo::workspaces::get(&state.pool, ws)
            .await?
            .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
        let eff = super::byo::effective(state, &org, wsrow.byo_posture.as_deref(), None);
        super::byo::require_creation(state, auth, ws, &eff).await?;
    }
    let brief = req.brief.as_deref().unwrap_or("");
    brief_ok(brief)?;
    if let Some(e) = &req.effort {
        effort_ok(e)?;
    }
    let proactive = req.proactive_messaging.as_deref().unwrap_or("ask");
    proactive_ok(proactive)?;
    let explicit = match &req.handle {
        Some(h) if !valid_handle(h) => {
            return Err(bad("/handle", "must match [a-z][a-z0-9-]{0,39}"));
        }
        Some(h) => Some(h.clone()),
        None => None,
    };
    let harness = if req.kind == "hosted" {
        Some(
            req.harness
                .clone()
                .unwrap_or_else(|| state.config.hosted.default_harness.clone()),
        )
    } else {
        req.harness.clone()
    };

    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let (max, count) = agents::lock_org_agent_limit(&mut tx, org_id).await?;
    if max.is_some_and(|m| count >= m) {
        return Err(ApiError::unprocessable(
            "agent_limit_reached",
            "The organization's agent limit has been reached.",
        ));
    }
    let handle = match explicit {
        Some(h) => h,
        None => {
            let base = derive_handle(&name);
            let mut cand = base.clone();
            let mut n = 2;
            while agents::handle_exists(&mut *tx, ws, &cand).await? {
                let suffix = format!("-{n}");
                let keep = base.len().min(40 - suffix.len());
                cand = format!("{}{suffix}", &base[..keep]);
                n += 1;
            }
            cand
        }
    };
    let row = agents::insert(
        &mut *tx,
        &NewAgent {
            id: Uuid::now_v7(),
            org_id,
            workspace_id: ws,
            handle: &handle,
            name: &name,
            kind: &req.kind,
            harness: harness.as_deref(),
            model_endpoint_id: req.model_endpoint_id.as_deref(),
            model_id: req.model_id.as_deref(),
            effort: req.effort.as_deref(),
            proactive_messaging: proactive,
            brief,
            created_by: c.user_id,
        },
    )
    .await
    .map_err(|e| conflict_code(e, &[(agents::HANDLE_CONSTRAINT, "handle_taken")]))?;
    let (tok, expires_at) = if row.kind == "byo" {
        let max = super::byo::token_max_age_days(state, &mut tx, org_id).await?;
        let (t, exp) = mint_agent_token(&mut tx, row.id, max).await?;
        (Some(t), exp)
    } else {
        (None, None)
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(Created {
        agent: row,
        token: tok,
        expires_at,
    })
}

/// Mints the agent's `aug_` token with `expires_at = now() + max age` (C-311).
async fn mint_agent_token(
    tx: &mut sqlx::PgConnection,
    agent_id: Uuid,
    max_age_days: Option<u32>,
) -> Result<(String, Option<chrono::DateTime<chrono::Utc>>), ApiError> {
    let t = token::generate(TokenPrefix::Gateway);
    let t = t.expose();
    let (_, exp) = credentials::insert_agent_credential(
        &mut *tx,
        agent_id,
        &token::hash(t),
        &token::display_prefix(t),
        max_age_days,
    )
    .await?;
    Ok((t.clone(), exp))
}

/// Revoke the previous agent + context credentials and mint a new token. The audit row
/// `agent.token_regenerate` is written in the same transaction (RFE-025): no audit, no new token.
pub async fn regenerate(
    state: &AppState,
    caller: &Caller,
    row: &AgentRow,
) -> Result<(String, Option<chrono::DateTime<chrono::Utc>>), ApiError> {
    let agent_id = row.id;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let max = super::byo::token_max_age_days(state, &mut tx, row.org_id).await?;
    credentials::revoke_for_agent(&mut *tx, agent_id, false).await?;
    let (t, expires_at) = mint_agent_token(&mut tx, agent_id, max).await?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(caller),
        "agent.token_regenerate",
        Outcome::Success,
    )
    .org(row.org_id)
    .workspace(row.workspace_id)
    .target("agent", agent_id);
    audit::insert_tx(&mut tx, &ev).await.map_err(|e| {
        tracing::error!(error = %e, "agent.token_regenerate audit write failed");
        ApiError::unavailable("audit_unavailable", "The audit log is unavailable.")
    })?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok((t, expires_at))
}

/// `PUT /v1/agents/{agent}/byo`: the agent's own source list and require-context flag, with the
/// audit row in the same transaction.
pub async fn put_byo(
    state: &AppState,
    caller: &Caller,
    row: &AgentRow,
    req: &super::byo::PutByoRequest,
) -> Result<AgentRow, ApiError> {
    if row.kind != "byo" {
        return Err(ApiError::unprocessable(
            "not_byo",
            "Source lists and context-credential requirements apply to BYO agents only.",
        ));
    }
    let org = super::byo::org_byo(state, row.org_id).await?;
    let list = match &req.source_cidrs {
        None => None,
        Some(l) => {
            let inherited = super::byo::inherited_list(state, &org);
            Some(super::byo::validate_source_list(l, inherited.as_deref())?)
        }
    };
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let updated = agents::set_byo_settings(&mut *tx, row.id, list.as_deref(), req.require_context)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(caller),
        "agent.byo_settings.update",
        Outcome::Success,
    )
    .org(row.org_id)
    .workspace(row.workspace_id)
    .target("agent", row.id)
    .metadata(serde_json::json!({
        "source_cidrs": list,
        "previous_source_cidrs": row.byo_source_cidrs,
        "require_context": req.require_context,
        "previous_require_context": row.byo_require_context,
    }));
    audit::insert_tx(&mut tx, &ev).await.map_err(|e| {
        tracing::error!(error = %e, "agent.byo_settings.update audit write failed");
        ApiError::unavailable("audit_unavailable", "The audit log is unavailable.")
    })?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", row.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(updated)
}

pub async fn patch(
    state: &AppState,
    c: &Caller,
    id: Uuid,
    req: &PatchAgentRequest,
) -> Result<AgentRow, ApiError> {
    let name = req.name.as_deref().map(name_ok).transpose()?;
    if let Some(b) = &req.brief {
        brief_ok(b)?;
    }
    if let Some(e) = &req.effort {
        effort_ok(e)?;
    }
    if let Some(p) = &req.proactive_messaging {
        proactive_ok(p)?;
    }
    if req.resource_overrides.is_some() && !c.is_instance_admin {
        return Err(ApiError::forbidden(
            "Only instance administrators can set resource overrides.",
        ));
    }
    let updated = agents::update(
        &state.pool,
        id,
        &AgentPatch {
            name: name.as_deref(),
            brief: req.brief.as_deref(),
            harness: req.harness.as_deref(),
            model_endpoint_id: req.model_endpoint_id.as_deref(),
            model_id: req.model_id.as_deref(),
            effort: req.effort.as_deref(),
            proactive_messaging: req.proactive_messaging.as_deref(),
            resource_overrides: req.resource_overrides.as_ref(),
        },
    )
    .await?;
    if let Some(n) = &name {
        // Dedicated channel bots follow the agent's name (best effort: the patch succeeded).
        if let Err(e) = crate::domains::channels::presences::on_agent_renamed(state, id, n).await {
            tracing::warn!(error = %e.message(), agent_id = %id, "presence.rename not queued");
        }
    }
    Ok(updated)
}

pub async fn delete(state: &AppState, id: Uuid) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    credentials::revoke_for_agent(&mut *tx, id, true).await?;
    // Dedicated channel presences are deprovisioned before the cascade removes them.
    crate::domains::channels::presences::enqueue_deprovision_for_agent(&mut tx, id).await?;
    agents::delete(&mut *tx, id).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Sanitize a context label: strip control characters, trim, 1..=128 chars.
pub fn sanitize_label(l: &str) -> Result<String, ApiError> {
    let s: String = l.chars().filter(|c| !c.is_control()).collect();
    let s = s.trim().to_owned();
    if !(1..=128).contains(&s.chars().count()) {
        return Err(ApiError::invalid_request(
            "Invalid context credential.",
            vec![FieldError::new("/label", "must be 1-128 characters")],
        ));
    }
    Ok(s)
}

pub async fn mint_context(
    state: &AppState,
    agent_id: Uuid,
    req: ContextCredentialRequest,
) -> Result<ContextCredential, ApiError> {
    let label = sanitize_label(&req.label)?;
    let ttl = req.ttl_secs.unwrap_or(3600);
    if !(60..=86400).contains(&ttl) {
        return Err(ApiError::invalid_request(
            "Invalid context credential.",
            vec![FieldError::new("/ttl_secs", "must be between 60 and 86400")],
        ));
    }
    if let Some(conv) = req.conversation_id
        && !credentials::conversation_of_agent(&state.pool, conv, agent_id).await?
    {
        return Err(ApiError::not_found("Conversation not found."));
    }
    let t = token::generate(TokenPrefix::Context);
    let t = t.expose();
    let (_, exp) = credentials::insert_context_credential(
        &state.pool,
        agent_id,
        req.conversation_id,
        &token::hash(t),
        &token::display_prefix(t),
        &label,
        ttl,
    )
    .await?;
    Ok(ContextCredential {
        credential: t.clone(),
        expires_at: exp,
        proxy_url: proxy_url_with(state, t),
    })
}

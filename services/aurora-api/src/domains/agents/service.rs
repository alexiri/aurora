use super::dto::*;
use crate::app::AppState;
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

/// Bootstrap document; `token` embeds the agent token, otherwise `<AGENT_TOKEN>`.
pub async fn container_config(
    state: &AppState,
    token: Option<&str>,
) -> Result<ContainerConfig, ApiError> {
    let ca_pem = read_ca(state).await?;
    let proxy_url = proxy_url_with(state, token.unwrap_or("<AGENT_TOKEN>"));
    let mut env = BTreeMap::new();
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        env.insert(k.to_owned(), proxy_url.clone());
    }
    env.insert("NO_PROXY".into(), "localhost,127.0.0.1".into());
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
        // Placeholders only; filled once model selection exists.
        llm_env: BTreeMap::new(),
        // Credential stubs arrive with M3 apps (08 §9).
        stubs: Vec::new(),
    })
}

pub struct Created {
    pub agent: AgentRow,
    pub token: Option<String>,
}

pub async fn create(
    state: &AppState,
    c: &Caller,
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
    let tok = if row.kind == "byo" {
        Some(mint_agent_token(&mut tx, row.id).await?)
    } else {
        None
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(Created {
        agent: row,
        token: tok,
    })
}

async fn mint_agent_token(tx: &mut sqlx::PgConnection, agent_id: Uuid) -> Result<String, ApiError> {
    let t = token::generate(TokenPrefix::Gateway);
    let t = t.expose();
    credentials::insert_agent_credential(
        &mut *tx,
        agent_id,
        &token::hash(t),
        &token::display_prefix(t),
    )
    .await?;
    Ok(t.clone())
}

/// Revoke the previous agent + context credentials and mint a new token.
pub async fn regenerate(state: &AppState, agent_id: Uuid) -> Result<String, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    credentials::revoke_for_agent(&mut *tx, agent_id, false).await?;
    let t = mint_agent_token(&mut tx, agent_id).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(t)
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

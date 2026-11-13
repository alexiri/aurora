//! The SCIM management routes of `/v1` (docs/05 §SCIM management): settings, tokens and the
//! Directory sync status — org `owner` only, every write security-critical audited, PUT/POST
//! need a recent re-authentication when they change something (412 `reauth_required`).
//! Plain functions over [`super::protocol`]-style orchestration is not needed here: these
//! are normal `/v1` routes in the v1 router (session auth via `SessionCaller`), but they are
//! deliberately **not** utoipa paths (the SCIM management surface stays out of
//! `openapi.json` like the plane itself — the dashboard calls them by path).

use super::auth::OrgScim;
use super::settings;
use crate::app::AppState;
use crate::audit::{AuditActor, AuditEvent, Outcome};
use crate::domains::me::service::REAUTH_WINDOW_SECS;
use crate::http::auth::{Caller, SessionCaller};
use aurora_core::limits;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::repo::{audit as audit_repo, scim as repo, sessions};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

/// `DbError` → `ApiError` (the management routes return the `/v1` error envelope).
fn dberr(e: aurora_db::DbError) -> aurora_core::ApiError {
    e.into()
}

/// `sqlx::Error` → `ApiError` (ad-hoc queries that do not go through the repo).
fn sqlerr(e: sqlx::Error) -> aurora_core::ApiError {
    aurora_db::DbError::from(e).into()
}

fn owner_only(a: &crate::http::auth::OrgAuth) -> Result<(), aurora_core::ApiError> {
    a.require_owner()
}

fn reauth_required() -> aurora_core::ApiError {
    aurora_core::ApiError::new(
        StatusCode::PRECONDITION_FAILED,
        "reauth_required",
        "Re-authenticate with your sign-in provider to change SCIM settings.",
    )
}

fn scim_unavailable() -> aurora_core::ApiError {
    aurora_core::ApiError::conflict(
        "scim_unavailable",
        "SCIM is not enabled on this Aurora instance.",
    )
}

fn session_id(c: &Caller) -> Result<Uuid, aurora_core::ApiError> {
    match c.principal {
        crate::http::auth::Principal::Session { id } => Ok(id),
        _ => Err(aurora_core::ApiError::forbidden(
            "Sign in with your account to manage SCIM.",
        )),
    }
}

async fn recent_reauth(state: &AppState, session: Uuid) -> Result<(), aurora_core::ApiError> {
    let age = sessions::reauth_age_secs(&state.pool, session).await?;
    if !age.is_some_and(|a| (0.0..=REAUTH_WINDOW_SECS).contains(&a)) {
        return Err(reauth_required());
    }
    Ok(())
}

/// `GET /v1/orgs/{org}/scim` — `{ available, enabled, email_domains, exclusive, base_url }`.
pub async fn get_settings(
    State(state): State<AppState>,
    c: SessionCaller,
    Path(org): Path<Uuid>,
) -> Result<Json<serde_json::Value>, aurora_core::ApiError> {
    let a = c.org(&state, org).await?;
    owner_only(&a)?;
    let scim = settings::org_scim(&state.pool, org).await.map_err(dberr)?;
    Ok(Json(settings_payload(&state, &scim)))
}

fn settings_payload(state: &AppState, scim: &OrgScim) -> serde_json::Value {
    json!({
        "available": state.config.scim.enabled,
        "enabled": scim.enabled,
        "email_domains": scim.email_domains,
        "exclusive": scim.exclusive,
        "base_url": format!("{}/scim/v2", state.config.external_url.trim_end_matches('/')),
    })
}

/// The domains of a PUT body: sanitized, lower-cased, deduplicated, at most 20.
fn clean_domains(raw: &[String]) -> Result<Vec<String>, aurora_core::ApiError> {
    use aurora_core::FieldError;
    let mut out: Vec<String> = Vec::new();
    let mut fields = Vec::new();
    for d in raw {
        let d = d.trim().to_ascii_lowercase();
        if d.is_empty() {
            continue;
        }
        let bad = d.contains('@')
            || d.contains('*')
            || !d.contains('.')
            || d.starts_with('.')
            || d.ends_with('.')
            || d.len() > 253
            || !d
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-');
        if bad {
            fields.push(FieldError::new(
                "/email_domains",
                format!("`{d}` is not a domain (like `example.com`)"),
            ));
            continue;
        }
        if !out.contains(&d) {
            out.push(d);
        }
    }
    if out.len() > 20 {
        fields.push(FieldError::new("/email_domains", "at most 20 domains"));
    }
    if !fields.is_empty() {
        return Err(aurora_core::ApiError::invalid_request(
            "Invalid request.",
            fields,
        ));
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct PutSettings {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub email_domains: Option<Vec<String>>,
    #[serde(default)]
    pub exclusive: Option<bool>,
}

/// `PUT /v1/orgs/{org}/scim` — owner + recent re-auth; validates domains (422
/// `scim_domain_invalid` / `scim_domain_not_allowed` / `scim_domains_required` when
/// enabling); 409 `scim_unavailable` while the instance switch is off.
pub async fn put_settings(
    State(state): State<AppState>,
    c: SessionCaller,
    Path(org): Path<Uuid>,
    body: Json<PutSettings>,
) -> Result<Json<serde_json::Value>, aurora_core::ApiError> {
    let a = c.org(&state, org).await?;
    owner_only(&a)?;
    if !state.config.scim.enabled {
        return Err(scim_unavailable());
    }
    let session = session_id(&c)?;
    let p = body.0;
    let enabling = p.enabled == Some(true);
    if enabling || p.email_domains.is_some() || p.exclusive.is_some() {
        recent_reauth(&state, session).await?;
    }
    // Validate the domains against the shape rules and the instance allow-list.
    let domains = match &p.email_domains {
        None => None,
        Some(raw) => {
            let domains = clean_domains(raw)?;
            for d in &domains {
                if !OrgScim::instance_domain_allowed(
                    &state.config.scim.allowed_email_domains,
                    &format!("u@{d}"),
                ) {
                    return Err(aurora_core::ApiError::unprocessable(
                        "scim_domain_not_allowed",
                        format!("The domain `{d}` is not allowed on this Aurora instance."),
                    ));
                }
            }
            Some(domains)
        }
    };
    // Enabling requires at least one domain (04 §SCIM Enabling and tokens).
    let current = settings::org_scim(&state.pool, org).await.map_err(dberr)?;
    let will_enable = p.enabled.unwrap_or(current.enabled);
    let effective_domains = domains
        .clone()
        .unwrap_or_else(|| current.email_domains.clone());
    if will_enable && effective_domains.is_empty() {
        return Err(aurora_core::ApiError::unprocessable(
            "scim_domains_required",
            "At least one e-mail domain is required to enable SCIM.",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(sqlerr)?;
    let next = settings::update_scim_settings(&mut tx, org, p.enabled, domains, p.exclusive)
        .await
        .map_err(dberr)?;
    let ev = AuditEvent::new(AuditActor::from_caller(&c), "scim.update", Outcome::Success)
        .org(org)
        .target("organization", org)
        .metadata(json!({
            "enabled": p.enabled,
            "email_domains": p.email_domains.clone(),
            "exclusive": p.exclusive,
        }));
    crate::audit::commit_critical(tx, &ev).await?;
    let scim_value = next.get("scim").unwrap_or(&serde_json::Value::Null);
    let scim = OrgScim::from_settings(org, scim_value);
    Ok(Json(settings_payload(&state, &scim)))
}

/// `GET /v1/orgs/{org}/scim/tokens` — the org's tokens without material.
pub async fn list_tokens(
    State(state): State<AppState>,
    c: SessionCaller,
    Path(org): Path<Uuid>,
) -> Result<Json<serde_json::Value>, aurora_core::ApiError> {
    let a = c.org(&state, org).await?;
    owner_only(&a)?;
    let now = chrono::Utc::now();
    let rows = repo::tokens_of(&state.pool, org).await.map_err(dberr)?;
    let tokens: Vec<serde_json::Value> = rows
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "display_prefix": t.display_prefix,
                "created_at": t.created_at,
                "expires_at": t.expires_at,
                "last_used_at": t.last_used_at,
                "revoked": t.revoked_at.is_some(),
                "expiring": t.is_live(now) && t.expires_at <= now + chrono::Duration::days(14),
            })
        })
        .collect();
    Ok(Json(json!({ "tokens": tokens })))
}

#[derive(Deserialize)]
pub struct CreateToken {
    pub name: String,
    #[serde(default)]
    pub expires_in_days: Option<u32>,
}

/// `POST /v1/orgs/{org}/scim/tokens` — the `aud_` token **once**; 409 `scim_unavailable`
/// (instance off) / 422 `scim_token_limit_reached` (org off, limit); 412 `reauth_required`.
pub async fn create_token(
    State(state): State<AppState>,
    c: SessionCaller,
    Path(org): Path<Uuid>,
    body: Json<CreateToken>,
) -> Result<Response, aurora_core::ApiError> {
    let a = c.org(&state, org).await?;
    owner_only(&a)?;
    if !state.config.scim.enabled {
        return Err(scim_unavailable());
    }
    let scim = settings::org_scim(&state.pool, org).await.map_err(dberr)?;
    if !scim.enabled {
        return Err(aurora_core::ApiError::unprocessable(
            "scim_not_enabled",
            "Enable SCIM for the organization before creating a token.",
        ));
    }
    let session = session_id(&c)?;
    recent_reauth(&state, session).await?;
    let p = body.0;
    let name = aurora_core::sanitize::one_line(&p.name, 80);
    if name.is_empty() {
        return Err(aurora_core::ApiError::invalid_request(
            "Invalid request.",
            vec![aurora_core::FieldError::new("/name", "must not be empty")],
        ));
    }
    let days = p
        .expires_in_days
        .unwrap_or(limits::SCIM_TOKEN_DEFAULT_DAYS)
        .min(state.config.scim.token_max_days)
        .max(1);
    let mut tx = state.pool.begin().await.map_err(sqlerr)?;
    // Serialize the count + insert per org (the `organizations` row lock the codebase uses
    // for per-org caps): concurrent creates cannot both see 4 and exceed the cap (FIX-062).
    aurora_db::repo::groups::lock_org(&mut tx, org)
        .await
        .map_err(dberr)?;
    let live = repo::live_token_count(&mut *tx, org, chrono::Utc::now())
        .await
        .map_err(dberr)?;
    if live >= limits::SCIM_TOKENS_PER_ORG as i64 {
        return Err(aurora_core::ApiError::unprocessable(
            "scim_token_limit_reached",
            format!(
                "At most {} active SCIM tokens per organization.",
                limits::SCIM_TOKENS_PER_ORG
            ),
        ));
    }
    let tok = token::generate(TokenPrefix::Scim).expose().clone();
    let row = repo::insert_token(
        &mut *tx,
        &repo::NewToken {
            id: aurora_core::ids::new_id(),
            org_id: org,
            name: &name,
            token_hash: &token::hash(&tok),
            display_prefix: &token::display_prefix(&tok),
            created_by: c.user_id,
            expires_at: chrono::Utc::now() + chrono::Duration::days(i64::from(days)),
        },
    )
    .await
    .map_err(dberr)?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(&c),
        "scim_token.create",
        Outcome::Success,
    )
    .org(org)
    .target("scim_token", row.id)
    .metadata(json!({"name": name, "expires_in_days": days}));
    crate::audit::commit_critical(tx, &ev).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": row.id,
            "name": row.name,
            "display_prefix": row.display_prefix,
            "created_at": row.created_at,
            "expires_at": row.expires_at,
            // Shown once; never stored in cleartext.
            "token": tok,
        })),
    )
        .into_response())
}

/// `DELETE /v1/scim/tokens/{id}` — idempotent, effective at once; critical audit.
pub async fn revoke_token(
    State(state): State<AppState>,
    c: SessionCaller,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, aurora_core::ApiError> {
    // The token's org decides the authorization: resolve by id, then owner-only (an unknown
    // id is 204 — the revocation is idempotent).
    let Some((org,)) = sqlx::query_as::<_, (Uuid,)>("SELECT org_id FROM scim_tokens WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(sqlerr)?
    else {
        return Ok(StatusCode::NO_CONTENT);
    };
    let a = c.org(&state, org).await?;
    owner_only(&a)?;
    let mut tx = state.pool.begin().await.map_err(sqlerr)?;
    let revoked = repo::revoke_token(&mut tx, org, id).await.map_err(dberr)?;
    if revoked {
        let ev = AuditEvent::new(
            AuditActor::from_caller(&c),
            "scim_token.revoke",
            Outcome::Success,
        )
        .org(org)
        .target("scim_token", id)
        .metadata(json!({}));
        crate::audit::commit_critical(tx, &ev).await?;
    } else {
        tx.commit().await.map_err(sqlerr)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/orgs/{org}/scim/status` — the Directory sync page payload: last request, counts
/// by state, group count, and the last `SCIM_ERROR_LOG` sanitized errors.
pub async fn status(
    State(state): State<AppState>,
    c: SessionCaller,
    Path(org): Path<Uuid>,
) -> Result<Json<serde_json::Value>, aurora_core::ApiError> {
    let a = c.org(&state, org).await?;
    owner_only(&a)?;
    let scim = settings::org_scim(&state.pool, org).await.map_err(dberr)?;
    let (active, suspended, removed) = repo::user_counts(&state.pool, org).await.map_err(dberr)?;
    let groups: i64 =
        sqlx::query_scalar("SELECT count(*) FROM groups WHERE org_id = $1 AND source = 'scim'")
            .bind(org)
            .fetch_one(&state.pool)
            .await
            .map_err(sqlerr)?;
    // The token's `last_used_at` maximum is the "last request" proxy: the churn column is
    // written at most once a minute per token.
    let last: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT max(last_used_at) FROM scim_tokens WHERE org_id = $1")
            .bind(org)
            .fetch_one(&state.pool)
            .await
            .map_err(sqlerr)?;
    // The last sanitized errors: 4xx SCIM responses of this org from the request log is not
    // available per org, so the errors are the audit rows of SCIM failures (best-effort
    // actions) with `metadata.status`. Kept to the newest `SCIM_ERROR_LOG`.
    type ErrorRow = (
        chrono::DateTime<chrono::Utc>,
        String,
        String,
        i64,
        Option<String>,
    );
    let errors: Vec<ErrorRow> = sqlx::query_as(
        "SELECT created_at, COALESCE(metadata ->> 'method', ''), \
                    COALESCE(metadata ->> 'resource', ''), \
                    COALESCE((metadata ->> 'status')::int, 0), \
                    metadata ->> 'scim_type' \
             FROM audit_log WHERE org_id = $1 AND action = 'scim.error' \
             ORDER BY created_at DESC LIMIT $2",
    )
    .bind(org)
    .bind(i64::try_from(limits::SCIM_ERROR_LOG).unwrap_or(i64::MAX))
    .fetch_all(&state.pool)
    .await
    .map_err(sqlerr)?;
    let errors: Vec<serde_json::Value> = errors
        .into_iter()
        .map(|(at, method, resource, st, ty)| {
            json!({"at": at, "method": method, "resource": resource, "status": st, "scim_type": ty})
        })
        .collect();
    Ok(Json(json!({
        "enabled": scim.enabled,
        "last_request_at": last,
        "users": {"active": active, "suspended": suspended, "removed": removed},
        "groups": groups,
        "errors": errors,
    })))
}

/// Best-effort `scim.error` audit rows: sanitized, never a token or another user's e-mail.
pub async fn record_scim_error(
    state: &AppState,
    org: Uuid,
    method: &str,
    resource: &str,
    status: i64,
    scim_type: Option<&str>,
) {
    let a = audit_repo::NewAudit {
        org_id: Some(org),
        workspace_id: None,
        actor_kind: "service",
        actor_id: None,
        actor_label: "scim",
        action: "scim.error",
        target_kind: None,
        target_id: None,
        outcome: "failure",
        source: "api",
        ip: None,
        metadata: &json!({
            "method": method,
            "resource": resource,
            "status": status,
            "scim_type": scim_type,
        }),
    };
    if let Err(e) = audit_repo::insert(&state.pool, &a).await {
        tracing::debug!(error = %e, "scim: error-log row failed");
    }
}

/// The v1 management router (merged into the v1 router in `app.rs`; **not** utoipa).
pub fn router() -> axum::Router<AppState> {
    use axum::routing::{delete, get};
    axum::Router::new()
        .route("/v1/orgs/{org}/scim", get(get_settings).put(put_settings))
        .route("/v1/orgs/{org}/scim/status", get(status))
        .route(
            "/v1/orgs/{org}/scim/tokens",
            get(list_tokens).post(create_token),
        )
        .route("/v1/scim/tokens/{id}", delete(revoke_token))
}

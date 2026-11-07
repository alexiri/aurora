use super::dto::CreateKeyRequest;
use crate::app::AppState;
use crate::http::auth::{Caller, Principal};
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::api_keys::{self, ApiKeyRow, NewKey};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Validated `(name, expires_at)`.
pub fn validate(req: CreateKeyRequest) -> Result<(String, Option<DateTime<Utc>>), ApiError> {
    let name = req.name.trim().to_owned();
    let mut errs = Vec::new();
    let n = name.chars().count();
    if !(1..=80).contains(&n) || name.chars().any(char::is_control) {
        errs.push(FieldError::new(
            "/name",
            "must be 1-80 characters without control characters",
        ));
    }
    if req.expires_at.is_some_and(|t| t <= Utc::now()) {
        errs.push(FieldError::new("/expires_at", "must be in the future"));
    }
    if errs.is_empty() {
        Ok((name, req.expires_at))
    } else {
        Err(ApiError::invalid_request("Invalid API key.", errs))
    }
}

/// Create a `user` key for `c` (org key when `workspace` is `None`). Returns the row and token.
pub async fn create(
    s: &AppState,
    c: &Caller,
    org_id: Uuid,
    workspace: Option<Uuid>,
    req: CreateKeyRequest,
) -> Result<(ApiKeyRow, String), ApiError> {
    let (name, expires_at) = validate(req)?;
    let prefix = if workspace.is_some() {
        TokenPrefix::WorkspaceKey
    } else {
        TokenPrefix::OrgKey
    };
    let tok = token::generate(prefix);
    let tok = tok.expose();
    let row = api_keys::insert(
        &s.pool,
        &NewKey {
            id: Uuid::now_v7(),
            org_id,
            workspace_id: workspace,
            user_id: c.user_id,
            name: &name,
            token_hash: &token::hash(tok),
            display_prefix: &token::display_prefix(tok),
            expires_at,
        },
    )
    .await?;
    Ok((row, tok.clone()))
}

/// Revocation authorization: `Ok(())` if allowed; 404 invisible / 403 visible.
pub async fn authorize_revoke(s: &AppState, c: &Caller, k: &ApiKeyRow) -> Result<(), ApiError> {
    let nf = || ApiError::not_found("API key not found.");
    // Key callers stay inside their own confinement.
    if let Principal::ApiKey {
        org_id,
        workspace_id,
        ..
    } = &c.principal
    {
        if *org_id != k.org_id {
            return Err(nf());
        }
        if workspace_id.is_some() && *workspace_id != k.workspace_id {
            return Err(nf());
        }
    }
    if k.user_id == c.user_id {
        return Ok(());
    }
    match k.workspace_id {
        Some(ws) => {
            let a = c.workspace(s, ws).await.map_err(|_| nf())?;
            crate::http::auth::require_own(&a)
        }
        None => {
            let a = c.org(s, k.org_id).await.map_err(|_| nf())?;
            a.require_admin()
        }
    }
}

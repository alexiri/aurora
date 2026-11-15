//! Connector-host tokens (FIX-120, D-189, C-690..C-693): one `auh_` token per host, bound to the
//! connector types that host runs. Instance administrators issue (the token is shown once), list
//! and revoke them; every change is a security-critical audit row (SEC-69).

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::DbError;
use aurora_db::repo::connector_hosts::{self, TokenRow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

const NAME_MAX: usize = 80;
const TYPES_MAX: usize = 64;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueHostTokenRequest {
    /// 1–80 characters, unique among live tokens (for example the Deployment's name).
    pub name: String,
    /// The connector types this host may register, poll and report for (1–64).
    pub type_ids: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct HostTokenInfo {
    pub id: Uuid,
    /// `null` for the shared bootstrap token.
    pub name: Option<String>,
    /// First characters of the token (never enough to use it).
    pub prefix: Option<String>,
    /// `null` for the shared bootstrap token, which is not bound.
    pub type_ids: Option<Vec<String>>,
    /// `active` | `revoked` | `refused` (the shared bootstrap token once a bound token exists).
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct HostTokenList {
    pub items: Vec<HostTokenInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct HostTokenIssued {
    #[serde(flatten)]
    pub info: HostTokenInfo,
    /// `auh_…`, shown once.
    pub token: String,
}

fn info(r: TokenRow, bound_exist: bool) -> HostTokenInfo {
    let state = if r.revoked_at.is_some() {
        "revoked"
    } else if r.type_ids.is_none() && bound_exist {
        "refused"
    } else {
        "active"
    };
    HostTokenInfo {
        id: r.id,
        name: r.name,
        prefix: r.prefix,
        type_ids: r.type_ids,
        state: state.to_owned(),
        created_at: r.created_at,
        last_used_at: r.last_used_at,
        revoked_at: r.revoked_at,
    }
}

pub async fn list(s: &AppState) -> Result<HostTokenList, ApiError> {
    let bound = connector_hosts::bound_tokens_exist(&s.pool).await?;
    let items = connector_hosts::list_tokens(&s.pool)
        .await?
        .into_iter()
        .map(|r| info(r, bound))
        .collect();
    Ok(HostTokenList { items })
}

fn event(actor: &AuditActor, action: &str, id: Uuid) -> AuditEvent {
    AuditEvent::new(actor.clone(), action, Outcome::Success).target("connector_host_token", id)
}

pub async fn issue(
    s: &AppState,
    req: IssueHostTokenRequest,
    by: Uuid,
    actor: &AuditActor,
) -> Result<HostTokenIssued, ApiError> {
    let name = req.name.trim().to_owned();
    let mut errs = Vec::new();
    if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control) {
        errs.push(FieldError::new("/name", "must be 1-80 characters"));
    }
    let mut types: Vec<String> = req.type_ids.iter().map(|t| t.trim().to_owned()).collect();
    types.sort();
    types.dedup();
    if types.is_empty() || types.len() > TYPES_MAX {
        errs.push(FieldError::new(
            "/type_ids",
            "must list 1-64 connector types",
        ));
    }
    if types
        .iter()
        .any(|t| !aurora_registry::manifest::valid_id(t))
    {
        errs.push(FieldError::new(
            "/type_ids",
            "connector type ids match [a-z][a-z0-9_-]{1,47}",
        ));
    }
    if !errs.is_empty() {
        return Err(ApiError::invalid_request(
            "Invalid connector host token.",
            errs,
        ));
    }
    let tok = token::generate(TokenPrefix::ConnectorHost);
    let id = ids::new_id();
    let mut tx = audit::begin(s).await?;
    let row = connector_hosts::insert_bound_token(
        &mut *tx,
        id,
        &token::hash(tok.expose()),
        &token::display_prefix(tok.expose()),
        &name,
        &types,
        Some(by),
    )
    .await
    .map_err(|e| aurora_db::conflict_code(e, &[("connector_host_tokens_name_idx", "conflict")]))?;
    let ev = event(actor, "admin.connector_host_token_issue", id)
        .metadata(serde_json::json!({"name": name, "type_ids": types}));
    audit::commit_critical(tx, &ev).await?;
    Ok(HostTokenIssued {
        info: info(row, true),
        token: tok.expose().clone(),
    })
}

pub async fn revoke(s: &AppState, id: Uuid, actor: &AuditActor) -> Result<(), ApiError> {
    let mut tx = audit::begin(s).await?;
    let row = connector_hosts::revoke_token(&mut *tx, id)
        .await
        .map_err(|e: DbError| ApiError::from(e))?
        .ok_or_else(|| ApiError::not_found("Connector host token not found."))?;
    let ev = event(actor, "admin.connector_host_token_revoke", id)
        .metadata(serde_json::json!({"name": row.name, "type_ids": row.type_ids}));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}

//! The user-token routes' logic (P4-UTOK, C-853, C-855): add and verify, list, rename, revoke and
//! grants. Only the caller's own tokens exist for the caller (404 otherwise).

use super::custody::{self, Discards};
use super::dto::*;
use super::end;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::channels::inbox::send::Ctx;
use crate::domains::connectors::ops::{self, CORRELATED_WAIT, CorrelatedError, NewOp};
use crate::http::auth::{Caller, SessionCaller};
use aurora_channels::Capability;
use aurora_core::limits::USER_TOKENS_PER_PERSON;
use aurora_core::{ApiError, sanitize};
use aurora_db::repo::{agents, channel_integrations, connectors, user_links, user_tokens as repo};
use aurora_db::{DbError, conflict_code};
use aurora_protocol::connector::UserTokenVerifyResult;
use serde_json::json;
use uuid::Uuid;

const TOKEN_MAX: usize = 256;
const LABEL_MAX: usize = 80;
const LIVE_PER_PLATFORM_USER: &str = "user_tokens_live_per_platform_user_idx";

fn token_404() -> ApiError {
    ApiError::not_found("Token not found.")
}

fn unsupported() -> ApiError {
    ApiError::unprocessable(
        "user_tokens_unsupported",
        "This chat platform does not support posting as a person.",
    )
}

fn clean_label(label: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(l) = label else { return Ok(None) };
    if l.chars().count() > LABEL_MAX {
        return Err(ApiError::invalid_field(
            "Invalid request.",
            "/label",
            "must be at most 80 characters",
        ));
    }
    let l = sanitize::one_line(l, LABEL_MAX);
    let l = l.trim();
    Ok((!l.is_empty()).then(|| l.to_owned()))
}

async fn view(s: &AppState, rows: Vec<repo::UserTokenRow>) -> Result<Vec<UserToken>, ApiError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let grants = repo::grants_for(&s.pool, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|r| UserToken::build(r, &grants))
        .collect())
}

pub async fn list(s: &AppState, c: &Caller) -> Result<UserTokenList, ApiError> {
    let rows = repo::list_for_user(&s.pool, c.user_id).await?;
    Ok(UserTokenList {
        tokens: view(s, rows).await?,
    })
}

/// `POST /v1/me/user-tokens`: seal, verify against the platform, compare with the caller's own
/// link, then store. Every refusal discards the sealed value and stores nothing.
pub async fn create(
    s: &AppState,
    c: &SessionCaller,
    req: CreateUserTokenRequest,
) -> Result<UserToken, ApiError> {
    let token = req.token.trim();
    if token.is_empty() || token.chars().count() > TOKEN_MAX {
        return Err(ApiError::invalid_field(
            "Invalid request.",
            "/token",
            "must be 1 to 256 characters",
        ));
    }
    let label = clean_label(req.label.as_deref())?;
    let integ = channel_integrations::get(&s.pool, req.integration_id)
        .await?
        .filter(|i| i.status == "active")
        .ok_or_else(|| ApiError::not_found("Integration not found."))?;
    // The person's own link on this integration is what the token is compared with.
    let link = user_links::list_for_user_in(&s.pool, c.user_id, integ.id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            ApiError::unprocessable(
                "user_link_required",
                "Link your chat account to this workspace before adding a token.",
            )
        })?;
    let manifest = connectors::get(&s.pool, &integ.connector_id)
        .await?
        .and_then(|k| crate::domains::connectors::effective_manifest(&k));
    let org = integ.org_id;
    if !Ctx::new(integ.clone(), manifest)
        .caps
        .has(&Capability::UserTokens)
    {
        return Err(unsupported());
    }
    if repo::list_for_user(&s.pool, c.user_id).await?.len() >= USER_TOKENS_PER_PERSON {
        return Err(limit());
    }

    let token_id = Uuid::now_v7();
    let op_id = Uuid::now_v7();
    let sealed_token = custody::seal_token(s, token_id, org, token).await?;
    let sealed_copy = match custody::seal_op_copy_of(s, op_id, org, token).await {
        Ok(r) => r,
        Err(e) => {
            custody::discard_unused_token(s, token_id, org, &sealed_token).await;
            return Err(e);
        }
    };
    let verified = ops::call_correlated_with_id(
        s,
        op_id,
        NewOp {
            kind: "user_token.verify".to_owned(),
            connector_id: None,
            integration_id: Some(integ.id),
            presence_id: None,
            payload: json!({
                "config": integ.config,
                "user_credential_ref": sealed_copy.to_column(),
                "user_org_id": org,
            }),
            dedupe_key: None,
        },
        CORRELATED_WAIT,
    )
    .await;
    // The op's terminal state discards the copy; this covers an op that never ran.
    custody::discard_unused_copy(s, op_id, org, &sealed_copy).await;
    let refuse = |e: ApiError| refuse_add(s, token_id, org, &sealed_token, e);
    let who = match verified {
        Ok(v) => match serde_json::from_value::<UserTokenVerifyResult>(v) {
            Ok(w) => w,
            Err(_) => {
                return refuse(ApiError::new(
                    http::StatusCode::BAD_GATEWAY,
                    "connector_error",
                    "The chat platform returned an unreadable answer.",
                ))
                .await;
            }
        },
        Err(CorrelatedError::Failed { code, .. }) if code == "credential_dead" => {
            return refuse(ApiError::unprocessable(
                "user_token_invalid",
                "The chat platform rejected this token.",
            ))
            .await;
        }
        Err(e) => return refuse(e.into()).await,
    };
    if who.platform_user_ref != link.platform_user_ref {
        return refuse(ApiError::unprocessable(
            "user_token_mismatch",
            "This token belongs to a different chat account than yours.",
        ))
        .await;
    }

    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    if repo::lock_and_count_live(&mut tx, c.user_id).await? >= USER_TOKENS_PER_PERSON as i64 {
        drop(tx);
        return refuse(limit()).await;
    }
    let handle = sanitize::label(&who.handle);
    let inserted = repo::insert(
        &mut tx,
        &repo::NewUserToken {
            id: token_id,
            user_id: c.user_id,
            integration_id: integ.id,
            platform_user_ref: &link.platform_user_ref,
            platform_user_label: &handle,
            label: label.as_deref(),
            token_ref: &sealed_token.to_column(),
        },
    )
    .await;
    let row = match inserted {
        Ok(r) => r,
        Err(e) => {
            drop(tx);
            let api = conflict_code(e, &[(LIVE_PER_PLATFORM_USER, "user_token_exists")]);
            return refuse(api).await;
        }
    };
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "user_token.create",
            Outcome::Success,
        )
        .org(org)
        .target("user_token", token_id)
        .metadata(json!({"integration_id": integ.id})),
    );
    Ok(UserToken::build(row, &[]))
}

/// The caller's own proposed posts, with the text (nobody else's).
pub async fn own_sends(
    s: &AppState,
    c: &SessionCaller,
    status: Option<&str>,
) -> Result<OwnSendApprovalList, ApiError> {
    crate::domains::action_approvals::service::validate_status(status)?;
    let rows = aurora_db::repo::action_approvals::list_send_as_me_for_owner(
        &s.pool, c.user_id, status, 100,
    )
    .await?;
    Ok(OwnSendApprovalList {
        items: rows
            .into_iter()
            .filter_map(OwnSendApproval::build)
            .collect(),
    })
}

/// An add that is refused: the sealed value is discarded and nothing was stored.
async fn refuse_add(
    s: &AppState,
    token_id: Uuid,
    org: Uuid,
    sealed: &aurora_secrets::SecretRef,
    e: ApiError,
) -> Result<UserToken, ApiError> {
    custody::discard_unused_token(s, token_id, org, sealed).await;
    Err(e)
}

fn limit() -> ApiError {
    ApiError::conflict(
        "user_token_limit",
        format!("You can keep at most {USER_TOKENS_PER_PERSON} tokens. Revoke one first."),
    )
}

pub async fn rename(
    s: &AppState,
    c: &SessionCaller,
    id: Uuid,
    req: PatchUserTokenRequest,
) -> Result<UserToken, ApiError> {
    let label = clean_label(req.label.as_deref())?;
    let row = repo::get_own(&s.pool, c.user_id, id)
        .await?
        .ok_or_else(token_404)?;
    repo::set_label(&s.pool, id, label.as_deref()).await?;
    let row = repo::get(&s.pool, row.id).await?.ok_or_else(token_404)?;
    let mut v = view(s, vec![row]).await?;
    v.pop().ok_or_else(token_404)
}

/// `DELETE /v1/me/user-tokens/{id}`: the owner revokes (D-284(a)).
pub async fn revoke(s: &AppState, c: &SessionCaller, id: Uuid) -> Result<(), ApiError> {
    let mut tx = audit::begin(s).await?;
    repo::get_own_for_update(&mut tx, c.user_id, id)
        .await?
        .ok_or_else(token_404)?;
    let actor = AuditActor::from_caller(c);
    let discards = end::revoke(&mut tx, &[id], &actor, "owner").await?;
    tx.commit().await.map_err(DbError::from)?;
    discards.run(s).await;
    Ok(())
}

/// `PUT /v1/me/user-tokens/{id}/agents/{agent}`: idempotent.
pub async fn grant(s: &AppState, c: &SessionCaller, id: Uuid, agent: Uuid) -> Result<(), ApiError> {
    let token = repo::get_own(&s.pool, c.user_id, id)
        .await?
        .ok_or_else(token_404)?;
    let agent_row = agents::get(&s.pool, agent)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    // 404 for an agent the caller cannot see (or in another organization than the integration),
    // 403 for a visible one the caller may not use.
    c.workspace(s, agent_row.workspace_id).await?;
    if agent_row.org_id != token.org_id {
        return Err(ApiError::not_found("Agent not found."));
    }
    if token.status != "active" {
        return Err(ApiError::conflict(
            "token_inactive",
            "This token is not active; add a new one.",
        ));
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::get_own_for_update(&mut tx, c.user_id, id)
        .await?
        .ok_or_else(token_404)?;
    let added = repo::put_grant(&mut tx, id, agent).await?;
    tx.commit().await.map_err(DbError::from)?;
    if added {
        audit::record(
            s,
            AuditEvent::new(
                AuditActor::from_caller(c),
                "user_token.grant",
                Outcome::Success,
            )
            .org(token.org_id)
            .target("user_token", id)
            .metadata(json!({"agent_id": agent})),
        );
    }
    Ok(())
}

/// `DELETE /v1/me/user-tokens/{id}/agents/{agent}`: idempotent; the agent's pending sends end.
pub async fn ungrant(
    s: &AppState,
    c: &SessionCaller,
    id: Uuid,
    agent: Uuid,
) -> Result<(), ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let token = repo::get_own_for_update(&mut tx, c.user_id, id)
        .await?
        .ok_or_else(token_404)?;
    let removed = repo::delete_grant(&mut tx, id, agent).await?;
    let discards = if removed {
        end::end_grant(&mut tx, id, agent).await?
    } else {
        Discards::default()
    };
    tx.commit().await.map_err(DbError::from)?;
    discards.run(s).await;
    if removed {
        audit::record(
            s,
            AuditEvent::new(
                AuditActor::from_caller(c),
                "user_token.ungrant",
                Outcome::Success,
            )
            .org(token.org_id)
            .target("user_token", id)
            .metadata(json!({"agent_id": agent})),
        );
    }
    Ok(())
}

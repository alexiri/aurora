//! The connector claim's branch for an op that acts with a person's token (P4-UTOK, C-857,
//! D-282(c), SEC-112): a `user_token.verify` or a `message.post` whose `sender` is a `user_token`.
//!
//! For a send the token must still be `active`, the agent's grant present and the owner's link
//! on the integration there; otherwise the op fails `credential_revoked` and its copy is
//! discarded. The per-op copy is opened here and nowhere else, and returned to be delivered as
//! `Operation.user_credential`; the stored payload keeps only the (stripped) reference until the
//! terminal state discards it.

use super::custody::{self, OpenError};
use super::events;
use crate::app::AppState;
use crate::domains::connectors::ops;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::channel_ops::{self, Claimed};
use aurora_db::repo::user_tokens;
use serde_json::json;
use uuid::Uuid;

/// What the claim does with the op.
pub enum Outcome {
    /// Hand the op out with this token.
    Deliver(String),
    /// The op was failed `credential_revoked`; do not hand it out.
    Refused,
    /// The store is unavailable: put the op back to `pending`.
    Retry,
}

/// Decide a claimed user-token op (see the module docs).
pub async fn on_claim(s: &AppState, r: &Claimed) -> Result<Outcome, ApiError> {
    let token_id = r
        .user_token_id
        .as_deref()
        .and_then(|t| t.parse::<Uuid>().ok());
    let agent_id = r
        .user_meta
        .as_ref()
        .and_then(|m| m.get("agent_id"))
        .and_then(|a| a.as_str())
        .and_then(|t| t.parse::<Uuid>().ok());
    // A send checks the token, the grant and the link; a verify has no token row yet.
    let mut owner = None;
    if r.kind == "message.post" {
        let Some(token_id) = token_id else {
            return refuse(s, r, None, None).await;
        };
        let standing = standing(s, r, token_id, agent_id).await?;
        let Some(user) = standing else {
            return refuse(s, r, Some(token_id), None).await;
        };
        owner = Some(user);
    }
    let (Some(sealed), Some(org)) = (r.user_credential_ref.as_deref(), r.user_org_id) else {
        return refuse(s, r, token_id, owner).await;
    };
    match custody::open_op_copy(s, r.id, org, sealed).await {
        Ok(token) => {
            if let Some(t) = token_id {
                user_tokens::touch_last_used(&s.pool, t)
                    .await
                    .map_err(ApiError::from)?;
            }
            Ok(Outcome::Deliver(token))
        }
        Err(OpenError::Gone) => refuse(s, r, token_id, owner).await,
        Err(OpenError::Unavailable) => Ok(Outcome::Retry),
    }
}

/// The owner's user id when the send may go: the token is `active` on this op's integration, the
/// agent holds the grant and the owner's link is there.
async fn standing(
    s: &AppState,
    r: &Claimed,
    token_id: Uuid,
    agent_id: Option<Uuid>,
) -> Result<Option<Uuid>, ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let Some(token) = user_tokens::get(&mut *conn, token_id).await? else {
        return Ok(None);
    };
    if Some(token.integration_id) != r.integration_id {
        return Ok(None);
    }
    let Some(agent) = agent_id else {
        return Ok(None);
    };
    Ok(super::send::standing(&mut conn, &token, agent)
        .await?
        .then_some(token.user_id))
}

/// Fail the op `credential_revoked`, discard its copy and audit the refused send.
async fn refuse(
    s: &AppState,
    r: &Claimed,
    token_id: Option<Uuid>,
    owner: Option<Uuid>,
) -> Result<Outcome, ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    channel_ops::finish(
        &mut *tx,
        r.id,
        "failed",
        Some(&json!({"error": {"code": "credential_revoked",
            "message": "The person's token is no longer usable."}})),
        Some("credential_revoked"),
    )
    .await?;
    ops::notify_op_result(&mut tx, r.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Some(sealed) = r.user_credential_ref.as_deref() {
        custody::discard_op_copy(s, r.id, r.user_org_id, sealed).await;
    }
    if let (Some(t), Some(org)) = (token_id, r.user_org_id) {
        let meta = r.user_meta.clone().unwrap_or_default();
        events::record_send(s, org, t, owner, &meta, false, Some("credential_revoked"));
    }
    Ok(Outcome::Refused)
}

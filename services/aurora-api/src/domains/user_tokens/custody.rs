//! Custody of a person's chat-platform token (P4-UTOK, D-282, SEC-112).
//!
//! This is the **only** module that seals, opens or discards under the purposes `user_token` and
//! `channel_op_user_token` (a static test in `tests/it/user_tokens.rs` scans the source tree for
//! any other use). The token's life:
//!
//! * [`seal_token`]: the pasted value is sealed at once under `user_token` (owner = the row id);
//! * [`seal_op_copy_of`]: a per-op copy, for one `user_token.verify` (from the pasted value) or one
//!   `message.post` ([`copy_for_send`], from the stored token), owner = the op id;
//! * [`open_op_copy`]: opened **only** by the connector claim, which hands the value to the
//!   connector as `Operation.user_credential` for that one operation;
//! * [`discard_op_copy`] at the op's terminal state (and by [`sweep`], which retries a failed
//!   discard), and [`Discards::run`] after an end of standing committed.
//!
//! Nothing here logs a value; errors carry no plaintext.

use crate::app::AppState;
use aurora_core::{ApiError, Secret};
use aurora_db::repo::{channel_ops, user_tokens as repo};
use aurora_secrets::{Purpose, SealContext, SecretError, SecretRef, ValueSource};
use uuid::Uuid;

/// Seal context of a token row's sealed value.
fn token_ctx(token_id: Uuid, org_id: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::UserToken,
        owner_id: token_id,
        org_id,
        workspace_id: None,
    }
}

/// Seal context of one op's copy.
fn op_ctx(op_id: Uuid, org_id: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::ChannelOpUserToken,
        owner_id: op_id,
        org_id,
        workspace_id: None,
    }
}

/// 503 `secret_store_unavailable`: nothing was stored.
pub fn store_unavailable() -> ApiError {
    ApiError::unavailable(
        "secret_store_unavailable",
        "The secret store is unavailable; nothing was saved. Try again shortly.",
    )
}

/// Seal the pasted token for the row `token_id`.
pub async fn seal_token(
    s: &AppState,
    token_id: Uuid,
    org_id: Uuid,
    plain: &str,
) -> Result<SecretRef, ApiError> {
    s.vault
        .seal(
            &token_ctx(token_id, org_id),
            Secret::from(plain.as_bytes().to_vec()),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "user token not sealed");
            store_unavailable()
        })
}

/// Seal a copy of `plain` for the op `op_id` (a verify).
pub async fn seal_op_copy_of(
    s: &AppState,
    op_id: Uuid,
    org_id: Uuid,
    plain: &str,
) -> Result<SecretRef, ApiError> {
    s.vault
        .seal(
            &op_ctx(op_id, org_id),
            Secret::from(plain.as_bytes().to_vec()),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "user token copy not sealed");
            store_unavailable()
        })
}

/// Why a stored token could not be copied for a send.
#[derive(Debug)]
pub enum CopyError {
    /// The stored value is gone or foreign: the token cannot be used.
    Gone,
    Unavailable,
}

/// Copy the stored token (`token_ref` of an `active` row) into a per-op copy for `op_id`.
pub async fn copy_for_send(
    s: &AppState,
    token_id: Uuid,
    org_id: Uuid,
    token_ref: &str,
    op_id: Uuid,
) -> Result<SecretRef, CopyError> {
    let sealed: SecretRef = token_ref.parse().map_err(|_| CopyError::Gone)?;
    let plain = s
        .vault
        .open(&token_ctx(token_id, org_id), &sealed)
        .await
        .map_err(|e| match e {
            SecretError::StoreUnavailable | SecretError::Internal => CopyError::Unavailable,
            _ => CopyError::Gone,
        })?;
    s.vault
        .seal(&op_ctx(op_id, org_id), plain)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "user token copy not sealed");
            CopyError::Unavailable
        })
}

/// Why an op's copy could not be opened at claim.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No copy (revoked or discarded) or one that no longer opens: fail the op.
    Gone,
    /// The store is down: the op goes back to `pending`.
    Unavailable,
}

/// The claim's opening of an op's copy: the token as the connector receives it.
pub async fn open_op_copy(
    s: &AppState,
    op_id: Uuid,
    org_id: Uuid,
    sealed: &str,
) -> Result<String, OpenError> {
    let r: SecretRef = sealed.parse().map_err(|_| OpenError::Gone)?;
    let plain = s
        .vault
        .open(&op_ctx(op_id, org_id), &r)
        .await
        .map_err(|e| match e {
            SecretError::StoreUnavailable | SecretError::Internal => OpenError::Unavailable,
            _ => OpenError::Gone,
        })?;
    String::from_utf8(plain.expose().clone()).map_err(|_| OpenError::Gone)
}

/// Discard a per-op copy that was sealed but never used (best effort; never logs the value).
pub async fn discard_unused_copy(s: &AppState, op_id: Uuid, org_id: Uuid, sealed: &SecretRef) {
    if let Err(e) = s
        .vault
        .discard(&op_ctx(op_id, org_id), sealed, ValueSource::Managed)
        .await
    {
        tracing::warn!(error = %e, op_id = %op_id, "user token copy not discarded");
    }
}

/// Discard a stored token's sealed value without a row (an add that was refused). Returns
/// whether the value is gone (a failed discard is logged).
pub async fn discard_unused_token(
    s: &AppState,
    token_id: Uuid,
    org_id: Uuid,
    sealed: &SecretRef,
) -> bool {
    match s
        .vault
        .discard(&token_ctx(token_id, org_id), sealed, ValueSource::Managed)
        .await
    {
        Ok(()) | Err(SecretError::NotFound) => true,
        Err(e) => {
            tracing::warn!(error = %e, token_id = %token_id, "user token not discarded");
            false
        }
    }
}

/// Discard an ended token's value and null its reference (a failed discard leaves the
/// reference for [`sweep`]).
async fn discard_ended_token(s: &AppState, token_id: Uuid, org_id: Uuid, sealed: &str) {
    let gone = match sealed.parse::<SecretRef>() {
        Ok(r) => discard_unused_token(s, token_id, org_id, &r).await,
        // Not a reference we can act on: drop it so it is not kept.
        Err(_) => true,
    };
    if gone && let Err(e) = repo::clear_ended_ref(&s.pool, token_id).await {
        tracing::warn!(error = %e, token_id = %token_id, "user token reference not cleared");
    }
}

/// Discard an op's stored copy at its terminal state and clear the reference. A failed discard
/// leaves the reference for [`sweep`].
pub async fn discard_op_copy(s: &AppState, op_id: Uuid, org_id: Option<Uuid>, sealed: &str) {
    let (Some(org), Ok(r)) = (org_id, sealed.parse::<SecretRef>()) else {
        // Not a reference we can act on: drop it so it is not kept.
        if let Err(e) = channel_ops::clear_user_copy_ref(&s.pool, op_id).await {
            tracing::warn!(error = %e, op_id = %op_id, "user token copy reference not cleared");
        }
        return;
    };
    match s
        .vault
        .discard(&op_ctx(op_id, org), &r, ValueSource::Managed)
        .await
    {
        Ok(()) | Err(SecretError::NotFound) => {
            if let Err(e) = channel_ops::clear_user_copy_ref(&s.pool, op_id).await {
                tracing::warn!(error = %e, op_id = %op_id, "user token copy reference not cleared");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, op_id = %op_id, "user token copy not discarded");
        }
    }
}

/// Sealed values an end of standing took out of the database: discarded after its transaction
/// committed (the rows no longer point at them, so nothing can open them again; a store failure
/// here leaves a stale version in the store and is logged).
#[derive(Debug, Default)]
pub struct Discards {
    tokens: Vec<(Uuid, Uuid, String)>,
    copies: Vec<(Uuid, Uuid, String)>,
}

impl Discards {
    pub fn token(&mut self, token_id: Uuid, org_id: Uuid, sealed: Option<String>) {
        if let Some(r) = sealed {
            self.tokens.push((token_id, org_id, r));
        }
    }

    pub fn copy(&mut self, op_id: Uuid, org_id: Option<Uuid>, sealed: Option<String>) {
        if let (Some(org), Some(r)) = (org_id, sealed) {
            self.copies.push((op_id, org, r));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty() && self.copies.is_empty()
    }

    pub fn extend(&mut self, other: Discards) {
        self.tokens.extend(other.tokens);
        self.copies.extend(other.copies);
    }

    /// Run the discards (best effort).
    pub async fn run(self, s: &AppState) {
        for (token, org, r) in self.tokens {
            discard_ended_token(s, token, org, &r).await;
        }
        for (op, org, r) in self.copies {
            match r.parse::<SecretRef>() {
                Ok(r) => discard_unused_copy(s, op, org, &r).await,
                Err(_) => tracing::warn!(op_id = %op, "user token copy reference unreadable"),
            }
        }
    }
}

/// One pass: finished ops whose per-op copy is still referenced get it discarded (idempotent,
/// safe on every replica). Returns how many copies were pruned.
pub async fn sweep(s: &AppState) -> Result<usize, ApiError> {
    let mut n = 0;
    for e in channel_ops::finished_with_user_copy(&s.pool, 200).await? {
        discard_op_copy(s, e.id, e.org_id, &e.sealed).await;
        n += 1;
    }
    // Tokens that ended (the owner, a link, SCIM, a platform `401`) and still hold their value.
    for e in repo::ended_with_ref(&s.pool, 200).await? {
        discard_ended_token(s, e.id, e.org_id, &e.token_ref).await;
        n += 1;
    }
    Ok(n)
}

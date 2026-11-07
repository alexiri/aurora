//! Action tokens (docs/13 §4.4): `v2.<key_id>.<b64url(payload)>.<b64url(HMAC)>` signed
//! with the active signing key's HKDF `action-token` key (RFE-007); phase 0 `v1.<payload>.<HMAC>`
//! tokens still verify by trying each ring key. This module signs and verifies and exposes
//! the token id (`jti`); single use is enforced by the API (`action_token_uses`).

use aurora_crypto::{DerivedKeys, Purpose};
use aurora_protocol::vocab::BoundedString;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Phase 0 token version (verify only).
pub const VERSION_V1: &str = "v1";
/// Current token version: carries the signing-key id.
pub const VERSION: &str = "v2";
/// Upper bound on a whole token (fits platform button value limits comfortably).
pub const MAX_TOKEN_LEN: usize = 2048;
pub const MAX_ALLOWED_AGENTS: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Deny,
}

/// Token kind plus kind-specific `data`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data")]
pub enum ActionKind {
    #[serde(rename = "approval.decide")]
    ApprovalDecide { decision: Decision },
    #[serde(rename = "approval.batch_decide")]
    ApprovalBatchDecide {
        batch_id: Uuid,
        agent_id: Uuid,
        decision: Decision,
    },
    #[serde(rename = "action_approval.decide")]
    ActionApprovalDecide { decision: Decision },
    #[serde(rename = "reach.decide")]
    ReachDecide { decision: Decision },
    #[serde(rename = "binding.choose")]
    BindingChoose {
        space_ref: BoundedString<256>,
        allowed_agent_ids: Vec<Uuid>,
    },
    #[serde(rename = "dm_default.choose")]
    DmDefaultChoose { allowed_agent_ids: Vec<Uuid> },
    #[serde(rename = "turn.stop")]
    TurnStop { turn_id: Uuid },
    #[serde(rename = "followup.send_now")]
    FollowupSendNow { turn_id: Uuid },
    #[serde(rename = "agent_link.decide")]
    AgentLinkDecide {
        side: BoundedString<32>,
        decision: Decision,
    },
}

impl ActionKind {
    fn allowed(&self) -> Option<&[Uuid]> {
        match self {
            Self::BindingChoose {
                allowed_agent_ids, ..
            }
            | Self::DmDefaultChoose { allowed_agent_ids } => Some(allowed_agent_ids),
            _ => None,
        }
    }
}

/// Token payload `{ jti, kind, integration_id, subject_id, data, exp }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Claims {
    /// Token id; the key of the single-use ledger.
    pub jti: Uuid,
    pub integration_id: Uuid,
    /// The card / approval / batch the token is bound to.
    pub subject_id: Uuid,
    /// Expiry, unix seconds.
    pub exp: i64,
    #[serde(flatten)]
    pub action: ActionKind,
}

impl Claims {
    /// New claims with a fresh time-ordered `jti`.
    pub fn new(integration_id: Uuid, subject_id: Uuid, exp: i64, action: ActionKind) -> Self {
        Self {
            jti: Uuid::now_v7(),
            integration_id,
            subject_id,
            exp,
            action,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TokenError {
    #[error("malformed action token")]
    Malformed,
    #[error("bad action token signature")]
    BadSignature,
    #[error("action token expired")]
    Expired,
    #[error("action token belongs to another integration")]
    WrongIntegration,
    #[error("action token too large or has too many allowed agents")]
    TooLarge,
}

/// Namespace for signing and verification.
pub struct ActionToken;

impl ActionToken {
    pub fn sign(keys: &DerivedKeys, claims: &Claims) -> Result<String, TokenError> {
        if claims
            .action
            .allowed()
            .is_some_and(|a| a.len() > MAX_ALLOWED_AGENTS)
        {
            return Err(TokenError::TooLarge);
        }
        let payload = serde_json::to_vec(claims).map_err(|_| TokenError::Malformed)?;
        let tag = keys.sign(Purpose::ActionToken, &payload);
        let token = format!(
            "{VERSION}.{}.{}.{}",
            keys.active_id(),
            URL_SAFE_NO_PAD.encode(&payload),
            URL_SAFE_NO_PAD.encode(tag)
        );
        if token.len() > MAX_TOKEN_LEN {
            return Err(TokenError::TooLarge);
        }
        Ok(token)
    }

    /// Verifies signature, expiry (`now` in unix seconds; `exp <= now` is
    /// expired) and that the token's integration equals `integration_id`.
    pub fn verify(
        keys: &DerivedKeys,
        token: &str,
        integration_id: Uuid,
        now: i64,
    ) -> Result<Claims, TokenError> {
        if token.len() > MAX_TOKEN_LEN {
            return Err(TokenError::Malformed);
        }
        let parts: Vec<&str> = token.split('.').collect();
        let (key_id, p, t) = match parts.as_slice() {
            [VERSION, id, p, t] => (Some(*id), *p, *t),
            [VERSION_V1, p, t] => (None, *p, *t),
            _ => return Err(TokenError::Malformed),
        };
        let payload = URL_SAFE_NO_PAD
            .decode(p)
            .map_err(|_| TokenError::Malformed)?;
        let tag = URL_SAFE_NO_PAD
            .decode(t)
            .map_err(|_| TokenError::Malformed)?;
        let good = match key_id {
            // `v2`: one lookup; an id outside the ring is refused.
            Some(id) => keys.verify_with(id, Purpose::ActionToken, &payload, &tag),
            // `v1`: no id, try each ring key.
            None => keys.verify(Purpose::ActionToken, &payload, &tag),
        };
        if !good {
            return Err(TokenError::BadSignature);
        }
        let claims: Claims = serde_json::from_slice(&payload).map_err(|_| TokenError::Malformed)?;
        if claims.exp <= now {
            return Err(TokenError::Expired);
        }
        if claims.integration_id != integration_id {
            return Err(TokenError::WrongIntegration);
        }
        Ok(claims)
    }
}

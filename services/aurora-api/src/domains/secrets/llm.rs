//! LLM key validation and origin verification at save (08 §6, §11.3).
use aurora_core::{ApiError, Secret};
use aurora_llm::governance::{EffectiveEndpoint, OriginStatus, StoredOrigin};
use aurora_llm::{
    AuthMode, Endpoint, KeyCheck, KeyOrigin, LlmError, LlmProvider, OriginMethod, origin,
};
use chrono::Utc;
use http::StatusCode;

pub enum KeyFail {
    InvalidKey,
    /// (C-801) The token endpoint of a minted mode refused the credential.
    MintRejected,
    /// (C-801) STS refused `AssumeRole`: the role's trust policy.
    AssumeRoleDenied,
    Unreachable,
}

impl KeyFail {
    pub fn into_error(self) -> ApiError {
        match self {
            Self::MintRejected => ApiError::unprocessable(
                "llm_key_invalid",
                "The token endpoint rejected this credential.",
            )
            .with_details(serde_json::json!({ "reason": "mint_rejected" })),
            Self::AssumeRoleDenied => ApiError::unprocessable(
                "llm_key_invalid",
                "The role could not be assumed. Its trust policy must let the source key assume it and require this organization's external id.",
            )
            .with_details(serde_json::json!({ "reason": "assume_role_denied" })),
            Self::InvalidKey => {
                ApiError::unprocessable("llm_key_invalid", "The provider rejected this API key.")
            }
            Self::Unreachable => ApiError::unprocessable(
                "llm_endpoint_unreachable",
                "The LLM endpoint could not be reached to verify the key.",
            ),
        }
    }
}

/// (P4-LLM-4, D-261) The context a provider validates a secret in: its auth mode and, for `aws_assume_role`, the
/// organization and its own `external_id` (read here from the database, never taken from the request).
///
/// # Errors
/// 500 when the organization cannot be read.
pub async fn key_check(
    pool: &sqlx::PgPool,
    org: uuid::Uuid,
    mode: AuthMode,
) -> Result<KeyCheck, ApiError> {
    let mut c = KeyCheck::new(mode);
    if mode == AuthMode::AwsAssumeRole {
        c.org_id = org.to_string();
        c.org_external_id = aurora_db::repo::connections::org_external_id(pool, org)
            .await
            .map_err(|_| ApiError::internal("Internal error."))?;
    }
    Ok(c)
}

fn origin_of(
    status: OriginStatus,
    method: &str,
    ko: Option<&KeyOrigin>,
    version: i64,
) -> StoredOrigin {
    StoredOrigin {
        status,
        method: Some(method.to_owned()),
        organization_id: ko.and_then(|k| k.organization_id.clone()),
        workspace_id: ko.and_then(|k| k.workspace_id.clone()),
        team_id: ko.and_then(|k| k.team_id.clone()),
        user_id: ko.and_then(|k| k.user_id.clone()),
        value_version: version,
        checked_at: Some(Utc::now().to_rfc3339()),
        detail: None,
    }
}

/// (C-1065, P5-REFINE-20) The origin of a key whose endpoint did not answer the check while no
/// origin rule applies: `pending` (usable, unproven), with why and when it was tried.
pub fn unanswered(version: i64) -> StoredOrigin {
    let mut o = origin_of(OriginStatus::Pending, "endpoint", None, version);
    o.detail = Some(origin::detail_of(&LlmError::Unreachable(String::new())));
    o
}

/// [`check_key`] that gives up after `budget` when no origin rule applies (C-1065, amended by
/// P5-REFINE-20): an endpoint that does not answer leaves the key `pending` (the background
/// re-check goes on) instead of holding the caller for the HTTP timeout. With a rule the proof is
/// required, so the check runs to its end as before.
#[allow(clippy::too_many_arguments)]
pub async fn check_key_within(
    budget: std::time::Duration,
    http: &reqwest::Client,
    provider: &dyn LlmProvider,
    ep: &Endpoint,
    check: &KeyCheck,
    eff: &EffectiveEndpoint,
    key: &Secret<String>,
    version: i64,
) -> Result<StoredOrigin, KeyFail> {
    let run = check_key(http, provider, ep, check, eff, key, version);
    if eff.origin_rule.is_some() {
        return run.await;
    }
    tokio::time::timeout(budget, run)
        .await
        .unwrap_or_else(|_| Ok(unanswered(version)))
}

/// Validate the key (list models) and verify its origin against the effective rule.
/// The returned origin may carry any status; callers refuse `mismatch`/`unverifiable`.
pub async fn check_key(
    http: &reqwest::Client,
    provider: &dyn LlmProvider,
    ep: &Endpoint,
    check: &KeyCheck,
    eff: &EffectiveEndpoint,
    key: &Secret<String>,
    version: i64,
) -> Result<StoredOrigin, KeyFail> {
    let rule = eff.origin_rule.as_ref();
    // (P4-LLM-2) A provider whose origin proof is the validation (Vertex: the mint) is called once, here.
    let prefetched = if provider.validates_by_origin() {
        Some(
            provider
                .verify_key_origin_as(ep, check, OriginMethod::ProviderAccount, key, http)
                .await,
        )
    } else {
        None
    };
    // (FIX-105, CRED-6) The strict call: an endpoint that did not answer is not a verified key.
    let listed = match &prefetched {
        Some(Ok(_)) => Ok(Vec::new()),
        Some(Err(LlmError::InvalidKey)) => Err(LlmError::InvalidKey),
        Some(Err(LlmError::MintRejected)) => Err(LlmError::MintRejected),
        Some(Err(LlmError::AssumeRoleDenied)) => Err(LlmError::AssumeRoleDenied),
        Some(Err(_)) => Err(LlmError::Unreachable("origin proof unavailable".into())),
        None => provider.list_models_strict_as(ep, check, key, http).await,
    };
    match listed {
        Ok(_) => {}
        Err(LlmError::InvalidKey) => return Err(KeyFail::InvalidKey),
        Err(LlmError::MintRejected) => return Err(KeyFail::MintRejected),
        Err(LlmError::AssumeRoleDenied) => return Err(KeyFail::AssumeRoleDenied),
        Err(_) if rule.is_none() => return Ok(unanswered(version)),
        Err(_) => return Err(KeyFail::Unreachable),
    }
    let Some(rule) = rule else {
        return Ok(origin_of(OriginStatus::Verified, "endpoint", None, version));
    };
    let proof = match prefetched {
        Some(r) if rule.method == OriginMethod::ProviderAccount => r,
        _ => {
            provider
                .verify_key_origin_as(ep, check, rule.method, key, http)
                .await
        }
    };
    match proof {
        Ok(ko) if origin::requires_team(&ko, rule) => {
            let mut o = origin_of(
                OriginStatus::Unverifiable,
                rule.method.as_str(),
                Some(&ko),
                version,
            );
            o.detail = Some(origin::KEY_REPORTS_NO_TEAM.to_owned());
            Ok(o)
        }
        Ok(ko) => {
            let status = if origin::complies(&ko, rule) {
                OriginStatus::Verified
            } else {
                OriginStatus::Mismatch
            };
            Ok(origin_of(status, rule.method.as_str(), Some(&ko), version))
        }
        Err(LlmError::InvalidKey) => Err(KeyFail::InvalidKey),
        Err(LlmError::MintRejected) => Err(KeyFail::MintRejected),
        Err(LlmError::AssumeRoleDenied) => Err(KeyFail::AssumeRoleDenied),
        Err(LlmError::Unreachable(_)) => Err(KeyFail::Unreachable),
        Err(e) => {
            let mut o = origin_of(
                OriginStatus::Unverifiable,
                rule.method.as_str(),
                None,
                version,
            );
            o.detail = Some(origin::detail_of(&e));
            Ok(o)
        }
    }
}

/// Save-time refusal of non-compliant origins (nothing is saved).
pub fn require_compliant(o: &StoredOrigin) -> Result<(), ApiError> {
    match o.status {
        OriginStatus::Mismatch => Err(with_detail(
            ApiError::unprocessable(
                "llm_key_wrong_organization",
                "This key belongs to an account that is not permitted for this organization.",
            ),
            o,
        )),
        OriginStatus::Unverifiable => Err(with_detail(
            ApiError::unprocessable(
                "llm_key_unverifiable",
                "The provider cannot report which account this key belongs to.",
            ),
            o,
        )),
        _ => Ok(()),
    }
}

/// `details.detail` (C-502): why the verification was refused. Never key material.
fn with_detail(e: ApiError, o: &StoredOrigin) -> ApiError {
    match &o.detail {
        Some(d) => e.with_details(serde_json::json!({ "detail": d })),
        None => e,
    }
}

pub fn not_permitted() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "llm_key_not_permitted",
        "LLM keys for this provider endpoint and scope are not permitted by governance.",
    )
}

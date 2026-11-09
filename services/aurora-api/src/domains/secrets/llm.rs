//! LLM key validation and origin verification at save (08 §6, §11.3).
use aurora_core::{ApiError, Secret};
use aurora_llm::governance::{EffectiveEndpoint, OriginStatus, StoredOrigin};
use aurora_llm::{Endpoint, KeyOrigin, LlmError, LlmProvider, origin};
use chrono::Utc;
use http::StatusCode;

pub enum KeyFail {
    InvalidKey,
    Unreachable,
}

impl KeyFail {
    pub fn into_error(self) -> ApiError {
        match self {
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

/// Validate the key (list models) and verify its origin against the effective rule.
/// The returned origin may carry any status; callers refuse `mismatch`/`unverifiable`.
pub async fn check_key(
    http: &reqwest::Client,
    provider: &dyn LlmProvider,
    ep: &Endpoint,
    eff: &EffectiveEndpoint,
    key: &Secret<String>,
    version: i64,
) -> Result<StoredOrigin, KeyFail> {
    let rule = eff.origin_rule.as_ref();
    let default_method = rule.map_or("endpoint", |r| r.method.as_str());
    match provider.list_models(ep, key, http).await {
        Ok(_) => {}
        Err(LlmError::InvalidKey) => return Err(KeyFail::InvalidKey),
        Err(_) if rule.is_none() => {
            return Ok(origin_of(
                OriginStatus::Pending,
                default_method,
                None,
                version,
            ));
        }
        Err(_) => return Err(KeyFail::Unreachable),
    }
    let Some(rule) = rule else {
        return Ok(origin_of(OriginStatus::Verified, "endpoint", None, version));
    };
    match provider.verify_key_origin(ep, rule.method, key, http).await {
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

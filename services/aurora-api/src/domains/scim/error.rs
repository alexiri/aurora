//! The SCIM error body (`16 §W15` Errors): `application/scim+json`,
//! `urn:ietf:params:scim:api:messages:2.0:Error`, `status` a string, `scimType` where RFC 7644
//! §3.12 names one. `detail` is sanitized: never a token, another user's e-mail or an internal
//! id. The 401 is one identical body for every authentication failure (no oracle).

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScimType {
    None,
    InvalidSyntax,
    InvalidValue,
    InvalidFilter,
    InvalidPath,
    NoTarget,
    Mutability,
    TooMany,
    Uniqueness,
}

impl ScimType {
    fn as_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::InvalidSyntax => Some("invalidSyntax"),
            Self::InvalidValue => Some("invalidValue"),
            Self::InvalidFilter => Some("invalidFilter"),
            Self::InvalidPath => Some("invalidPath"),
            Self::NoTarget => Some("noTarget"),
            Self::Mutability => Some("mutability"),
            Self::TooMany => Some("tooMany"),
            Self::Uniqueness => Some("uniqueness"),
        }
    }
}

#[derive(Serialize)]
struct Body {
    schemas: [&'static str; 1],
    status: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "scimType")]
    scim_type: Option<&'static str>,
    detail: String,
}

/// A failure of a SCIM request, rendered as the SCIM error.
#[derive(Debug, Clone)]
pub struct ScimError {
    pub status: StatusCode,
    pub scim_type: ScimType,
    pub detail: String,
    /// Only ever set on 429 (`retry-after`).
    pub retry_after_secs: Option<u64>,
}

impl ScimError {
    pub fn new(status: StatusCode, scim_type: ScimType, detail: impl Into<String>) -> Self {
        Self {
            status,
            scim_type,
            detail: detail.into(),
            retry_after_secs: None,
        }
    }

    // 400s
    pub fn invalid_syntax(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ScimType::InvalidSyntax, detail)
    }
    pub fn invalid_value(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ScimType::InvalidValue, detail)
    }
    pub fn invalid_filter(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ScimType::InvalidFilter, detail)
    }
    pub fn invalid_path(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ScimType::InvalidPath, detail)
    }
    pub fn no_target(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ScimType::NoTarget, detail)
    }
    pub fn mutability(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ScimType::Mutability, detail)
    }
    /// Over a limit (`tooMany`); for an oversized body the answer is 413 (below).
    pub fn too_many(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ScimType::TooMany, detail)
    }

    /// The one 401 of the whole surface: the same body for every authentication failure
    /// (missing, malformed, unknown, revoked, expired, org disabled, SCIM off).
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ScimType::None,
            "Authentication failed.",
        )
    }

    /// 403 (owner mutation, seat limit, SCIM suspended by the operator).
    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, ScimType::None, detail)
    }

    /// 404 (unknown or foreign resource, SCIM off at the instance).
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, ScimType::None, "Resource not found.")
    }

    /// 409 `uniqueness`.
    pub fn uniqueness(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, ScimType::Uniqueness, detail)
    }

    /// 413 for a body above `SCIM_BODY_MAX`.
    pub fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ScimType::TooMany,
            "The request body is too large.",
        )
    }

    /// 429 with `retry-after`.
    pub fn rate_limited(retry_after_secs: u64) -> Self {
        let mut e = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            ScimType::None,
            "Too many requests. Slow down and retry after the indicated delay.",
        );
        e.retry_after_secs = Some(retry_after_secs.max(1));
        e
    }

    /// 503 for a failed critical audit insert ("no audit, no change"): the change is not
    /// applied and the IdP retries.
    pub fn audit_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ScimType::None,
            "The audit log is unavailable.",
        )
    }

    /// 500 sanitized (a database error other than audit; the IdP retries).
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ScimType::None,
            "Internal error.",
        )
    }
}

impl IntoResponse for ScimError {
    fn into_response(self) -> Response {
        let b = Body {
            schemas: ["urn:ietf:params:scim:api:messages:2.0:Error"],
            status: self.status.as_u16().to_string(),
            scim_type: self.scim_type.as_str(),
            detail: self.detail,
        };
        let mut r = (self.status, axum::Json(b)).into_response();
        // The wire format is `application/scim+json` (16 §W15 Transport).
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/scim+json"),
        );
        // RFC 7644 §3.12 / docs/16 §W15: the 401 always carries the challenge.
        if self.status == StatusCode::UNAUTHORIZED {
            r.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }
        if let Some(s) = self.retry_after_secs
            && let Ok(v) = s.to_string().parse()
        {
            r.headers_mut().insert(header::RETRY_AFTER, v);
        }
        r
    }
}

pub type ScimResult<T> = Result<T, ScimError>;

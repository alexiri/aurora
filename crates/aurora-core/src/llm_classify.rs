//! (P4-LLM-1, C-808, `08 §Error classification`) The one table that sorts an upstream LLM error. The gateway calls it
//! with the status and, for Bedrock only, the `x-amzn-ErrorType` header (it never reads a response body); the harness
//! adapters call it with the body's error type or the stream event's and refine the turn code. (P4-LLM-5) Lives in
//! `aurora-core` (re-exported by `aurora-llm`) so the supervisor, which has no use for the provider stack, shares it.

/// What kind of failure an upstream answer is. `Other` is not recorded as a request-log `error_code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmErrorClass {
    Auth,
    RateLimited,
    Quota,
    Overloaded,
    Refusal,
    Other,
}

impl LlmErrorClass {
    /// The metric label (`aurora_gateway_llm_upstream_total{class}`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimited => "rate_limited",
            Self::Quota => "quota",
            Self::Overloaded => "overloaded",
            Self::Refusal => "refusal",
            Self::Other => "other",
        }
    }

    /// The request-log `error_code`; none for `Other`.
    pub fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Auth => Some("llm_auth"),
            Self::RateLimited => Some("llm_rate_limited"),
            Self::Quota => Some("llm_quota"),
            Self::Overloaded => Some("llm_overloaded"),
            Self::Refusal => Some("llm_refusal"),
            Self::Other => None,
        }
    }
}

/// Classifies an upstream answer. `error_type` is the provider's own error name when the caller has one (OpenAI
/// `insufficient_quota`, Bedrock `ThrottlingException`, Vertex `RESOURCE_EXHAUSTED`, Azure `content_filter`, …),
/// matched case-insensitively; otherwise the status decides. A 2xx is `Other` (not an error). `provider` is part of
/// the signature so a provider-specific row can be added without touching callers; today every provider shares the
/// table.
pub fn classify(provider: &str, status: u16, error_type: Option<&str>) -> LlmErrorClass {
    let _ = provider;
    if (200..300).contains(&status) {
        return LlmErrorClass::Other;
    }
    if let Some(t) = error_type.map(str::trim).filter(|t| !t.is_empty()) {
        let t = t.to_ascii_lowercase();
        // A Bedrock header can carry a `:`-suffixed URL (`ThrottlingException:http://internal…`): keep the name.
        let name = t.split(':').next().unwrap_or("");
        match name {
            "invalid_api_key"
            | "authentication_error"
            | "permission_error"
            | "unrecognizedclientexception"
            | "invalidsignatureexception"
            | "expiredtokenexception"
            | "accessdeniedexception"
            | "unauthenticated"
            | "permission_denied"
            | "invalidclienttokenid"
            | "missingauthenticationtoken" => return LlmErrorClass::Auth,
            "insufficient_quota" | "servicequotaexceededexception" | "billing_error" => {
                return LlmErrorClass::Quota;
            }
            "rate_limit_exceeded"
            | "rate_limit_error"
            | "throttlingexception"
            | "toomanyrequestsexception"
            | "resource_exhausted" => return LlmErrorClass::RateLimited,
            "overloaded_error"
            | "serviceunavailableexception"
            | "modelnotreadyexception"
            | "modeltimeoutexception"
            | "internalserverexception"
            | "unavailable" => return LlmErrorClass::Overloaded,
            "content_filter"
            | "content_policy_violation"
            | "safety"
            | "guardrail_intervened"
            | "guardrailinterventionexception" => return LlmErrorClass::Refusal,
            _ => {}
        }
    }
    match status {
        401 | 403 => LlmErrorClass::Auth,
        429 => LlmErrorClass::RateLimited,
        500..=599 => LlmErrorClass::Overloaded,
        _ => LlmErrorClass::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LlmErrorClass::*;

    #[test]
    fn every_row_of_the_table() {
        let rows: &[(&str, u16, Option<&str>, LlmErrorClass)] = &[
            // OpenAI
            ("openai", 401, Some("invalid_api_key"), Auth),
            ("openai", 401, None, Auth),
            ("openai", 429, Some("rate_limit_exceeded"), RateLimited),
            ("openai", 429, Some("insufficient_quota"), Quota),
            ("openai", 429, None, RateLimited), // the gateway never reads a body: a 429 is rate_limited
            ("openai", 500, None, Overloaded),
            ("openai", 503, None, Overloaded),
            ("openai", 400, Some("content_filter"), Refusal),
            // Azure
            ("azure_openai", 401, None, Auth),
            ("azure_openai", 400, Some("content_filter"), Refusal),
            ("azure_openai", 429, None, RateLimited),
            // Bedrock (x-amzn-ErrorType)
            ("bedrock", 403, Some("UnrecognizedClientException"), Auth),
            ("bedrock", 403, Some("InvalidSignatureException"), Auth),
            ("bedrock", 400, Some("ExpiredTokenException"), Auth),
            ("bedrock", 403, Some("AccessDeniedException"), Auth),
            ("bedrock", 429, Some("ThrottlingException"), RateLimited),
            (
                "bedrock",
                429,
                Some("ThrottlingException:http://internal.amazon.com/coral/"),
                RateLimited,
            ),
            ("bedrock", 400, Some("ServiceQuotaExceededException"), Quota),
            (
                "bedrock",
                503,
                Some("ServiceUnavailableException"),
                Overloaded,
            ),
            ("bedrock", 429, Some("ModelNotReadyException"), Overloaded),
            (
                "bedrock",
                400,
                Some("GuardrailInterventionException"),
                Refusal,
            ),
            ("bedrock", 400, Some("ValidationException"), Other),
            // Vertex
            ("vertex", 401, Some("UNAUTHENTICATED"), Auth),
            ("vertex", 403, Some("PERMISSION_DENIED"), Auth),
            ("vertex", 429, Some("RESOURCE_EXHAUSTED"), RateLimited),
            ("vertex", 400, Some("SAFETY"), Refusal),
            // Anthropic, compatible, litellm: by status
            ("anthropic", 529, None, Overloaded),
            ("anthropic", 401, None, Auth),
            ("openai_compatible", 502, None, Overloaded),
            ("litellm", 404, None, Other),
            ("litellm", 422, None, Other),
            // 2xx is not an error
            ("openai", 200, None, Other),
            ("bedrock", 200, Some("ThrottlingException"), Other),
        ];
        for (p, s, t, want) in rows {
            assert_eq!(classify(p, *s, *t), *want, "{p} {s} {t:?}");
        }
    }

    #[test]
    fn codes_and_labels() {
        assert_eq!(RateLimited.error_code(), Some("llm_rate_limited"));
        assert_eq!(Other.error_code(), None);
        assert_eq!(Quota.as_str(), "quota");
    }
}

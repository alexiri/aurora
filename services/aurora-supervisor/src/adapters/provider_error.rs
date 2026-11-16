//! (P4-LLM-5, C-808, `12 §Provider refusal classification`, D-260(d), D-261) What both adapters share when a turn fails
//! on a model request: the gateway envelope's mapping and the shared `aurora-core` table applied to the error type a
//! provider's body or stream event carries. The adapters keep their own status/phrase heuristics for what the table does
//! not name (the CLIs report errors as free text).

use aurora_core::llm_classify::{LlmErrorClass, classify};
use serde_json::Value;

/// The turn error code of a table class (`12 §Provider refusal classification`).
fn turn_code(c: LlmErrorClass) -> Option<&'static str> {
    match c {
        LlmErrorClass::Auth => Some("provider_auth"),
        LlmErrorClass::RateLimited => Some("provider_rate_limited"),
        LlmErrorClass::Quota => Some("provider_quota"),
        LlmErrorClass::Overloaded => Some("provider_overloaded"),
        LlmErrorClass::Refusal => Some("provider_refusal"),
        LlmErrorClass::Other => None,
    }
}

/// A gateway envelope's (`{"error":{"code":…}}`) turn code. Every gateway refusal is `gateway_refused`, except a
/// 502 `credential_unavailable` whose `details.kind` is `llm_mint` and `details.reason` `mint_rejected`: the
/// credential, not the gateway, is at fault, so it is `provider_auth` (the gateway code stays in the message).
pub fn gateway_code(err: &Value) -> &'static str {
    let details = err.get("details");
    let field = |k: &str| details.and_then(|d| d.get(k)).and_then(Value::as_str);
    if err.get("code").and_then(Value::as_str) == Some("credential_unavailable")
        && field("kind") == Some("llm_mint")
        && field("reason") == Some("mint_rejected")
    {
        "provider_auth"
    } else {
        "gateway_refused"
    }
}

/// The turn code the shared table gives the provider's own error type, if it names one: `error.type` (Anthropic,
/// OpenAI, Azure), `error.code` (OpenAI's `insufficient_quota`), `error.status` (Vertex `RESOURCE_EXHAUSTED`), a
/// top-level `__type`/`type` (Bedrock JSON), or an `…Exception` name in the text (a Bedrock event-stream exception
/// frame as the CLI prints it). The status is deliberately not consulted: the adapters keep their status rules.
pub fn table_code(error: Option<&Value>, body: Option<&Value>, text: &str) -> Option<&'static str> {
    let s = |v: Option<&Value>, k: &str| {
        v.and_then(|v| v.get(k))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let candidates = [
        s(error, "type"),
        s(error, "code"),
        s(error, "status"),
        s(body, "__type"),
        s(body, "type"),
    ];
    for name in candidates.into_iter().flatten() {
        if let Some(c) = turn_code(classify("", 0, Some(&name))) {
            return Some(c);
        }
    }
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 9 && t.to_ascii_lowercase().ends_with("exception"))
        .find_map(|t| turn_code(classify("", 0, Some(t))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_rejected_mint_is_the_credentials_fault_every_other_gateway_code_is_the_gateways() {
        let mint = |reason: &str| json!({"code": "credential_unavailable", "details": {"kind": "llm_mint", "reason": reason}});
        assert_eq!(gateway_code(&mint("mint_rejected")), "provider_auth");
        assert_eq!(gateway_code(&mint("mint_unavailable")), "gateway_refused");
        assert_eq!(
            gateway_code(&json!({"code": "credential_unavailable", "details": {"kind": "secret"}})),
            "gateway_refused"
        );
        assert_eq!(
            gateway_code(&json!({"code": "llm_path_not_permitted"})),
            "gateway_refused"
        );
        assert_eq!(
            gateway_code(&json!({"code": "llm_request_too_large"})),
            "gateway_refused"
        );
    }

    #[test]
    fn the_shared_table_names_provider_errors_from_bodies_and_stream_exceptions() {
        let body = |v: Value| table_code(v.get("error"), Some(&v), "");
        // OpenAI: a 429 whose body says the quota is spent.
        assert_eq!(
            body(json!({"error": {"type": "insufficient_quota", "code": "insufficient_quota"}})),
            Some("provider_quota")
        );
        assert_eq!(
            body(json!({"error": {"type": "rate_limit_exceeded"}})),
            Some("provider_rate_limited")
        );
        // Vertex.
        assert_eq!(
            body(json!({"error": {"status": "RESOURCE_EXHAUSTED"}})),
            Some("provider_rate_limited")
        );
        assert_eq!(
            body(json!({"error": {"status": "PERMISSION_DENIED"}})),
            Some("provider_auth")
        );
        // Bedrock: JSON `__type` and an event-stream exception printed by the CLI.
        assert_eq!(
            body(json!({"__type": "ThrottlingException"})),
            Some("provider_rate_limited")
        );
        assert_eq!(
            table_code(
                None,
                None,
                "stream error: ThrottlingException: Too many requests"
            ),
            Some("provider_rate_limited")
        );
        assert_eq!(
            table_code(None, None, "ServiceQuotaExceededException: limit"),
            Some("provider_quota")
        );
        assert_eq!(
            table_code(None, None, "ModelStreamErrorException: x"),
            None,
            "an exception the table does not name is left to the adapter's rules"
        );
        assert_eq!(table_code(None, None, "the word safety appears here"), None);
        assert_eq!(body(json!({"error": {"type": "api_error"}})), None);
    }
}

use crate::ModelRef;
use http::Method;

const MODEL_PATHS: &[&str] = &[
    "/v1/messages/count_tokens",
    "/v1/messages",
    "/v1/chat/completions",
    "/v1/completions",
    "/v1/responses",
    "/v1/embeddings",
];

/// Locate the requested model. POSTs to model-invoking paths (also under any route
/// prefix, matched by path suffix) carry it at JSON pointer `/model`; a truncated,
/// unparsable or model-less body yields `Unknown` (fail closed). Anything else is `NoModel`.
pub fn model_from_request(
    method: &Method,
    path: &str,
    body_prefix: &[u8],
    truncated: bool,
) -> ModelRef {
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    if method != Method::POST || !MODEL_PATHS.iter().any(|p| path.ends_with(p)) {
        return ModelRef::NoModel;
    }
    if truncated {
        return ModelRef::Unknown;
    }
    match serde_json::from_slice::<serde_json::Value>(body_prefix) {
        Ok(v) => match v.pointer("/model").and_then(|m| m.as_str()) {
            Some(m) if !m.is_empty() => ModelRef::Model(m.to_string()),
            _ => ModelRef::Unknown,
        },
        Err(_) => ModelRef::Unknown,
    }
}

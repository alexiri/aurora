//! `POST /v1/admin/settings/llm/endpoint-test` (SET-3, C-779): one unauthenticated GET of an LLM
//! endpoint's models path from the API, through the destination guard (`gateway.allowed_private`
//! applies), within 10 s. Never a credential: no stored key, no `Authorization` header — a reply
//! of 401 or 403 still proves the endpoint is there. Nothing is saved.

use super::service::{field, invalid};
use crate::app::AppState;
use crate::domains::connect::env::classify_send_error;
use aurora_config::{LlmEndpoint, LlmSection};
use aurora_core::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::IpAddr;
use utoipa::ToSchema;

/// Request body (C-779).
#[derive(Deserialize, ToSchema)]
pub struct EndpointTestRequest {
    /// One endpoint entry, as in `llm.endpoints` (TOML shape as JSON).
    #[schema(value_type = Object)]
    pub endpoint: Value,
}

/// What the probe found.
#[derive(Serialize, ToSchema)]
pub struct EndpointTestResult {
    pub reachable: bool,
    /// The HTTP status of the answer.
    pub status: Option<u16>,
    /// `destination_refused`, `dns`, `timeout`, `connect`, `tls`, `unexpected_status` or
    /// `unreachable`.
    pub error_code: Option<String>,
    /// Model ids, only when the endpoint answers a 2xx OpenAI-style list without a key.
    pub models: Option<Vec<String>>,
}

/// The answer body is read up to this many bytes (a models list, not a download).
const BODY_MAX: usize = 256 * 1024;
const MODELS_MAX: usize = 200;

fn refuse(path: &str, code: &str, message: &str) -> ApiError {
    invalid(vec![field(path.to_owned(), code, message)])
}

pub async fn run(
    state: &AppState,
    req: EndpointTestRequest,
) -> Result<EndpointTestResult, ApiError> {
    // Shape first: the same refusals as a save (credential-like keys never get as far as a request).
    let problems = super::rules::entry_problems("llm.endpoints", &req.endpoint);
    if !problems.is_empty() {
        return Err(invalid(
            problems
                .into_iter()
                .map(|p| field(format!("/endpoint/{}", p.field), p.code, p.message))
                .collect(),
        ));
    }
    let entry: LlmEndpoint = serde_json::from_value(req.endpoint)
        .map_err(|_| refuse("/endpoint", "invalid_value", "not an endpoint entry"))?;
    let fields: Vec<Value> =
        aurora_config::endpoint_field_errors(&entry, aurora_config::EntrySource::Helm)
            .into_iter()
            .map(|e| field(format!("/endpoint/{}", e.field), e.code, e.code))
            .collect();
    if !fields.is_empty() {
        return Err(invalid(fields));
    }
    // The resolved endpoint (its base URL and models path) from the provider's own rules.
    let section = LlmSection {
        enabled_providers: vec![entry.provider.clone()],
        provider_preference: Vec::new(),
        endpoints: vec![entry],
        // (C-812) Tests and development: a provider's fixed origin may be replaced by a loopback mock.
        dev_upstream_overrides: state
            .settings
            .snapshot()
            .effective
            .raw
            .llm
            .dev_upstream_overrides
            .clone(),
    };
    let registry = aurora_llm::ProviderRegistry::from_config(&section)
        .map_err(|e| refuse("/endpoint", "invalid_value", &e.to_string()))?;
    let ep = registry
        .endpoints()
        .first()
        .ok_or_else(|| refuse("/endpoint", "invalid_value", "not an endpoint entry"))?;
    let (base, path) = probe_target(&registry, ep);
    let url = reqwest::Url::parse(&format!("{}{path}", base.trim_end_matches('/')))
        .map_err(|_| refuse("/endpoint/base_url", "base_url_invalid", "not a URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(refuse(
            "/endpoint/base_url",
            "base_url_invalid",
            "http or https, without credentials",
        ));
    }
    let env = &state.connect;
    // Names are judged by the client's resolver at connect time; a literal never reaches it.
    if let Some(host) = url.host_str() {
        let bare = host.trim_matches(['[', ']']);
        if let Ok(ip) = bare.parse::<IpAddr>()
            && !env.allow.load().permits(ip, bare)
        {
            return Ok(failed("destination_refused", None));
        }
    }
    let sent = env
        .http_instance
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(env.probe_timeout())
        .send()
        .await;
    let mut resp = match sent {
        Ok(r) => r,
        Err(e) => return Ok(failed(classify_send_error(&e), None)),
    };
    let status = resp.status().as_u16();
    let mut body = Vec::new();
    if resp.status().is_success() {
        while body.len() < BODY_MAX {
            match resp.chunk().await {
                Ok(Some(c)) => body.extend_from_slice(&c),
                _ => break,
            }
        }
    }
    // Any answer below 500 proves the endpoint is there (a 401 is the expected one without a key).
    if status >= 500 {
        return Ok(failed("unexpected_status", Some(status)));
    }
    Ok(EndpointTestResult {
        reachable: true,
        status: Some(status),
        error_code: None,
        models: models_of(&body),
    })
}

/// (C-803) What the probe is a `GET` of, per provider: the provider's own base (its fixed or
/// regional host, an Azure resource, a Bedrock VPC endpoint, the entry's `base_url`) and a path
/// that answers without a credential (401 and 403 count as reachable).
fn probe_target(
    registry: &aurora_llm::ProviderRegistry,
    ep: &aurora_llm::Endpoint,
) -> (String, String) {
    let base = registry
        .provider_for(ep)
        .map_or_else(|| ep.base_url.clone(), |p| p.base_url(ep));
    let path = match ep.provider.as_str() {
        "azure_openai" => "/openai/v1/models".to_owned(),
        "bedrock" => "/".to_owned(),
        "vertex" => format!(
            "/v1/projects/{}/locations/{}",
            ep.project_id.as_deref().unwrap_or_default(),
            ep.location.as_deref().unwrap_or_default()
        ),
        // openai, anthropic: `/v1/models`; litellm and openai_compatible: the entry's models path.
        _ => ep.models_path.as_deref().unwrap_or("/v1/models").to_owned(),
    };
    (base, path)
}

fn failed(code: &str, status: Option<u16>) -> EndpointTestResult {
    EndpointTestResult {
        reachable: false,
        status,
        error_code: Some(code.to_owned()),
        models: None,
    }
}

/// Model ids of an OpenAI-style list (`{"data":[{"id":…}]}`); `None` for anything else.
fn models_of(body: &[u8]) -> Option<Vec<String>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let list = v.get("data")?.as_array()?;
    let ids: Vec<String> = list
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty() && id.len() <= 200)
        .take(MODELS_MAX)
        .map(str::to_owned)
        .collect();
    Some(ids)
}

#[cfg(test)]
mod tests {
    use super::models_of;

    #[test]
    fn models_are_read_from_an_openai_style_list_only() {
        assert_eq!(
            models_of(br#"{"data":[{"id":"a"},{"id":"b"},{"x":1}]}"#),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(models_of(b"not json"), None);
        assert_eq!(models_of(br#"{"models":[]}"#), None);
    }
}

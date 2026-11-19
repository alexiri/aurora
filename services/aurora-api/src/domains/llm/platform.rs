//! `PlatformLlm` (P4-EVAL-3; docs/08 §Platform-internal LLM calls, C-869, SEC-115): the one place
//! the API itself calls a model provider with a workspace's LLM secret. Purposes are explicit (an
//! enum; `eval_judge` only so far). A call is refused, with no request sent, unless the secret is
//! active, granted to the agent, admitted by governance for its endpoint and scope and usable per
//! readiness; the request is one user message with the platform's own instructions (no streaming,
//! no tools, no agent text beyond what the caller passes), under a timeout and an output cap, and
//! goes through the shared `llm_http` client to the endpoint's configured base URL only.
//! There is no fallback key, provider or retry. The key value and the request headers are never
//! logged, audited or returned; a failure carries a code and nothing else.
//!
//! Supported: providers whose key goes into one plain header (`Bearer`, `x-api-key` or a named
//! header; `api_key` mode) and that speak Anthropic Messages (`/v1/messages`) or OpenAI chat
//! completions (`/v1/chat/completions`). Bedrock (SigV4), Vertex, `aws_assume_role` and Azure Entra
//! need a signature or a mint and are refused as `unavailable` (and at settings time as not usable).

use crate::app::AppState;
use crate::domains::hosted::model::{self, Reason};
use crate::domains::secrets::{gov, service as secrets};
use aurora_db::repo::agents::AgentRow;
use aurora_llm::{AuthMode, Dialect, InjectionHeader, ModelRef};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// What an internal call is for. Later purposes are added here, explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    EvalJudge,
}

/// One request: the platform's instructions and one user message. Nothing else.
pub struct Request {
    pub model: String,
    pub system: &'static str,
    pub user: String,
    pub max_tokens: u32,
    pub timeout: Duration,
}

/// A completion: the text and its token counts (summed by the caller; never a turn's usage).
#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Why a call produced no completion. Each is a refusal before any request, except
/// `Unavailable`/`Timeout`/`Unparseable` which concern the request itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The secret is not granted to the agent.
    NotGranted,
    /// Governance does not admit the endpoint, scope or model.
    NotPermitted,
    /// The secret or its endpoint is not usable (not active, origin unverified, …).
    NotUsable,
    /// Transport failure, a non-2xx answer, an unsupported provider or an unreadable body.
    Unavailable,
    Timeout,
}

impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotGranted => "not_granted",
            Self::NotPermitted => "not_permitted",
            Self::NotUsable => "not_usable",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
        }
    }
}

/// The dialect and path `PlatformLlm` speaks to this endpoint with a key of `mode`, or `None` when
/// it cannot call it (a key that needs a signature or a mint, or no dialect it knows). Also asked
/// when an eval's judge key is chosen, so a key that could never be called is refused then.
pub fn supported(
    provider: &dyn aurora_llm::LlmProvider,
    ep: &aurora_llm::Endpoint,
    mode: AuthMode,
) -> Option<(Dialect, &'static str)> {
    if mode != AuthMode::ApiKey || !provider.injection_for(ep, &mode).header.is_plain() {
        return None;
    }
    let dialects = provider.dialects(ep);
    if dialects.contains(&Dialect::AnthropicMessages) {
        Some((Dialect::AnthropicMessages, "/v1/messages"))
    } else if dialects.contains(&Dialect::OpenaiChat) {
        Some((Dialect::OpenaiChat, "/v1/chat/completions"))
    } else {
        None
    }
}

/// Cap on a provider's answer body read.
const BODY_MAX: usize = 256 * 1024;

fn internal(e: impl std::fmt::Display) -> Failure {
    tracing::warn!(error = %e, "platform llm call could not be prepared");
    Failure::Unavailable
}

/// Run `req` for `purpose` with `secret`, on behalf of `agent` (the agent the grant is checked for).
///
/// # Errors
/// A [`Failure`]; no fallback is tried.
pub async fn complete(
    s: &AppState,
    purpose: Purpose,
    agent: &AgentRow,
    secret: Uuid,
    req: Request,
) -> Result<Completion, Failure> {
    // 1-4. checks, in the order of docs/08 §Model readiness.
    let llm = s.llm();
    let harness = agent
        .harness
        .clone()
        .unwrap_or_else(|| s.ops().hosted.default_harness.clone());
    let mut conn = s.pool.acquire().await.map_err(internal)?;
    // (D-372) Only the keys' reasons are read here: no key listing is fetched.
    let r = model::readiness_cached(
        s,
        &llm,
        &mut conn,
        agent.id,
        agent.org_id,
        agent.workspace_id,
        &harness,
    )
    .await
    .map_err(internal)?;
    let Some(key) = r.keys.iter().find(|k| k.secret.id == secret) else {
        return Err(Failure::NotGranted);
    };
    match key.reason {
        Some(Reason::ScopeNotPermitted) => return Err(Failure::NotPermitted),
        Some(_) => return Err(Failure::NotUsable),
        None => {}
    }
    let Some(ep) = key.ep else {
        return Err(Failure::NotUsable);
    };
    let eff = gov::effective_for(s, agent.org_id)
        .await
        .map_err(internal)?;
    if eff
        .model_allowed(&ep.id, &ModelRef::Model(req.model.clone()))
        .is_err()
    {
        return Err(Failure::NotPermitted);
    }
    let row = aurora_db::repo::secrets::get(&mut *conn, secret)
        .await
        .map_err(internal)?
        .filter(|r| r.status == "active" && r.kind == "llm")
        .ok_or(Failure::NotUsable)?;
    drop(conn);
    let Some(provider) = llm.provider_for(ep) else {
        return Err(Failure::NotUsable);
    };
    // The provider's own recipe; only a plain one-header key is supported here.
    let mode = row
        .llm_auth_mode
        .as_deref()
        .and_then(AuthMode::parse)
        .unwrap_or(AuthMode::ApiKey);
    let recipe = provider.injection_for(ep, &mode);
    let Some((dialect, suffix)) = supported(provider.as_ref(), ep, mode) else {
        return Err(Failure::Unavailable);
    };
    let base = provider.base_url(ep);
    let base = base.trim_end_matches('/');
    let url = format!("{base}{suffix}");
    // The destination guard: the path must be one the sanctioned key may be injected on.
    let path = url
        .parse::<http::Uri>()
        .map(|u| u.path().to_owned())
        .map_err(internal)?;
    if !llm.path_permitted(&ep.id, &http::Method::POST, &path) {
        return Err(Failure::NotPermitted);
    }
    // 5. the request. The key exists in memory only for this call.
    let key_value = secrets::open_llm_key(s, &row).await.map_err(internal)?;
    let body = request_body(
        dialect,
        &req,
        ep.provider == "openai",
        // only the purpose's fixed instructions; the caller supplies the one user message
        purpose,
    );
    let mut http_req = s
        .llm_http
        .post(&url)
        .timeout(req.timeout)
        .header(http::header::CONTENT_TYPE, "application/json")
        .json(&body);
    http_req = match recipe.header {
        InjectionHeader::Bearer => http_req.bearer_auth(key_value.expose()),
        InjectionHeader::XApiKey => http_req.header("x-api-key", key_value.expose()),
        InjectionHeader::Header(name) => http_req.header(name, key_value.expose()),
        _ => return Err(Failure::Unavailable),
    };
    for (name, value) in &recipe.set_headers {
        http_req = http_req.header(*name, value);
    }
    if dialect == Dialect::AnthropicMessages {
        http_req = http_req.header("anthropic-version", "2023-06-01");
    }
    let mut resp = http_req.send().await.map_err(|e| {
        if e.is_timeout() {
            Failure::Timeout
        } else {
            Failure::Unavailable
        }
    })?;
    if !resp.status().is_success() {
        return Err(Failure::Unavailable);
    }
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| {
        if e.is_timeout() {
            Failure::Timeout
        } else {
            Failure::Unavailable
        }
    })? {
        if bytes.len() + chunk.len() > BODY_MAX {
            return Err(Failure::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_response(dialect, &bytes).ok_or(Failure::Unavailable)
}

/// The JSON body of a request: one system instruction, one user message, an output cap; no tools
/// and no streaming.
fn request_body(dialect: Dialect, r: &Request, completion_tokens_field: bool, p: Purpose) -> Value {
    let Purpose::EvalJudge = p;
    match dialect {
        Dialect::AnthropicMessages => json!({
            "model": r.model, "max_tokens": r.max_tokens, "system": r.system,
            "messages": [{"role": "user", "content": r.user}], "stream": false,
        }),
        _ => {
            let cap = if completion_tokens_field {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            json!({
                "model": r.model, cap: r.max_tokens, "stream": false,
                "messages": [
                    {"role": "system", "content": r.system},
                    {"role": "user", "content": r.user},
                ],
            })
        }
    }
}

/// The text and usage of an answer; `None` when it has no text.
fn parse_response(dialect: Dialect, bytes: &[u8]) -> Option<Completion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let n = |p: &str| v.pointer(p).and_then(Value::as_i64).unwrap_or(0).max(0);
    if dialect == Dialect::AnthropicMessages {
        let text: String = v
            .get("content")?
            .as_array()?
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect();
        Some(Completion {
            text,
            input_tokens: n("/usage/input_tokens"),
            output_tokens: n("/usage/output_tokens"),
        })
    } else {
        let text = v
            .pointer("/choices/0/message/content")?
            .as_str()?
            .to_owned();
        Some(Completion {
            text,
            input_tokens: n("/usage/prompt_tokens"),
            output_tokens: n("/usage/completion_tokens"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_and_openai_answers_parse_with_usage() {
        let a = br#"{"content":[{"type":"text","text":"he"},{"type":"tool_use"},{"type":"text","text":"llo"}],
            "usage":{"input_tokens":7,"output_tokens":3}}"#;
        assert_eq!(
            parse_response(Dialect::AnthropicMessages, a),
            Some(Completion {
                text: "hello".into(),
                input_tokens: 7,
                output_tokens: 3
            })
        );
        let o = br#"{"choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#;
        assert_eq!(
            parse_response(Dialect::OpenaiChat, o),
            Some(Completion {
                text: "hi".into(),
                input_tokens: 4,
                output_tokens: 2
            })
        );
        assert_eq!(parse_response(Dialect::OpenaiChat, b"{}"), None);
        assert_eq!(parse_response(Dialect::AnthropicMessages, b"nope"), None);
    }

    #[test]
    fn the_request_has_no_tools_no_streaming_and_the_output_cap() {
        let r = Request {
            model: "m".into(),
            system: "sys",
            user: "u".into(),
            max_tokens: 512,
            timeout: Duration::from_secs(1),
        };
        for (d, f) in [
            (Dialect::AnthropicMessages, false),
            (Dialect::OpenaiChat, false),
            (Dialect::OpenaiChat, true),
        ] {
            let b = request_body(d, &r, f, Purpose::EvalJudge);
            assert_eq!(b["stream"], false);
            assert!(b.get("tools").is_none());
            let cap = b
                .get("max_tokens")
                .or_else(|| b.get("max_completion_tokens"));
            assert_eq!(cap, Some(&json!(512)));
        }
    }
}

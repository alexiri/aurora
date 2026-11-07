//! JSON wire format of W9 <-> WIT types.
//!
//! Field names are the snake_case WIT record fields; `kv` lists are arrays of
//! `{key, value}`; options are `null` (all fields required). `exchange.source`
//! is `{"oauth_callback": [kv]}` or `{"form": [kv]}`; the exchange output is
//! `{"connected": {...}}` or `{"continue": {"url", "continuation"}}`.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::runtime::{CallError, Invocation, Outcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    Authorize,
    Exchange,
    Mint,
    Summarize,
}

impl HookKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "connect.authorize" => Self::Authorize,
            "connect.exchange" => Self::Exchange,
            "token.mint" => Self::Mint,
            "approval.summarize" => Self::Summarize,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Authorize => "connect.authorize",
            Self::Exchange => "connect.exchange",
            Self::Mint => "token.mint",
            Self::Summarize => "approval.summarize",
        }
    }

    pub fn registry(self) -> aurora_registry::manifest::Hook {
        use aurora_registry::manifest::Hook;
        match self {
            Self::Authorize => Hook::ConnectAuthorize,
            Self::Exchange => Hook::ConnectExchange,
            Self::Mint => Hook::TokenMint,
            Self::Summarize => Hook::ApprovalSummarize,
        }
    }

    /// `connect.authorize` and `approval.summarize` never get network access.
    pub fn egress_allowed(self) -> bool {
        matches!(self, Self::Exchange | Self::Mint)
    }

    pub fn wall(self) -> std::time::Duration {
        match self {
            Self::Summarize => aurora_core::limits::HOOK_WALL_SUMMARIZE,
            _ => aurora_core::limits::HOOK_WALL,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub package_id: String,
    pub package_digest: String,
    pub app_id: String,
    pub input: Value,
}

fn bad(m: impl Into<String>) -> CallError {
    CallError::new("invalid_input", m)
}

/// Decode `input`, forcing `app_id` and dropping context keys the package did not declare.
pub fn parse_invocation(
    kind: HookKind,
    app_id: &str,
    mut input: Value,
    allowed_context: &[String],
) -> Result<Invocation, CallError> {
    let obj = input
        .as_object_mut()
        .ok_or_else(|| bad("input must be an object"))?;
    obj.insert("app_id".into(), Value::String(app_id.to_owned()));
    if let Some(Value::Array(ctx)) = obj.get_mut("context") {
        ctx.retain(|e| {
            e.get("key")
                .and_then(Value::as_str)
                .is_some_and(|k| allowed_context.iter().any(|a| a == k))
        });
    }
    if kind == HookKind::Exchange
        && let Some(Value::Object(src)) = obj.get_mut("source")
    {
        for (from, to) in [("oauth_callback", "OauthCallback"), ("form", "Form")] {
            if let Some(v) = src.remove(from) {
                src.insert(to.into(), v);
            }
        }
    }
    let e = |e: serde_json::Error| bad(format!("invalid input: {e}"));
    Ok(match kind {
        HookKind::Authorize => Invocation::Authorize(serde_json::from_value(input).map_err(e)?),
        HookKind::Exchange => Invocation::Exchange(serde_json::from_value(input).map_err(e)?),
        HookKind::Mint => Invocation::Mint(serde_json::from_value(input).map_err(e)?),
        HookKind::Summarize => Invocation::Summarize(serde_json::from_value(input).map_err(e)?),
    })
}

/// Literal secret values in the input, to be masked in hook logs.
pub fn secrets(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["credentials", "client_secret", "source", "metadata"] {
        if let Some(Value::Array(a)) = input.get(key) {
            out.extend(
                a.iter()
                    .filter_map(|e| e.get("value").and_then(Value::as_str))
                    .map(str::to_owned),
            );
        } else if let Some(Value::Object(o)) = input.get(key) {
            for v in o.values() {
                if let Value::Array(a) = v {
                    out.extend(
                        a.iter()
                            .filter_map(|e| e.get("value").and_then(Value::as_str))
                            .map(str::to_owned),
                    );
                }
            }
        }
    }
    for key in ["pkce_verifier", "continuation"] {
        if let Some(s) = input.get(key).and_then(Value::as_str) {
            out.push(s.to_owned());
        }
    }
    out
}

pub fn output_json(o: &Outcome) -> Value {
    match o {
        Outcome::Authorize(v) => json!(v),
        Outcome::Mint(v) => json!(v),
        Outcome::Summarize(v) => json!(v),
        Outcome::Exchange(crate::runtime::types::ExchangeOutput::Connected(c)) => {
            json!({ "connected": c })
        }
        Outcome::Exchange(crate::runtime::types::ExchangeOutput::Continue(c)) => {
            json!({ "continue": c })
        }
    }
}

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
    /// The connection's instance host (or the app's `default_host`): what `{instance}` in the
    /// package's egress list resolves to (D-74). Absent: `{instance}` resolves to nothing.
    #[serde(default)]
    pub instance_host: Option<String>,
    pub input: Value,
}

/// Resolve `{instance}` entries of an egress list to the connection's instance host. The host
/// must be a plain lower-case DNS name; anything else resolves to nothing (fail closed, SEC-04).
pub fn resolve_egress(list: Vec<String>, instance_host: Option<&str>) -> Vec<String> {
    let ok = instance_host.filter(|h| {
        !h.is_empty()
            && h.contains('.')
            && h.parse::<std::net::IpAddr>().is_err()
            && h.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
            && !h.starts_with(['.', '-'])
            && !h.contains("..")
    });
    list.into_iter()
        .filter_map(|e| {
            if e == "{instance}" {
                ok.map(str::to_owned)
            } else {
                Some(e)
            }
        })
        .collect()
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

#[cfg(test)]
mod instance_egress_tests {
    use super::resolve_egress;

    fn l(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// SEC-04: `{instance}` in a package's egress list is the connection's instance host only.
    #[test]
    fn instance_placeholder_resolves_to_the_connection_host_or_nothing() {
        assert_eq!(
            resolve_egress(l(&["{instance}", "auth.acme.test"]), Some("desk.acme.test")),
            l(&["desk.acme.test", "auth.acme.test"])
        );
        // Unknown instance: the placeholder resolves to nothing, static entries stay.
        assert_eq!(
            resolve_egress(l(&["{instance}", "auth.acme.test"]), None),
            l(&["auth.acme.test"])
        );
        // A malformed host never becomes an egress entry (IP, port, userinfo, wildcard, caps).
        for bad in [
            "10.0.0.1",
            "a.test:8443",
            "a@evil.test",
            "*.test",
            "Desk.ACME.test",
            "",
        ] {
            assert!(
                resolve_egress(l(&["{instance}"]), Some(bad)).is_empty(),
                "{bad}"
            );
        }
    }
}

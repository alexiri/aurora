//! Applying a secret to a request (docs/06 step 11, docs/08 §5 and §6).
//! Agent-supplied values for injected headers/params are overwritten, never merged.

use crate::resolve::{Injection, SecretCand};
use aurora_llm::InjectionHeader;
use aurora_secrets::InjectionRecipe;
use base64::{Engine, engine::general_purpose::STANDARD};
use http::{HeaderMap, HeaderName, HeaderValue, header};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// Everything except RFC 3986 unreserved characters.
pub(crate) const ENC: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Debug, PartialEq, Eq)]
pub struct InjectError;

pub struct Target<'a> {
    pub headers: &'a mut HeaderMap,
    pub path: &'a mut String,
    pub query: &'a mut Option<String>,
}

pub(crate) fn header_value(bytes: &[u8]) -> Result<HeaderValue, InjectError> {
    if bytes.iter().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
        return Err(InjectError);
    }
    let mut v = HeaderValue::from_bytes(bytes).map_err(|_| InjectError)?;
    v.set_sensitive(true);
    Ok(v)
}

fn text(value: &[u8]) -> Result<&str, InjectError> {
    std::str::from_utf8(value).map_err(|_| InjectError)
}

fn format_value(format: &str, value: &[u8]) -> Result<Vec<u8>, InjectError> {
    let mut out = Vec::with_capacity(format.len() + value.len());
    let mut rest = format;
    while let Some(i) = rest.find("{value}") {
        out.extend_from_slice(&rest.as_bytes()[..i]);
        out.extend_from_slice(value);
        rest = &rest[i + "{value}".len()..];
    }
    out.extend_from_slice(rest.as_bytes());
    Ok(out)
}

pub(crate) fn set_query(query: &mut Option<String>, name: &str, value: &str) {
    let kept: Vec<&str> = query
        .as_deref()
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let k = p.split_once('=').map_or(*p, |(k, _)| k);
            percent_decode_str(k).decode_utf8_lossy() != name
        })
        .collect();
    let mut q = kept.join("&");
    if !q.is_empty() {
        q.push('&');
    }
    q.push_str(&utf8_percent_encode(name, ENC).to_string());
    q.push('=');
    q.push_str(&utf8_percent_encode(value, ENC).to_string());
    *query = Some(q);
}

/// Replace the remainder of the first segment starting with `prefix` by `enc`
/// (already percent-encoded).
pub(crate) fn replace_prefixed_segment(path: &mut String, prefix: &str, enc: &str) {
    let bare = prefix.strip_prefix('/').unwrap_or(prefix);
    let mut done = false;
    let segs: Vec<String> = path
        .split('/')
        .map(|s| {
            if !done && !s.is_empty() && s.starts_with(bare) {
                done = true;
                format!("{bare}{enc}")
            } else {
                s.to_owned()
            }
        })
        .collect();
    *path = segs.join("/");
}

/// Shortest injected value that is searched for in a response; shorter ones would match by
/// chance.
const ECHO_MIN: usize = 4;

/// (FIX-104, SEC-01) The forms of every injected value that ended up in the request URL (path or
/// query, raw or percent-encoded): an upstream that echoes the URL must not hand them back to the
/// agent. Values that are not in the URL (headers, bodies) are not searched for.
pub(crate) fn url_needles<'a>(
    values: impl IntoIterator<Item = &'a [u8]>,
    path: &str,
    query: Option<&str>,
) -> Vec<Vec<u8>> {
    let url = format!("{path}?{}", query.unwrap_or(""));
    let mut out: Vec<Vec<u8>> = Vec::new();
    for v in values.into_iter().filter(|v| v.len() >= ECHO_MIN) {
        let enc = utf8_percent_encode(&String::from_utf8_lossy(v), ENC).to_string();
        let raw = String::from_utf8_lossy(v).into_owned();
        if url.contains(&enc) || url.contains(&raw) {
            for n in [v.to_vec(), enc.into_bytes()] {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// Whether `hay` contains any of `needles`.
pub(crate) fn echoes(needles: &[Vec<u8>], hay: &[u8]) -> bool {
    needles
        .iter()
        .any(|n| !n.is_empty() && hay.windows(n.len()).any(|w| w == n.as_slice()))
}

pub fn apply(cand: &SecretCand, value: &[u8], t: Target<'_>) -> Result<(), InjectError> {
    apply_value(cand, value, false, t)
}

/// (P4-LLM-2, C-806) Inject a credential the mint service produced (`value` is the minted token, never the stored key
/// file): `gcp_token` and `entra_token` recipes become `Authorization: Bearer`. A recipe that is not minted, or a value
/// that is not, is refused.
pub fn apply_minted(cand: &SecretCand, token: &[u8], t: Target<'_>) -> Result<(), InjectError> {
    apply_value(cand, token, true, t)
}

fn apply_value(
    cand: &SecretCand,
    value: &[u8],
    minted: bool,
    t: Target<'_>,
) -> Result<(), InjectError> {
    match &cand.injection {
        Injection::Llm(l) => {
            // Strip every agent-supplied auth header, then set the provider's.
            t.headers.remove(header::AUTHORIZATION);
            t.headers.remove("x-api-key");
            t.headers.remove("api-key");
            for h in &l.strip_request_headers {
                t.headers.remove(*h);
            }
            // (C-806) Prefix families (`x-amz-*`, `x-goog-*`) are removed whole, never merged.
            if !l.strip_request_prefixes.is_empty() {
                let drop: Vec<HeaderName> = t
                    .headers
                    .keys()
                    .filter(|k| {
                        l.strip_request_prefixes
                            .iter()
                            .any(|p| k.as_str().starts_with(p))
                    })
                    .cloned()
                    .collect();
                for k in drop {
                    t.headers.remove(k);
                }
            }
            for (name, value) in &l.set_headers {
                t.headers.insert(
                    HeaderName::from_static(name),
                    header_value(value.as_bytes())?,
                );
            }
            // A recipe that needs a mint or a signature is not implemented yet (P4-LLM-2..4): refuse, never send the
            // request without the credential (the caller answers 502 `credential_unavailable`).
            let bearer_from_mint = minted
                && matches!(
                    l.header,
                    InjectionHeader::GcpToken | InjectionHeader::EntraToken
                );
            if !l.header.is_plain() && !bearer_from_mint {
                return Err(InjectError);
            }
            if minted && !bearer_from_mint {
                return Err(InjectError);
            }
            match l.header {
                InjectionHeader::Header(name) => {
                    t.headers
                        .insert(HeaderName::from_static(name), header_value(value)?);
                }
                InjectionHeader::AwsSigV4 | InjectionHeader::AwsAssumeRole => {
                    return Err(InjectError);
                }
                InjectionHeader::Bearer
                | InjectionHeader::GcpToken
                | InjectionHeader::EntraToken => {
                    let mut v = b"Bearer ".to_vec();
                    v.extend_from_slice(value);
                    t.headers.insert(header::AUTHORIZATION, header_value(&v)?);
                }
                InjectionHeader::XApiKey => {
                    t.headers
                        .insert(HeaderName::from_static("x-api-key"), header_value(value)?);
                }
            }
            Ok(())
        }
        Injection::Recipe(r) => match r {
            InjectionRecipe::Bearer => {
                let mut v = b"Bearer ".to_vec();
                v.extend_from_slice(value);
                t.headers.insert(header::AUTHORIZATION, header_value(&v)?);
                Ok(())
            }
            InjectionRecipe::Header { name, format } => {
                let n = HeaderName::from_bytes(name.as_bytes()).map_err(|_| InjectError)?;
                t.headers
                    .insert(n, header_value(&format_value(format, value)?)?);
                Ok(())
            }
            InjectionRecipe::Query { name, format } => {
                let v = format_value(format, value)?;
                set_query(t.query, name, text(&v)?);
                Ok(())
            }
            InjectionRecipe::Basic { username } => {
                let mut raw = username.as_bytes().to_vec();
                raw.push(b':');
                raw.extend_from_slice(value);
                let v = format!("Basic {}", STANDARD.encode(raw));
                t.headers
                    .insert(header::AUTHORIZATION, header_value(v.as_bytes())?);
                Ok(())
            }
            InjectionRecipe::PathTemplate { prefix } => {
                let enc = utf8_percent_encode(text(value)?, ENC).to_string();
                replace_prefixed_segment(t.path, prefix, &enc);
                Ok(())
            }
            InjectionRecipe::PathRegex {
                pattern,
                replacement,
            } => {
                let re = regex::Regex::new(pattern).map_err(|_| InjectError)?;
                let enc = utf8_percent_encode(text(value)?, ENC).to_string();
                let repl = replacement.replace("{value}", &enc);
                let out = re.replace(t.path.as_str(), regex::NoExpand(&repl));
                *t.path = out.into_owned();
                Ok(())
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::SecretCand;
    use aurora_policy::facts::{Level, SecretKind};
    use aurora_secrets::SecretRef;
    use uuid::Uuid;

    fn cand(r: InjectionRecipe) -> SecretCand {
        SecretCand {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            workspace_id: None,
            name: "s".into(),
            scope: Level::Workspace,
            kind: SecretKind::Generic,
            path: None,
            claim_port: None,
            injection: Injection::Recipe(r),
            vref: SecretRef::ext("m", "p", None, "1").unwrap(),
            version: 1,
            llm_endpoint: None,
            provider: None,
            origin: None,
        }
    }

    fn run(
        r: InjectionRecipe,
        v: &[u8],
        path: &str,
        q: Option<&str>,
    ) -> (HeaderMap, String, Option<String>) {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer agent".parse().unwrap());
        let (mut p, mut qq) = (path.to_owned(), q.map(str::to_owned));
        apply(
            &cand(r),
            v,
            Target {
                headers: &mut h,
                path: &mut p,
                query: &mut qq,
            },
        )
        .unwrap();
        (h, p, qq)
    }

    #[test]
    fn recipes() {
        let (h, ..) = run(InjectionRecipe::Bearer, b"k", "/", None);
        assert_eq!(h.get_all(header::AUTHORIZATION).iter().count(), 1);
        assert_eq!(h[header::AUTHORIZATION], "Bearer k");
        let (h, ..) = run(
            InjectionRecipe::Basic {
                username: "u".into(),
            },
            b"p",
            "/",
            None,
        );
        assert_eq!(
            h[header::AUTHORIZATION],
            format!("Basic {}", STANDARD.encode("u:p"))
        );
        let (h, ..) = run(
            InjectionRecipe::Header {
                name: "X-Api-Key".into(),
                format: "t-{value}".into(),
            },
            b"k",
            "/",
            None,
        );
        assert_eq!(h["x-api-key"], "t-k");
        let (_, _, q) = run(
            InjectionRecipe::Query {
                name: "api_key".into(),
                format: "{value}".into(),
            },
            b"a b&c",
            "/",
            Some("api_key=agent&x=1"),
        );
        assert_eq!(q.as_deref(), Some("x=1&api_key=a%20b%26c"));
        let (_, p, _) = run(
            InjectionRecipe::PathTemplate {
                prefix: "/bot".into(),
            },
            b"12:A/B",
            "/botPLACEHOLDER/send",
            None,
        );
        assert_eq!(p, "/bot12%3AA%2FB/send");
        let (_, p, _) = run(
            InjectionRecipe::PathRegex {
                pattern: "^/v1/([^/]+)/".into(),
                replacement: "/v1/{value}/".into(),
            },
            b"k$1",
            "/v1/x/y",
            None,
        );
        assert_eq!(p, "/v1/k%241/y");
    }

    #[test]
    fn rejects_crlf() {
        let mut h = HeaderMap::new();
        let (mut p, mut q) = ("/".to_owned(), None);
        let r = apply(
            &cand(InjectionRecipe::Bearer),
            b"a\r\nb",
            Target {
                headers: &mut h,
                path: &mut p,
                query: &mut q,
            },
        );
        assert_eq!(r, Err(InjectError));
    }

    fn llm_cand(i: aurora_llm::LlmInjection) -> SecretCand {
        SecretCand {
            kind: SecretKind::Llm,
            injection: Injection::Llm(i),
            ..cand(InjectionRecipe::Bearer)
        }
    }

    fn agent_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("authorization", "Bearer agent-key"),
            ("x-api-key", "agent-key"),
            ("api-key", "agent-key"),
            ("openai-organization", "org-evil"),
            ("openai-project", "proj-evil"),
            ("x-amz-security-token", "agent-token"),
            ("x-goog-api-key", "agent-key"),
            ("x-aurora-keep", "yes"),
        ] {
            h.insert(HeaderName::from_static(k), v.parse().unwrap());
        }
        h
    }

    fn apply_llm(i: aurora_llm::LlmInjection) -> (HeaderMap, Result<(), InjectError>) {
        let mut h = agent_headers();
        let (mut p, mut q) = ("/v1/chat/completions".to_owned(), None);
        let r = apply(
            &llm_cand(i),
            b"sk-real",
            Target {
                headers: &mut h,
                path: &mut p,
                query: &mut q,
            },
        );
        (h, r)
    }

    /// (P4-LLM-1, C-806) Every agent-supplied credential header is removed, never merged; the provider's own is set.
    #[test]
    fn llm_recipes_replace_every_agent_credential_header() {
        use aurora_llm::{InjectionHeader as H, LlmInjection};
        // openai: Bearer, the configured organization and project replace the agent's
        let mut i = LlmInjection::new(
            H::Bearer,
            &[
                "authorization",
                "x-api-key",
                "api-key",
                "openai-organization",
                "openai-project",
            ],
        );
        i.set_headers = vec![("openai-organization", "org-acme".into())];
        let (h, r) = apply_llm(i);
        r.unwrap();
        assert_eq!(h["authorization"], "Bearer sk-real");
        assert_eq!(h["openai-organization"], "org-acme");
        assert!(
            h.get("openai-project").is_none()
                && h.get("x-api-key").is_none()
                && h.get("api-key").is_none()
        );
        assert_eq!(h["x-aurora-keep"], "yes");
        // compatible with `auth_header = "api-key"`
        let (h, r) = apply_llm(LlmInjection::new(
            H::Header("api-key"),
            &["authorization", "x-api-key", "api-key"],
        ));
        r.unwrap();
        assert_eq!(h["api-key"], "sk-real");
        assert!(h.get("authorization").is_none() && h.get("x-api-key").is_none());
        // compatible with `auth_header = "x-api-key"`
        let (h, r) = apply_llm(LlmInjection::new(
            H::XApiKey,
            &["authorization", "x-api-key", "api-key"],
        ));
        r.unwrap();
        assert_eq!(h["x-api-key"], "sk-real");
        assert!(h.get("authorization").is_none() && h.get("api-key").is_none());
        // prefix families go whole
        let mut i = LlmInjection::new(H::Bearer, &["authorization"]);
        i.strip_request_prefixes = vec!["x-amz-", "x-goog-"];
        let (h, r) = apply_llm(i);
        r.unwrap();
        assert!(h.get("x-amz-security-token").is_none() && h.get("x-goog-api-key").is_none());
    }

    /// A minted or signed recipe that no lane implements yet refuses (502 `credential_unavailable`) and never lets
    /// the agent's own credential through.
    #[test]
    fn unimplemented_llm_recipes_refuse() {
        use aurora_llm::{InjectionHeader as H, LlmInjection};
        for h in [H::AwsSigV4, H::AwsAssumeRole, H::GcpToken, H::EntraToken] {
            let (_, r) = apply_llm(LlmInjection::new(h, &["authorization"]));
            assert_eq!(r, Err(InjectError), "{h:?}");
        }
    }
}

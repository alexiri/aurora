//! Applying a secret to a request (docs/06 step 11, docs/08 §5 and §6).
//! Agent-supplied values for injected headers/params are overwritten, never merged.

use crate::resolve::{Injection, SecretCand};
use aurora_llm::InjectionHeader;
use aurora_secrets::InjectionRecipe;
use base64::{Engine, engine::general_purpose::STANDARD};
use http::{HeaderMap, HeaderName, HeaderValue, header};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// Everything except RFC 3986 unreserved characters.
const ENC: &AsciiSet = &NON_ALPHANUMERIC
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

fn header_value(bytes: &[u8]) -> Result<HeaderValue, InjectError> {
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

fn set_query(query: &mut Option<String>, name: &str, value: &str) {
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

pub fn apply(cand: &SecretCand, value: &[u8], t: Target<'_>) -> Result<(), InjectError> {
    match &cand.injection {
        Injection::Llm(l) => {
            // Strip every agent-supplied auth header, then set the provider's.
            t.headers.remove(header::AUTHORIZATION);
            t.headers.remove("x-api-key");
            for h in &l.strip_request_headers {
                t.headers.remove(*h);
            }
            match l.header {
                InjectionHeader::Bearer => {
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
                let bare = prefix.strip_prefix('/').unwrap_or(prefix);
                let mut done = false;
                let segs: Vec<String> = t
                    .path
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
                *t.path = segs.join("/");
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
            injection: Injection::Recipe(r),
            vref: SecretRef::ext("m", "p", None, "1").unwrap(),
            version: 1,
            llm_endpoint: None,
            provider: None,
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
}

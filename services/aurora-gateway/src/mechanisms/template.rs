//! Template rendering with escaping by destination (docs/09 §4).

use super::Unavailable;
use aurora_registry::template::{Ns, Part, Template};
use percent_encoding::utf8_percent_encode;
use std::collections::BTreeSet;

/// Where the rendered text ends up; decides how substituted values are escaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// Header value: CR/LF/NUL in the result abort the request.
    Header,
    /// Query value: substituted values are percent-encoded.
    Query,
    /// Path: substituted values are percent-encoded (per segment, so `/` is encoded).
    Path,
    /// Hostname: substituted values must be DNS characters.
    Host,
    /// Used as-is (basic components, signing keys); the caller decides.
    Raw,
}

/// Values a template may reference for one request.
pub struct Vars<'a> {
    pub credentials: &'a serde_json::Map<String, serde_json::Value>,
    /// Declared credential names that may be absent (`optional: true`).
    pub optional: &'a BTreeSet<&'a str>,
    pub metadata: &'a serde_json::Value,
    pub tenant_host: Option<&'a str>,
    /// `permissions.context` of the package.
    pub allowed_context: &'a [String],
    pub context: &'a std::collections::HashMap<String, String>,
}

pub struct Rendered {
    pub text: String,
    /// A referenced value was empty (matters for `bearer`/`basic`).
    pub had_empty: bool,
}

fn value_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn lookup(ns: Ns, name: &str, v: &Vars<'_>) -> Result<String, Unavailable> {
    match ns {
        Ns::Credentials => match v.credentials.get(name).and_then(value_str) {
            Some(s) => Ok(s),
            None if v.optional.contains(name) => Ok(String::new()),
            None => Err(Unavailable),
        },
        Ns::Metadata => Ok(v.metadata.get(name).and_then(value_str).unwrap_or_default()),
        Ns::Connection if name == "tenant_host" => {
            v.tenant_host.map(str::to_owned).ok_or(Unavailable)
        }
        Ns::Context if v.allowed_context.iter().any(|c| c == name) => {
            v.context.get(name).cloned().ok_or(Unavailable)
        }
        _ => Err(Unavailable),
    }
}

fn dns_ok(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        && !s.starts_with('.')
        && !s.contains("..")
}

pub fn render(t: &Template, dest: Dest, v: &Vars<'_>) -> Result<Rendered, Unavailable> {
    let mut text = String::new();
    let mut had_empty = false;
    for p in t.parts() {
        match p {
            Part::Lit(l) => text.push_str(l),
            Part::Ref(r) => {
                let val = lookup(r.ns, &r.name, v)?;
                had_empty |= val.is_empty();
                match dest {
                    Dest::Query | Dest::Path => {
                        text.push_str(&utf8_percent_encode(&val, crate::inject::ENC).to_string());
                    }
                    Dest::Host => {
                        if !dns_ok(&val) {
                            return Err(Unavailable);
                        }
                        text.push_str(&val);
                    }
                    Dest::Header | Dest::Raw => text.push_str(&val),
                }
            }
        }
    }
    if dest == Dest::Header && text.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
        return Err(Unavailable);
    }
    Ok(Rendered { text, had_empty })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vars<'a>(
        c: &'a serde_json::Map<String, serde_json::Value>,
        o: &'a BTreeSet<&'a str>,
        m: &'a serde_json::Value,
        x: &'a std::collections::HashMap<String, String>,
    ) -> Vars<'a> {
        Vars {
            credentials: c,
            optional: o,
            metadata: m,
            tenant_host: Some("acme.example.com"),
            allowed_context: &[],
            context: x,
        }
    }

    #[test]
    fn escaping_by_destination() {
        let c = json!({"t": "a b/c", "bad": "x\r\ny", "h": "ex.com"});
        let c = c.as_object().unwrap();
        let (o, m) = (BTreeSet::from(["opt"]), json!({}));
        let x = std::collections::HashMap::new();
        let v = vars(c, &o, &m, &x);
        let t = |s| Template::parse(s).unwrap();
        let r = render(&t("k={credentials.t}"), Dest::Query, &v).unwrap();
        assert_eq!(r.text, "k=a%20b%2Fc");
        let r = render(&t("/p/{credentials.t}"), Dest::Path, &v).unwrap();
        assert_eq!(r.text, "/p/a%20b%2Fc");
        assert!(render(&t("{credentials.bad}"), Dest::Header, &v).is_err());
        assert!(render(&t("{credentials.t}.x.com"), Dest::Host, &v).is_err());
        let r = render(&t("{credentials.h}"), Dest::Host, &v).unwrap();
        assert_eq!(r.text, "ex.com");
        // Optional absent -> empty; undeclared absent -> error.
        let r = render(&t("{credentials.opt}"), Dest::Raw, &v).unwrap();
        assert!(r.had_empty && r.text.is_empty());
        assert!(render(&t("{credentials.nope}"), Dest::Raw, &v).is_err());
    }
}

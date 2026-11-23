//! (FIX-233, C-1159, D-476) A sign-in app's own value in the path of a fixed OAuth URL.
//!
//! `https://login.microsoftonline.com/{client.tenant}/oauth2/v2.0/token`: the placeholder fills
//! one whole path segment with the client's non-secret field (else the field's `default`). The
//! host stays literal, so token hosts, egress and claims are decided by the manifest alone.
use super::ClientField;
use serde_json::{Map, Value};

/// The opening of a client-field placeholder.
pub const CLIENT_REF: &str = "{client.";

/// Whether `v` may fill a path segment: 1–253 characters of `[A-Za-z0-9.-]`, starting and ending
/// with a letter or digit, without `..` (a GUID, a domain name, `common`).
pub fn segment_ok(v: &str) -> bool {
    let b = v.as_bytes();
    (1..=253).contains(&b.len())
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && !v.contains("..")
}

/// One placeholder: the field name and the byte range of `{client.<name>}` in the URL.
struct Ref<'a> {
    name: &'a str,
    start: usize,
    end: usize,
}

/// Every `{client.<name>}` of `u`, in order; `Err` names the first malformed one.
fn refs(u: &str) -> Result<Vec<Ref<'_>>, String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = u[from..].find(CLIENT_REF) {
        let start = from + i;
        let rest = &u[start + CLIENT_REF.len()..];
        let close = rest
            .find('}')
            .ok_or_else(|| "unclosed `{client.` placeholder".to_owned())?;
        let name = &rest[..close];
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err(format!("`{{client.{name}}}` is not a field name"));
        }
        let end = start + CLIENT_REF.len() + close + 1;
        out.push(Ref { name, start, end });
        from = end;
    }
    Ok(out)
}

/// Whether `u` uses a client-field placeholder at all.
pub fn has_client_refs(u: &str) -> bool {
    u.contains(CLIENT_REF)
}

/// The field names `u` places in its path (empty when malformed or none).
pub fn client_ref_names(u: &str) -> Vec<&str> {
    refs(u)
        .map(|r| r.into_iter().map(|r| r.name).collect())
        .unwrap_or_default()
}

/// The manifest rule for a fixed OAuth URL with client-field placeholders (09 §3.3): an `https`
/// URL with a literal host; every placeholder is a whole path segment and names a declared,
/// non-secret field with a `default` that is itself a valid segment; no other braces.
/// Returns the problems found (empty when the URL is acceptable).
pub fn check_client_url(u: &str, fields: &[ClientField]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(rest) = u.strip_prefix("https://") else {
        return vec!["must be https".into()];
    };
    let path_at = "https://".len() + rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let rs = match refs(u) {
        Ok(r) => r,
        Err(e) => return vec![e],
    };
    let mut stripped = String::new();
    let mut last = 0;
    for r in &rs {
        stripped.push_str(&u[last..r.start]);
        last = r.end;
        if r.start < path_at {
            out.push(format!(
                "`{{client.{}}}` must be in the path, not the host",
                r.name
            ));
            continue;
        }
        let before = u[..r.start].ends_with('/');
        let after = u[r.end..].is_empty() || u[r.end..].starts_with(['/', '?', '#']);
        if !before || !after {
            out.push(format!(
                "`{{client.{}}}` must be a whole path segment",
                r.name
            ));
        }
        match fields.iter().find(|f| f.name == r.name) {
            None => out.push(format!("`{{client.{}}}` names no declared field", r.name)),
            Some(f) if f.secret => {
                out.push(format!("`{{client.{}}}` names a secret field", r.name));
            }
            Some(f) => match f.default.as_deref() {
                None => out.push(format!(
                    "field `{}` used in a URL needs a `default`",
                    r.name
                )),
                Some(d) if !segment_ok(d) => out.push(format!(
                    "the default of field `{}` is not a valid path segment",
                    r.name
                )),
                Some(_) => {}
            },
        }
    }
    stripped.push_str(&u[last..]);
    if stripped.contains(['{', '}']) {
        out.push("only `{client.<field>}` placeholders are allowed here".into());
    }
    out
}

/// Fill every `{client.<name>}` of `u` with the client's stored public value, else the field's
/// `default`. `None` (fail closed) when a name is not a declared non-secret field, has neither
/// value nor default, or the value is not a valid segment. A URL without placeholders is
/// returned unchanged.
pub fn fill_client_url(
    u: &str,
    fields: &[ClientField],
    public: &Map<String, Value>,
) -> Option<String> {
    let rs = refs(u).ok()?;
    let mut out = String::with_capacity(u.len());
    let mut last = 0;
    for r in &rs {
        let f = fields.iter().find(|f| f.name == r.name && !f.secret)?;
        let v = public
            .get(r.name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .or(f.default.as_deref())?;
        if !segment_ok(v) {
            return None;
        }
        out.push_str(&u[last..r.start]);
        out.push_str(v);
        last = r.end;
    }
    out.push_str(&u[last..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields() -> Vec<ClientField> {
        vec![
            ClientField {
                name: "client_id".into(),
                label: "Client ID".into(),
                secret: false,
                default: None,
            },
            ClientField {
                name: "client_secret".into(),
                label: "Secret".into(),
                secret: true,
                default: None,
            },
            ClientField {
                name: "tenant".into(),
                label: "Directory (tenant) ID".into(),
                secret: false,
                default: Some("common".into()),
            },
        ]
    }

    const URL: &str = "https://login.microsoftonline.com/{client.tenant}/oauth2/v2.0/token";

    #[test]
    fn segments() {
        for ok in [
            "common",
            "c80d3499-4a40-4a8c-986e-abce017d6b19",
            "cern.onmicrosoft.com",
            "a",
        ] {
            assert!(segment_ok(ok), "{ok}");
        }
        for bad in [
            "", "..", "a..b", ".a", "a.", "-a", "a/b", "a?b", "a#b", "a b", "a%2f", "a@b", "a:b",
            "é",
        ] {
            assert!(!segment_ok(bad), "{bad}");
        }
        assert!(!segment_ok(&"a".repeat(254)));
    }

    #[test]
    fn fill_uses_the_value_then_the_default() {
        let f = fields();
        assert_eq!(
            fill_client_url(URL, &f, &Map::new()).as_deref(),
            Some("https://login.microsoftonline.com/common/oauth2/v2.0/token")
        );
        let public = json!({"tenant": "c80d3499-4a40-4a8c-986e-abce017d6b19"});
        assert_eq!(
            fill_client_url(URL, &f, public.as_object().unwrap()).as_deref(),
            Some(
                "https://login.microsoftonline.com/c80d3499-4a40-4a8c-986e-abce017d6b19/oauth2/v2.0/token"
            )
        );
        // A blank value falls back to the default; a bad one fails closed.
        let blank = json!({"tenant": "  "});
        assert!(
            fill_client_url(URL, &f, blank.as_object().unwrap())
                .unwrap()
                .contains("/common/")
        );
        for bad in ["evil.com/x", "..", "a?b", "x@evil.com"] {
            let p = json!({ "tenant": bad });
            assert_eq!(
                fill_client_url(URL, &f, p.as_object().unwrap()),
                None,
                "{bad}"
            );
        }
        // A secret field never fills a URL; an undeclared one fails closed.
        let s = "https://h.example/{client.client_secret}/t";
        assert_eq!(fill_client_url(s, &f, &Map::new()), None);
        assert_eq!(
            fill_client_url("https://h.example/{client.nope}/t", &f, &Map::new()),
            None
        );
        // No placeholder: unchanged.
        assert_eq!(
            fill_client_url("https://h.example/t", &f, &Map::new()).as_deref(),
            Some("https://h.example/t")
        );
    }

    #[test]
    fn check_rules() {
        let f = fields();
        assert!(check_client_url(URL, &f).is_empty());
        assert!(check_client_url("https://h.example/a/{client.tenant}", &f).is_empty());
        assert!(check_client_url("https://h.example/{client.tenant}?x=1", &f).is_empty());
        let bad = [
            ("http://h.example/{client.tenant}/t", "https"),
            ("https://{client.tenant}.example/t", "not the host"),
            ("https://h.example/x{client.tenant}/t", "whole path segment"),
            ("https://h.example/{client.tenant}x/t", "whole path segment"),
            ("https://h.example/{client.client_secret}/t", "secret"),
            (
                "https://h.example/{client.client_id}/t",
                "needs a `default`",
            ),
            ("https://h.example/{client.nope}/t", "no declared field"),
            (
                "https://h.example/{client.tenant}/{connection.x}",
                "only `{client",
            ),
            ("https://h.example/{client.Tenant}/t", "not a field name"),
            ("https://h.example/{client.tenant", "unclosed"),
        ];
        for (u, want) in bad {
            let got = check_client_url(u, &f);
            assert!(got.iter().any(|m| m.contains(want)), "{u}: {got:?}");
        }
        let mut f2 = fields();
        f2[2].default = Some("a/b".into());
        assert!(
            check_client_url(URL, &f2)
                .iter()
                .any(|m| m.contains("not a valid path segment"))
        );
    }
}

//! Parameter-level tool matching (phase 4, P4-GRANTS, Q8, D-124, SEC-68; docs/09 §3.4 tools).
//!
//! A tool may carry a `params` list: 1–4 AND-ed conditions on one request parameter each. The
//! classifier **fails closed**: a condition holds only when its parameter is provably there,
//! exactly once, in a source that is safe to read, with a value that equals / is in / starts with
//! the declared one. Anything else (missing, repeated, conflicting, undecodable, compressed or
//! truncated) makes the condition *unknown*, the tool does not match, and classification falls
//! through to the next tool or to `tool = None`. Doubt therefore never yields a `read` tool.
//!
//! Credentials, cookies and injected headers are never sources: `header` conditions are limited
//! to [`ALLOWED_HEADERS`].
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};

/// Most conditions on one tool.
pub const MAX_CONDITIONS: usize = 4;
/// Longest parameter name.
pub const NAME_MAX: usize = 64;
/// Longest matched value.
pub const VALUE_MAX: usize = 128;
/// Longest `in` list.
pub const IN_MAX: usize = 32;
/// Shortest `prefix` value.
pub const PREFIX_MIN: usize = 4;
/// Headers a `header` condition may read (phase 4). Never `authorization`, `cookie`, `proxy-*`,
/// `x-api-key` or an injected header.
pub const ALLOWED_HEADERS: [&str; 1] = ["x-amz-target"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamSource {
    Query,
    Form,
    Header,
}

/// Exactly one of `equals`, `in`, `prefix` (the linter proves it; the matcher treats any other
/// shape as never matching).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

impl ParamMatch {
    /// How many of the three forms are set (a valid match has exactly one).
    pub fn forms(&self) -> usize {
        usize::from(self.equals.is_some())
            + usize::from(self.any_of.is_some())
            + usize::from(self.prefix.is_some())
    }

    fn accepts(&self, v: &str) -> bool {
        if self.forms() != 1 {
            return false;
        }
        match (&self.equals, &self.any_of, &self.prefix) {
            (Some(e), None, None) => v == e,
            (None, Some(l), None) => l.iter().any(|x| x == v),
            (None, None, Some(p)) => !p.is_empty() && v.starts_with(p.as_str()),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamCondition {
    pub source: ParamSource,
    pub name: String,
    #[serde(rename = "match")]
    pub matcher: ParamMatch,
}

/// The request parts a condition may read. `headers` carries every request header (any case).
#[derive(Debug, Clone, Copy)]
pub struct ParamInput<'a> {
    pub query: Option<&'a str>,
    pub headers: &'a [(&'a str, &'a str)],
    /// The buffered body prefix (empty when none was buffered).
    pub body: &'a [u8],
    /// The body is longer than what was buffered.
    pub truncated: bool,
}

impl ParamInput<'_> {
    /// No parameters known: every condition is unknown (used by lanes that have no HTTP request).
    pub const NONE: ParamInput<'static> = ParamInput {
        query: None,
        headers: &[],
        body: &[],
        truncated: false,
    };
}

/// A `form` condition needs the request body.
pub fn needs_body(conds: &[ParamCondition]) -> bool {
    conds.iter().any(|c| c.source == ParamSource::Form)
}

/// All conditions hold (and every one is knowable). Empty list: never (a `params` tool has 1–4).
pub fn matches(conds: &[ParamCondition], input: &ParamInput<'_>) -> bool {
    !conds.is_empty()
        && conds.len() <= MAX_CONDITIONS
        && conds.iter().all(|c| outcome(c, input) == Outcome::Match)
}

/// Why a condition holds or not (the linter's fixture requirements are stated on these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The parameter is there exactly once and the value is accepted.
    Match,
    /// The parameter is there exactly once with a value the condition does not accept.
    OtherValue,
    Missing,
    Repeated,
    /// A `form` parameter that is also in the query.
    Conflict,
    /// A `form` condition on a body that is not urlencoded UTF-8 (wrong or missing content type,
    /// other charset, invalid UTF-8 or escape, unreadable header).
    WrongContentType,
    /// A `form` condition on a compressed body.
    Compressed,
    /// A `form` condition on a body longer than the buffer.
    Truncated,
}

/// The state of one condition against a request. Anything but [`Outcome::Match`] means the tool
/// does not match (fail closed).
pub fn outcome(c: &ParamCondition, input: &ParamInput<'_>) -> Outcome {
    let value = match c.source {
        ParamSource::Query => match input.query {
            Some(q) => once(q.as_bytes(), &c.name, false),
            None => Err(Outcome::Missing),
        },
        ParamSource::Header => header_once(input.headers, &c.name),
        ParamSource::Form => form_value(&c.name, input),
    };
    match value {
        Ok(v) if c.matcher.accepts(&v) => Outcome::Match,
        Ok(_) => Outcome::OtherValue,
        Err(o) => o,
    }
}

fn header_once(headers: &[(&str, &str)], name: &str) -> Result<String, Outcome> {
    if !ALLOWED_HEADERS.iter().any(|h| h.eq_ignore_ascii_case(name)) {
        return Err(Outcome::WrongContentType);
    }
    let mut it = headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name));
    let Some((_, v)) = it.next() else {
        return Err(Outcome::Missing);
    };
    if it.next().is_some() {
        return Err(Outcome::Repeated);
    }
    Ok((*v).to_owned())
}

fn is_urlencoded(content_type: &str) -> bool {
    let mut parts = content_type.split(';');
    let media = parts.next().unwrap_or("").trim();
    if !media.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        return false;
    }
    let mut rest = parts.map(str::trim).filter(|p| !p.is_empty());
    match (rest.next(), rest.next()) {
        (None, _) => true,
        (Some(p), None) => p.split_once('=').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("charset")
                && v.trim().trim_matches('"').eq_ignore_ascii_case("utf-8")
        }),
        _ => false,
    }
}

fn form_value(name: &str, input: &ParamInput<'_>) -> Result<String, Outcome> {
    // The body must be a complete, uncompressed, urlencoded UTF-8 document.
    if input.truncated {
        return Err(Outcome::Truncated);
    }
    let mut ct = input
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    let Some((_, ct_val)) = ct.next() else {
        return Err(Outcome::WrongContentType);
    };
    if ct.next().is_some() || !is_urlencoded(ct_val) {
        return Err(Outcome::WrongContentType);
    }
    if input.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-encoding")
            && !v.trim().is_empty()
            && !v.trim().eq_ignore_ascii_case("identity")
    }) {
        return Err(Outcome::Compressed);
    }
    if std::str::from_utf8(input.body).is_err() {
        return Err(Outcome::WrongContentType);
    }
    // A form parameter also present in the query is a conflict, even with the same value.
    if let Some(q) = input.query {
        match once(q.as_bytes(), name, false) {
            Err(Outcome::Missing) => {}
            _ => return Err(Outcome::Conflict),
        }
    }
    once(input.body, name, true)
}

/// The decoded value of `name` when it occurs exactly once and everything is decodable.
/// A pair whose name cannot be decoded might have been ours: doubt (`WrongContentType`).
fn once(raw: &[u8], name: &str, plus_is_space: bool) -> Result<String, Outcome> {
    let mut found: Option<String> = None;
    for (k, v) in pairs(raw) {
        let Some(k) = decode(k, plus_is_space) else {
            return Err(Outcome::WrongContentType);
        };
        if k == name {
            if found.is_some() {
                return Err(Outcome::Repeated);
            }
            found = Some(decode(v, plus_is_space).ok_or(Outcome::WrongContentType)?);
        }
    }
    found.ok_or(Outcome::Missing)
}

fn pairs(raw: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    raw.split(|b| *b == b'&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.iter().position(|b| *b == b'=') {
            Some(i) => {
                let (k, rest) = p.split_at(i);
                (k, rest.get(1..).unwrap_or_default())
            }
            None => (p, <&[u8]>::default()),
        })
}

/// Percent-decodes once; `None` for a malformed escape or a result that is not valid UTF-8.
fn decode(raw: &[u8], plus_is_space: bool) -> Option<String> {
    let s = std::str::from_utf8(raw).ok()?;
    // A `%` must start a complete escape: lenient decoders would pass it through, hiding doubt.
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let ok = b.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit);
            if !ok {
                return None;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    let s = if plus_is_space {
        std::borrow::Cow::Owned(s.replace('+', " "))
    } else {
        std::borrow::Cow::Borrowed(s)
    };
    percent_decode_str(&s)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(source: ParamSource, name: &str, m: ParamMatch) -> ParamCondition {
        ParamCondition {
            source,
            name: name.to_owned(),
            matcher: m,
        }
    }
    fn eq(v: &str) -> ParamMatch {
        ParamMatch {
            equals: Some(v.to_owned()),
            ..Default::default()
        }
    }
    fn prefix(v: &str) -> ParamMatch {
        ParamMatch {
            prefix: Some(v.to_owned()),
            ..Default::default()
        }
    }
    fn one_of(v: &[&str]) -> ParamMatch {
        ParamMatch {
            any_of: Some(v.iter().map(|s| (*s).to_owned()).collect()),
            ..Default::default()
        }
    }
    const FORM: (&str, &str) = ("Content-Type", "application/x-www-form-urlencoded");
    fn form_input<'a>(
        query: Option<&'a str>,
        headers: &'a [(&'a str, &'a str)],
        body: &'a str,
        truncated: bool,
    ) -> ParamInput<'a> {
        ParamInput {
            query,
            headers,
            body: body.as_bytes(),
            truncated,
        }
    }
    fn f(m: ParamMatch) -> Vec<ParamCondition> {
        vec![cond(ParamSource::Form, "Action", m)]
    }

    #[test]
    fn equals_prefix_in_on_form() {
        let h = [FORM];
        let i = form_input(None, &h, "Action=DescribeInstances&Version=1", false);
        assert!(matches(&f(eq("DescribeInstances")), &i));
        assert!(!matches(&f(eq("Describe")), &i));
        assert!(matches(&f(prefix("Describe")), &i));
        assert!(!matches(&f(prefix("RunIn")), &i));
        assert!(matches(&f(one_of(&["A", "DescribeInstances"])), &i));
        assert!(!matches(&f(one_of(&["A", "B"])), &i));
        // case differences never match
        assert!(!matches(&f(eq("describeinstances")), &i));
    }

    #[test]
    fn fail_closed_form_cases() {
        let c = f(prefix("Describe"));
        let ok = [FORM];
        // missing
        assert!(!matches(&c, &form_input(None, &ok, "Version=1", false)));
        // repeated
        assert!(!matches(
            &c,
            &form_input(None, &ok, "Action=DescribeX&Action=RunInstances", false)
        ));
        // query/body conflict, same or different value
        assert!(!matches(
            &c,
            &form_input(Some("Action=DescribeX"), &ok, "Action=DescribeX", false)
        ));
        assert!(!matches(
            &c,
            &form_input(Some("Action=RunInstances"), &ok, "Action=DescribeX", false)
        ));
        // other content type, none
        let json = [("content-type", "application/json")];
        assert!(!matches(
            &c,
            &form_input(None, &json, "Action=DescribeX", false)
        ));
        assert!(!matches(
            &c,
            &form_input(None, &[], "Action=DescribeX", false)
        ));
        // charset other than utf-8
        let l1 = [(
            "content-type",
            "application/x-www-form-urlencoded; charset=latin1",
        )];
        assert!(!matches(
            &c,
            &form_input(None, &l1, "Action=DescribeX", false)
        ));
        let u8h = [(
            "content-type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )];
        assert!(matches(
            &c,
            &form_input(None, &u8h, "Action=DescribeX", false)
        ));
        // compressed
        let gz = [FORM, ("content-encoding", "gzip")];
        assert!(!matches(
            &c,
            &form_input(None, &gz, "Action=DescribeX", false)
        ));
        let id = [FORM, ("content-encoding", "identity")];
        assert!(matches(
            &c,
            &form_input(None, &id, "Action=DescribeX", false)
        ));
        // truncated
        assert!(!matches(
            &c,
            &form_input(None, &ok, "Action=DescribeX", true)
        ));
        // invalid UTF-8 after decoding, or in the raw body
        assert!(!matches(
            &c,
            &form_input(None, &ok, "Action=Describe%FF", false)
        ));
        let raw = ParamInput {
            query: None,
            headers: &ok,
            body: b"Action=Describe\xff",
            truncated: false,
        };
        assert!(!matches(&c, &raw));
        // malformed escape in an unrelated name is doubt too
        assert!(!matches(
            &c,
            &form_input(None, &ok, "Bad%zz=1&Action=DescribeX", false)
        ));
        // another name merely containing the name does not count
        assert!(matches(
            &c,
            &form_input(None, &ok, "XAction=Run&Action=DescribeX", false)
        ));
    }

    #[test]
    fn percent_and_plus_handling() {
        let ok = [FORM];
        let c = f(eq("Describe Instances"));
        assert!(matches(
            &c,
            &form_input(None, &ok, "Action=Describe+Instances", false)
        ));
        assert!(matches(
            &c,
            &form_input(None, &ok, "Action=Describe%20Instances", false)
        ));
        // a percent-encoded name is decoded once
        let c = f(eq("GetCallerIdentity"));
        assert!(matches(
            &c,
            &form_input(None, &ok, "%41ction=GetCallerIdentity", false)
        ));
        // double encoding is not decoded twice
        assert!(!matches(
            &c,
            &form_input(None, &ok, "Action=Get%2543allerIdentity", false)
        ));
    }

    #[test]
    fn query_source() {
        let c = vec![cond(ParamSource::Query, "Action", eq("GetCallerIdentity"))];
        let i = |q| ParamInput {
            query: q,
            ..ParamInput::NONE
        };
        assert!(matches(
            &c,
            &i(Some("Action=GetCallerIdentity&Version=2011-06-15"))
        ));
        assert!(!matches(&c, &i(Some("Action=AssumeRole"))));
        assert!(!matches(
            &c,
            &i(Some("Action=GetCallerIdentity&Action=AssumeRole"))
        ));
        assert!(!matches(&c, &i(None)));
        assert!(!matches(&c, &i(Some("action=GetCallerIdentity"))));
    }

    #[test]
    fn header_source() {
        let c = vec![cond(
            ParamSource::Header,
            "x-amz-target",
            prefix("DynamoDB_20120810.Query"),
        )];
        let h1 = [("X-Amz-Target", "DynamoDB_20120810.Query")];
        let i = |h| ParamInput {
            headers: h,
            ..ParamInput::NONE
        };
        assert!(matches(&c, &i(&h1)));
        let dup = [h1[0], ("x-amz-target", "DynamoDB_20120810.DeleteTable")];
        assert!(!matches(&c, &i(&dup)));
        let other = [("x-amz-target", "DynamoDB_20120810.DeleteTable")];
        assert!(!matches(&c, &i(&other)));
        assert!(!matches(&c, &i(&[])));
        // a header outside the allow-list never matches
        let c = vec![cond(ParamSource::Header, "authorization", prefix("Bearer"))];
        assert!(!matches(&c, &i(&[("authorization", "Bearer abc")])));
    }

    #[test]
    fn all_conditions_and_empty_never_match() {
        let ok = [FORM];
        let c = vec![
            cond(ParamSource::Form, "Action", eq("A1ction")),
            cond(ParamSource::Form, "Version", eq("1")),
        ];
        assert!(matches(
            &c,
            &form_input(None, &ok, "Action=A1ction&Version=1", false)
        ));
        assert!(!matches(
            &c,
            &form_input(None, &ok, "Action=A1ction&Version=2", false)
        ));
        assert!(!matches(&[], &form_input(None, &ok, "", false)));
    }
}

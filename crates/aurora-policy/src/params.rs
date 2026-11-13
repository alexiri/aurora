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

/// A `form` condition reads the request body; a `query` condition must prove the body does not
/// carry the same parameter (SEC-68), so both need it buffered.
pub fn needs_body(conds: &[ParamCondition]) -> bool {
    conds
        .iter()
        .any(|c| matches!(c.source, ParamSource::Form | ParamSource::Query))
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
            // SEC-68: servers merge the query and an urlencoded body, so a request that has a
            // body must prove the parameter is not also there, or it does not match.
            Some(q) => once(q.as_bytes(), &c.name, false).and_then(|v| {
                query_without_body_conflict(&c.name, input)?;
                Ok(v)
            }),
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

/// A body is present (buffered bytes, or longer than the buffer).
fn has_body(input: &ParamInput<'_>) -> bool {
    !input.body.is_empty() || input.truncated
}

/// For a `query` condition: with no body there is nothing to conflict; with a body it must be a
/// complete, uncompressed, urlencoded UTF-8 document that does not carry the parameter at all.
/// A body that cannot be checked (other type, compressed, oversized) is doubt: no match.
fn query_without_body_conflict(name: &str, input: &ParamInput<'_>) -> Result<(), Outcome> {
    if !has_body(input) {
        return Ok(());
    }
    if input.truncated {
        return Err(Outcome::Truncated);
    }
    let mut ct = input
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    match (ct.next(), ct.next()) {
        (Some((_, v)), None) if is_urlencoded(v) => {}
        _ => return Err(Outcome::WrongContentType),
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
    match once(input.body, name, true) {
        Err(Outcome::Missing) => Ok(()),
        _ => Err(Outcome::Conflict),
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
    // (FIX-085, SEC-68) A raw `;` is a pair separator for some servers (the older query-string
    // grammar) and an ordinary value character for others, so one pair for us may be two for them
    // (`Action=Describe;Action=Terminate`). Whichever the upstream does, we cannot know: doubt.
    // `%3B` is exact and stays an ordinary character.
    if raw.contains(&b';') {
        return Err(Outcome::WrongContentType);
    }
    let mut found: Option<String> = None;
    let mut spellings = 0_usize;
    for (k, v) in pairs(raw) {
        let Some(k) = decode(k, plus_is_space) else {
            return Err(Outcome::WrongContentType);
        };
        // (FIX-085) Names are compared without case: a server that folds case sees `ACTION` and
        // `Action` as one parameter given twice. The value is taken only from the exact spelling;
        // two spellings are repeated, and a lone variant is doubt, never "absent": a caller that
        // needs the parameter absent (the query-versus-body check) must not read it as absent.
        if k.eq_ignore_ascii_case(name) {
            spellings += 1;
            if spellings > 1 {
                return Err(Outcome::Repeated);
            }
            if k == name {
                found = Some(decode(v, plus_is_space).ok_or(Outcome::WrongContentType)?);
            }
        }
    }
    match (found, spellings) {
        (Some(v), _) => Ok(v),
        (None, 0) => Err(Outcome::Missing),
        (None, _) => Err(Outcome::Repeated),
    }
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
    } else if s.contains('+') {
        // Query strings: most servers read `+` as a space, others keep it. Whichever the
        // upstream does, we cannot know, so a literal `+` is encoding doubt (`%2B` is exact).
        return None;
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
    fn a_plus_in_a_query_is_encoding_doubt_and_never_matches() {
        let i = |q| ParamInput {
            query: q,
            ..ParamInput::NONE
        };
        for expected in ["A B", "A+B"] {
            let c = vec![cond(ParamSource::Query, "Action", eq(expected))];
            assert!(!matches(&c, &i(Some("Action=A+B"))), "{expected}");
            let c = vec![cond(ParamSource::Query, "Action", one_of(&[expected]))];
            assert!(!matches(&c, &i(Some("Action=A+B"))), "{expected}");
        }
        // a `+` in a name (it might have been ours) is doubt too; an unrelated value is not read
        let c = vec![cond(ParamSource::Query, "Action", eq("A"))];
        assert!(!matches(&c, &i(Some("Action=A&x+y=1"))));
        assert!(!matches(&c, &i(Some("Ac+tion=A"))));
        assert!(matches(&c, &i(Some("Action=A&x=1+2"))));
        // the exact encodings still work
        assert!(matches(&c, &i(Some("Action=A&x=1%2B2"))));
        let c = vec![cond(ParamSource::Query, "Action", eq("A B"))];
        assert!(matches(&c, &i(Some("Action=A%20B"))));
        let c = vec![cond(ParamSource::Query, "Action", eq("A+B"))];
        assert!(matches(&c, &i(Some("Action=A%2BB"))));
    }

    /// FIX-085 (SEC-68): a raw `;` is a pair separator for some servers, so the body
    /// `Action=Describe...;Action=Terminate...` may carry two actions; case variants of the
    /// name are the same parameter to a server that folds case. All of it is doubt.
    #[test]
    fn semicolons_and_case_variants_of_the_name_are_doubt() {
        let c = f(prefix("Describe"));
        let h = [FORM];
        let m = |body: &str| matches(&c, &form_input(None, &h, body, false));
        assert!(m("Action=DescribeInstances&Version=1"), "control");
        // The differential the review named: one `Action` for us, two for a `;` splitter.
        assert!(!m(
            "Action=DescribeInstances;Action=TerminateInstances&Version=1"
        ));
        assert!(!m("Version=1;Action=DescribeInstances"));
        assert!(!m("Action=DescribeInstances&Note=a;b"));
        // An encoded semicolon is exact and stays an ordinary character.
        assert!(m("Action=DescribeInstances&Note=a%3Bb"));
        // The exact spelling plus a case variant: one parameter given twice.
        assert_eq!(
            outcome(
                &c[0],
                &form_input(
                    None,
                    &h,
                    "ACTION=TerminateInstances&Action=DescribeInstances",
                    false
                )
            ),
            Outcome::Repeated
        );
        assert!(!m("Action=DescribeInstances&action=TerminateInstances"));
        // A lone variant is not our parameter: no match (and not a match by folding either).
        assert!(!m("ACTION=DescribeInstances"));
        // In the query, and in the query-versus-body conflict check.
        let q = vec![cond(ParamSource::Query, "Action", prefix("Describe"))];
        let qi = |query: &str| matches(&q, &form_input(Some(query), &[], "", false));
        assert!(qi("Action=DescribeInstances"));
        assert!(!qi("Action=DescribeInstances;Action=TerminateInstances"));
        assert!(!qi("Action=DescribeInstances&ACTION=TerminateInstances"));
        let with_body = form_input(
            Some("Action=DescribeInstances"),
            &h,
            "ACTION=Terminate;x=1",
            false,
        );
        assert!(!matches(&q, &with_body));
        let with_variant = form_input(
            Some("Action=DescribeInstances"),
            &h,
            "ACTION=TerminateInstances",
            false,
        );
        assert!(
            !matches(&q, &with_variant),
            "a case variant in the body may be the same parameter"
        );
    }

    #[test]
    fn query_source_fails_closed_on_a_body() {
        let c = vec![cond(ParamSource::Query, "Action", eq("GetCallerIdentity"))];
        let q = Some("Action=GetCallerIdentity");
        let ok = [FORM];
        // no body: matches; an unrelated urlencoded body: matches
        assert!(matches(&c, &form_input(q, &ok, "", false)));
        assert!(matches(&c, &form_input(q, &ok, "Version=1", false)));
        // the same parameter in the body, different or equal value: conflict
        assert!(!matches(
            &c,
            &form_input(q, &ok, "Action=AssumeRole", false)
        ));
        assert!(!matches(
            &c,
            &form_input(q, &ok, "Action=GetCallerIdentity", false)
        ));
        // body that cannot be checked: other type, none, compressed, oversized, invalid UTF-8
        let json = [("content-type", "application/json")];
        assert!(!matches(&c, &form_input(q, &json, "{}", false)));
        assert!(!matches(&c, &form_input(q, &[], "x=1", false)));
        let gz = [FORM, ("content-encoding", "gzip")];
        assert!(!matches(&c, &form_input(q, &gz, "x=1", false)));
        assert!(!matches(&c, &form_input(q, &ok, "x=1", true)));
        assert!(!matches(&c, &form_input(q, &ok, "Bad%zz=1", false)));
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

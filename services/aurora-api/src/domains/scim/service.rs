//! SCIM service core: settings validation, e-mail rules, the filter subset and pagination
//! (`16 §W15`). Resource handlers live in [`super::resource_users`] and
//! [`super::resource_groups`]; deprovisioning effects in [`super::deprovision`].

use super::auth::OrgScim;
use super::error::{ScimError, ScimResult};
use aurora_core::limits;
use aurora_core::sanitize;
use chrono::Utc;
use uuid::Uuid;

/// One SCIM request acting through a token (the audit actor of every change).
pub fn actor(token_name: &str) -> crate::audit::AuditActor {
    crate::audit::AuditActor {
        kind: "service",
        id: None,
        label: format!("scim:{token_name}"),
        source: "api",
        ip: None,
    }
}

/// The `metadata.via` of every SCIM audit row.
pub const VIA: &str = "scim";

/// Syntactically valid, ≤ 254 chars (the same rule as invitations, `04`): no whitespace or
/// control characters, exactly one `@` with non-empty local part and domain with a dot.
pub fn valid_email(e: &str) -> bool {
    e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control())
        && e.contains('@')
        && !e.starts_with('@')
        && !e.ends_with('@')
        && e.rsplit_once('@').is_some_and(|(l, d)| {
            !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')
        })
}

/// The org's e-mail must sit in the org's domains *and* the instance allow-list
/// (`04 §SCIM` Enabling and tokens / Users step 1).
pub fn email_domain_allowed(org: &OrgScim, instance_domains: &[String], email: &str) -> bool {
    org.domain_allowed(email) && OrgScim::instance_domain_allowed(instance_domains, email)
}

/// Sanitized and truncated stored names (`display_name`, `given_name`, `family_name`: ≤ 120
/// in the table, ≤ 80 for a global `users.display_name`).
pub fn clean_name(v: Option<&str>, max: usize) -> Option<String> {
    let v = v?;
    let s = sanitize::one_line(v, max);
    if s.is_empty() {
        return None;
    }
    Some(s)
}

/// Sanitized `externalId` (≤ 256; the table CHECK enforces it too).
pub fn clean_external_id(v: Option<&str>) -> ScimResult<Option<String>> {
    match v {
        None => Ok(None),
        Some(s) => {
            if s.chars().count() > 256 {
                return Err(ScimError::invalid_value(
                    "externalId must be at most 256 characters.",
                ));
            }
            if s.chars().any(|c| c.is_control()) {
                return Err(ScimError::invalid_value(
                    "externalId must not contain control characters.",
                ));
            }
            Ok(Some(s.to_owned()))
        }
    }
}

/// `startIndex`/`count` with the clamps of `16 §W15` (default 50, max 100; `count = 0` is
/// legal and returns only `totalResults`).
pub fn page_params(start_index: Option<u64>, count: Option<u64>) -> ScimResult<(u64, usize)> {
    let start = start_index.unwrap_or(1).max(1);
    let count = match count {
        Some(c) => (c as usize).min(limits::SCIM_PAGE_MAX),
        None => limits::SCIM_PAGE_DEFAULT,
    };
    Ok((start, count))
}

/// The filter subset: one `<attr> eq "<value>"`, case-insensitive attribute name, the value
/// unquoted (SCIM also allows bare tokens); case-sensitive except `userName`, e-mail and
/// `displayName`. Everything else is 400 `invalidFilter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    UserName(String),
    ExternalId(String),
    Id(Uuid),
    Email(String),
    DisplayName(String),
}

impl Filter {
    /// Case-insensitive comparisons (users' `userName`/e-mail and `displayName`).
    pub fn ci(&self) -> bool {
        matches!(
            self,
            Filter::UserName(_) | Filter::Email(_) | Filter::DisplayName(_)
        )
    }
}

const SINGLE_EQ: &str = "Only a single `attr eq \"value\"` filter is supported.";

/// Tokenize `attrPath SP compareOp SP compValue` (RFC 7644 §3.4.2.2). The operator and
/// parenthesis restrictions apply to the *structure*: a quoted value may contain anything
/// (`"R and D"`, `"Ops (EU)"`, `"a\"b@x.io"`). `compValue` is a JSON string (JSON escape
/// rules), or a bare token (`true`/`false`/`null`/a number, or — leniently, as before — an
/// unquoted or single-quoted string without whitespace). Anything after the value (`or …`,
/// `and …`), a grouping, a value filter (`emails[…]`) or a second operator is 400.
fn tokenize_filter(raw: &str) -> ScimResult<(&str, String)> {
    let bad = || ScimError::invalid_filter(SINGLE_EQ);
    // attrPath: ATTRNAME *("." ATTRNAME), optionally URI-prefixed; no grouping or brackets.
    let attr_end = raw.find(char::is_whitespace).ok_or_else(bad)?;
    let attr = &raw[..attr_end];
    if attr.is_empty()
        || !attr
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '_' | '-' | '$'))
    {
        return Err(bad());
    }
    let rest = raw[attr_end..].trim_start();
    let op_end = rest.find(char::is_whitespace).ok_or_else(bad)?;
    let op = &rest[..op_end];
    if !op.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(bad());
    }
    if !op.eq_ignore_ascii_case("eq") {
        return Err(ScimError::invalid_filter("Unsupported filter operator."));
    }
    let rest = rest[op_end..].trim_start();
    let (value, tail) = if rest.starts_with('"') {
        // A JSON string: escapes (`\"`, `\\`, `\uXXXX`) follow JSON rules.
        let mut it = serde_json::Deserializer::from_str(rest).into_iter::<String>();
        let v = match it.next() {
            Some(Ok(v)) => v,
            _ => {
                return Err(ScimError::invalid_filter(
                    "The filter value is not a valid quoted string.",
                ));
            }
        };
        (v, &rest[it.byte_offset()..])
    } else if let Some(body) = rest.strip_prefix('\'') {
        // Lenient single quotes (no escapes), accepted before RFC parsing as well.
        let end = body.find('\'').ok_or_else(|| {
            ScimError::invalid_filter("The filter value is not a valid quoted string.")
        })?;
        (body[..end].to_owned(), &body[end + 1..])
    } else {
        // A bare token: `true`, `false`, `null`, a number, or a lenient unquoted string.
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let tok = &rest[..end];
        if tok.contains(['"', '\'', '(', ')', '[', ']']) {
            return Err(bad());
        }
        (tok.to_owned(), &rest[end..])
    };
    if !tail.trim().is_empty() {
        return Err(bad());
    }
    Ok((attr, value))
}

pub fn parse_filter(raw: &str) -> ScimResult<Option<Filter>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let (attr, value) = tokenize_filter(raw)?;
    let value = value.as_str();
    if value.is_empty() {
        return Err(ScimError::invalid_filter("The filter value is empty."));
    }
    match attr.to_ascii_lowercase().as_str() {
        "username" => Ok(Some(Filter::UserName(value.to_owned()))),
        "externalid" => Ok(Some(Filter::ExternalId(value.to_owned()))),
        "id" => {
            let id = Uuid::parse_str(value)
                .map_err(|_| ScimError::invalid_filter("`id` must be a UUID."))?;
            Ok(Some(Filter::Id(id)))
        }
        "emails.value" | "emails" => Ok(Some(Filter::Email(value.to_owned()))),
        "displayname" => Ok(Some(Filter::DisplayName(value.to_owned()))),
        _ => Err(ScimError::invalid_filter(
            "The filter attribute is not supported.",
        )),
    }
}

/// `now` in RFC 3339 for `meta.created` / `meta.lastModified`.
pub fn rfc3339(t: chrono::DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The unsupported *input* attributes of a User body (`16 §W15`): refused with 400
/// `invalidValue` so an IdP cannot believe it set a role or a password.
pub const USER_REFUSED_ATTRS: &[&str] = &[
    "roles",
    "entitlements",
    "password",
    "x509Certificates",
    "groups",
];

/// The unsupported *input* attributes of a Group body.
pub const GROUP_REFUSED_ATTRS: &[&str] = &["roles", "entitlements"];

/// Does the body (at any depth, but only at the attribute level) carry one of the refused
/// attributes with a non-null value?
pub fn refused_attr(body: &serde_json::Value, attrs: &[&'static str]) -> Option<&'static str> {
    let obj = body.as_object()?;
    for a in attrs {
        if let Some(v) = obj.get(*a)
            && !v.is_null()
        {
            return Some(a);
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn ok(raw: &str) -> Filter {
        parse_filter(raw).unwrap().expect("a filter")
    }

    fn invalid(raw: &str) {
        let e = parse_filter(raw).expect_err(raw);
        assert_eq!(e.status, axum::http::StatusCode::BAD_REQUEST, "{raw}");
        assert_eq!(
            e.scim_type,
            super::super::error::ScimType::InvalidFilter,
            "{raw}"
        );
    }

    /// FIX-062: operators and parentheses inside a quoted value are data, not structure.
    #[test]
    fn quoted_values_carry_operators_and_parentheses() {
        assert_eq!(
            ok(r#"displayName eq "R and D""#),
            Filter::DisplayName("R and D".into())
        );
        assert_eq!(
            ok(r#"displayName eq "Ops (EU)""#),
            Filter::DisplayName("Ops (EU)".into())
        );
        assert_eq!(
            ok(r#"userName eq "a or b""#),
            Filter::UserName("a or b".into())
        );
        assert_eq!(
            ok(r#"userName eq "a\"b@x.io""#),
            Filter::UserName("a\"b@x.io".into())
        );
        assert_eq!(
            ok(r#"displayName eq "café \\ not pr""#),
            Filter::DisplayName("café \\ not pr".into())
        );
        // Case-insensitive attribute and operator; surrounding whitespace is fine.
        assert_eq!(
            ok(r#"  USERNAME   EQ   "ann@x.io"  "#),
            Filter::UserName("ann@x.io".into())
        );
        assert_eq!(
            ok(r#"emails.value eq "bob@x.io""#),
            Filter::Email("bob@x.io".into())
        );
    }

    /// The lenient forms accepted before the tokenizer still parse.
    #[test]
    fn bare_and_single_quoted_values_still_parse() {
        assert_eq!(
            ok("userName eq ann@x.io"),
            Filter::UserName("ann@x.io".into())
        );
        assert_eq!(
            ok("displayName eq 'Ops (EU)'"),
            Filter::DisplayName("Ops (EU)".into())
        );
        assert_eq!(ok("externalId eq 42"), Filter::ExternalId("42".into()));
        let id = Uuid::nil();
        assert_eq!(ok(&format!("id eq \"{id}\"")), Filter::Id(id));
    }

    /// Structure beyond one `attr eq value` is still refused.
    #[test]
    fn compound_and_unsupported_filters_stay_invalid() {
        for f in [
            r#"userName eq "x" or 1 eq 1"#,
            r#"userName eq "x" and displayName eq "y""#,
            r#"not (userName eq "x")"#,
            r#"(userName eq "x")"#,
            r#"emails[type eq "work"].value eq "x""#,
            r#"userName sw "a""#,
            r#"userName ne "a""#,
            "userName pr",
            r#"userName eq "unterminated"#,
            r#"userName eq "a"b"#,
            r#"userName eq """#,
            r#"title eq "x""#,
            r#"id eq "not-a-uuid""#,
            "userName eq x)",
        ] {
            invalid(f);
        }
        assert!(parse_filter("   ").unwrap().is_none());
    }
}

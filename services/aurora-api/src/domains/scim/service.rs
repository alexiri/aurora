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

pub fn parse_filter(raw: &str) -> ScimResult<Option<Filter>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    // Reject every operator/structure beyond one `eq`: parentheses, and/or/not, comparison
    // operators, and attribute paths (`name.givenName eq`).
    let lower = raw.to_ascii_lowercase();
    for op in [
        " and ", " or ", " not ", " ne ", " co ", " sw ", " ew ", " pr ", " gt ", " lt ", " ge ",
        " le ", " ca ", " st ", " ew(", "pr)", "and(", "or(",
    ] {
        if lower.contains(op) || raw.contains(['(', ')']) {
            return Err(ScimError::invalid_filter(
                "Only a single `attr eq \"value\"` filter is supported.",
            ));
        }
    }
    let (attr, value) = raw.split_once(char::is_whitespace).ok_or_else(|| {
        ScimError::invalid_filter("Only a single `attr eq \"value\"` filter is supported.")
    })?;
    let rest = value.trim();
    let Some((op, value)) = rest.split_once(char::is_whitespace) else {
        return Err(ScimError::invalid_filter(
            "Only a single `attr eq \"value\"` filter is supported.",
        ));
    };
    if !op.eq_ignore_ascii_case("eq") {
        return Err(ScimError::invalid_filter("Unsupported filter operator."));
    }
    let value = value.trim();
    // Strip quotes (single or double) when present.
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    if value.is_empty() {
        return Err(ScimError::invalid_filter("The filter value is empty."));
    }
    match attr.trim().to_ascii_lowercase().as_str() {
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

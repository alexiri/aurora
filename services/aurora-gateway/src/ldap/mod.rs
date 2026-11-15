//! (phase 5, P5-CERN-ADDONS, D-251; `docs/06 §LDAP lane`) The gateway LDAP lane: a package's tool
//! requests classify on the synthetic host `<app id>.ldap.aurora.internal` like any HTTP request
//! (grants, rules, approvals, rate limits and the request log are unchanged); an admitted request
//! is not forwarded but translated into one LDAP search against the server the approved manifest
//! names — LDAPS, certificate verified, anonymous bind, search only — and answered as JSON.
pub mod ber;
pub mod client;
pub mod filter;

use std::collections::BTreeSet;

use aurora_core::ApiError;
use aurora_registry::manifest::{LDAP_HOST_SUFFIX, Ldap};
use http::{HeaderValue, Response, StatusCode, header};
use serde::Deserialize;

use crate::{Body, full_body};
use client::{Lane, LdapError, Query, Target};

/// Largest request body the lane reads.
pub const MAX_REQUEST_BODY: usize = 16 * 1024;

#[must_use]
pub fn is_ldap_host(host: &str) -> bool {
    host.len() > LDAP_HOST_SUFFIX.len() && host.to_ascii_lowercase().ends_with(LDAP_HOST_SUFFIX)
}

/// `POST /search` body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchBody {
    pub filter: String,
    #[serde(default)]
    pub attributes: Option<Vec<String>>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

fn invalid(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "ldap_request_invalid", msg)
}

/// Whether `base` is `root` or lies below it (case-insensitive, spaces around separators
/// ignored; an escaped comma never counts as a separator).
#[must_use]
pub fn under(base: &str, root: &str) -> bool {
    let norm = |s: &str| -> String {
        let mut out = String::new();
        let mut esc = false;
        for c in s.trim().chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                out.push(c);
                esc = true;
            } else if c == ' ' && (out.ends_with(',') || out.is_empty()) {
                // drop spaces after a separator
            } else {
                out.push(c.to_ascii_lowercase());
            }
        }
        out.replace(" ,", ",")
    };
    let (b, r) = (norm(base), norm(root));
    if b == r {
        return true;
    }
    let Some(head) = b.strip_suffix(&r) else {
        return false;
    };
    // The match must start at an unescaped separator.
    let Some(head) = head.strip_suffix(',') else {
        return false;
    };
    let escapes = head.bytes().rev().take_while(|c| *c == b'\\').count();
    escapes % 2 == 0
}

/// Validates the request against the approved manifest block and builds the query.
///
/// # Errors
/// `400 ldap_request_invalid` naming what is wrong.
pub fn build_query(cfg: &Ldap, body: &[u8]) -> Result<Query, ApiError> {
    let b: SearchBody = serde_json::from_slice(body).map_err(|_| {
        invalid("The body must be JSON: {\"filter\": \"(uid=x)\", \"attributes\": [..]}.")
    })?;
    let filter = filter::encode(&b.filter, &cfg.attributes).map_err(|e| invalid(e.0))?;
    let attributes = match b.attributes {
        None => cfg.attributes.clone(),
        Some(list) => {
            if list.is_empty() || list.len() > cfg.attributes.len() {
                return Err(invalid(
                    "`attributes` lists at least one and at most the app's attributes.",
                ));
            }
            let mut seen = BTreeSet::new();
            let mut out = Vec::new();
            for a in list {
                let Some(spelled) = cfg.attributes.iter().find(|x| x.eq_ignore_ascii_case(&a))
                else {
                    return Err(invalid(format!(
                        "`{a}` is not an attribute this app may read."
                    )));
                };
                if seen.insert(spelled.to_ascii_lowercase()) {
                    out.push(spelled.clone());
                }
            }
            out
        }
    };
    let scope = match b.scope.as_deref() {
        None | Some("sub") => 2,
        Some("one") => 1,
        Some("base") => 0,
        Some(_) => return Err(invalid("`scope` is `sub`, `one` or `base`.")),
    };
    let base = match b.base {
        None => cfg.base_dn.clone(),
        Some(base) => {
            if base.len() > 512 || base.chars().any(char::is_control) || !under(&base, &cfg.base_dn)
            {
                return Err(invalid(
                    "`base` must be a DN at or below the app's base DN.",
                ));
            }
            base
        }
    };
    let limit = b.limit.unwrap_or(cfg.max_entries).clamp(1, cfg.max_entries);
    Ok(Query {
        base,
        scope,
        filter,
        attributes,
        limit,
    })
}

fn json(status: StatusCode, body: Vec<u8>) -> Response<Body> {
    let mut r = Response::new(full_body(body));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// The refusal an LDAP failure is answered with (never carries a server diagnostic).
#[must_use]
pub fn failure(e: &LdapError) -> ApiError {
    match e {
        LdapError::Dial(crate::guard::DialError::Refused(h)) => {
            crate::guidance::destination_refused(h)
        }
        LdapError::Timeout => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "ldap_timeout",
            "The directory did not answer in time.",
        ),
        LdapError::Server(_) => ApiError::bad_gateway(
            "ldap_refused",
            "The directory refused the search (for example, the filter or base is not allowed there).",
        ),
        _ => ApiError::bad_gateway("ldap_unavailable", "Could not search the directory."),
    }
}

/// Runs one admitted request: `POST /search` with a JSON body already read in full.
pub async fn run(
    lane: &Lane,
    cfg: &Ldap,
    method: &http::Method,
    path: &str,
    body: &[u8],
) -> Response<Body> {
    if method != http::Method::POST || path != "/search" {
        return crate::guidance::response(&ApiError::new(
            StatusCode::NOT_FOUND,
            "ldap_operation_unknown",
            "The LDAP lane answers `POST /search` only.",
        ));
    }
    let q = match build_query(cfg, body) {
        Ok(q) => q,
        Err(e) => return crate::guidance::response(&e),
    };
    let target = Target {
        server: &cfg.server,
        port: cfg.port,
    };
    match lane.search(&target, &q).await {
        Ok(found) => {
            let v = serde_json::json!({ "entries": found.entries, "truncated": found.truncated });
            json(StatusCode::OK, serde_json::to_vec(&v).unwrap_or_default())
        }
        Err(e) => {
            tracing::warn!(error = %e, server = %cfg.server, "ldap lane search failed");
            crate::guidance::response(&failure(&e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Ldap {
        Ldap {
            server: "xldap.cern.ch".into(),
            port: 636,
            base_dn: "DC=cern,DC=ch".into(),
            attributes: ["uid", "mail", "displayName"].map(String::from).to_vec(),
            max_entries: 10,
        }
    }

    #[test]
    fn hosts() {
        assert!(is_ldap_host("cern-phonebook.ldap.aurora.internal"));
        assert!(is_ldap_host("X.LDAP.aurora.internal"));
        assert!(!is_ldap_host(".ldap.aurora.internal"));
        assert!(!is_ldap_host("ldap.aurora.internal"));
        assert!(!is_ldap_host("x.db.aurora.internal"));
        assert!(!is_ldap_host("x.ldap.aurora.internal.evil.com"));
    }

    #[test]
    fn confinement() {
        let r = "DC=cern,DC=ch";
        assert!(under("DC=cern,DC=ch", r));
        assert!(under("dc=CERN, dc=ch", r));
        assert!(under("OU=Users,DC=cern,DC=ch", r));
        assert!(under(r"CN=Doe\, John,OU=Users,DC=cern,DC=ch", r));
        assert!(!under("DC=ch", r));
        assert!(!under("DC=evil,DC=ch", r));
        assert!(!under("DC=xcern,DC=ch", r));
        assert!(
            !under(r"OU=x\,DC=cern,DC=ch", r),
            "an escaped comma is not a separator"
        );
        assert!(under(r"OU=x\\,DC=cern,DC=ch", r), "an escaped backslash is");
        assert!(!under("", r));
    }

    #[test]
    fn queries() {
        let c = cfg();
        let q = build_query(&c, br#"{"filter":"(uid=jdoe)"}"#).unwrap();
        assert_eq!(
            (q.scope, q.limit, q.base.as_str()),
            (2, 10, "DC=cern,DC=ch")
        );
        assert_eq!(q.attributes, c.attributes);
        let q = build_query(
            &c,
            br#"{"filter":"(mail=a*)","attributes":["MAIL"],"scope":"one","limit":500}"#,
        )
        .unwrap();
        assert_eq!(
            (q.scope, q.limit, q.attributes),
            (1, 10, vec!["mail".to_owned()])
        );
        for bad in [
            &br#"{"filter":"(uid=a)","extra":1}"#[..],
            br#"{"filter":"(uid=a)","attributes":["userPassword"]}"#,
            br#"{"filter":"(uid=a)","attributes":[]}"#,
            br#"{"filter":"(userPassword=*)"}"#,
            br#"{"filter":"(uid=a)","scope":"x"}"#,
            br#"{"filter":"(uid=a)","base":"DC=ch"}"#,
            br#"{"filter":"(uid=a)","base":"DC=cern,DC=ch\n"}"#,
            b"not json",
            br#"{"attributes":["uid"]}"#,
        ] {
            assert!(
                build_query(&c, bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}

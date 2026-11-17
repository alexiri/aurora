//! Request fingerprint v1 of a **parked** approval (docs/API_CONTRACTS.md C-821, docs/07 §Park
//! and resume, SEC-74). Pure over typed inputs: the gateway builds [`Inputs`] from the request it
//! has just evaluated, once when it parks the request and once per candidate grant on a retry.
//!
//! ```text
//! canonical   = "aurora-park-v1\0" ‖ agent_id (16) ‖ conversation_id (16)
//!               ‖ F(METHOD) ‖ F(host-lower ":" port) ‖ F(path) ‖ F(raw query, "" when absent)
//!               ‖ SHA-256(complete body) (32) ‖ requiring rule logical_id (16)
//!               ‖ F("connection" | "secret" | "none") ‖ winning credential id (16, zeros: none)
//!               ‖ F(tool id or "")
//! F(x)        = u32 big-endian length ‖ UTF-8 bytes
//! fingerprint = SHA-256(fp_salt (16) ‖ canonical)
//! ```
//!
//! The canonical bytes are never stored or logged; the stored value is salted per row, so it is
//! neither a confirmation oracle for a guessable body nor comparable across rows.

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Length of the per-row salt.
pub const SALT_LEN: usize = 16;
/// Length of a fingerprint (SHA-256).
pub const FINGERPRINT_LEN: usize = 32;

const DOMAIN: &[u8] = b"aurora-park-v1\0";

/// The kind of the winning credential of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRef {
    /// An app connection (its id).
    Connection(Uuid),
    /// A stored secret (its id).
    Secret(Uuid),
    /// No credential would be injected.
    None,
}

/// Everything the fingerprint covers (C-821).
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    /// Case-insensitive: folded to ASCII upper case.
    pub method: &'a str,
    /// Case-insensitive: folded to ASCII lower case.
    pub host: &'a str,
    pub port: u16,
    /// The path as normalized by step 3 of the pipeline (exact).
    pub path: &'a str,
    /// The raw query after `?`, byte-exact (order and encoding significant); `None` = absent.
    pub query: Option<&'a str>,
    /// SHA-256 of the complete body (of the empty string when there is none).
    pub body_sha256: [u8; 32],
    /// The `logical_id` of the requiring rule.
    pub rule_logical_id: Uuid,
    pub credential: CredentialRef,
    pub tool_id: Option<&'a str>,
}

/// SHA-256 of a complete request body.
#[must_use]
pub fn body_digest(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

fn field(out: &mut Vec<u8>, x: &str) {
    // A parked body is bounded far below 4 GiB and so is every other field; saturate to be total.
    let len = u32::try_from(x.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(x.as_bytes());
}

fn canonical(i: &Inputs<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(DOMAIN);
    out.extend_from_slice(i.agent_id.as_bytes());
    out.extend_from_slice(i.conversation_id.as_bytes());
    field(&mut out, &i.method.to_ascii_uppercase());
    field(
        &mut out,
        &format!("{}:{}", i.host.to_ascii_lowercase(), i.port),
    );
    field(&mut out, i.path);
    field(&mut out, i.query.unwrap_or(""));
    out.extend_from_slice(&i.body_sha256);
    out.extend_from_slice(i.rule_logical_id.as_bytes());
    let (kind, id) = match i.credential {
        CredentialRef::Connection(id) => ("connection", id),
        CredentialRef::Secret(id) => ("secret", id),
        CredentialRef::None => ("none", Uuid::nil()),
    };
    field(&mut out, kind);
    out.extend_from_slice(id.as_bytes());
    field(&mut out, i.tool_id.unwrap_or(""));
    out
}

/// `SHA-256(salt ‖ canonical)`.
#[must_use]
pub fn fingerprint(salt: &[u8; SALT_LEN], i: &Inputs<'_>) -> [u8; FINGERPRINT_LEN] {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(canonical(i));
    h.finalize().into()
}

/// A fresh salt from the operating system's CSPRNG. `None` only if the OS refuses randomness
/// (the caller fails closed: nothing is parked).
#[must_use]
pub fn new_salt() -> Option<[u8; SALT_LEN]> {
    let mut salt = [0u8; SALT_LEN];
    aws_lc_rs::rand::fill(&mut salt).ok()?;
    Some(salt)
}

/// Constant-time comparison of a stored fingerprint with a recomputed one. A stored value of the
/// wrong length never matches.
#[must_use]
pub fn matches(stored: &[u8], salt: &[u8], i: &Inputs<'_>) -> bool {
    let Ok(salt) = <&[u8; SALT_LEN]>::try_from(salt) else {
        return false;
    };
    let computed = fingerprint(salt, i);
    aws_lc_rs::constant_time::verify_slices_are_equal(stored, &computed).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn salt() -> [u8; SALT_LEN] {
        [7u8; SALT_LEN]
    }

    fn base<'a>() -> Inputs<'a> {
        Inputs {
            agent_id: id(1),
            conversation_id: id(2),
            method: "POST",
            host: "api.example.com",
            port: 443,
            path: "/v1/items",
            query: Some("a=1&b=2"),
            body_sha256: body_digest(b"{\"x\":1}"),
            rule_logical_id: id(3),
            credential: CredentialRef::Connection(id(4)),
            tool_id: Some("create_item"),
        }
    }

    fn hex(b: &[u8]) -> String {
        use std::fmt::Write;
        b.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    }

    // The vectors were computed with an independent implementation (Python `hashlib`, building
    // the canonical form of C-821 by hand) and checked in; the Rust implementation must agree.
    #[test]
    fn vectors() {
        let cases: [(&str, Inputs<'_>, &str); 7] = [
            (
                "base",
                base(),
                "250da7e96bcd4daddaf8413db70ad6d53ec0689064c5269b24b36eafae384a20",
            ),
            (
                "empty body, no query, no tool, no credential",
                Inputs {
                    method: "get",
                    host: "API.Example.COM",
                    query: None,
                    body_sha256: body_digest(b""),
                    credential: CredentialRef::None,
                    tool_id: None,
                    ..base()
                },
                "b8c4514f0e45a26dd902e978e4df47191d68d479eebffa72aa9fc45c79c0c7c8",
            ),
            (
                "query order swapped",
                Inputs {
                    query: Some("b=2&a=1"),
                    ..base()
                },
                "3ccf39a2faaafcda0c9ef6d0117f3916543ccf90e02384fcf0c212e4f81457ef",
            ),
            (
                "one body byte",
                Inputs {
                    body_sha256: body_digest(b"{\"x\":2}"),
                    ..base()
                },
                "14f3a152aab8ed111d135b79876b89fc1752713443664edde5c1cb4b96571c24",
            ),
            (
                "secret credential",
                Inputs {
                    credential: CredentialRef::Secret(id(4)),
                    ..base()
                },
                "7e9bf8f8599dd2a398fe857984ca26822fa2fd42905d9e0f4f8f09d0acfa0626",
            ),
            (
                "other rule",
                Inputs {
                    rule_logical_id: id(9),
                    ..base()
                },
                "c5e5d9db0d8188ea86d3e560270e297f3cd3ccd33d1599f01b955b9008d46071",
            ),
            (
                "other conversation",
                Inputs {
                    conversation_id: id(8),
                    ..base()
                },
                "d433d33c5b98f9f8f00b223c1d9a878cdf97319dc06c9aa269aa10fc39ec6c12",
            ),
        ];
        for (name, inputs, want) in cases {
            assert_eq!(hex(&fingerprint(&salt(), &inputs)), want, "{name}");
        }
    }

    #[test]
    fn every_component_changes_the_fingerprint() {
        let b = fingerprint(&salt(), &base());
        let variants: Vec<(&str, Inputs<'_>)> = vec![
            (
                "agent",
                Inputs {
                    agent_id: id(9),
                    ..base()
                },
            ),
            (
                "conversation",
                Inputs {
                    conversation_id: id(9),
                    ..base()
                },
            ),
            (
                "method",
                Inputs {
                    method: "PUT",
                    ..base()
                },
            ),
            (
                "host",
                Inputs {
                    host: "api.example.org",
                    ..base()
                },
            ),
            (
                "port",
                Inputs {
                    port: 8443,
                    ..base()
                },
            ),
            (
                "path",
                Inputs {
                    path: "/v1/items/",
                    ..base()
                },
            ),
            (
                "query",
                Inputs {
                    query: Some("a=1&b=3"),
                    ..base()
                },
            ),
            (
                "query absent",
                Inputs {
                    query: None,
                    ..base()
                },
            ),
            (
                "query encoding",
                Inputs {
                    query: Some("a=1&b=%32"),
                    ..base()
                },
            ),
            (
                "body",
                Inputs {
                    body_sha256: body_digest(b"{\"x\":1} "),
                    ..base()
                },
            ),
            (
                "rule",
                Inputs {
                    rule_logical_id: id(9),
                    ..base()
                },
            ),
            (
                "credential id",
                Inputs {
                    credential: CredentialRef::Connection(id(9)),
                    ..base()
                },
            ),
            (
                "credential kind",
                Inputs {
                    credential: CredentialRef::Secret(id(4)),
                    ..base()
                },
            ),
            (
                "no credential",
                Inputs {
                    credential: CredentialRef::None,
                    ..base()
                },
            ),
            (
                "tool",
                Inputs {
                    tool_id: Some("create_items"),
                    ..base()
                },
            ),
            (
                "no tool",
                Inputs {
                    tool_id: None,
                    ..base()
                },
            ),
        ];
        for (name, v) in variants {
            assert_ne!(
                fingerprint(&salt(), &v),
                b,
                "{name} must change the fingerprint"
            );
        }
    }

    #[test]
    fn method_and_host_are_case_folded() {
        let a = Inputs {
            method: "post",
            host: "API.EXAMPLE.COM",
            ..base()
        };
        assert_eq!(fingerprint(&salt(), &a), fingerprint(&salt(), &base()));
    }

    #[test]
    fn fields_cannot_be_shifted_across_boundaries() {
        // Length prefixes keep `("ab", "c")` and `("a", "bc")` apart.
        let a = Inputs {
            path: "/ab",
            query: Some("c"),
            ..base()
        };
        let b = Inputs {
            path: "/a",
            query: Some("bc"),
            ..base()
        };
        assert_ne!(fingerprint(&salt(), &a), fingerprint(&salt(), &b));
    }

    #[test]
    fn two_salts_give_different_fingerprints() {
        let (s1, s2) = (new_salt().unwrap(), new_salt().unwrap());
        assert_ne!(s1, s2);
        assert_ne!(fingerprint(&s1, &base()), fingerprint(&s2, &base()));
    }

    #[test]
    fn matches_is_exact() {
        let fp = fingerprint(&salt(), &base());
        assert!(matches(&fp, &salt(), &base()));
        assert!(!matches(&fp, &[8u8; SALT_LEN], &base()));
        assert!(!matches(
            &fp,
            &salt(),
            &Inputs {
                path: "/other",
                ..base()
            }
        ));
        assert!(!matches(&fp[..31], &salt(), &base()));
        assert!(!matches(&fp, &salt()[..15], &base()));
    }
}

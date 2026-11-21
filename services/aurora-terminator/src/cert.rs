//! The certificate law the terminator applies offline, right after the client's signature
//! proved possession of the certified key and before any API call (`16 §W20` Authentication,
//! SEC-92, D-356, SEC-147): CA signature by one of the registration's CA keys, type `user`, inside
//! the validity window (60 s skew), principals `[<agent uuid>]` or `[<agent uuid>, <handle>]` with
//! the typed user name one of them, no critical options, key id in the `aurora:u=…:k=…:a=…:w=…`
//! shape with its agent the first principal. The law answers the **key id's agent**: the session
//! is opened for it, never for an agent looked up from the typed name (D-445). The reason is for
//! metrics only; the client is never told.

use std::time::{SystemTime, UNIX_EPOCH};

use aurora_core::ids::is_valid_handle;
use ssh_key::Fingerprint;
use ssh_key::certificate::CertType;
use uuid::Uuid;

/// Clock skew tolerated on the validity window.
pub const SKEW_SECS: u64 = 60;

/// Why an authentication was refused: the metrics label (`aurora_terminator_auth_failures_total
/// {reason}`), never anything the client sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotCertificate,
    BadSignature,
    Expired,
    Principal,
    CriticalOptions,
    KeyId,
    Denied,
    Limit,
    ApiError,
}

impl Refusal {
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotCertificate => "not_certificate",
            Self::BadSignature => "bad_signature",
            Self::Expired => "expired",
            Self::Principal => "principal",
            Self::CriticalOptions => "critical_options",
            Self::KeyId => "key_id",
            Self::Denied => "denied",
            Self::Limit => "limit",
            Self::ApiError => "api_error",
        }
    }
}

/// The principal rule (SEC-147): one or two principals, the first the agent's UUID in its
/// canonical form (lowercase, hyphenated), the second (if any) a valid handle, and the typed user
/// name one of them. Answers the first principal's UUID; the typed name chooses nothing.
fn principal_agent(principals: &[String], username: &str) -> Option<Uuid> {
    let (first, rest) = principals.split_first()?;
    if rest.len() > 1 {
        return None;
    }
    let agent = Uuid::parse_str(first).ok()?;
    if agent.to_string() != *first {
        return None;
    }
    if rest.first().is_some_and(|h| !is_valid_handle(h)) {
        return None;
    }
    principals.iter().any(|p| p == username).then_some(agent)
}

/// The key id's shape, and its agent (`a=`) equal to `agent`.
fn key_id_ok(id: &str, agent: Uuid) -> bool {
    let mut parts = id.split(':');
    if parts.next() != Some("aurora") {
        return false;
    }
    let mut named = None;
    for tag in ["u", "k", "a", "w"] {
        let Some(v) = parts
            .next()
            .and_then(|p| p.strip_prefix(tag))
            .and_then(|p| p.strip_prefix('='))
        else {
            return false;
        };
        let Ok(u) = Uuid::parse_str(v) else {
            return false;
        };
        if tag == "a" {
            named = Some(u);
        }
    }
    parts.next().is_none() && named == Some(agent)
}

/// Applies the law at `now`; `Ok(agent)` only when every condition holds, `agent` being the key
/// id's agent (the certificate's first principal), the only agent a session may open for.
pub fn check(
    cert: &ssh_key::Certificate,
    username: &str,
    cas: &[Fingerprint],
    now: SystemTime,
) -> Result<Uuid, Refusal> {
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // Signature and CA only, at a moment inside the certificate's own window; the window itself
    // (with skew), the type, the principals and the key id are checked below.
    if cert.valid_after() >= cert.valid_before() {
        return Err(Refusal::Expired);
    }
    cert.validate_at(cert.valid_after(), cas.iter())
        .map_err(|_| Refusal::BadSignature)?;
    if now.saturating_add(SKEW_SECS) < cert.valid_after()
        || now > cert.valid_before().saturating_add(SKEW_SECS)
    {
        return Err(Refusal::Expired);
    }
    if cert.cert_type() != CertType::User {
        return Err(Refusal::Principal);
    }
    let agent = principal_agent(cert.valid_principals(), username).ok_or(Refusal::Principal)?;
    if !cert.critical_options().is_empty() {
        return Err(Refusal::CriticalOptions);
    }
    if !key_id_ok(cert.key_id(), agent) {
        return Err(Refusal::KeyId);
    }
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssh_key::PrivateKey;
    use ssh_key::certificate::Builder;
    use ssh_key::private::Ed25519Keypair;

    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
    }

    const NOW: u64 = 2_000_000_000;

    fn kid(agent: Uuid) -> String {
        format!(
            "aurora:u={}:k={}:a={agent}:w={}",
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4()
        )
    }

    /// A good certificate for the key id `key_id`, with exactly `principals`.
    fn cert(key_id: &str, principals: &[&str]) -> ssh_key::Certificate {
        let mut b = Builder::new(
            [7u8; 32],
            key(0x11).public_key().key_data().clone(),
            NOW - 60,
            NOW + 900,
        )
        .unwrap();
        b.serial(1).unwrap();
        b.cert_type(CertType::User).unwrap();
        b.key_id(key_id).unwrap();
        if principals.is_empty() {
            // OpenSSH reads an empty list as "any user": the law must refuse it.
            b.all_principals_valid().unwrap();
        }
        for p in principals {
            b.valid_principal(*p).unwrap();
        }
        b.extension("permit-pty", "").unwrap();
        b.sign(&key(0x5a)).unwrap()
    }

    fn run(c: &ssh_key::Certificate, user: &str) -> Result<Uuid, Refusal> {
        let ca = key(0x5a).public_key().fingerprint(Default::default());
        check(
            c,
            user,
            &[ca],
            UNIX_EPOCH + std::time::Duration::from_secs(NOW),
        )
    }

    #[test]
    fn key_id_shape() {
        let a = Uuid::new_v4();
        let ok = kid(a);
        assert!(key_id_ok(&ok, a));
        assert!(!key_id_ok(&ok, Uuid::new_v4()));
        for bad in [
            "",
            "aurora:",
            "x:u=1:k=2:a=3:w=4",
            &format!("{ok}:extra"),
            &ok.replace(":w=", ":x="),
        ] {
            assert!(!key_id_ok(bad, a), "{bad}");
        }
    }

    /// `[uuid]` with the uuid typed; `[uuid, handle]` with either typed: each time the answer is
    /// the key id's agent.
    #[test]
    fn accepts_the_id_or_the_handle_and_returns_the_key_ids_agent() {
        let a = Uuid::new_v4();
        let id = a.to_string();
        let old = cert(&kid(a), &[&id]);
        assert_eq!(run(&old, &id), Ok(a));
        let both = cert(&kid(a), &[&id, "daneel"]);
        assert_eq!(run(&both, &id), Ok(a));
        assert_eq!(run(&both, "daneel"), Ok(a));
    }

    /// Every other shape is a `principal` refusal (a first principal that is not the key id's
    /// agent is a `key_id` refusal).
    #[test]
    fn refuses_every_other_principal_shape() {
        let a = Uuid::new_v4();
        let id = a.to_string();
        let k = kid(a);
        for (what, principals, user) in [
            (
                "typed name not a principal",
                vec![id.as_str(), "daneel"],
                "giskard",
            ),
            (
                "typed name not the only principal",
                vec![id.as_str()],
                "daneel",
            ),
            ("handle alone", vec!["daneel"], "daneel"),
            ("handle first", vec!["daneel", id.as_str()], "daneel"),
            (
                "handle first, id typed",
                vec!["daneel", id.as_str()],
                id.as_str(),
            ),
            (
                "three principals",
                vec![id.as_str(), "daneel", "giskard"],
                "daneel",
            ),
            ("no principal", vec![], id.as_str()),
            ("not a handle shape", vec![id.as_str(), "Daneel"], "Daneel"),
            (
                "an empty second principal",
                vec![id.as_str(), ""],
                id.as_str(),
            ),
        ] {
            assert_eq!(
                run(&cert(&k, &principals), user),
                Err(Refusal::Principal),
                "{what}"
            );
        }
        // A UUID in a non-canonical form (upper case, braces, simple): refused, even typed as is.
        for odd in [
            id.to_uppercase(),
            format!("{{{id}}}"),
            a.simple().to_string(),
        ] {
            assert_eq!(
                run(&cert(&k, &[&odd, "daneel"]), &odd),
                Err(Refusal::Principal),
                "{odd}"
            );
        }
        // The first principal is a UUID, but not the key id's agent.
        let other = Uuid::new_v4().to_string();
        assert_eq!(
            run(&cert(&k, &[&other, "daneel"]), "daneel"),
            Err(Refusal::KeyId)
        );
        assert_eq!(run(&cert(&k, &[&other]), &other), Err(Refusal::KeyId));
    }

    /// A certificate for agent A presented with agent B's handle as the user is refused: B's
    /// handle is not among A's principals (no lookup could turn it into B).
    #[test]
    fn another_agents_handle_is_refused() {
        let a = Uuid::new_v4();
        let for_a = cert(&kid(a), &[&a.to_string(), "daneel"]);
        assert_eq!(run(&for_a, "giskard"), Err(Refusal::Principal));
        // And B's own id too.
        assert_eq!(
            run(&for_a, &Uuid::new_v4().to_string()),
            Err(Refusal::Principal)
        );
    }

    /// A handle may spell a UUID (`[a-z][a-z0-9-]{0,39}` admits `abcdef01-…`). Agent A whose handle
    /// spells agent B's id: typing that name opens A, the key id's agent, never B.
    #[test]
    fn a_handle_spelling_another_agents_uuid_still_opens_the_certificates_agent() {
        let a = Uuid::new_v4();
        let b: Uuid = "abcdef01-2345-4678-89ab-cdef01234567".parse().unwrap();
        let handle = b.to_string();
        assert!(is_valid_handle(&handle));
        let for_a = cert(&kid(a), &[&a.to_string(), &handle]);
        assert_eq!(run(&for_a, &handle), Ok(a));
        assert_ne!(run(&for_a, &handle), Ok(b));
        // The same principals under a key id naming B: refused (the first principal is A).
        let forged_kid = cert(&kid(b), &[&a.to_string(), &handle]);
        assert_eq!(run(&forged_kid, &handle), Err(Refusal::KeyId));
    }

    /// Handles never change (C-12), but the law does not depend on that: a certificate minted
    /// under an old handle connects with that handle until it expires and lands on its own agent;
    /// a newer handle is not among its principals.
    #[test]
    fn an_old_handle_keeps_working_until_expiry_and_a_new_one_does_not() {
        let a = Uuid::new_v4();
        let minted = cert(&kid(a), &[&a.to_string(), "daneel-old"]);
        assert_eq!(run(&minted, "daneel-old"), Ok(a));
        assert_eq!(run(&minted, "daneel"), Err(Refusal::Principal));
        let ca = key(0x5a).public_key().fingerprint(Default::default());
        let after = UNIX_EPOCH + std::time::Duration::from_secs(NOW + 900 + SKEW_SECS + 1);
        assert_eq!(
            check(&minted, "daneel-old", &[ca], after),
            Err(Refusal::Expired)
        );
    }
}

//! The certificate law the terminator applies offline, right after the client's signature
//! proved possession of the certified key and before any API call (`16 §W20` Authentication,
//! SEC-92, D-356): CA signature by one of the registration's CA keys, type `user`, inside the
//! validity window (60 s skew), principals exactly `[<username>]` with the username a UUID, no
//! critical options, key id in the `aurora:u=…:k=…:a=…:w=…` shape (the agent in it the
//! username). The reason is for metrics only; the client is never told.

use std::time::{SystemTime, UNIX_EPOCH};

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

fn key_id_ok(id: &str, username: &str) -> bool {
    let mut parts = id.split(':');
    if parts.next() != Some("aurora") {
        return false;
    }
    let mut agent = None;
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
            agent = Some(u);
        }
    }
    parts.next().is_none() && agent.is_some_and(|a| a.to_string() == username)
}

/// Applies the law at `now`; `Ok` only when every condition holds.
pub fn check(
    cert: &ssh_key::Certificate,
    username: &str,
    cas: &[Fingerprint],
    now: SystemTime,
) -> Result<(), Refusal> {
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
    if cert.cert_type() != CertType::User
        || cert.valid_principals().len() != 1
        || cert.valid_principals()[0] != username
        || Uuid::parse_str(username).is_err()
    {
        return Err(Refusal::Principal);
    }
    if !cert.critical_options().is_empty() {
        return Err(Refusal::CriticalOptions);
    }
    if !key_id_ok(cert.key_id(), username) {
        return Err(Refusal::KeyId);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_id_shape() {
        let a = Uuid::new_v4().to_string();
        let ok = format!(
            "aurora:u={}:k={}:a={a}:w={}",
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4()
        );
        assert!(key_id_ok(&ok, &a));
        assert!(!key_id_ok(&ok, &Uuid::new_v4().to_string()));
        for bad in [
            "",
            "aurora:",
            "x:u=1:k=2:a=3:w=4",
            &format!("{ok}:extra"),
            &ok.replace(":w=", ":x="),
        ] {
            assert!(!key_id_ok(bad, &a), "{bad}");
        }
    }
}

//! Guard (`tenant`) and route (`rewrite_host`) stages (docs/09 §5, SEC-17).

use super::Unavailable;
use super::template::{Dest, Vars, render};
use aurora_policy::HostPattern;
use aurora_registry::manifest::{RewriteHost, Tenant};

#[derive(Debug, PartialEq, Eq)]
pub enum GuardError {
    /// Request host differs from the stored tenant host (421 `host_mismatch`).
    Mismatch { expected: String },
    /// Stored tenant host missing or outside the declared zone.
    Unavailable,
}

fn in_zone(zone: &str, host: &str) -> bool {
    let z = HostPattern::lenient(zone);
    z.is_valid() && z.matches(host, 443)
}

/// The request host must equal the connection's stored host, which itself must sit
/// inside the zone the manifest declared (checked at runtime too).
pub fn tenant_guard(t: &Tenant, stored: Option<&str>, host: &str) -> Result<(), GuardError> {
    let stored = stored.ok_or(GuardError::Unavailable)?;
    if !in_zone(&t.zone, &stored.to_ascii_lowercase()) {
        return Err(GuardError::Unavailable);
    }
    if stored.eq_ignore_ascii_case(host) {
        Ok(())
    } else {
        Err(GuardError::Mismatch {
            expected: stored.to_ascii_lowercase(),
        })
    }
}

/// The upstream host (and TLS server name) replaced by the rendered template, which
/// must stay inside the zone.
pub fn rewrite_host(r: &RewriteHost, v: &Vars<'_>) -> Result<String, Unavailable> {
    let host = render(&r.template, Dest::Host, v)?
        .text
        .to_ascii_lowercase();
    if host.is_empty() || !in_zone(&r.zone, &host) {
        return Err(Unavailable);
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> Tenant {
        Tenant {
            field: "connection.tenant_host".into(),
            zone: "*.example.com".into(),
        }
    }

    #[test]
    fn guard() {
        let t = tenant();
        assert!(tenant_guard(&t, Some("acme.example.com"), "ACME.example.com").is_ok());
        assert_eq!(
            tenant_guard(&t, Some("acme.example.com"), "evil.example.com"),
            Err(GuardError::Mismatch {
                expected: "acme.example.com".into()
            })
        );
        // Stored host outside the zone never passes, even when equal to the request.
        assert_eq!(
            tenant_guard(&t, Some("evil.example.org"), "evil.example.org"),
            Err(GuardError::Unavailable)
        );
        assert_eq!(
            tenant_guard(&t, None, "a.example.com"),
            Err(GuardError::Unavailable)
        );
    }
}

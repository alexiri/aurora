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

/// (FIX-104, D-180, SEC-04) A credential claimed by a package's host entry or a generic secret's
/// host pattern is injected only into a request that leaves over TLS to the claimed port (443
/// when the claim names none). Plain HTTP or another port: an uncredentialed request.
pub fn claim_transport_ok(claim_port: Option<u16>, https: bool, port: u16) -> bool {
    https && port == claim_port.unwrap_or(443)
}

/// The port a host-entry claim names (`host:port`), if any.
pub fn claim_port(claim: &str) -> Option<u16> {
    HostPattern::lenient(claim).port()
}

/// `gateway.insecure_loopback_injection` (tests only): the destination is a loopback address
/// literal or `localhost`, which no network peer can be reached through.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']);
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            }
        })
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

/// Instance guard (D-74, SEC-04/SEC-59): an instance-bound connection is injected only when the
/// request host equals its instance's host, exactly. `on_instance` is the registered instance
/// the request host resolved to, if any: a connection without an instance never rides on an
/// instance host, and one bound to another instance answers 421 `host_mismatch`.
pub fn instance_guard(
    conn_instance: Option<uuid::Uuid>,
    conn_instance_host: Option<&str>,
    on_instance: Option<uuid::Uuid>,
    host: &str,
) -> Result<(), GuardError> {
    let Some(id) = conn_instance else {
        if on_instance.is_some() {
            return Err(GuardError::Unavailable);
        }
        // A default-host connection of an instance-bound app (`conn_instance_host` is the app's
        // `default_host`) is injected on that exact host only.
        return match conn_instance_host {
            Some(d) if !d.eq_ignore_ascii_case(host) => Err(GuardError::Mismatch {
                expected: d.to_ascii_lowercase(),
            }),
            _ => Ok(()),
        };
    };
    let stored = conn_instance_host.ok_or(GuardError::Unavailable)?;
    if stored.eq_ignore_ascii_case(host) && on_instance.is_none_or(|o| o == id) {
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
    fn claim_transport_is_tls_to_the_claimed_port() {
        assert!(claim_transport_ok(None, true, 443));
        assert!(!claim_transport_ok(None, false, 443));
        assert!(!claim_transport_ok(None, false, 80));
        assert!(!claim_transport_ok(None, true, 8443));
        assert!(claim_transport_ok(Some(8443), true, 8443));
        assert!(!claim_transport_ok(Some(8443), true, 443));
        assert!(!claim_transport_ok(Some(8443), false, 8443));
        assert_eq!(claim_port("api.example.com:8443"), Some(8443));
        assert_eq!(claim_port("*.example.com"), None);
        for h in [
            "127.0.0.1",
            "127.9.9.9",
            "::1",
            "[::1]",
            "localhost",
            "LOCALHOST",
        ] {
            assert!(is_loopback_host(h), "{h}");
        }
        for h in [
            "10.0.0.1",
            "api.example.com",
            "localhost.evil.test",
            "0.0.0.0",
        ] {
            assert!(!is_loopback_host(h), "{h}");
        }
    }

    #[test]
    fn instance_guard_is_exact_host_only() {
        use uuid::Uuid;
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        // Bound connection on its own instance host, with or without the request resolving to it.
        assert!(instance_guard(Some(a), Some("Desk.Acme.test"), Some(a), "desk.acme.test").is_ok());
        assert!(instance_guard(Some(a), Some("desk.acme.test"), None, "desk.acme.test").is_ok());
        // Any other host, including a lookalike or the instance's parent: 421 naming the host.
        for h in [
            "desk.acme.test.evil.test",
            "acme.test",
            "evil.test",
            "xdesk.acme.test",
        ] {
            assert_eq!(
                instance_guard(Some(a), Some("desk.acme.test"), None, h),
                Err(GuardError::Mismatch {
                    expected: "desk.acme.test".into()
                }),
                "{h}"
            );
        }
        // On another instance's host the connection of instance `a` is refused.
        assert!(matches!(
            instance_guard(Some(a), Some("desk.acme.test"), Some(b), "desk.acme.test"),
            Err(GuardError::Mismatch { .. })
        ));
        // A connection without an instance never rides on an instance host.
        assert_eq!(
            instance_guard(None, None, Some(b), "desk.acme.test"),
            Err(GuardError::Unavailable)
        );
        assert!(instance_guard(None, None, None, "saas.test").is_ok());
        // A default-host connection (host = the app's default_host): that host only.
        assert!(instance_guard(None, Some("saas.test"), None, "SaaS.test").is_ok());
        assert_eq!(
            instance_guard(None, Some("saas.test"), None, "saas.test.evil.test"),
            Err(GuardError::Mismatch {
                expected: "saas.test".into()
            })
        );
        assert_eq!(
            instance_guard(None, Some("saas.test"), Some(b), "desk.acme.test"),
            Err(GuardError::Unavailable)
        );
        // A bound connection whose instance host is unknown fails closed.
        assert_eq!(
            instance_guard(Some(a), None, Some(a), "desk.acme.test"),
            Err(GuardError::Unavailable)
        );
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

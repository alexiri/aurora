//! Which incoming `traceparent` is trusted on the public listener (SEC-88, D-232 (c), C-732 (2)).
//!
//! One constant, pinned by the test below: changing it is a security decision.

use aurora_core::service::{TraceTrust, Trust};

/// Public listener (4100): browser adoption by default (trace id and parent span id kept, the
/// sampling rule decides), parent-based adoption for token-authenticated machine callers, and no
/// adoption for third-party ingress.
pub const PUBLIC_TRUST: TraceTrust = TraceTrust {
    default: Trust::Browser,
    overrides: &[
        // Machine callers authenticated by a token (runner plane, connector host, relay).
        ("/v1/runner/", Trust::Parent),
        ("/v1/connector-host/", Trust::Parent),
        ("/v1/relay/", Trust::Parent),
        // External connectors are not machine-trusted: a new root, the incoming id as a link.
        ("/v1/connector/", Trust::Link),
        // Third-party ingress: SCIM, OAuth/OIDC callbacks, agent webhooks.
        ("/scim/", Trust::Ignore),
        ("/v1/oauth/callback", Trust::Ignore),
        ("/v1/auth/oidc/", Trust::Ignore),
        ("/v1/hooks/", Trust::Ignore),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trust_table_is_pinned() {
        let got: Vec<(&str, Trust)> = PUBLIC_TRUST.overrides.to_vec();
        assert_eq!(
            got,
            vec![
                ("/v1/runner/", Trust::Parent),
                ("/v1/connector-host/", Trust::Parent),
                ("/v1/relay/", Trust::Parent),
                ("/v1/connector/", Trust::Link),
                ("/scim/", Trust::Ignore),
                ("/v1/oauth/callback", Trust::Ignore),
                ("/v1/auth/oidc/", Trust::Ignore),
                ("/v1/hooks/", Trust::Ignore),
            ]
        );
        assert_eq!(PUBLIC_TRUST.default, Trust::Browser);
    }

    #[test]
    fn paths_resolve_to_the_documented_trust() {
        let t = |p: &str| PUBLIC_TRUST.resolve(p);
        assert_eq!(t("/v1/runner/work"), Trust::Parent);
        assert_eq!(t("/v1/runner/preview-streams/x"), Trust::Parent);
        assert_eq!(t("/v1/connector-host/register"), Trust::Parent);
        assert_eq!(t("/v1/relay/resolve"), Trust::Parent);
        assert_eq!(t("/v1/connector/events"), Trust::Link);
        assert_eq!(t("/scim/v2/Users"), Trust::Ignore);
        assert_eq!(t("/v1/hooks/tok"), Trust::Ignore);
        // Everything else under /v1 is a browser (or API client) route.
        assert_eq!(t("/v1/conversations/abc/turns"), Trust::Browser);
        assert_eq!(t("/v1/connectors/x/icon"), Trust::Browser);
        assert_eq!(t("/v1/runners"), Trust::Browser);
        assert_eq!(t("/v1/telemetry/traces"), Trust::Browser);
    }
}

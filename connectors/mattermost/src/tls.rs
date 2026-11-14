//! Extra trusted CAs for the connector's outbound TLS (docs/15 `tls.extra_ca_bundle`).
//!
//! The connector reads the PEM bundle named by `AURORA_TLS_EXTRA_CA_BUNDLE` (the deployment mounts
//! the same file the other workloads read as `tls.extra_ca_bundle`) once, and both its clients, the
//! REST client and the WebSocket, trust it in addition to their normal roots. An unreadable or empty
//! bundle is logged and ignored: a connector that cannot reach a private-CA server says so with its
//! own TLS error instead of refusing to start.

use std::sync::OnceLock;

use base64::Engine;

const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const END: &str = "-----END CERTIFICATE-----";

/// DER certificates of a PEM bundle (everything that is not a certificate block is skipped).
pub fn parse_pem_bundle(pem: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(BEGIN) {
        let body_start = start + BEGIN.len();
        let Some(end) = rest[body_start..].find(END) else {
            break;
        };
        let b64: String = rest[body_start..body_start + end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(b64) {
            out.push(der);
        }
        rest = &rest[body_start + end + END.len()..];
    }
    out
}

fn bundle() -> &'static [Vec<u8>] {
    static DER: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    DER.get_or_init(|| {
        let Ok(path) = std::env::var("AURORA_TLS_EXTRA_CA_BUNDLE") else {
            return Vec::new();
        };
        if path.trim().is_empty() {
            return Vec::new();
        }
        match std::fs::read_to_string(&path) {
            Ok(pem) => {
                let ders = parse_pem_bundle(&pem);
                if ders.is_empty() {
                    tracing::warn!(%path, "tls.extra_ca_bundle holds no certificate");
                }
                ders
            }
            Err(e) => {
                tracing::warn!(%path, error = %e, "tls.extra_ca_bundle cannot be read");
                Vec::new()
            }
        }
    })
}

/// The extra roots for the `reqwest` client.
pub fn extra_roots() -> Vec<reqwest::Certificate> {
    bundle()
        .iter()
        .filter_map(|d| reqwest::Certificate::from_der(d).ok())
        .collect()
}

/// The extra roots as DER, for the WebSocket's rustls configuration.
pub fn extra_root_ders() -> &'static [Vec<u8>] {
    bundle()
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn pem_bundle_blocks_are_decoded() {
        let pem = format!(
            "junk\n{BEGIN}\nAAEC\nAw==\n{END}\n# comment\n{BEGIN}\nBAUG\n{END}\n{BEGIN}\nnot base64!!\n{END}\n"
        );
        assert_eq!(
            parse_pem_bundle(&pem),
            vec![vec![0, 1, 2, 3], vec![4, 5, 6]]
        );
        assert!(parse_pem_bundle("").is_empty());
    }
}

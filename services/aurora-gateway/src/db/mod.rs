//! The database lane (docs/21 §Phase 2 — Database lane, docs/06 §Database lane).
//!
//! * This module is the **engine-agnostic core**: the synthetic policy request
//!   (`POST https://<connection_id>.db.aurora.internal/<class>`), the stored endpoint and its
//!   TLS mode, and the authorizer a session calls for every statement (policy, approvals, rate
//!   limits and the request log live behind it, in [`crate::decide`]).
//! * [`pg`] is the PostgreSQL wire adapter: sniffing, TLS, login, classification, relay.
//!   A second engine is another adapter next to it.

pub mod pg;

use aurora_core::ApiError;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

/// DNS suffix of the synthetic hosts; a connection id is the label in front of it.
pub const HOST_SUFFIX: &str = ".db.aurora.internal";

/// 16 bytes from the operating system's CSPRNG (via the TLS crypto provider). An entropy
/// failure is unrecoverable: no key or nonce may ever be predictable.
#[allow(clippy::expect_used)]
pub fn random16() -> [u8; 16] {
    let mut b = [0u8; 16];
    rustls::crypto::aws_lc_rs::default_provider()
        .secure_random
        .fill(&mut b)
        .expect("system entropy source failed");
    b
}

/// `<connection_id>.db.aurora.internal`.
pub fn synthetic_host(connection_id: Uuid) -> String {
    format!("{connection_id}{HOST_SUFFIX}")
}

/// The connection id of a synthetic host (case-insensitive).
pub fn connection_id_of_host(host: &str) -> Option<Uuid> {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h.strip_suffix(HOST_SUFFIX)?.parse().ok()
}

/// libpq `sslmode` of the server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    Disable,
    Allow,
    Prefer,
    /// TLS without verification (verifies the chain when a `ca_pem` is stored, as libpq does).
    Require,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "disable" => Self::Disable,
            "allow" => Self::Allow,
            "prefer" => Self::Prefer,
            "require" => Self::Require,
            "verify-ca" => Self::VerifyCa,
            "verify-full" => Self::VerifyFull,
            _ => return None,
        })
    }
}

/// The stored credentials of a database connection (docs/09 §3 `database`).
pub struct DbEndpoint {
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    /// The database password: never leaves the gateway.
    pub password: Zeroizing<String>,
    pub sslmode: SslMode,
    pub ca_pem: Option<String>,
}

impl std::fmt::Debug for DbEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbEndpoint")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("dbname", &self.dbname)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("sslmode", &self.sslmode)
            .finish()
    }
}

impl DbEndpoint {
    /// Parses the decrypted credentials document. A missing `sslmode` is `require`: TLS is the
    /// default, plaintext needs an explicit `disable`/`allow`/`prefer`.
    pub fn from_credentials(raw: &[u8]) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_slice(raw).map_err(|_| "credentials are not JSON".to_owned())?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
        let need = |k: &str| s(k).filter(|x| !x.is_empty()).ok_or(format!("missing {k}"));
        let port = match v.get("port") {
            Some(serde_json::Value::Number(n)) => n.as_u64().and_then(|p| u16::try_from(p).ok()),
            Some(serde_json::Value::String(p)) => p.parse().ok(),
            None => Some(5432),
            _ => None,
        }
        .filter(|p| *p != 0)
        .ok_or("bad port")?;
        let sslmode = match s("sslmode").filter(|m| !m.is_empty()) {
            None => SslMode::Require,
            Some(m) => SslMode::parse(&m).ok_or("bad sslmode")?,
        };
        Ok(Self {
            host: need("host")?,
            port,
            dbname: need("dbname")?,
            user: need("user")?,
            password: Zeroizing::new(s("password").unwrap_or_default()),
            sslmode,
            ca_pem: s("ca_pem").filter(|p| !p.trim().is_empty()),
        })
    }
}

/// The policy verdict for one statement.
#[derive(Debug)]
pub enum DbVerdict {
    Allow,
    /// Refused (blocked, default-denied, rate limited, approval denied or expired): the agent
    /// receives the guidance message under SQLSTATE `42501`.
    Deny(ApiError),
    /// Policy could not be evaluated right now (store down): SQLSTATE `57P03`.
    Unavailable(ApiError),
}

/// Per-session policy: called once per executed statement (never for a statement that was
/// refused by classification).
#[async_trait::async_trait]
pub trait DbAuthorizer: Send + Sync {
    /// `class` is the strictest class of the statement; `statement` its text (the redacted form
    /// is shown to reviewers and stored, the raw text only feeds rule conditions).
    async fn authorize(&self, class: aurora_sqlclass::Class, statement: &str) -> DbVerdict;
}

/// An opened (admitted) database session.
pub struct DbOpened {
    pub endpoint: DbEndpoint,
    pub authorizer: Arc<dyn DbAuthorizer>,
    pub label: String,
}

pub enum DbOpenError {
    /// Not permitted (unknown, not granted, disabled, frozen, not a database app…).
    Refused(ApiError),
    /// Cannot decide right now.
    Unavailable(ApiError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_roundtrip() {
        let id = Uuid::now_v7();
        assert_eq!(connection_id_of_host(&synthetic_host(id)), Some(id));
        assert_eq!(
            connection_id_of_host(&synthetic_host(id).to_uppercase()),
            Some(id)
        );
        assert_eq!(connection_id_of_host("x.db.aurora.internal"), None);
        assert_eq!(connection_id_of_host("example.com"), None);
    }

    #[test]
    fn endpoint_parsing() {
        let e = DbEndpoint::from_credentials(
            br#"{"host":"h","port":"6543","dbname":"d","user":"u","password":"p"}"#,
        )
        .unwrap();
        assert_eq!((e.port, e.sslmode), (6543, SslMode::Require));
        assert!(!format!("{e:?}").contains("\"p\""));
        assert!(
            DbEndpoint::from_credentials(br#"{"host":"h","dbname":"d","user":"u","sslmode":"x"}"#)
                .is_err()
        );
        assert!(DbEndpoint::from_credentials(br#"{"dbname":"d","user":"u"}"#).is_err());
        let e = DbEndpoint::from_credentials(
            br#"{"host":"h","dbname":"d","user":"u","sslmode":"verify-full"}"#,
        )
        .unwrap();
        assert_eq!((e.port, e.sslmode), (5432, SslMode::VerifyFull));
    }
}

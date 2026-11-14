//! Extra trusted CAs for outbound TLS clients (`tls.extra_ca_bundle`, docs/15).
//!
//! One helper for every `reqwest` client that talks to a server an organization's private CA may
//! have signed (Vault, OIDC providers, LLM endpoints, registered self-hosted app instances). The
//! bundle only *adds* trust anchors; the client's normal roots stay.

/// Why a bundle could not be used. Never carries file contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PemError {
    Unreadable,
    Invalid,
    Empty,
}

impl PemError {
    #[must_use]
    pub fn why(self) -> &'static str {
        match self {
            Self::Unreadable => "file is not readable",
            Self::Invalid => "not a valid PEM certificate bundle",
            Self::Empty => "contains no certificate",
        }
    }
}

/// The certificates of the PEM bundle at `path`.
pub fn load_pem_bundle(path: &str) -> Result<Vec<reqwest::Certificate>, PemError> {
    let bytes = std::fs::read(std::path::Path::new(path)).map_err(|_| PemError::Unreadable)?;
    let certs = reqwest::Certificate::from_pem_bundle(&bytes).map_err(|_| PemError::Invalid)?;
    if certs.is_empty() {
        return Err(PemError::Empty);
    }
    Ok(certs)
}

/// Add the `tls.extra_ca_bundle` certificates (when set) to `builder` as root certificates.
pub fn with_extra_roots(
    mut builder: reqwest::ClientBuilder,
    extra_ca_bundle: Option<&str>,
) -> Result<reqwest::ClientBuilder, String> {
    if let Some(path) = extra_ca_bundle {
        for c in load_pem_bundle(path).map_err(|e| format!("tls.extra_ca_bundle: {}", e.why()))? {
            builder = builder.add_root_certificate(c);
        }
    }
    Ok(builder)
}

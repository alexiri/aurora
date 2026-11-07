//! Gateway CA: generation, loading, and cached leaf minting (06 §Subcommands,
//! §MITM lane).

use lru::LruCache;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    PKCS_RSA_SHA256, SanType,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use std::net::IpAddr;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use time::OffsetDateTime;

pub const LEAF_CACHE_SIZE: usize = 10_000;
const LEAF_VALIDITY: Duration = Duration::from_secs(7 * 86_400);
const LEAF_MIN_REMAINING: Duration = Duration::from_secs(86_400);

#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("{0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate generation failed: {0}")]
    Rcgen(#[from] rcgen::Error),
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
}

/// A leaf issued for the proxy listener.
pub struct IssuedLeaf {
    pub key: Arc<CertifiedKey>,
    pub not_after: std::time::SystemTime,
}

pub struct GeneratedCa {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Mints a new CA: ECDSA P-256, 10 years, certSign, pathlen 0.
pub fn generate(instance: &str) -> Result<GeneratedCa, CaError> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, format!("Aurora Gateway CA {instance}"));
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let now = OffsetDateTime::now_utc();
    p.not_before = now - time::Duration::hours(1);
    p.not_after = now + time::Duration::days(3650);
    let cert = p.self_signed(&key)?;
    Ok(GeneratedCa {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

/// Writes `ca.pem` (0644) and `ca.key` (0600) into `dir`, refusing to
/// overwrite either.
pub fn write_new(dir: &Path, ca: &GeneratedCa) -> Result<(), CaError> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let pem = dir.join("ca.pem");
    let key = dir.join("ca.key");
    if pem.exists() || key.exists() {
        return Err(CaError::Invalid(format!(
            "refusing to overwrite an existing CA in {}",
            dir.display()
        )));
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(dir)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key)?;
    f.write_all(ca.key_pem.as_bytes())?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&pem)?;
    f.write_all(ca.cert_pem.as_bytes())?;
    Ok(())
}

struct Leaf {
    config: Arc<ServerConfig>,
    minted: Instant,
}

pub struct Ca {
    issuer: Certificate,
    key: KeyPair,
    cert_pem: String,
    leaves: Mutex<LruCache<String, Leaf>>,
    minted: AtomicU64,
}

impl Ca {
    pub fn load(dir: &Path) -> Result<Self, CaError> {
        let cert_pem = std::fs::read_to_string(dir.join("ca.pem"))?;
        let key_pem = std::fs::read_to_string(dir.join("ca.key"))?;
        Self::from_pem(&cert_pem, &key_pem)
    }

    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, CaError> {
        let key = KeyPair::from_pem(key_pem)?;
        if !key.is_compatible(&PKCS_ECDSA_P256_SHA256) && !key.is_compatible(&PKCS_RSA_SHA256) {
            return Err(CaError::Invalid("CA key must be ECDSA P-256 or RSA".into()));
        }
        // Fails when the certificate is not a CA.
        let params = CertificateParams::from_ca_cert_pem(cert_pem)
            .map_err(|e| CaError::Invalid(format!("not a usable CA certificate: {e}")))?;
        if !params.key_usages.contains(&KeyUsagePurpose::KeyCertSign) {
            return Err(CaError::Invalid("CA certificate lacks keyCertSign".into()));
        }
        let issuer = params.self_signed(&key)?;
        let size = NonZeroUsize::new(LEAF_CACHE_SIZE).unwrap_or(NonZeroUsize::MIN);
        Ok(Self {
            issuer,
            key,
            cert_pem: cert_pem.to_owned(),
            leaves: Mutex::new(LruCache::new(size)),
            minted: AtomicU64::new(0),
        })
    }

    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Number of leaves minted so far (cache effectiveness).
    pub fn minted(&self) -> u64 {
        self.minted.load(Ordering::Relaxed)
    }

    /// TLS server config presenting a leaf for `host` (ALPN h2, http/1.1).
    pub fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>, CaError> {
        let host = crate::guard::normalize_host(host);
        {
            let mut c = self.leaves.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(l) = c.get(&host)
                && l.minted.elapsed() + LEAF_MIN_REMAINING < LEAF_VALIDITY
            {
                return Ok(l.config.clone());
            }
        }
        let config = self.mint(&host)?;
        self.minted.fetch_add(1, Ordering::Relaxed);
        let mut c = self.leaves.lock().unwrap_or_else(|e| e.into_inner());
        c.put(
            host,
            Leaf {
                config: config.clone(),
                minted: Instant::now(),
            },
        );
        Ok(config)
    }

    /// (phase 1, RFE-002) Issues the proxy listener's leaf: `serverAuth`, ECDSA P-256, SANs =
    /// `names` (DNS names and IP literals), valid for `validity`. The key is generated in memory
    /// and never stored (docs/06 §Proxy listener TLS).
    pub fn issue_proxy_leaf(
        &self,
        names: &[String],
        validity: Duration,
    ) -> Result<IssuedLeaf, CaError> {
        if names.is_empty() {
            return Err(CaError::Invalid(
                "proxy certificate needs at least one SAN".into(),
            ));
        }
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut p = CertificateParams::default();
        let mut sans = Vec::with_capacity(names.len());
        for n in names {
            sans.push(match n.parse::<IpAddr>() {
                Ok(ip) => SanType::IpAddress(ip),
                Err(_) => SanType::DnsName(
                    n.clone()
                        .try_into()
                        .map_err(|_| CaError::Invalid(format!("invalid SAN {n:?}")))?,
                ),
            });
        }
        p.subject_alt_names = sans;
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            names[0].chars().take(64).collect::<String>(),
        );
        p.distinguished_name = dn;
        p.is_ca = IsCa::NoCa;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        // Strict verifiers (OpenSSL 3.x `VERIFY_X509_STRICT`, as Python 3.13+ enables) require the
        // leaf to carry the issuer's key identifier.
        p.use_authority_key_identifier_extension = true;
        let now = OffsetDateTime::now_utc();
        p.not_before = now - time::Duration::hours(1);
        let secs = i64::try_from(validity.as_secs()).unwrap_or(i64::MAX / 2);
        let not_after = now + time::Duration::seconds(secs);
        p.not_after = not_after;
        let cert = p.signed_by(&key, &self.issuer, &self.key)?;
        let chain = vec![CertificateDer::from(cert.der().to_vec())];
        let pk = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let signer = rustls::crypto::aws_lc_rs::sign::any_ecdsa_type(&pk)?;
        Ok(IssuedLeaf {
            key: Arc::new(CertifiedKey::new(chain, signer)),
            not_after: std::time::SystemTime::UNIX_EPOCH
                + Duration::from_secs(u64::try_from(not_after.unix_timestamp()).unwrap_or(0)),
        })
    }

    fn mint(&self, host: &str) -> Result<Arc<ServerConfig>, CaError> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut p = CertificateParams::default();
        p.subject_alt_names = vec![match host.parse::<IpAddr>() {
            Ok(ip) => SanType::IpAddress(ip),
            Err(_) => SanType::DnsName(
                host.to_owned()
                    .try_into()
                    .map_err(|_| CaError::Invalid(format!("invalid host {host:?}")))?,
            ),
        }];
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            host.chars().take(64).collect::<String>(),
        );
        p.distinguished_name = dn;
        p.is_ca = IsCa::NoCa;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.use_authority_key_identifier_extension = true;
        let now = OffsetDateTime::now_utc();
        p.not_before = now - time::Duration::hours(1);
        p.not_after = now + time::Duration::days(7);
        let cert = p.signed_by(&key, &self.issuer, &self.key)?;
        let chain = vec![CertificateDer::from(cert.der().to_vec())];
        let pk = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut cfg = ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(chain, pk)?;
        cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Arc::new(cfg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_load_mint_cache() {
        let g = generate("test").unwrap();
        let ca = Ca::from_pem(&g.cert_pem, &g.key_pem).unwrap();
        let a = ca.server_config("Example.com.").unwrap();
        let b = ca.server_config("example.com").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(ca.minted(), 1);
        ca.server_config("127.0.0.1").unwrap();
        assert_eq!(ca.minted(), 2);
    }

    #[test]
    fn write_modes_and_no_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("aurora-ca-{}", uuid::Uuid::now_v7()));
        let g = generate("t").unwrap();
        write_new(&d, &g).unwrap();
        let m = |f: &str| std::fs::metadata(d.join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(m("ca.key"), 0o600);
        assert_eq!(m("ca.pem"), 0o644);
        assert!(write_new(&d, &g).is_err());
        Ca::load(&d).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rejects_non_ca() {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["a.test".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        assert!(Ca::from_pem(&cert.pem(), &key.serialize_pem()).is_err());
    }
}

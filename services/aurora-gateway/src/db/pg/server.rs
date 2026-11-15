//! The gateway's own connection to the real PostgreSQL server: guarded dial, TLS per `sslmode`,
//! startup and authentication (cleartext, MD5, SCRAM-SHA-256) with the *stored* credentials.
//! The database password is used here and nowhere else; it never reaches the agent.

use super::wire::{self, CANCEL_REQUEST, SSL_REQUEST};
use crate::db::{DbEndpoint, SslMode};
use crate::guard::{DialError, Guard};
use crate::upstream::{EgressProxy, dial};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, RootCertStore, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// Whole-connection budget (dial, TLS, authentication).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub type BoxIo = Box<dyn Io>;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("dial: {0:?}")]
    Dial(DialError),
    #[error("{0}")]
    Tls(String),
    #[error("{0}")]
    Protocol(String),
    /// The server answered with an ErrorResponse during startup (authentication outcome).
    #[error("server refused the connection: {0}")]
    Server(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("timed out")]
    Timeout,
}

/// An authenticated server session, positioned after the first `ReadyForQuery`.
pub struct ServerConn {
    pub io: BoxIo,
    pub pid: u32,
    pub key: u32,
    /// `ParameterStatus` messages (already framed) to replay to the client.
    pub params: Vec<Vec<u8>>,
}

#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(m, c, s, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(m, c, s, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// `verify-ca`: the chain is verified, the host name is not.
#[derive(Debug)]
struct ChainOnly(Arc<rustls::client::WebPkiServerVerifier>);

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        inter: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        match self.0.verify_server_cert(end, inter, name, ocsp, now) {
            Err(TlsError::InvalidCertificate(
                rustls::CertificateError::NotValidForName
                | rustls::CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.0.verify_tls12_signature(m, c, s)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.0.verify_tls13_signature(m, c, s)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

fn roots(ca_pem: Option<&str>) -> Result<RootCertStore, ConnectError> {
    let mut roots = RootCertStore::empty();
    match ca_pem {
        Some(pem) => {
            let mut any = false;
            for c in CertificateDer::pem_slice_iter(pem.as_bytes()) {
                let c = c.map_err(|e| ConnectError::Tls(format!("bad ca_pem: {e}")))?;
                roots
                    .add(c)
                    .map_err(|e| ConnectError::Tls(format!("bad ca_pem: {e}")))?;
                any = true;
            }
            if !any {
                return Err(ConnectError::Tls("ca_pem holds no certificate".into()));
            }
        }
        None => {
            for c in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(c);
            }
        }
    }
    Ok(roots)
}

/// TLS client configuration for `sslmode` (`None` = no TLS).
pub fn tls_config(ep: &DbEndpoint) -> Result<Option<Arc<ClientConfig>>, ConnectError> {
    if ep.sslmode == SslMode::Disable {
        return Ok(None);
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let b = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| ConnectError::Tls(e.to_string()))?;
    let cfg = match ep.sslmode {
        SslMode::VerifyFull => b
            .with_root_certificates(roots(ep.ca_pem.as_deref())?)
            .with_no_client_auth(),
        SslMode::VerifyCa => {
            let v = rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots(ep.ca_pem.as_deref())?),
                provider,
            )
            .build()
            .map_err(|e| ConnectError::Tls(e.to_string()))?;
            b.dangerous()
                .with_custom_certificate_verifier(Arc::new(ChainOnly(v)))
                .with_no_client_auth()
        }
        // libpq: `require` with a root certificate behaves as `verify-ca`.
        _ if ep.ca_pem.is_some() => {
            let v = rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots(ep.ca_pem.as_deref())?),
                provider,
            )
            .build()
            .map_err(|e| ConnectError::Tls(e.to_string()))?;
            b.dangerous()
                .with_custom_certificate_verifier(Arc::new(ChainOnly(v)))
                .with_no_client_auth()
        }
        // (FIX-106, D-178 Q-R5-7) `require` without a stored CA verifies the server certificate
        // (chain and host name) against the system roots: the stored password is not sent to a
        // server that cannot prove who it is. The only way around it is the connection's explicit,
        // audited opt-out (`metadata.tls_verification = "off"`, for a private CA whose
        // certificate is not stored), which is counted and logged on every session.
        SslMode::Require if ep.verify_server => {
            b.with_root_certificates(roots(None)?).with_no_client_auth()
        }
        SslMode::Require => {
            metrics::counter!("aurora_gateway_db_tls_unverified_total").increment(1);
            tracing::warn!(
                host = %ep.host,
                "database TLS server certificate is not verified (the connection's tls_verification opt-out)"
            );
            b.dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
                .with_no_client_auth()
        }
        // `prefer` / `allow` are opportunistic by definition (libpq): TLS when offered, unverified.
        _ => b
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth(),
    };
    Ok(Some(Arc::new(cfg)))
}

/// SSLRequest negotiation and TLS upgrade per `sslmode`.
async fn negotiate(mut tcp: TcpStream, ep: &DbEndpoint) -> Result<BoxIo, ConnectError> {
    let Some(cfg) = tls_config(ep)? else {
        return Ok(Box::new(tcp));
    };
    let mut req = 8u32.to_be_bytes().to_vec();
    req.extend_from_slice(&SSL_REQUEST.to_be_bytes());
    tcp.write_all(&req)
        .await
        .map_err(|e| ConnectError::Protocol(e.to_string()))?;
    let mut answer = [0u8; 1];
    tcp.read_exact(&mut answer)
        .await
        .map_err(|e| ConnectError::Protocol(e.to_string()))?;
    match answer[0] {
        b'S' => {
            let name = ServerName::try_from(ep.host.clone())
                .map_err(|_| ConnectError::Tls("invalid server name".into()))?;
            let tls = TlsConnector::from(cfg)
                .connect(name, tcp)
                .await
                .map_err(|e| ConnectError::Tls(format!("server TLS handshake failed: {e}")))?;
            Ok(Box::new(tls))
        }
        b'N' if matches!(ep.sslmode, SslMode::Allow | SslMode::Prefer) => Ok(Box::new(tcp)),
        b'N' => Err(ConnectError::Tls(
            "the server does not support TLS and the connection requires it".into(),
        )),
        _ => Err(ConnectError::Protocol(
            "unexpected SSLRequest answer".into(),
        )),
    }
}

async fn open(
    guard: &Guard,
    egress: Option<&EgressProxy>,
    ep: &DbEndpoint,
) -> Result<BoxIo, ConnectError> {
    let tcp = dial(guard, egress, &ep.host, ep.port)
        .await
        .map_err(ConnectError::Dial)?;
    negotiate(tcp, ep).await
}

/// Connects, authenticates and reads the startup messages up to `ReadyForQuery`.
pub async fn connect(
    guard: &Guard,
    egress: Option<&EgressProxy>,
    ep: &DbEndpoint,
    application_name: Option<&str>,
) -> Result<ServerConn, ConnectError> {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let mut io = open(guard, egress, ep).await?;
        let mut params = vec![
            ("user", ep.user.as_str()),
            ("database", ep.dbname.as_str()),
            // The classifier reads statement text as UTF-8; the session must too.
            ("client_encoding", "UTF8"),
        ];
        if let Some(a) = application_name {
            params.push(("application_name", a));
        }
        wire::write_all(&mut io, &wire::startup_message(&params))
            .await
            .map_err(|e| ConnectError::Protocol(e.to_string()))?;
        authenticate(&mut io, ep).await?;
        let mut out = ServerConn {
            io,
            pid: 0,
            key: 0,
            params: Vec::new(),
        };
        loop {
            let f = recv(&mut out.io).await?;
            match f.tag {
                b'S' => out.params.push(wire::encode(b'S', &f.body)),
                b'K' if f.body.len() == 8 => {
                    out.pid = u32::from_be_bytes([f.body[0], f.body[1], f.body[2], f.body[3]]);
                    out.key = u32::from_be_bytes([f.body[4], f.body[5], f.body[6], f.body[7]]);
                }
                b'N' | b'v' => {}
                b'Z' => return Ok(out),
                b'E' => return Err(ConnectError::Server(wire::error_message(&f.body))),
                t => {
                    return Err(ConnectError::Protocol(format!(
                        "unexpected message {:?} during startup",
                        t as char
                    )));
                }
            }
        }
    })
    .await
    .map_err(|_| ConnectError::Timeout)?
}

/// Sends a `CancelRequest` for the server-side session `(pid, key)`; best effort.
pub async fn cancel(
    guard: &Guard,
    egress: Option<&EgressProxy>,
    ep: &DbEndpoint,
    pid: u32,
    key: u32,
) -> Result<(), ConnectError> {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let mut io = open(guard, egress, ep).await?;
        let mut m = 16u32.to_be_bytes().to_vec();
        m.extend_from_slice(&CANCEL_REQUEST.to_be_bytes());
        m.extend_from_slice(&pid.to_be_bytes());
        m.extend_from_slice(&key.to_be_bytes());
        wire::write_all(&mut io, &m)
            .await
            .map_err(|e| ConnectError::Protocol(e.to_string()))?;
        // The server closes without a reply; wait for it so the request is not cut short.
        let mut b = [0u8; 1];
        let _ = tokio::time::timeout(Duration::from_secs(2), io.read(&mut b)).await;
        Ok(())
    })
    .await
    .map_err(|_| ConnectError::Timeout)?
}

async fn recv(io: &mut BoxIo) -> Result<wire::Frame, ConnectError> {
    wire::read_frame(io, 1 << 20, 1 << 20)
        .await
        .map_err(|e| ConnectError::Protocol(e.to_string()))?
        .ok_or_else(|| ConnectError::Protocol("server closed the connection".into()))
}

async fn send(io: &mut BoxIo, tag: u8, body: &[u8]) -> Result<(), ConnectError> {
    wire::write_all(io, &wire::encode(tag, body))
        .await
        .map_err(|e| ConnectError::Protocol(e.to_string()))
}

fn password_body(p: &str) -> Vec<u8> {
    let mut b = p.as_bytes().to_vec();
    b.push(0);
    b
}

async fn authenticate(io: &mut BoxIo, ep: &DbEndpoint) -> Result<(), ConnectError> {
    let mut scram: Option<Scram> = None;
    loop {
        let f = recv(io).await?;
        match f.tag {
            b'E' => return Err(ConnectError::Server(wire::error_message(&f.body))),
            b'N' | b'v' => continue,
            b'R' if f.body.len() >= 4 => {}
            t => {
                return Err(ConnectError::Protocol(format!(
                    "unexpected message {:?} during authentication",
                    t as char
                )));
            }
        }
        let code = u32::from_be_bytes([f.body[0], f.body[1], f.body[2], f.body[3]]);
        let rest = &f.body[4..];
        match code {
            0 => return Ok(()),
            3 => send(io, b'p', &password_body(&ep.password)).await?,
            5 if rest.len() == 4 => {
                use md5::{Digest as _, Md5};
                let inner = hex::encode(Md5::digest(format!("{}{}", *ep.password, ep.user)));
                let mut h = Md5::new();
                h.update(inner.as_bytes());
                h.update(rest);
                send(
                    io,
                    b'p',
                    &password_body(&format!("md5{}", hex::encode(h.finalize()))),
                )
                .await?;
            }
            10 => {
                let mechs: Vec<&[u8]> = rest.split(|b| *b == 0).collect();
                if !mechs.contains(&&b"SCRAM-SHA-256"[..]) {
                    return Err(ConnectError::Unsupported(
                        "the server offers no supported SASL mechanism".into(),
                    ));
                }
                let s = Scram::new();
                let first = s.client_first();
                let mut body = b"SCRAM-SHA-256\0".to_vec();
                body.extend_from_slice(&u32::try_from(first.len()).unwrap_or(0).to_be_bytes());
                body.extend_from_slice(first.as_bytes());
                send(io, b'p', &body).await?;
                scram = Some(s);
            }
            11 => {
                let s = scram
                    .as_mut()
                    .ok_or_else(|| ConnectError::Protocol("unexpected SASLContinue".into()))?;
                let fin = s
                    .client_final(
                        std::str::from_utf8(rest).map_err(|_| bad_scram())?,
                        &ep.password,
                    )
                    .ok_or_else(bad_scram)?;
                send(io, b'p', fin.as_bytes()).await?;
            }
            12 => {
                let s = scram
                    .as_ref()
                    .ok_or_else(|| ConnectError::Protocol("unexpected SASLFinal".into()))?;
                if !s.verify_server(std::str::from_utf8(rest).map_err(|_| bad_scram())?) {
                    return Err(ConnectError::Protocol(
                        "the server failed SCRAM verification".into(),
                    ));
                }
            }
            c => {
                return Err(ConnectError::Unsupported(format!(
                    "authentication method {c} is not supported"
                )));
            }
        }
    }
}

fn bad_scram() -> ConnectError {
    ConnectError::Protocol("malformed SCRAM exchange".into())
}

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length; the error arm cannot happen.
    match <HmacSha256 as Mac>::new_from_slice(key) {
        Ok(mut m) => {
            m.update(data);
            m.finalize().into_bytes().to_vec()
        }
        Err(_) => Vec::new(),
    }
}

/// PBKDF2-HMAC-SHA256 with a single output block.
fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut s = salt.to_vec();
    s.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password, &s);
    let mut out = u.clone();
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (o, x) in out.iter_mut().zip(&u) {
            *o ^= x;
        }
    }
    out
}

struct Scram {
    nonce: String,
    client_first_bare: String,
    auth_message: String,
    server_key_sig: Vec<u8>,
}

/// Most iterations accepted from a server (a malicious one could otherwise burn CPU).
const MAX_SCRAM_ITERATIONS: u32 = 1_000_000;

impl Scram {
    fn new() -> Self {
        let nonce = B64.encode(crate::db::random16());
        Self {
            client_first_bare: format!("n=*,r={nonce}"),
            nonce,
            auth_message: String::new(),
            server_key_sig: Vec::new(),
        }
    }

    fn client_first(&self) -> String {
        format!("n,,{}", self.client_first_bare)
    }

    fn client_final(&mut self, server_first: &str, password: &str) -> Option<String> {
        let (mut r, mut s, mut i) = (None, None, None);
        for part in server_first.split(',') {
            match part.split_once('=')? {
                ("r", v) => r = Some(v),
                ("s", v) => s = Some(v),
                ("i", v) => i = Some(v),
                _ => {}
            }
        }
        let (r, s, i) = (r?, B64.decode(s?).ok()?, i?.parse::<u32>().ok()?);
        if !r.starts_with(&self.nonce) || i == 0 || i > MAX_SCRAM_ITERATIONS {
            return None;
        }
        let salted = pbkdf2(password.as_bytes(), &s, i);
        let client_key = hmac(&salted, b"Client Key");
        let stored = Sha256::digest(&client_key);
        let without_proof = format!("c=biws,r={r}");
        self.auth_message = format!("{},{server_first},{without_proof}", self.client_first_bare);
        let sig = hmac(&stored, self.auth_message.as_bytes());
        let proof: Vec<u8> = client_key.iter().zip(&sig).map(|(a, b)| a ^ b).collect();
        let server_key = hmac(&salted, b"Server Key");
        self.server_key_sig = hmac(&server_key, self.auth_message.as_bytes());
        Some(format!("{without_proof},p={}", B64.encode(proof)))
    }

    fn verify_server(&self, server_final: &str) -> bool {
        let Some(v) = server_final.strip_prefix("v=") else {
            return false;
        };
        let Ok(got) = B64.decode(v.trim()) else {
            return false;
        };
        // constant-time comparison through the MAC verifier is overkill here: both are
        // derived from the exchange; compare without early exit anyway.
        got.len() == self.server_key_sig.len()
            && got
                .iter()
                .zip(&self.server_key_sig)
                .fold(0u8, |a, (x, y)| a | (x ^ y))
                == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7677 test vector (user "user", password "pencil").
    #[test]
    fn scram_rfc7677() {
        let mut s = Scram {
            nonce: "rOprNGfwEbeRWgbNEkqO".into(),
            client_first_bare: "n=user,r=rOprNGfwEbeRWgbNEkqO".into(),
            auth_message: String::new(),
            server_key_sig: Vec::new(),
        };
        let fin = s
            .client_final(
                "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
                "pencil",
            )
            .unwrap();
        assert_eq!(
            fin,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        assert!(s.verify_server("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="));
        assert!(!s.verify_server("v=AAAA"));
    }

    #[test]
    fn scram_rejects_foreign_nonce_and_huge_iterations() {
        let mut s = Scram::new();
        assert!(s.client_final("r=other,s=AAAA,i=4096", "p").is_none());
        let n = s.nonce.clone();
        assert!(
            s.client_final(&format!("r={n}x,s=AAAA,i=99999999"), "p")
                .is_none()
        );
        assert!(s.client_final(&format!("r={n}x,s=AAAA,i=2"), "p").is_some());
    }
}

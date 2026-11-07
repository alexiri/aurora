//! `aurora run --forward`: a loopback forwarder for clients that cannot speak to an HTTPS proxy
//! (stock Java, some embedded HTTP stacks). It listens on `127.0.0.1` (plain HTTP, no credentials
//! on that hop), adds the gateway credential and speaks **TLS** to the gateway proxy listener —
//! the same pattern as the sandbox supervisor (docs/06 §Proxy listener TLS, docs/12). Verification
//! is never optional and there is no plaintext fallback.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;

const MAX_HEAD: usize = 64 * 1024;

/// Parsed `scheme://[user[:pass]@]host[:port]` gateway proxy URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUrl {
    pub scheme: String,
    pub user: String,
    pub pass: String,
    pub host: String,
    pub port: u16,
}

impl ProxyUrl {
    pub fn parse(url: &str) -> Result<Self> {
        let (scheme, rest) = url
            .split_once("://")
            .with_context(|| format!("`{}` is not a URL", redact(url)))?;
        let auth = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let (userinfo, hostport) = match auth.rsplit_once('@') {
            Some((u, h)) => (u, h),
            None => ("", auth),
        };
        let (user, pass) = userinfo.split_once(':').unwrap_or((userinfo, ""));
        let (host, port) = if let Some(r) = hostport.strip_prefix('[') {
            let (h, p) = r.split_once(']').context("bad IPv6 host")?;
            (h, p.strip_prefix(':'))
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (hostport, None),
            }
        };
        if host.is_empty() {
            bail!("the gateway proxy URL has no host");
        }
        let port = match port {
            Some(p) => p.parse().context("bad port in the gateway proxy URL")?,
            None if scheme == "https" => 443,
            None => 80,
        };
        Ok(Self {
            scheme: scheme.to_ascii_lowercase(),
            user: user.to_owned(),
            pass: pass.to_owned(),
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    pub fn is_loopback(&self) -> bool {
        self.host == "localhost"
            || self
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }
}

fn redact(url: &str) -> String {
    match (url.split_once("://"), url.rsplit_once('@')) {
        (Some((s, _)), Some((_, h))) => format!("{s}://…@{h}"),
        _ => url.to_owned(),
    }
}

/// SEC-40: a gateway URL for a non-loopback host must be `https://`; the CLI neither emits nor
/// accepts a cleartext one.
pub fn require_tls_url(url: &str) -> Result<()> {
    let u = ProxyUrl::parse(url)?;
    if u.scheme != "https" && !u.is_loopback() {
        bail!(
            "refusing the gateway URL {}: the gateway proxy listener is TLS-only, so a non-loopback gateway must be reached with https://",
            redact(url)
        );
    }
    Ok(())
}

/// The TLS-only upstream: trust anchors, dialled address, verified name, credential.
#[derive(Clone)]
pub struct Upstream {
    addr: (String, u16),
    connector: TlsConnector,
    name: ServerName<'static>,
    auth: Arc<str>,
}

impl Upstream {
    /// `ca_pem` is trusted for the proxy leg; with `system_roots` (operator certificate mode) the
    /// platform roots are trusted instead.
    pub fn new(url: &ProxyUrl, ca_pem: &str, system_roots: bool) -> Result<Self> {
        if url.scheme != "https" {
            bail!("the gateway proxy URL must be https:// (TLS-only listener)");
        }
        let mut roots = RootCertStore::empty();
        if system_roots {
            for c in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(c);
            }
        } else {
            for c in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
                roots.add(c.context("gateway CA PEM")?)?;
            }
        }
        if roots.is_empty() {
            bail!("no trust anchor for the gateway proxy certificate");
        }
        let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        let name = ServerName::try_from(url.host.clone())
            .with_context(|| format!("bad gateway host {:?}", url.host))?;
        let raw = format!("{}:{}", url.user, url.pass);
        let auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        );
        Ok(Self {
            addr: (url.host.clone(), url.port),
            connector: TlsConnector::from(Arc::new(cfg)),
            name,
            auth: auth.into(),
        })
    }

    async fn connect(&self) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let tcp = TcpStream::connect((self.addr.0.as_str(), self.addr.1)).await?;
        let _ = tcp.set_nodelay(true);
        self.connector.connect(self.name.clone(), tcp).await
    }
}

/// A running forwarder; dropping it stops accepting.
pub struct Forwarder {
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Forwarder {
    /// `http://127.0.0.1:<port>` (no credentials: the forwarder adds them).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Listen on `127.0.0.1:0` and forward to `upstream` over TLS.
pub async fn spawn(upstream: Upstream) -> Result<Forwarder> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("cannot bind the loopback forwarder")?;
    let addr = listener.local_addr()?;
    let up = Arc::new(upstream);
    let task = tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else {
                continue;
            };
            let up = up.clone();
            tokio::spawn(async move {
                let _ = handle(s, &up).await;
            });
        }
    });
    Ok(Forwarder { addr, task })
}

async fn read_head<R: AsyncReadExt + Unpin>(s: &mut R) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(pos + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(std::io::Error::other("head too large"));
        }
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn respond(s: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(msg.as_bytes()).await
}

async fn handle(mut client: TcpStream, up: &Upstream) -> std::io::Result<()> {
    let (head, rest) = read_head(&mut client).await?;
    let Ok(head) = String::from_utf8(head) else {
        return respond(&mut client, "400 Bad Request", "").await;
    };
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (method, target) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let connect = method.eq_ignore_ascii_case("CONNECT");
    if connect && !target.contains(':') || !connect && !target.starts_with("http://") {
        return respond(&mut client, "400 Bad Request", "").await;
    }
    let mut upstream = match up.connect().await {
        Ok(s) => s,
        Err(e) => {
            // Fails closed: no credential is written unless the handshake verified.
            return respond(
                &mut client,
                "502 Bad Gateway",
                &format!("aurora: cannot reach the gateway over TLS: {e}\n"),
            )
            .await;
        }
    };
    if connect {
        let req = format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n\r\n",
            up.auth
        );
        upstream.write_all(req.as_bytes()).await?;
        let (resp_head, resp_rest) = read_head(&mut upstream).await?;
        client.write_all(&resp_head).await?;
        client.write_all(&resp_rest).await?;
        if !(resp_head.starts_with(b"HTTP/1.1 2") || resp_head.starts_with(b"HTTP/1.0 2")) {
            return Ok(());
        }
        upstream.write_all(&rest).await?;
    } else {
        let mut out = String::with_capacity(head.len() + 128);
        out.push_str(request_line);
        out.push_str("\r\n");
        for line in lines.filter(|l| !l.is_empty()) {
            let name = line.split(':').next().unwrap_or_default().trim();
            if ["proxy-authorization", "proxy-connection", "connection"]
                .iter()
                .any(|h| name.eq_ignore_ascii_case(h))
            {
                continue;
            }
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str(&format!(
            "Proxy-Authorization: {}\r\nConnection: close\r\n\r\n",
            up.auth
        ));
        upstream.write_all(out.as_bytes()).await?;
        upstream.write_all(&rest).await?;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proxy_urls() {
        let u = ProxyUrl::parse("https://x:aug_tok@gw.example.org:4102").unwrap();
        assert_eq!(
            (u.user.as_str(), u.pass.as_str(), u.host.as_str(), u.port),
            ("x", "aug_tok", "gw.example.org", 4102)
        );
        let u = ProxyUrl::parse("https://[::1]:4102").unwrap();
        assert!(u.is_loopback() && u.host == "::1");
        assert!(ProxyUrl::parse("no-scheme").is_err());
    }

    #[test]
    fn cleartext_gateway_urls_are_refused_except_on_loopback() {
        assert!(require_tls_url("https://x:t@gw.example.org:4102").is_ok());
        assert!(require_tls_url("http://127.0.0.1:3128").is_ok());
        assert!(require_tls_url("http://localhost:3128").is_ok());
        let e = require_tls_url("http://x:aug_secret@gw.example.org:4102")
            .unwrap_err()
            .to_string();
        assert!(e.contains("TLS-only"), "{e}");
        assert!(
            !e.contains("aug_secret"),
            "the token must not leak into errors: {e}"
        );
    }

    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa,
        KeyPair, KeyUsagePurpose,
    };
    use std::sync::Mutex;

    /// TLS stub gateway on 127.0.0.1 (leaf for `localhost`); records the CONNECT head it sees.
    async fn stub_gateway() -> (String, u16, Arc<Mutex<Vec<String>>>) {
        let ca_key = KeyPair::generate().unwrap();
        let mut p = CertificateParams::default();
        p.distinguished_name = DistinguishedName::new();
        p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca = p.self_signed(&ca_key).unwrap();
        let key = KeyPair::generate().unwrap();
        let mut lp = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
        lp.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf = lp.signed_by(&key, &ca, &ca_key).unwrap();
        let chain = vec![CertificateDer::from(leaf.der().to_vec())];
        let pk = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()),
        );
        let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, pk)
        .unwrap();
        let acc = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let s2 = seen.clone();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = l.accept().await.unwrap();
                let (acc, seen) = (acc.clone(), s2.clone());
                tokio::spawn(async move {
                    let Ok(mut t) = acc.accept(tcp).await else {
                        return;
                    };
                    let Ok((head, _)) = read_head(&mut t).await else {
                        return;
                    };
                    seen.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&head).into_owned());
                    let _ = t
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await;
                    let _ = t.shutdown().await;
                });
            }
        });
        (ca.pem(), port, seen)
    }

    async fn roundtrip(f: &Forwarder) -> String {
        let mut c = TcpStream::connect(f.addr).await.unwrap();
        c.write_all(b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ =
            tokio::time::timeout(std::time::Duration::from_secs(5), c.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn forwarder_speaks_tls_upstream_and_adds_the_credential() {
        let (ca, port, seen) = stub_gateway().await;
        let url = ProxyUrl::parse(&format!("https://x:aug_tok@localhost:{port}")).unwrap();
        let f = spawn(Upstream::new(&url, &ca, false).unwrap())
            .await
            .unwrap();
        assert!(f.url().starts_with("http://127.0.0.1:"));
        let r = roundtrip(&f).await;
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        let head = seen.lock().unwrap()[0].clone();
        let want = base64::engine::general_purpose::STANDARD.encode("x:aug_tok");
        assert!(
            head.contains(&format!("Proxy-Authorization: Basic {want}")),
            "{head}"
        );
    }

    #[tokio::test]
    async fn forwarder_refuses_wrong_name_and_untrusted_ca_without_sending_a_credential() {
        let (ca, port, seen) = stub_gateway().await;
        // wrong name: dial 127.0.0.1 but the certificate only covers `localhost`
        let url = ProxyUrl::parse(&format!("https://x:aug_tok@127.0.0.1:{port}")).unwrap();
        let f = spawn(Upstream::new(&url, &ca, false).unwrap())
            .await
            .unwrap();
        let r = roundtrip(&f).await;
        assert!(r.starts_with("HTTP/1.1 502"), "{r}");
        // untrusted CA
        let (other_ca, _, _) = stub_gateway().await;
        let url = ProxyUrl::parse(&format!("https://x:aug_tok@localhost:{port}")).unwrap();
        let f = spawn(Upstream::new(&url, &other_ca, false).unwrap())
            .await
            .unwrap();
        let r = roundtrip(&f).await;
        assert!(r.starts_with("HTTP/1.1 502"), "{r}");
        assert!(seen.lock().unwrap().is_empty(), "no credential may be sent");
    }

    #[test]
    fn plaintext_upstream_url_is_rejected() {
        let url = ProxyUrl::parse("http://x:t@gw:4102").unwrap();
        assert!(Upstream::new(&url, "", false).is_err());
    }
}

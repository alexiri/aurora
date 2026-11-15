//! Loopback proxies (`docs/12-supervisor-harness.md` §Loopback proxies).
//!
//! Each listener forwards to the gateway proxy address, strips any client-supplied
//! `Proxy-Authorization` and adds its own credential. Absolute-form requests are forwarded with
//! `Connection: close` so the credential is re-evaluated for every request.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use tokio_rustls::TlsConnector;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const MAX_HEAD: usize = 64 * 1024;

/// The credential a proxy adds: a fixed boot token, optionally overridden by a turn context
/// credential while a turn is active.
pub struct Credential {
    username: &'static str,
    boot: String,
    context: Mutex<Option<String>>,
}

// (FIX-114, SEC-01) Never prints the boot token or a context credential.
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("boot", &"[redacted]")
            .field("context", &"[redacted]")
            .finish()
    }
}

impl Credential {
    pub fn new(username: &'static str, boot_token: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            username,
            boot: boot_token.into(),
            context: Mutex::new(None),
        })
    }

    /// Switch to `auc_…` (turn start) or back to the boot token (`None`, turn end).
    pub fn set_context(&self, ctx: Option<String>) {
        if let Ok(mut g) = self.context.lock() {
            *g = ctx;
        }
    }

    /// The secret in force now: the turn context credential, else the boot token (database relay,
    /// `crate::pgrelay`; never handed to anything the agent can read).
    pub fn secret(&self) -> String {
        self.context
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| self.boot.clone())
    }

    /// Value of the `Proxy-Authorization` header for the current state.
    pub fn header_value(&self) -> String {
        let secret = self
            .context
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| self.boot.clone());
        let raw = format!("{}:{secret}", self.username);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }
}

/// (phase 1, RFE-002) The gateway proxy address and how to reach it: always TLS, verified against
/// the boot document's CA and the gateway's server name; never a plaintext fallback.
#[derive(Clone)]
pub struct GatewayUpstream {
    /// `host:port` dialled.
    pub addr: String,
    connector: TlsConnector,
    /// Same trust, no ALPN: the PostgreSQL lane (`connect_db`).
    db_connector: TlsConnector,
    name: ServerName<'static>,
}

impl std::fmt::Debug for GatewayUpstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayUpstream")
            .field("addr", &self.addr)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl GatewayUpstream {
    /// Trusts exactly `ca_pem` (the gateway CA, or the operator CA) and verifies `server_name`.
    pub fn new(addr: impl Into<String>, ca_pem: &str, server_name: &str) -> Result<Self, String> {
        let mut roots = RootCertStore::empty();
        let mut n = 0;
        for c in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
            let c = c.map_err(|e| format!("gateway CA PEM: {e}"))?;
            roots.add(c).map_err(|e| format!("gateway CA: {e}"))?;
            n += 1;
        }
        if n == 0 {
            return Err("gateway CA PEM holds no certificate".into());
        }
        let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let db_cfg = cfg.clone();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        let name = ServerName::try_from(server_name.to_owned())
            .map_err(|e| format!("gateway server name {server_name:?}: {e}"))?;
        Ok(Self {
            addr: addr.into(),
            connector: TlsConnector::from(Arc::new(cfg)),
            db_connector: TlsConnector::from(Arc::new(db_cfg)),
            name,
        })
    }

    /// PostgreSQL lane (`crate::pgrelay`): dial, `SSLRequest`, `S`, then TLS verified against the
    /// CA and `name` (the connection's `<id>.db.aurora.internal`, else the gateway's server name).
    /// Nothing but the `SSLRequest` is written before verification succeeded.
    pub(crate) async fn connect_db(
        &self,
        name: Option<&str>,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>, std::io::Error> {
        let name = match name {
            Some(n) => ServerName::try_from(n.to_owned())
                .map_err(|e| std::io::Error::other(format!("server name {n:?}: {e}")))?,
            None => self.name.clone(),
        };
        let mut tcp = TcpStream::connect(&self.addr).await?;
        let _ = tcp.set_nodelay(true);
        tcp.write_all(&[0, 0, 0, 8, 4, 210, 22, 47]).await?;
        let mut answer = [0u8; 1];
        tcp.read_exact(&mut answer).await?;
        if answer[0] != b'S' {
            return Err(std::io::Error::other("gateway refused TLS for PostgreSQL"));
        }
        self.db_connector.connect(name, tcp).await
    }

    /// Dial + handshake. No credential is written until verification succeeded.
    async fn connect(&self) -> Result<tokio_rustls::client::TlsStream<TcpStream>, UpstreamError> {
        let tcp = TcpStream::connect(&self.addr)
            .await
            .map_err(UpstreamError::Connect)?;
        let _ = tcp.set_nodelay(true);
        self.connector
            .connect(self.name.clone(), tcp)
            .await
            .map_err(UpstreamError::Tls)
    }
}

enum UpstreamError {
    Connect(std::io::Error),
    Tls(std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    /// CONNECT and absolute-form.
    Mitm,
    /// CONNECT only; absolute-form gets 405 (can never become an injecting proxy).
    ConnectOnly,
}

/// A running listener; dropping it stops accepting (established tunnels continue).
pub struct ProxyHandle {
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl ProxyHandle {
    pub(crate) fn new(addr: SocketAddr, task: JoinHandle<()>) -> Self {
        Self { addr, task }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Bind `bind` and forward to the gateway over TLS.
pub async fn spawn_proxy(
    bind: SocketAddr,
    upstream: GatewayUpstream,
    cred: Arc<Credential>,
    mode: ProxyMode,
) -> std::io::Result<ProxyHandle> {
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let upstream = Arc::new(upstream);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (upstream, cred) = (upstream.clone(), cred.clone());
            tokio::spawn(async move {
                if let Err(e) = handle(stream, &upstream, &cred, mode).await {
                    tracing::debug!(error = %e, "proxy connection ended");
                }
            });
        }
    });
    Ok(ProxyHandle { addr, task })
}

/// Read until the blank line; returns (head, bytes after head).
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

async fn respond<W: AsyncWriteExt + Unpin>(
    s: &mut W,
    status: &str,
    extra: &str,
) -> std::io::Result<()> {
    let msg = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n");
    s.write_all(msg.as_bytes()).await
}

async fn handle(
    mut client: TcpStream,
    upstream: &GatewayUpstream,
    cred: &Credential,
    mode: ProxyMode,
) -> std::io::Result<()> {
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
    let auth = cred.header_value();

    if method.eq_ignore_ascii_case("CONNECT") {
        if target.is_empty() || !target.contains(':') {
            return respond(&mut client, "400 Bad Request", "").await;
        }
        let mut up = match upstream.connect().await {
            Ok(s) => s,
            Err(e) => return upstream_failed(&mut client, e).await,
        };
        let req = format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {auth}\r\n\r\n"
        );
        up.write_all(req.as_bytes()).await?;
        let (resp_head, resp_rest) = read_head(&mut up).await?;
        client.write_all(&resp_head).await?;
        client.write_all(&resp_rest).await?;
        let ok = resp_head.starts_with(b"HTTP/1.1 2") || resp_head.starts_with(b"HTTP/1.0 2");
        if !ok {
            return Ok(());
        }
        up.write_all(&rest).await?;
        tokio::io::copy_bidirectional(&mut client, &mut up).await?;
        return Ok(());
    }

    if mode == ProxyMode::ConnectOnly {
        return respond(&mut client, "405 Method Not Allowed", "Allow: CONNECT\r\n").await;
    }
    if !target.starts_with("http://") {
        return respond(&mut client, "400 Bad Request", "").await;
    }
    let mut up = match upstream.connect().await {
        Ok(s) => s,
        Err(e) => return upstream_failed(&mut client, e).await,
    };
    let mut out = String::with_capacity(head.len() + auth.len() + 64);
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
    out.push_str("Proxy-Authorization: ");
    out.push_str(&auth);
    out.push_str("\r\nConnection: close\r\n\r\n");
    up.write_all(out.as_bytes()).await?;
    up.write_all(&rest).await?;
    tokio::io::copy_bidirectional(&mut client, &mut up).await?;
    Ok(())
}

/// 502 with no credential sent. A TLS failure (untrusted CA, wrong name, plaintext peer) is
/// `gateway_tls_failed` (docs/12 §Loopback proxies).
async fn upstream_failed(client: &mut TcpStream, e: UpstreamError) -> std::io::Result<()> {
    match e {
        UpstreamError::Connect(e) => {
            tracing::debug!(error = %e, "gateway unreachable");
            respond(client, "502 Bad Gateway", "").await
        }
        UpstreamError::Tls(e) => {
            tracing::warn!(error = %e, "gateway TLS verification failed; refusing to proceed");
            let body = r#"{"error":{"code":"gateway_tls_failed","message":"The sandbox could not verify the Aurora gateway's TLS certificate; no credential was sent."}}"#;
            let msg = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            client.write_all(msg.as_bytes()).await
        }
    }
}

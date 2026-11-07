//! Loopback proxies (`docs/12-supervisor-harness.md` §Loopback proxies).
//!
//! Each listener forwards to the gateway proxy address, strips any client-supplied
//! `Proxy-Authorization` and adds its own credential. Absolute-form requests are forwarded with
//! `Connection: close` so the credential is re-evaluated for every request.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const MAX_HEAD: usize = 64 * 1024;

/// The credential a proxy adds: a fixed boot token, optionally overridden by a turn context
/// credential while a turn is active.
#[derive(Debug)]
pub struct Credential {
    username: &'static str,
    boot: String,
    context: Mutex<Option<String>>,
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
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Bind `bind` and forward to `upstream` (`host:port`).
pub async fn spawn_proxy(
    bind: SocketAddr,
    upstream: String,
    cred: Arc<Credential>,
    mode: ProxyMode,
) -> std::io::Result<ProxyHandle> {
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let upstream: Arc<str> = upstream.into();
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
async fn read_head(s: &mut TcpStream) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
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

async fn respond(s: &mut TcpStream, status: &str, extra: &str) -> std::io::Result<()> {
    let msg = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n");
    s.write_all(msg.as_bytes()).await
}

async fn handle(
    mut client: TcpStream,
    upstream: &str,
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
        let mut up = match TcpStream::connect(upstream).await {
            Ok(s) => s,
            Err(_) => return respond(&mut client, "502 Bad Gateway", "").await,
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
    let mut up = match TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(_) => return respond(&mut client, "502 Bad Gateway", "").await,
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

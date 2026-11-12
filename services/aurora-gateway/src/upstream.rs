//! Upstream client: guarded dialling, TLS, pooling, timeouts.

use crate::guard::{DialError, Guard};
use crate::pipeline::{PreparedRequest, Scheme};
use crate::{Body, BoxError, guidance};
use aurora_core::ApiError;
use http::{HeaderValue, Request, Response, Uri, header};
use hyper::body::Incoming;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

#[derive(Debug, Clone)]
pub struct EgressProxy {
    pub host: String,
    pub port: u16,
}

impl EgressProxy {
    /// Accepts `http://host:port` or `host:port`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.strip_prefix("http://").unwrap_or(s).trim_end_matches('/');
        let (h, p) = s
            .rsplit_once(':')
            .ok_or_else(|| format!("egress_proxy {s:?} needs host:port"))?;
        Ok(Self {
            host: h.trim_matches(['[', ']']).to_owned(),
            port: p.parse().map_err(|_| format!("bad port in {s:?}"))?,
        })
    }
}

/// Dials `host:port` through the guard (or through the egress proxy, after a
/// local name check).
pub async fn dial(
    guard: &Guard,
    egress: Option<&EgressProxy>,
    host: &str,
    port: u16,
) -> Result<TcpStream, DialError> {
    if let Some(p) = egress {
        guard.check_name(host, port)?;
        let mut s = connect_any(
            &tokio::net::lookup_host((p.host.as_str(), p.port))
                .await
                .map_err(|e| DialError::Resolution(e.to_string()))?
                .collect::<Vec<_>>(),
        )
        .await?;
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let req = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
        s.write_all(req.as_bytes())
            .await
            .map_err(DialError::Connect)?;
        let mut buf = Vec::new();
        let mut b = [0u8; 1];
        while !buf.ends_with(b"\r\n\r\n") {
            if buf.len() > 8192 || s.read(&mut b).await.map_err(DialError::Connect)? == 0 {
                return Err(DialError::Connect(std::io::Error::other(
                    "egress proxy closed",
                )));
            }
            buf.push(b[0]);
        }
        let ok = buf.starts_with(b"HTTP/1.1 200") || buf.starts_with(b"HTTP/1.0 200");
        if !ok {
            return Err(DialError::Connect(std::io::Error::other(
                "egress proxy refused CONNECT",
            )));
        }
        return Ok(s);
    }
    let addrs = guard.resolve(host, port).await?;
    connect_any(&addrs).await
}

async fn connect_any(addrs: &[std::net::SocketAddr]) -> Result<TcpStream, DialError> {
    let mut last = None;
    for a in addrs {
        match TcpStream::connect(a).await {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(DialError::Connect(
        last.unwrap_or_else(|| std::io::Error::other("no addresses")),
    ))
}

pub fn client_config(
    extra_ca_pem: Option<&[u8]>,
    alpn: &[&[u8]],
) -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for c in native.certs {
        let _ = roots.add(c);
    }
    if let Some(pem) = extra_ca_pem {
        for c in CertificateDer::pem_slice_iter(pem) {
            let c = c.map_err(|e| format!("bad extra CA bundle: {e}"))?;
            roots.add(c).map_err(|e| format!("bad extra CA: {e}"))?;
        }
    }
    let mut cfg = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    Ok(Arc::new(cfg))
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

pub struct Conn {
    io: TokioIo<Stream>,
    h2: bool,
}

impl tokio::io::AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}
impl tokio::io::AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

impl Read for Conn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl Write for Conn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
impl Connection for Conn {
    fn connected(&self) -> Connected {
        let c = Connected::new();
        if self.h2 { c.negotiated_h2() } else { c }
    }
}

#[derive(Clone)]
pub struct Connector {
    guard: Arc<Guard>,
    egress: Option<EgressProxy>,
    tls: TlsConnector,
}

impl tower::Service<Uri> for Connector {
    type Response = Conn;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Conn, BoxError>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let https = uri.scheme_str() == Some("https");
            let host = uri
                .host()
                .ok_or("missing host")?
                .trim_matches(['[', ']'])
                .to_owned();
            let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
            let tcp = dial(&this.guard, this.egress.as_ref(), &host, port).await?;
            if !https {
                return Ok(Conn {
                    io: TokioIo::new(Stream::Plain(tcp)),
                    h2: false,
                });
            }
            let name = ServerName::try_from(host.clone())?;
            let tls = this.tls.connect(name, tcp).await?;
            let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
            Ok(Conn {
                io: TokioIo::new(Stream::Tls(Box::new(tls))),
                h2,
            })
        })
    }
}

type HyperClient = Client<Connector, Body>;

pub struct Upstream {
    /// ALPN h2 + http/1.1.
    client: HyperClient,
    /// http/1.1 only (upgrades).
    client_h1: HyperClient,
    header_timeout: Duration,
}

impl Upstream {
    pub fn new(
        guard: Arc<Guard>,
        egress: Option<EgressProxy>,
        extra_ca_pem: Option<&[u8]>,
        header_timeout: Duration,
    ) -> Result<Self, String> {
        let mk = |alpn: &[&[u8]]| -> Result<HyperClient, String> {
            let c = Connector {
                guard: guard.clone(),
                egress: egress.clone(),
                tls: TlsConnector::from(client_config(extra_ca_pem, alpn)?),
            };
            Ok(Client::builder(TokioExecutor::new())
                .pool_idle_timeout(Duration::from_secs(60))
                .build(c))
        };
        Ok(Self {
            client: mk(&[b"h2", b"http/1.1"])?,
            client_h1: mk(&[b"http/1.1"])?,
            header_timeout,
        })
    }

    /// Relays the prepared request. No `Host` header is ever set here
    /// (FIX-052, D-166, C-631): hyper-util synthesizes `Host` on HTTP/1.1 hops
    /// from the URI (default ports dropped), and an HTTP/2 hop carries
    /// `:authority` only (RFC 9113 §8.3.1 — a `host` header beside it makes
    /// strict front doors such as nginx answer 400 before the app sees the
    /// request).
    pub async fn send(&self, p: PreparedRequest) -> Result<Response<Incoming>, ApiError> {
        let v6 = p.host.contains(':');
        let hostpart = if v6 {
            format!("[{}]", p.host)
        } else {
            p.host.clone()
        };
        let authority = format!("{hostpart}:{}", p.port);
        let pq = match &p.query {
            Some(q) => format!("{}?{q}", p.path),
            None => p.path.clone(),
        };
        let uri = Uri::builder()
            .scheme(p.scheme.as_str())
            .authority(authority)
            .path_and_query(pq)
            .build()
            .map_err(|_| guidance::ambiguous_path())?;
        let mut req = Request::new(p.body);
        *req.method_mut() = p.method;
        *req.uri_mut() = uri;
        *req.version_mut() = http::Version::HTTP_11;
        *req.headers_mut() = p.headers;
        let client = if let Some(up) = p.upgrade {
            req.headers_mut()
                .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
            req.headers_mut().insert(header::UPGRADE, up);
            &self.client_h1
        } else {
            &self.client
        };
        match tokio::time::timeout(self.header_timeout, client.request(req)).await {
            Err(_) => Err(guidance::upstream_timeout()),
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(map_error(&e)),
        }
    }
}

fn map_error(e: &(dyn std::error::Error + 'static)) -> ApiError {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(d) = err.downcast_ref::<DialError>() {
            return match d {
                DialError::Refused(h) => guidance::destination_refused(h),
                DialError::Resolution(_) => guidance::resolution_failed(),
                DialError::Connect(_) => guidance::bad_gateway(),
            };
        }
        cur = err.source();
    }
    tracing::debug!(error = %e, "upstream request failed");
    guidance::bad_gateway()
}

pub fn scheme_of(s: &str) -> Option<Scheme> {
    match s {
        "http" => Some(Scheme::Http),
        "https" => Some(Scheme::Https),
        _ => None,
    }
}

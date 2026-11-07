//! The Mattermost WebSocket (`/api/v4/websocket`): connect, authenticate with
//! the bot token, read events.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

use crate::events::WsEvent;

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_EVERY: Duration = Duration::from_secs(30);
const SILENCE_LIMIT: Duration = Duration::from_secs(90);

#[derive(Debug)]
pub enum WsError {
    /// The server refused the token (HTTP 401 on upgrade or a failed challenge).
    Auth,
    /// Anything else: the caller reconnects with backoff.
    Io(String),
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsError::Auth => write!(f, "websocket authentication refused"),
            WsError::Io(e) => write!(f, "websocket: {e}"),
        }
    }
}

pub struct WsConn {
    ws: WebSocketStream<Box<dyn Io>>,
    ping: tokio::time::Interval,
    last_read: tokio::time::Instant,
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, WsError> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| WsError::Io(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

impl WsConn {
    /// Connects and authenticates. `url` is `ws(s)://…/api/v4/websocket`.
    pub async fn connect(url: &str, token: &str) -> Result<Self, WsError> {
        tokio::time::timeout(CONNECT_TIMEOUT, Self::connect_inner(url, token))
            .await
            .map_err(|_| WsError::Io("connect timed out".into()))?
    }

    async fn connect_inner(url: &str, token: &str) -> Result<Self, WsError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| WsError::Io(e.to_string()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| WsError::Io("no host".into()))?
            .to_owned();
        let tls = parsed.scheme() == "wss";
        let port = parsed.port().unwrap_or(if tls { 443 } else { 80 });
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| WsError::Io(e.to_string()))?;
        let io: Box<dyn Io> = if tls {
            let name = rustls::pki_types::ServerName::try_from(host.clone())
                .map_err(|e| WsError::Io(e.to_string()))?;
            let connector = tokio_rustls::TlsConnector::from(tls_config()?);
            let stream = connector
                .connect(name, tcp)
                .await
                .map_err(|e| WsError::Io(e.to_string()))?;
            Box::new(stream)
        } else {
            Box::new(tcp)
        };
        let req = url
            .into_client_request()
            .map_err(|e| WsError::Io(e.to_string()))?;
        let (mut ws, _) = client_async(req, io).await.map_err(|e| match &e {
            tokio_tungstenite::tungstenite::Error::Http(r) if r.status().as_u16() == 401 => {
                WsError::Auth
            }
            _ => WsError::Io(e.to_string()),
        })?;
        let challenge = serde_json::json!({
            "seq": 1,
            "action": "authentication_challenge",
            "data": {"token": token},
        });
        ws.send(Message::text(challenge.to_string()))
            .await
            .map_err(|e| WsError::Io(e.to_string()))?;
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.reset();
        let mut conn = Self {
            ws,
            ping,
            last_read: tokio::time::Instant::now(),
        };
        // The challenge answer is the first reply frame (`seq_reply`); events
        // (`hello`) follow it.
        loop {
            let frame = conn.next_frame().await?;
            let Some(frame) = frame else {
                return Err(WsError::Io("closed during authentication".into()));
            };
            let v: serde_json::Value = match serde_json::from_str(&frame) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("seq_reply").is_some() {
                let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
                if status.eq_ignore_ascii_case("ok") {
                    return Ok(conn);
                }
                let code = v
                    .pointer("/error/status_code")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                return Err(if code == 401 || code == 0 {
                    WsError::Auth
                } else {
                    WsError::Io(format!("authentication failed ({code})"))
                });
            }
        }
    }

    async fn next_frame(&mut self) -> Result<Option<String>, WsError> {
        loop {
            tokio::select! {
                _ = self.ping.tick() => {
                    if self.last_read.elapsed() > SILENCE_LIMIT {
                        return Err(WsError::Io("no traffic from the server".into()));
                    }
                    self.ws
                        .send(Message::Ping(Vec::new().into()))
                        .await
                        .map_err(|e| WsError::Io(e.to_string()))?;
                }
                m = self.ws.next() => {
                    self.last_read = tokio::time::Instant::now();
                    return match m {
                        None | Some(Ok(Message::Close(_))) => Ok(None),
                        Some(Ok(Message::Text(t))) => Ok(Some(t.as_str().to_owned())),
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => Err(WsError::Io(e.to_string())),
                    };
                }
            }
        }
    }

    /// The next event frame; `Ok(None)` when the server closed the socket.
    pub async fn next_event(&mut self) -> Result<Option<WsEvent>, WsError> {
        loop {
            match self.next_frame().await? {
                None => return Ok(None),
                Some(frame) => {
                    if let Some(e) = WsEvent::parse(&frame) {
                        return Ok(Some(e));
                    }
                }
            }
        }
    }
}

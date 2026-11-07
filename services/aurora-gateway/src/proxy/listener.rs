//! Accept loop for the proxy port and graceful shutdown.

use super::tunnel::stopped;
use super::{DRAIN_TIMEOUT, Proxy};
use crate::Body;
use http::{Method, Request, Response};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

impl Proxy {
    async fn handle_plain(self: &Arc<Self>, req: Request<Incoming>) -> Response<Body> {
        if req.method() == Method::CONNECT {
            self.handle_connect(req).await
        } else {
            self.handle_absolute(req).await
        }
    }

    /// Serves until `shutdown` resolves, then stops accepting and drains
    /// in-flight work for up to [`DRAIN_TIMEOUT`].
    pub async fn run(self: Arc<Self>, listener: TcpListener, shutdown: impl Future<Output = ()>) {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                r = listener.accept() => match r {
                    Ok((stream, _)) => {
                        let _ = stream.set_nodelay(true);
                        let this = self.clone();
                        let live = self.track();
                        tokio::spawn(async move {
                            let _live = live;
                            this.serve_tls(stream).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
        drop(listener);
        let _ = self.stop.send(true);
        let drain = async {
            loop {
                let n = self.live.notify.notified();
                if self.live.count.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    break;
                }
                n.await;
            }
        };
        if tokio::time::timeout(DRAIN_TIMEOUT, drain).await.is_err() {
            tracing::warn!("drain timeout; closing remaining connections");
        }
    }

    /// (phase 1, RFE-002) TLS-only: a connection whose first bytes are not a TLS record is closed
    /// at once, unanswered and unread (no credential is ever parsed from cleartext); everything
    /// else is handshaken before any HTTP parsing.
    async fn serve_tls(self: &Arc<Self>, stream: tokio::net::TcpStream) {
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, looks_like_tls(&stream)).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                metrics::counter!("gateway_proxy_plaintext_refused_total").increment(1);
                return;
            }
            _ => return,
        }
        let accepted =
            tokio::time::timeout(HANDSHAKE_TIMEOUT, self.tls.acceptor().accept(stream)).await;
        let Ok(Ok(tls)) = accepted else {
            tracing::debug!("proxy TLS handshake failed");
            return;
        };
        self.serve_plain(tls).await;
    }

    async fn serve_plain<IO>(self: &Arc<Self>, stream: IO)
    where
        IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let this = self.clone();
        let svc = service_fn(move |req: Request<Incoming>| {
            let this = this.clone();
            async move { Ok::<_, Infallible>(this.handle_plain(req).await) }
        });
        let conn = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(Duration::from_secs(30))
            .serve_connection(TokioIo::new(stream), svc)
            .with_upgrades();
        tokio::pin!(conn);
        let mut stop = self.stop.subscribe();
        tokio::select! {
            r = conn.as_mut() => { if let Err(e) = r { tracing::debug!(error = %e, "proxy connection ended"); } }
            () = stopped(&mut stop) => {
                conn.as_mut().graceful_shutdown();
                let _ = conn.await;
            }
        }
    }
}

/// How long a client has to produce its first bytes and finish the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Peeks (without consuming) until the first bytes can be told apart from a TLS record header
/// (`0x16 0x03 0x0n` handshake, TLS 1.0-1.3 record versions).
async fn looks_like_tls(stream: &tokio::net::TcpStream) -> std::io::Result<bool> {
    let mut buf = [0u8; 3];
    loop {
        let n = stream.peek(&mut buf).await?;
        if n == 0 {
            return Ok(false);
        }
        if buf[0] != 0x16 || (n >= 2 && buf[1] != 0x03) || (n >= 3 && buf[2] > 0x04) {
            return Ok(false);
        }
        if n >= 3 {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

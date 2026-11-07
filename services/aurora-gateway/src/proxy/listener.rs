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
                            this.serve_plain(stream).await;
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

    async fn serve_plain(self: &Arc<Self>, stream: tokio::net::TcpStream) {
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

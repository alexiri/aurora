//! `CONNECT host:port`: MITM lane (TLS termination) and open lane (tunnel).

use super::tunnel::stopped;
use super::{Proxy, Target, bridge, dial_error};
use crate::auth::{CAP_BROWSER, Lane};
use crate::guard::normalize_host;
use crate::pipeline::{OpenLaneDecision, Scheme, TunnelReport};
use crate::upstream::dial;
use crate::{Body, empty_body, guidance};
use aurora_core::ApiError;
use http::{Request, Response};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio_rustls::TlsAcceptor;

impl Proxy {
    pub(super) async fn handle_connect(
        self: &Arc<Self>,
        mut req: Request<Incoming>,
    ) -> Response<Body> {
        let Some(auth) = req.uri().authority() else {
            return guidance::response(&guidance::not_a_proxy_request());
        };
        let host = normalize_host(auth.host());
        let port = auth.port_u16().unwrap_or(443);
        let (identity, lane) = match self.authenticate(req.headers()).await {
            Ok(x) => x,
            Err(e) => return guidance::response(&e),
        };
        let t = Target {
            identity,
            lane,
            scheme: Scheme::Https,
            host,
            port,
        };
        let on_upgrade = hyper::upgrade::on(&mut req);
        match lane {
            Lane::Open => {
                // (RFE-003 c) Capability first, before any policy evaluation: a client that
                // merely sets the username `open` gains nothing. Only `aub_` boot credentials
                // minted with `browser` hold it.
                if !t.identity.has_capability(CAP_BROWSER) {
                    return guidance::response(&guidance::open_lane_not_permitted());
                }
                if let OpenLaneDecision::Deny(e) = self
                    .handler
                    .open_lane_check(&t.identity, &t.host, t.port)
                    .await
                {
                    return guidance::response(&e);
                }
                let upstream = match dial(&self.guard, self.egress.as_ref(), &t.host, t.port).await
                {
                    Ok(s) => s,
                    Err(e) => return guidance::response(&dial_error(&e)),
                };
                let this = self.clone();
                let live = self.track();
                tokio::spawn(async move {
                    let _live = live;
                    let Ok(up) = on_upgrade.await else { return };
                    let started = Instant::now();
                    let (bytes_up, bytes_down) =
                        bridge(TokioIo::new(up), upstream, None, this.stop.subscribe()).await;
                    this.handler.tunnel_closed(TunnelReport {
                        identity: t.identity,
                        host: t.host,
                        port: t.port,
                        bytes_up,
                        bytes_down,
                        duration: started.elapsed(),
                    });
                });
            }
            Lane::Mitm => {
                let cfg = match self.ca.server_config(&t.host) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!(error = %e, "leaf minting failed");
                        return guidance::response(&ApiError::internal("TLS setup failed."));
                    }
                };
                let this = self.clone();
                let live = self.track();
                tokio::spawn(async move {
                    let _live = live;
                    let Ok(up) = on_upgrade.await else { return };
                    let Ok(tls) = TlsAcceptor::from(cfg).accept(TokioIo::new(up)).await else {
                        return;
                    };
                    this.serve_tunneled(tls, t).await;
                });
            }
        }
        Response::new(empty_body())
    }

    async fn serve_tunneled<IO>(self: &Arc<Self>, io: IO, t: Target)
    where
        IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let this = self.clone();
        let svc = service_fn(move |req: Request<Incoming>| {
            let this = this.clone();
            let t = t.clone();
            async move { Ok::<_, Infallible>(this.process(&t, req).await) }
        });
        let builder = auto::Builder::new(TokioExecutor::new());
        let conn = builder.serve_connection_with_upgrades(TokioIo::new(io), svc);
        tokio::pin!(conn);
        let mut stop = self.stop.subscribe();
        tokio::select! {
            r = conn.as_mut() => { if let Err(e) = r { tracing::debug!(error = %e, "tunnel connection ended"); } }
            () = stopped(&mut stop) => {
                conn.as_mut().graceful_shutdown();
                let _ = conn.await;
            }
        }
    }
}

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
        client: std::net::IpAddr,
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
        // (RFE-025) BYO admission: after authentication, before the lane is chosen.
        if let Err(e) = self.byo_gate(&identity, client, "CONNECT", &host) {
            return guidance::response(&e);
        }
        let guard = self
            .byo
            .tunnels
            .as_ref()
            .and_then(|r| r.register(&identity, client));
        let t = Target {
            identity,
            lane,
            scheme: Scheme::Https,
            host,
            port,
            kill: guard.as_ref().map(crate::byo::TunnelGuard::killed),
            closed_by: guard.as_ref().map(crate::byo::TunnelGuard::closed_by),
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
                    // (RFE-006) The open-lane tunnel dies when its agent is frozen (or its BYO
                    // admission tightens): the registry guard is held for the tunnel's life.
                    let guard = guard;
                    let mut kill = guard.as_ref().map(crate::byo::TunnelGuard::killed);
                    let Ok(up) = on_upgrade.await else { return };
                    let started = Instant::now();
                    let pipe = bridge(TokioIo::new(up), upstream, None, this.stop.subscribe());
                    let (bytes_up, bytes_down) = match kill.as_mut() {
                        Some(k) => {
                            tokio::select! {
                                r = pipe => r,
                                () = crate::byo::killed(k) => (0, 0),
                            }
                        }
                        None => pipe.await,
                    };
                    drop(guard);
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
                    let mut kill = guard.as_ref().map(crate::byo::TunnelGuard::killed);
                    let closed_by = guard.as_ref().map(crate::byo::TunnelGuard::closed_by);
                    let work = async {
                        let Ok(up) = on_upgrade.await else { return };
                        let Ok(tls) = TlsAcceptor::from(cfg).accept(TokioIo::new(up)).await else {
                            return;
                        };
                        this.serve_tunneled(tls, t).await;
                    };
                    // (RFE-025) The BYO admission of this tunnel changed: drop the connection at
                    // any stage (handshake or request loop). A freeze or BYO tightening first
                    // answers what is in flight (a request held for approval is refused 403 with
                    // its approval abandoned, P2-FU-2): `serve_tunneled` shuts the connection
                    // down gracefully after the answer; this is the hard stop, CLOSE_GRACE later.
                    match kill.as_mut() {
                        Some(k) => {
                            tokio::select! {
                                () = async {
                                    crate::byo::killed(k).await;
                                    if closed_by.as_ref().is_some_and(|c| c.borrow().is_some()) {
                                        tokio::time::sleep(crate::byo::CLOSE_GRACE).await;
                                    }
                                } => {}
                                () = work => {}
                            }
                        }
                        None => work.await,
                    }
                    drop(guard);
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
        let mut kill = t.kill.clone();
        let closed_by = t.closed_by.clone();
        let svc = service_fn(move |req: Request<Incoming>| {
            let this = this.clone();
            let t = t.clone();
            async move { Ok::<_, Infallible>(this.process(&t, req).await) }
        });
        let builder = auto::Builder::new(TokioExecutor::new());
        let conn = builder.serve_connection_with_upgrades(TokioIo::new(io), svc);
        tokio::pin!(conn);
        let mut stop = self.stop.subscribe();
        // Closed by a freeze or BYO tightening: let the answers go out, then shut the connection.
        let answered_close = async {
            match kill.as_mut() {
                Some(k) => {
                    crate::byo::killed(k).await;
                    if closed_by.as_ref().is_some_and(|c| c.borrow().is_some()) {
                        return;
                    }
                    std::future::pending::<()>().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            r = conn.as_mut() => { if let Err(e) = r { tracing::debug!(error = %e, "tunnel connection ended"); } }
            () = answered_close => {
                conn.as_mut().graceful_shutdown();
                let _ = tokio::time::timeout(crate::byo::CLOSE_GRACE, conn).await;
            }
            () = stopped(&mut stop) => {
                conn.as_mut().graceful_shutdown();
                let _ = conn.await;
            }
        }
    }
}

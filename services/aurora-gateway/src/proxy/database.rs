//! The database lane's front door (phase 2, P2-DBL-1): runs after the PROXY header (when
//! present) and the PostgreSQL sniff. TLS-only like the rest of the listener: the client's
//! `SSLRequest` is answered `S`, the handshake presents a leaf for
//! `<connection_id>.db.aurora.internal` (chosen by SNI), and only then is a password asked for.
//! User = connection id, password = the agent's gateway credential.

use super::Proxy;
use crate::auth::ResolveError;
use crate::db::pg::relay::Relay;
use crate::db::pg::server::{self, BoxIo, ConnectError};
use crate::db::pg::wire::{self, Initial};
use crate::db::{DbOpenError, connection_id_of_host, synthetic_host};
use crate::guidance;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::LazyConfigAcceptor;
use uuid::Uuid;

const STEP_TIMEOUT: Duration = Duration::from_secs(10);

async fn fatal(io: &mut BoxIo, sqlstate: &str, msg: &str) {
    let _ = wire::write_all(io, &wire::error_response("FATAL", sqlstate, msg, None)).await;
    let _ = io.shutdown().await;
}

fn count(result: &'static str) {
    metrics::counter!("aurora_gateway_db_logins_total", "result" => result).increment(1);
}

impl Proxy {
    pub(super) async fn serve_db(self: &Arc<Self>, stream: TcpStream, client: IpAddr) {
        let mut io: BoxIo = Box::new(stream);
        let mut secure = false;
        // SSLRequest (and GSSENCRequest, declined) may precede the StartupMessage once each.
        for _ in 0..3 {
            let body = match tokio::time::timeout(STEP_TIMEOUT, wire::read_initial(&mut io)).await {
                Ok(Ok(Some(b))) => b,
                _ => return,
            };
            match wire::parse_initial(&body) {
                Ok(Initial::Ssl) if !secure => {
                    if wire::write_all(&mut io, b"S").await.is_err() {
                        return;
                    }
                    let Some(tls) = self.db_tls(io).await else {
                        return;
                    };
                    io = tls;
                    secure = true;
                }
                Ok(Initial::GssEnc) if !secure => {
                    if wire::write_all(&mut io, b"N").await.is_err() {
                        return;
                    }
                }
                Ok(Initial::Cancel { pid, key }) => {
                    self.db_cancel_request(pid, key);
                    return;
                }
                Ok(Initial::Startup { minor, params }) => {
                    if !secure {
                        // Never ask for a password in cleartext.
                        count("tls_required");
                        fatal(
                            &mut io,
                            "28000",
                            "The Aurora gateway requires TLS for database connections (use sslmode=require or stronger).",
                        )
                        .await;
                        return;
                    }
                    self.db_login(io, client, minor, params).await;
                    return;
                }
                Ok(_) => {
                    fatal(&mut io, "08P01", "unexpected startup packet").await;
                    return;
                }
                Err(wire::InitialError::UnsupportedVersion) => {
                    fatal(
                        &mut io,
                        "0A000",
                        "unsupported frontend protocol; the gateway speaks version 3",
                    )
                    .await;
                    return;
                }
                Err(wire::InitialError::Malformed) => {
                    fatal(&mut io, "08P01", "malformed startup packet").await;
                    return;
                }
            }
        }
    }

    /// TLS with a leaf for the SNI host when it is `<connection_id>.db.aurora.internal`, else
    /// the listener's own certificate.
    async fn db_tls(&self, io: BoxIo) -> Option<BoxIo> {
        let start = tokio::time::timeout(
            STEP_TIMEOUT,
            LazyConfigAcceptor::new(rustls::server::Acceptor::default(), io),
        )
        .await
        .ok()?
        .ok()?;
        let sni = start
            .client_hello()
            .server_name()
            .map(str::to_owned)
            .filter(|s| connection_id_of_host(s).is_some());
        let cfg = match sni {
            Some(h) => match self.ca.server_config(&h) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "database leaf minting failed");
                    return None;
                }
            },
            None => self.tls.server_config(),
        };
        let tls = tokio::time::timeout(STEP_TIMEOUT, start.into_stream(cfg))
            .await
            .ok()?
            .ok()?;
        Some(Box::new(tls))
    }

    fn db_cancel_request(self: &Arc<Self>, pid: u32, key: u32) {
        let Some((ep, spid, skey)) = self.db_cancel.lookup((pid, key)) else {
            return; // unknown key: no answer, as PostgreSQL does
        };
        let this = self.clone();
        let live = self.track();
        tokio::spawn(async move {
            let _live = live;
            if let Err(e) = server::cancel(&this.guard, this.egress.as_ref(), &ep, spid, skey).await
            {
                tracing::debug!(error = %e, "database cancel failed");
            }
        });
    }

    async fn db_login(
        self: &Arc<Self>,
        mut io: BoxIo,
        client: IpAddr,
        minor: u16,
        params: Vec<(String, String)>,
    ) {
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let unknown: Vec<String> = params
            .iter()
            .filter(|(k, _)| k.starts_with("_pq_."))
            .map(|(k, _)| k.clone())
            .collect();
        if (minor != 0 || !unknown.is_empty())
            && wire::write_all(&mut io, &wire::negotiate_protocol(&unknown))
                .await
                .is_err()
        {
            return;
        }
        let Some(conn_id) = get("user").and_then(|u| u.parse::<Uuid>().ok()) else {
            count("bad_user");
            fatal(
                &mut io,
                "28000",
                "The user name must be the id of a database connection granted to the agent.",
            )
            .await;
            return;
        };
        let host = synthetic_host(conn_id);
        // Password (cleartext, inside TLS): the agent's gateway credential.
        if wire::write_all(&mut io, &wire::auth_request(3, &[]))
            .await
            .is_err()
        {
            return;
        }
        let frame =
            match tokio::time::timeout(STEP_TIMEOUT, wire::read_frame(&mut io, 4096, 4096)).await {
                Ok(Ok(Some(f))) if f.tag == b'p' && !f.oversize => f,
                _ => return,
            };
        let mut body = &frame.body[..];
        let Some(token) = wire::take_cstr(&mut body).filter(|t| !t.is_empty()) else {
            count("bad_password");
            fatal(
                &mut io,
                "28P01",
                "A gateway credential is required as the password.",
            )
            .await;
            return;
        };
        let identity = match self.resolver.resolve(&token).await {
            Ok(Some(id)) => Arc::new(id),
            Ok(None) => {
                count("bad_credential");
                fatal(
                    &mut io,
                    "28P01",
                    "password authentication failed: the gateway credential is unknown, revoked or expired",
                )
                .await;
                return;
            }
            Err(ResolveError::Frozen { level }) => {
                metrics::counter!("aurora_gateway_frozen_refused_total").increment(1);
                count("frozen");
                fatal(&mut io, "28000", guidance::frozen(level).message()).await;
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "credential resolution failed");
                count("unavailable");
                fatal(
                    &mut io,
                    "57P03",
                    guidance::credential_store_unavailable().message(),
                )
                .await;
                return;
            }
        };
        // BYO posture and source address, exactly as for HTTP.
        if let Err(e) = self.byo_gate(&identity, client, "DB", &host) {
            count("byo_refused");
            fatal(&mut io, "28000", e.message()).await;
            return;
        }
        let tunnel = self
            .byo
            .tunnels
            .as_ref()
            .and_then(|r| r.register(&identity, client));
        let kill = tunnel.as_ref().map(crate::byo::TunnelGuard::killed);
        let opened = match self.handler.clone().db_open(&identity, conn_id).await {
            Ok(o) => o,
            Err(DbOpenError::Refused(e)) => {
                count("refused");
                fatal(&mut io, "42501", e.message()).await;
                return;
            }
            Err(DbOpenError::Unavailable(e)) => {
                count("unavailable");
                fatal(&mut io, "57P03", e.message()).await;
                return;
            }
        };
        let app_name = get("application_name")
            .filter(|a| a.len() <= 64 && a.bytes().all(|b| (0x20..0x7f).contains(&b)));
        let endpoint = Arc::new(opened.endpoint);
        let server = match server::connect(&self.guard, self.egress.as_ref(), &endpoint, app_name)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                count("server_refused");
                tracing::warn!(connection = %conn_id, error = %e, "database connection failed");
                let (code, msg) = match &e {
                    ConnectError::Server(m) => (
                        "28P01",
                        format!("the database rejected the stored credentials: {m}"),
                    ),
                    ConnectError::Dial(d) => ("08001", super::dial_error(d).message().to_owned()),
                    other => (
                        "08006",
                        format!("could not connect to the database: {other}"),
                    ),
                };
                fatal(&mut io, code, &msg).await;
                return;
            }
        };
        // Replay the server's startup, with a gateway-generated cancel key.
        let gw_key = self
            .db_cancel
            .register(endpoint.clone(), server.pid, server.key);
        let mut hello = wire::auth_request(0, &[]);
        for p in &server.params {
            hello.extend_from_slice(p);
        }
        hello.extend_from_slice(&wire::backend_key_data(gw_key.0, gw_key.1));
        hello.extend_from_slice(&wire::ready_for_query(b'I'));
        if wire::write_all(&mut io, &hello).await.is_err() {
            self.db_cancel.unregister(gw_key);
            return;
        }
        count("ok");
        tracing::info!(connection = %conn_id, label = %opened.label, agent = %identity.agent_id, "database session started");
        Relay {
            authorizer: opened.authorizer,
            stop: self.stop.subscribe(),
            kill,
        }
        .run(io, server.io)
        .await;
        self.db_cancel.unregister(gw_key);
        drop(tunnel);
        tracing::info!(connection = %conn_id, agent = %identity.agent_id, "database session ended");
    }
}

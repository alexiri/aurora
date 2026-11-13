//! aurora-relay: the preview relay (phase 4, P4-PRV-2, D-146; docs/15 §Port previews).
//!
//! Edge-facing HTTP on `relay.listen` (:4112 behind the edge), ops on `relay.ops_port`
//! (:4193 health + metrics). Stateless: one anchor token (`aul_`), an in-memory resolve
//! cache, and the open-stream table. A restart loses only open streams.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use aurora_config::RelayConfig;
use aurora_core::service;
use clap::{Parser, Subcommand};

const SERVICE: &str = "aurora-relay";

#[derive(Parser)]
#[command(
    name = "aurora-relay",
    version,
    about = "Aurora preview relay: serves preview hostnames and tunnels byte streams to runners (port 4112, ops 4193)"
)]
struct Cli {
    /// Config file (else `AURORA_CONFIG`, else /etc/aurora/aurora.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the service.
    Serve,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg: RelayConfig = match aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.relay_config())
    {
        Ok(c) => c,
        Err(e) => return crate::exit_config(e),
    };
    let instance = service::init_telemetry(
        SERVICE,
        &cfg.telemetry.log_level,
        cfg.telemetry.otlp_endpoint.as_deref(),
    );
    tracing::info!(instance_id = %instance, "starting");
    match serve(cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn exit_config(e: aurora_config::ConfigErrors) -> ExitCode {
    eprintln!("configuration error: {e}");
    ExitCode::FAILURE
}

async fn serve(cfg: RelayConfig) -> std::io::Result<()> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    let (listen_host, listen_port) = cfg
        .listen
        .rsplit_once(':')
        .ok_or_else(|| invalid("relay.listen must be host:port".into()))?;
    let listen_port: u16 = listen_port
        .parse()
        .map_err(|_| invalid("bad relay.listen port".into()))?;
    let api = Arc::new(
        aurora_relay::api::RelayApi::new(&cfg.api_url, cfg.token.expose())
            .map_err(|e| invalid(format!("relay api client: {e}")))?,
    );
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    // Register first: the domain/external-url/wake-wait come from the API's answer, and the
    // relay refuses to serve when its own `previews_domain` differs from the API's.
    let Some((api_domain, external_url, wake_wait)) = aurora_relay::state::registration_loop(
        api.clone(),
        env!("CARGO_PKG_VERSION"),
        stop_rx.clone(),
    )
    .await
    else {
        return Err(invalid("registration failed; shutting down".into()));
    };
    if api_domain != cfg.previews_domain {
        tracing::error!(
            own = %cfg.previews_domain,
            api = %api_domain,
            "the API's previews.domain differs from the relay's; refusing to serve"
        );
        return Err(invalid("previews.domain mismatch with the API".into()));
    }
    // `relay.max_streams_per_user` may only lower the spec's per-user cap (docs/02).
    let streams = aurora_relay::stream::Streams::with_limits(
        aurora_core::limits::PREVIEW_STREAMS_PER_SHARE,
        cfg.max_streams_per_user
            .min(aurora_core::limits::PREVIEW_STREAMS_PER_USER),
    );
    let state = aurora_relay::state::State::new(api.clone(), api_domain, external_url, wake_wait);
    // The 15 s validate + heartbeat cycle.
    {
        let streams = streams.clone();
        let hooks = aurora_relay::state::ValidateHooks {
            live: {
                let streams = streams.clone();
                Arc::new(move || streams.live_pairs())
            },
            on_revoked: {
                let streams = streams.clone();
                Arc::new(move |revoked| streams.revoke(&revoked))
            },
            report: {
                let streams = streams.clone();
                Arc::new(move || streams.drain_report())
            },
            restore: {
                let streams = streams.clone();
                Arc::new(move |shares| streams.restore_report(shares))
            },
        };
        tokio::spawn(aurora_relay::state::heartbeat_loop(
            api.clone(),
            hooks,
            streams.timeouts().validate_every,
            stop_rx.clone(),
        ));
    }
    // Edge listener.
    let edge = service::bind(listen_host.trim_matches(['[', ']']), listen_port).await?;
    let ops_listener = service::bind("0.0.0.0", cfg.ops_port).await?;
    let metrics = service::install_metrics(SERVICE);
    let ops = service::ops_router(metrics, vec![]);
    let max = cfg.max_streams;
    tokio::spawn(async move {
        aurora_relay::edge::serve_listener(
            edge,
            state,
            streams,
            max,
            cfg.request_body_max_bytes,
            stop_rx,
        )
        .await;
    });
    let served = service::serve_all(vec![(ops_listener, ops)]).await;
    let _ = stop_tx.send(true);
    served
}

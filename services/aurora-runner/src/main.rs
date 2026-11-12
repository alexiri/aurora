//! aurora-runner: Aurora sandbox runner (ops port; sandbox-facing listener comes later)
#![forbid(unsafe_code)]

use aurora_config::RunnerConfig;
use aurora_core::service::{self, exit_config};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use aurora_runner::channel::{ChannelServer, ChannelSettings};
use aurora_runner::runner::RunnerError;
use aurora_runner::{Runner, RunnerSettings};

const SERVICE: &str = "aurora-runner";
const OPS_PORT: u16 = 4106;
const CHANNEL_PORT: u16 = 4105;
/// Relay-facing stream listener (P4-PRV-2, D-147, `16 §W18`).
const STREAM_PORT: u16 = 4108;

#[derive(Parser)]
#[command(
    name = "aurora-runner",
    version,
    about = "Aurora sandbox runner (ops port; sandbox-facing listener comes later)"
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
    let cfg: RunnerConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.runner_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_telemetry(
        SERVICE,
        &cfg.telemetry.log_level,
        cfg.telemetry.otlp_endpoint.as_deref(),
    );
    tracing::info!(instance_id = %instance, "starting");
    let Command::Serve = cli.command;
    match serve(&cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(cfg: &RunnerConfig) -> std::io::Result<()> {
    tracing::info!(backend = ?cfg.backend, "runner backend configured");
    let backend = aurora_runner::backends::build(cfg)
        .await
        .map_err(std::io::Error::other)?;
    let metrics = service::install_metrics(SERVICE);
    let app = service::ops_router(metrics, vec![]);
    if let Some(w) = aurora_config::bind_overlap_warning(&cfg.server.bind, &cfg.ops_bind) {
        tracing::warn!(channel_bind = %cfg.server.bind, ops_bind = %cfg.ops_bind, "{w}");
    }
    let stream_bind = cfg
        .runner
        .stream_bind
        .clone()
        .unwrap_or_else(|| cfg.server.bind.clone());
    if let Some(w) = aurora_config::bind_overlap_warning(&cfg.server.bind, &stream_bind) {
        tracing::warn!(channel_bind = %cfg.server.bind, stream_bind = %stream_bind, "{w}");
    }
    let listener = service::bind(cfg.ops_bind.as_str(), OPS_PORT).await?;

    // Supervisor channel (W6) on the sandbox-facing port.
    let channel_listener = service::bind(cfg.server.bind.as_str(), CHANNEL_PORT).await?;
    let hub = ChannelServer::new(ChannelSettings::new(*cfg.session_key.expose()));
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let channel_task = tokio::spawn({
        let (hub, stop) = (hub.clone(), stop_rx.clone());
        async move { hub.serve(channel_listener, stop).await }
    });
    let runner = Runner::new(
        RunnerSettings::from_config(cfg),
        backend,
        Arc::new(hub.clone()),
    );
    let task = tokio::spawn(runner.run(stop_rx.clone()));

    // Relay-facing stream listener (:4108, P4-PRV-2/D-147): bound eagerly, but the accept
    // loop starts only when the runner registers and the API reports `previews.enabled`
    // (`config.previews`); otherwise nothing is served and the port stays dark.
    let streams = aurora_runner::stream_listener::StreamListener::new(hub.clone());
    hub.set_streams(streams.clone());
    let stream_tls = match (
        &cfg.runner.stream_tls.cert_file,
        &cfg.runner.stream_tls.key_file,
    ) {
        (Some(c), Some(k)) => match load_tls_acceptor(c, k) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!(error = %e, "stream TLS disabled: certificate could not be loaded");
                None
            }
        },
        (None, None) => None,
        _ => {
            tracing::warn!("stream TLS disabled: cert_file and key_file must both be set");
            None
        }
    };
    let stream_task = tokio::spawn({
        let (hub, bind, stop, tls) = (
            hub.clone(),
            stream_bind.clone(),
            stop_rx.clone(),
            stream_tls,
        );
        let api_rx = hub.api_watch();
        async move {
            match service::bind(bind.as_str(), STREAM_PORT).await {
                Ok(l) => {
                    aurora_runner::stream_listener::serve(l, streams, api_rx, tls, stop).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "stream listener bind failed; previews disabled");
                }
            }
        }
    });

    let served = service::serve_all(vec![(listener, app)]).await;
    let _ = stop_tx.send(true);
    let _ = channel_task.await;
    let _ = stream_task.await;
    match task.await {
        Ok(Ok(())) | Ok(Err(RunnerError::Shutdown)) => {}
        Ok(Err(e)) => return Err(std::io::Error::other(e)),
        Err(e) => return Err(std::io::Error::other(e)),
    }
    served
}

/// Loads the stream listener's TLS certificate chain (`runner.stream_tls`, `16 §W18`).
fn load_tls_acceptor(cert_file: &str, key_file: &str) -> Result<tokio_rustls::TlsAcceptor, String> {
    use rustls::pki_types::pem::PemObject as _;
    let chain = rustls::pki_types::CertificateDer::pem_file_iter(cert_file)
        .map_err(|e| format!("read {cert_file}: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("parse {cert_file}: {e}"))?;
    if chain.is_empty() {
        return Err(format!("{cert_file}: no certificates"));
    }
    let key = rustls::pki_types::PrivateKeyDer::from_pem_file(key_file)
        .map_err(|e| format!("read {key_file}: {e}"))?;
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .map_err(|e| e.to_string())?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(server)))
}

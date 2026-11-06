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
    let instance = service::init_tracing(SERVICE, &cfg.telemetry.log_level);
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
    let listener = service::bind(cfg.server.bind.as_str(), OPS_PORT).await?;

    // Supervisor channel (W6) on the sandbox-facing port.
    let channel_listener = service::bind(cfg.server.bind.as_str(), CHANNEL_PORT).await?;
    let hub = ChannelServer::new(ChannelSettings::new(*cfg.session_key.expose()));
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let channel_task = tokio::spawn({
        let (hub, stop) = (hub.clone(), stop_rx.clone());
        async move { hub.serve(channel_listener, stop).await }
    });
    let runner = Runner::new(RunnerSettings::from_config(cfg), backend, Arc::new(hub));
    let task = tokio::spawn(runner.run(stop_rx));

    let served = service::serve_all(vec![(listener, app)]).await;
    let _ = stop_tx.send(true);
    let _ = channel_task.await;
    match task.await {
        Ok(Ok(())) | Ok(Err(RunnerError::Shutdown)) => {}
        Ok(Err(e)) => return Err(std::io::Error::other(e)),
        Err(e) => return Err(std::io::Error::other(e)),
    }
    served
}

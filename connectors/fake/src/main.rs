//! aurora-connector-fake: Aurora fake connector (health on 4190)
#![forbid(unsafe_code)]

use aurora_config::ConnectorConfig;
use aurora_core::service::{self, exit_config};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

const SERVICE: &str = "aurora-connector-fake";
const OPS_PORT: u16 = 4190;

#[derive(Parser)]
#[command(
    name = "aurora-connector-fake",
    version,
    about = "Aurora fake connector (health on 4190)"
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
    let cfg: ConnectorConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.connector_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_tracing(SERVICE, &cfg.telemetry.log_level);
    tracing::info!(instance_id = %instance, "starting");
    let Command::Serve = cli.command;
    match serve().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> std::io::Result<()> {
    let metrics = service::install_metrics(SERVICE);
    let app = service::ops_router(metrics, vec![]);
    let listener = service::bind("0.0.0.0", OPS_PORT).await?;
    service::serve_all(vec![(listener, app)]).await
}

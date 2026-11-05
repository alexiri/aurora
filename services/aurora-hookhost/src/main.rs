//! aurora-hookhost: Aurora hook host
#![forbid(unsafe_code)]

use aurora_config::HookhostConfig;
use aurora_core::service::{self, exit_config};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

const SERVICE: &str = "aurora-hookhost";
const OPS_PORT: u16 = 4104;

#[derive(Parser)]
#[command(name = "aurora-hookhost", version, about = "Aurora hook host")]
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
    let cfg: HookhostConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.hookhost_config())
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

async fn serve(cfg: &HookhostConfig) -> std::io::Result<()> {
    let metrics = service::install_metrics(SERVICE);
    let app = service::ops_router(metrics, vec![]);
    let listener = service::bind(cfg.server.bind.as_str(), OPS_PORT).await?;
    service::serve_all(vec![(listener, app)]).await
}

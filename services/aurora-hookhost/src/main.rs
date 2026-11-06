//! aurora-hookhost: Aurora hook host
#![forbid(unsafe_code)]

use aurora_config::HookhostConfig;
use aurora_core::service::{self, exit_config};
use aurora_hookhost::fetch::FetchPolicy;
use aurora_hookhost::runtime::Runtime;
use aurora_hookhost::server;
use aurora_hookhost::snapshot::Catalog;
use base64::Engine as _;
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
    let runtime = Runtime::new(FetchPolicy::default()).map_err(std::io::Error::other)?;
    let bearer = base64::engine::general_purpose::STANDARD.encode(cfg.internal_secret.expose());
    let catalog = Catalog::new(&cfg.hookhost.api_internal_url, bearer.clone(), runtime);
    let poller = catalog.spawn_poller();
    let c = catalog.clone();
    let ready = service::ready_check(move || {
        let c = c.clone();
        async move {
            if c.is_ready().await {
                Ok(())
            } else {
                Err("registry snapshot not loaded".into())
            }
        }
    });
    let app = service::ops_router(metrics, vec![("snapshot", ready)]).merge(server::router(
        server::AppState {
            catalog,
            bearer: bearer.into(),
        },
    ));
    let listener = service::bind(cfg.server.bind.as_str(), OPS_PORT).await?;
    let res = service::serve_all(vec![(listener, app)]).await;
    poller.abort();
    res
}

//! aurora-connector-fake: Aurora fake connector (health on 4190)
#![forbid(unsafe_code)]

use aurora_config::ConnectorConfig;
use aurora_connector_fake::{FakeConnector, control};
use aurora_connector_sdk::{Connector, RuntimeConfig};
use aurora_core::service::{self, exit_config};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

const SERVICE: &str = "aurora-connector-fake";
const DEFAULT_OPS_PORT: u16 = 4190;

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
    /// Base URL of the Aurora API (the connector/1 core).
    #[arg(
        long,
        global = true,
        env = "AURORA_CORE_URL",
        default_value = "http://127.0.0.1:4100"
    )]
    core_url: String,
    /// Mount the `/fake/*` test control API (test profile only).
    #[arg(long, global = true, env = "AURORA_FAKE_CONTROL")]
    control: bool,
    /// Port of the health/metrics (and `/fake/*`) listener.
    #[arg(long, global = true, env = "AURORA_FAKE_PORT", default_value_t = DEFAULT_OPS_PORT)]
    port: u16,
    /// Comma-separated capability names to declare (default: all the fake supports).
    #[arg(
        long,
        global = true,
        env = "AURORA_FAKE_CAPABILITIES",
        value_delimiter = ','
    )]
    capabilities: Option<Vec<String>>,
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
    let instance = service::init_telemetry(
        SERVICE,
        &cfg.telemetry.log_level,
        cfg.telemetry.otlp_endpoint.as_deref(),
    );
    tracing::info!(instance_id = %instance, "starting");
    let Command::Serve = cli.command;
    match serve(&cfg, &cli.core_url, cli.control, cli.port, cli.capabilities).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(
    cfg: &ConnectorConfig,
    core_url: &str,
    control_api: bool,
    port: u16,
    capabilities: Option<Vec<String>>,
) -> std::io::Result<()> {
    let metrics = service::install_metrics(SERVICE);
    let fake = Arc::new(match &capabilities {
        Some(names) => {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            FakeConnector::new(&names)
        }
        None => FakeConnector::default(),
    });
    let mut app = service::ops_router(metrics, vec![]);
    if control_api {
        tracing::warn!("test control API mounted at /fake/*");
        app = app.merge(control::router(fake.clone()));
    }
    if let Some(r) = fake.router() {
        app = app.merge(r);
    }
    let listener = service::bind("0.0.0.0", port).await?;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let runtime = tokio::spawn(aurora_connector_sdk::run(
        fake.clone(),
        RuntimeConfig::new(core_url, cfg.token.expose()),
        stop_rx,
    ));
    let served = service::serve_all(vec![(listener, app)]).await;
    let _ = stop_tx.send(true);
    match runtime.await {
        Ok(Err(e)) => tracing::error!(error = %e, "connector runtime stopped"),
        Err(e) => tracing::error!(error = %e, "connector runtime panicked"),
        Ok(Ok(())) => {}
    }
    served
}

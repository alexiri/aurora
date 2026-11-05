//! aurora-gateway: proxy (4102, placeholder until M2) and internal/ops (4103).
#![forbid(unsafe_code)]

use aurora_config::GatewayConfig;
use aurora_core::{
    ApiError,
    service::{self, exit_config, ready_check},
};
use axum::{Router, response::IntoResponse};
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const SERVICE: &str = "aurora-gateway";
const PROXY_PORT: u16 = 4102;
const INTERNAL_PORT: u16 = 4103;

#[derive(Parser)]
#[command(name = "aurora-gateway", version, about = "Aurora egress gateway")]
struct Cli {
    /// Config file (else `AURORA_CONFIG`, else /etc/aurora/aurora.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the gateway.
    Serve,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg: GatewayConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.gateway_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_tracing(SERVICE, &cfg.telemetry.log_level);
    tracing::info!(instance_id = %instance, "starting");
    let Command::Serve = cli.command;
    match serve(cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(cfg: GatewayConfig) -> Result<(), Box<dyn std::error::Error>> {
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect_lazy(cfg.database_url.expose())?;
    let metrics = service::install_metrics(SERVICE);
    let db_check = ready_check(move || {
        let pool = pool.clone();
        async move {
            sqlx::query("SELECT 1")
                .execute(&pool)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    });
    // Health and metrics are never served on the agent-reachable proxy port.
    let internal = service::ops_router(metrics, vec![("database", db_check)]);
    let proxy = Router::new().fallback(|| async {
        ApiError::unavailable(
            "gateway_not_ready",
            "The gateway proxy is not available yet.",
        )
        .into_response()
    });
    let bind = cfg.server.bind.as_str();
    let servers = vec![
        (service::bind(bind, PROXY_PORT).await?, proxy),
        (service::bind(bind, INTERNAL_PORT).await?, internal),
    ];
    service::serve_all(servers).await?;
    Ok(())
}

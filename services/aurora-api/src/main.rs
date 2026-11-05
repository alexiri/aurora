//! aurora-api: public API (4101), internal API (4111) and `migrate`.
#![forbid(unsafe_code)]

use aurora_config::ApiConfig;
use aurora_core::service::{self, exit_config, ready_check};
use axum::Router;
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const SERVICE: &str = "aurora-api";
const PUBLIC_PORT: u16 = 4101;
const INTERNAL_PORT: u16 = 4111;

#[derive(Parser)]
#[command(name = "aurora-api", version, about = "Aurora control-plane API")]
struct Cli {
    /// Config file (else `AURORA_CONFIG`, else /etc/aurora/aurora.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the API server.
    Serve,
    /// Apply database migrations (and gateway-role grants if configured).
    Migrate,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg: ApiConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.api_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_tracing(SERVICE, &cfg.telemetry.log_level);
    tracing::info!(instance_id = %instance, "starting");
    let result = match cli.command {
        Command::Serve => serve(cfg).await,
        Command::Migrate => migrate(cfg).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn migrate(cfg: ApiConfig) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 2).await?;
    aurora_db::migrate(&pool).await?;
    tracing::info!("migrations applied");
    if let Some(role) = cfg.gateway_role.as_deref() {
        aurora_db::grants::apply_gateway_grants(&pool, role).await?;
        tracing::info!(role, "gateway grants applied");
    }
    Ok(())
}

async fn serve(cfg: ApiConfig) -> Result<(), Box<dyn std::error::Error>> {
    // Lazy pool: the process starts without the DB and `/readyz` reports 503.
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
    let public = service::ops_router(metrics, vec![("database", db_check)]);
    let internal = Router::new().fallback(service::not_found);
    let bind = cfg.server.bind.as_str();
    let servers = vec![
        (service::bind(bind, PUBLIC_PORT).await?, public),
        (service::bind(bind, INTERNAL_PORT).await?, internal),
    ];
    service::serve_all(servers).await?;
    Ok(())
}

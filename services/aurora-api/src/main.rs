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
    /// Print the OpenAPI document (or write it to `--out`).
    Openapi {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Apply database migrations (and gateway-role grants if configured).
    Migrate,
    /// Create (or promote) a verified instance admin and print a one-time password-set link.
    BootstrapAdmin {
        #[arg(long)]
        email: String,
    },
    /// Instance-admin management.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Clear the instance-admin flag of a user.
    Revoke { email: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Openapi { out } = &cli.command {
        let json = aurora_api::openapi::document_json();
        return match out {
            Some(path) => match std::fs::write(path, json) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            None => {
                print!("{json}");
                ExitCode::SUCCESS
            }
        };
    }
    let cfg: ApiConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.api_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_tracing(SERVICE, &cfg.telemetry.log_level);
    tracing::info!(instance_id = %instance, "starting");
    let result = match cli.command {
        Command::Serve => serve(cfg).await,
        Command::Migrate => migrate(cfg).await,
        Command::BootstrapAdmin { email } => bootstrap_admin(cfg, &email).await,
        Command::Admin {
            command: AdminCommand::Revoke { email },
        } => revoke_admin(cfg, &email).await,
        Command::Openapi { .. } => Ok(()),
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

async fn bootstrap_admin(cfg: ApiConfig, email: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 2).await?;
    let link =
        aurora_api::domains::admin::service::bootstrap_admin(&pool, &cfg.external_url, email)
            .await
            .map_err(|e| e.message().to_owned())?;
    println!("{link}");
    Ok(())
}

async fn revoke_admin(cfg: ApiConfig, email: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 2).await?;
    let found = aurora_api::domains::admin::service::revoke_admin(&pool, email)
        .await
        .map_err(|e| e.message().to_owned())?;
    if !found {
        return Err(format!("no user with email {email}").into());
    }
    println!("instance-admin flag cleared for {email}");
    Ok(())
}

async fn serve(cfg: ApiConfig) -> Result<(), Box<dyn std::error::Error>> {
    // Lazy pool: the process starts without the DB and `/readyz` reports 503.
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect_lazy(cfg.database_url.expose())?;
    let metrics = service::install_metrics(SERVICE);
    let check_pool = pool.clone();
    let db_check = ready_check(move || {
        let pool = check_pool.clone();
        async move {
            sqlx::query("SELECT 1")
                .execute(&pool)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    });
    let ops = service::ops_router(metrics, vec![("database", db_check)]);
    let state = aurora_api::app::AppState::try_new(pool.clone(), cfg.clone())?;
    aurora_api::http::auth::spawn_revocation_listener(pool.clone(), state.session_cache.clone());
    aurora_api::jobs::spawn(pool, cfg.retention.request_log_days);
    let public = aurora_api::app::router(state, ops);
    let internal = Router::new().fallback(service::not_found);
    let bind = cfg.server.bind.as_str();
    let servers = vec![
        (service::bind(bind, PUBLIC_PORT).await?, public),
        (service::bind(bind, INTERNAL_PORT).await?, internal),
    ];
    serve_all(servers).await?;
    Ok(())
}

/// Like `service::serve_all`, but exposes the peer address (`ConnectInfo`) so the
/// client-IP middleware can resolve it.
async fn serve_all(servers: Vec<(tokio::net::TcpListener, Router)>) -> std::io::Result<()> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        service::shutdown_signal().await;
        let _ = tx.send(true);
    });
    let mut set = tokio::task::JoinSet::new();
    for (listener, app) in servers {
        if let Ok(addr) = listener.local_addr() {
            tracing::info!(%addr, "listening");
        }
        let mut rx = rx.clone();
        set.spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = rx.wait_for(|stop| *stop).await;
            })
            .await
        });
    }
    let mut result = Ok(());
    while let Some(joined) = set.join_next().await {
        let r = joined.unwrap_or_else(|e| Err(std::io::Error::other(e)));
        if r.is_err() && result.is_ok() {
            result = r;
        }
    }
    result
}

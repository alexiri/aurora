//! aurora-gateway: proxy (4102, placeholder until M2) and internal/ops (4103).
#![forbid(unsafe_code)]

use aurora_config::GatewayConfig;
use aurora_core::service::{self, exit_config, ready_check};
use aurora_gateway::auth::DenyAllResolver;
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::guard::Guard;
use aurora_gateway::pipeline::PassThrough;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use clap::{Parser, Subcommand};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
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
    /// Mint the gateway CA (ca.pem + ca.key); refuses to overwrite.
    GenerateCa {
        #[arg(long)]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::GenerateCa { out } = &cli.command {
        let instance = uuid::Uuid::now_v7().simple().to_string();
        let r = ca::generate(&instance).and_then(|g| ca::write_new(out, &g));
        return match r {
            Ok(()) => {
                println!(
                    "wrote {}/ca.pem and {}/ca.key",
                    out.display(),
                    out.display()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let cfg: GatewayConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.gateway_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_tracing(SERVICE, &cfg.telemetry.log_level);
    tracing::info!(instance_id = %instance, "starting");
    let Command::Serve = cli.command else {
        return ExitCode::FAILURE;
    };
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
    let ca = Arc::new(Ca::load(Path::new(&cfg.gateway.ca_dir))?);
    let bind = cfg.server.bind.as_str();
    let bind_ip: IpAddr = bind.parse()?;
    let mut own = vec![
        SocketAddr::new(bind_ip, PROXY_PORT),
        SocketAddr::new(bind_ip, INTERNAL_PORT),
    ];
    let db: PgConnectOptions = cfg.database_url.expose().parse()?;
    if let Ok(addrs) = tokio::net::lookup_host((db.get_host(), db.get_port())).await {
        own.extend(addrs);
    }
    let guard = Arc::new(Guard::new(&cfg.gateway.allowed_private, own)?);
    let extra_ca_pem = match &cfg.tls.extra_ca_bundle {
        Some(p) => Some(std::fs::read(p)?),
        None => None,
    };
    let proxy = Proxy::new(ProxyParts {
        ca,
        resolver: Arc::new(DenyAllResolver),
        handler: Arc::new(PassThrough),
        guard,
        header_timeout: Duration::from_secs(cfg.gateway.upstream_header_timeout_secs),
        extra_ca_pem,
        egress_proxy: cfg.gateway.egress_proxy.clone(),
    })?;
    let proxy_listener = service::bind(bind, PROXY_PORT).await?;
    let internal_listener = service::bind(bind, INTERNAL_PORT).await?;
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        service::shutdown_signal().await;
        let _ = tx.send(true);
    });
    let mut rx2 = rx.clone();
    let internal_task = tokio::spawn(async move {
        axum::serve(internal_listener, internal)
            .with_graceful_shutdown(async move {
                let _ = rx2.wait_for(|s| *s).await;
            })
            .await
    });
    let mut rx3 = rx;
    proxy
        .run(proxy_listener, async move {
            let _ = rx3.wait_for(|s| *s).await;
        })
        .await;
    internal_task.await??;
    Ok(())
}

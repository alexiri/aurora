//! aurora-terminator: the SSH front door (phase 5, P4-SSH-2, D-239; docs/15 §SSH access).
//!
//! SSH on `terminator.listen` (2222 behind the LoadBalancer), ops on `terminator.ops_port`
//! (4194 health + metrics). Stateless apart from the in-memory session table.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use aurora_config::TerminatorConfig;
use aurora_core::service;
use aurora_terminator::limits::PreAuth;
use aurora_terminator::plane::Plane;
use aurora_terminator::state::{Settings, Shared};
use clap::{Parser, Subcommand};

const SERVICE: &str = "aurora-terminator";

#[derive(Parser)]
#[command(
    name = "aurora-terminator",
    version,
    about = "Aurora SSH terminator: certificate-only SSH access into hosted agents' sandboxes (port 2222, ops 4194)"
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
    let cfg: TerminatorConfig = match aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.terminator_config())
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let instance = service::init_telemetry(
        SERVICE,
        &cfg.telemetry.log_level,
        cfg.telemetry.otlp_endpoint.as_deref(),
        cfg.telemetry.sample_ratio,
    );
    tracing::info!(instance_id = %instance, "starting");
    match run(cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cfg: TerminatorConfig) -> std::io::Result<()> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    // Validated at load; parsed again here for the key itself.
    let host_key = aurora_config::parse_ssh_ed25519_private_key(cfg.host_key.expose())
        .map_err(|e| invalid(format!("AURORA_SSH_HOST_KEY: {e}")))?;
    let host_public = host_key
        .public_key()
        .to_openssh()
        .map_err(|e| invalid(format!("host key: {e}")))?;
    let (host, port) = cfg
        .listen
        .rsplit_once(':')
        .ok_or_else(|| invalid("terminator.listen must be host:port".into()))?;
    let port: u16 = port
        .parse()
        .map_err(|_| invalid("bad terminator.listen port".into()))?;
    let plane = Plane::new(&cfg.api_url, cfg.token.expose())
        .map_err(|e| invalid(format!("terminator plane client: {e}")))?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let shared = Shared::new(
        plane,
        Settings::default(),
        PreAuth::new(
            cfg.max_preauth,
            cfg.max_preauth_per_ip,
            cfg.connect_rate_per_min,
            cfg.connect_burst,
        ),
        stop_rx.clone(),
        host_public,
    );
    tokio::spawn(shared.clone().run_plane());
    let ssh = service::bind(host.trim_matches(['[', ']']), port).await?;
    let ops_listener = service::bind("0.0.0.0", cfg.ops_port).await?;
    let metrics = service::install_metrics(SERVICE);
    // Ready once registered: a replica that cannot reach the API must not take traffic.
    let registered = {
        let shared = shared.clone();
        service::ready_check(move || {
            let healthy = shared.is_healthy();
            async move {
                if healthy {
                    Ok(())
                } else {
                    Err("not registered with the API".to_owned())
                }
            }
        })
    };
    let ops = service::ops_router(metrics, vec![("api", registered)]);
    let ops = service::traced(ops, service::INTERNAL_TRUST);
    let ssh_task = tokio::spawn(aurora_terminator::server::serve(
        ssh,
        shared.clone(),
        host_key,
        stop_rx,
    ));
    // SIGTERM/ctrl-c: stop accepting, say goodbye to every session, then exit.
    let stopper = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop_tx.send(true);
    });
    let served = service::serve_all(vec![(ops_listener, ops)]).await;
    stopper.abort();
    let _ = ssh_task.await;
    served
}

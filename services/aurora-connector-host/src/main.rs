//! aurora-connector-host: runs the built-in connector types (callbacks on 4110, health on 4192)
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use aurora_config::ConnectorHostConfig;
use aurora_connector_host::{Host, HostConfig, compiled, signing};
use aurora_core::service::{self, exit_config};
use clap::{Parser, Subcommand};

const SERVICE: &str = "aurora-connector-host";

#[derive(Parser)]
#[command(
    name = "aurora-connector-host",
    version,
    about = "Aurora connector host: runs the built-in connector types (callbacks on 4110, health on 4192)"
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
    /// Release build step: sign the compiled-in manifests with the project key, one
    /// `<type_id>.sig` per production type, for the next build to embed
    /// (`AURORA_CONNECTOR_SIGNATURES_DIR`).
    SignManifests {
        /// File holding the base64 ed25519 seed (as written by `aurora plugin keygen`).
        #[arg(long)]
        key_file: PathBuf,
        /// Directory the `.sig` files are written to.
        #[arg(long)]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::SignManifests { key_file, out } = &cli.command {
        return match signing::read_key(key_file).and_then(|k| signing::sign_all(&k, out)) {
            Ok(done) => {
                println!("signed manifests: {}", done.join(", "));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let cfg: ConnectorHostConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.connector_host_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_telemetry(
        SERVICE,
        &cfg.telemetry.log_level,
        cfg.telemetry.otlp_endpoint.as_deref(),
    );
    tracing::info!(instance_id = %instance, "starting");
    match serve(&cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(cfg: &ConnectorHostConfig) -> std::io::Result<()> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    if cfg.test_types {
        tracing::warn!(
            "connector_host.test_types is on: the `fake` test type is announced (development and e2e only)"
        );
    }
    let host = Host::new(
        HostConfig::new(&cfg.api_url, cfg.token.expose()),
        compiled(cfg.test_types),
    )
    .map_err(|e| invalid(e.to_string()))?;
    let host = std::sync::Arc::new(host);

    let metrics = service::install_metrics(SERVICE);
    let ready = host.ready();
    let mut ops = service::ops_router(
        metrics,
        vec![(
            "registered",
            service::ready_check(move || {
                let ok = ready.load(Ordering::Relaxed);
                async move {
                    if ok {
                        Ok(())
                    } else {
                        Err("not registered with the API".to_owned())
                    }
                }
            }),
        )],
    );
    if cfg.test_types {
        // The public half of the key that signs the test types, so the e2e can trust it.
        ops = ops.route(
            "/test/signing-key",
            axum::routing::get(|| async {
                axum::Json(aurora_connector_host::catalogue::test_key_info())
            }),
        );
        // The inner router sees the full path, so it can strip the `/test/` prefix itself.
        ops = ops.route(
            "/test/{*rest}",
            axum::routing::any_service(host.test_router()),
        );
    }
    let (listen_host, listen_port) = cfg
        .listen
        .rsplit_once(':')
        .ok_or_else(|| invalid("connector_host.listen must be host:port".into()))?;
    let listen_port: u16 = listen_port
        .parse()
        .map_err(|_| invalid("bad connector_host.listen port".into()))?;
    let public = service::bind(listen_host.trim_matches(['[', ']']), listen_port).await?;
    let ops_listener = service::bind("0.0.0.0", cfg.ops_port).await?;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let runner = {
        let host = host.clone();
        tokio::spawn(async move { host.run(stop_rx).await })
    };
    let served =
        service::serve_all(vec![(public, host.callback_router()), (ops_listener, ops)]).await;
    let _ = stop_tx.send(true);
    if let Err(e) = runner.await {
        tracing::error!(error = %e, "connector host task failed");
    }
    served
}

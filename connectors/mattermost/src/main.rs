//! aurora-connector-mattermost: Aurora Mattermost connector (callbacks on 4110, health on 4191)
#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use aurora_config::ConnectorConfig;
use aurora_connector_mattermost::{MattermostConnector, Settings};
use aurora_connector_sdk::{Connector, RuntimeConfig};
use aurora_core::service::{self, exit_config};
use base64::Engine;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

const SERVICE: &str = "aurora-connector-mattermost";
const MAX_CATCHUP_SECS: u64 = 86_400;

#[derive(Parser)]
#[command(
    name = "aurora-connector-mattermost",
    version,
    about = "Aurora Mattermost connector (callbacks on 4110, health on 4191)"
)]
struct Cli {
    /// Config file (else `AURORA_CONFIG`, else /etc/aurora/aurora.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Base URL of the Aurora API (the connector/1 core).
    #[arg(long, global = true, env = "AURORA_API_URL")]
    api_url: Option<String>,
    /// Same as `--api-url` (the name the other connectors and the conformance suite use).
    #[arg(long, global = true, env = "AURORA_CORE_URL", hide = true)]
    core_url: Option<String>,
    /// Address of the public callback listener (`connector.listen`).
    #[arg(
        long,
        global = true,
        env = "AURORA_CONNECTOR_LISTEN",
        default_value = "0.0.0.0:4110"
    )]
    listen: String,
    /// Port of the health/metrics listener.
    #[arg(
        long,
        global = true,
        env = "AURORA_CONNECTOR_OPS_PORT",
        default_value_t = 4191
    )]
    ops_port: u16,
    /// URL Mattermost reaches the callbacks at (`connector.public_url`).
    #[arg(long, global = true, env = "AURORA_CONNECTOR_PUBLIC_URL")]
    public_url: Option<String>,
    /// Seconds between checks that a bot's token still works (a revoked token does not close its socket).
    #[arg(
        long,
        global = true,
        env = "AURORA_CONNECTOR_TOKEN_CHECK_SECS",
        default_value_t = 30
    )]
    token_check_secs: u64,
    /// Key of the interactive-action signatures (`connector.callback_hmac_key`); at least 32 bytes.
    #[arg(
        long,
        global = true,
        env = "AURORA_CONNECTOR_CALLBACK_HMAC_KEY",
        hide_env_values = true
    )]
    callback_hmac_key: Option<String>,
    /// Catch-up window after (re)connect, seconds (`connector.catchup_window_secs`, max 86400).
    #[arg(
        long,
        global = true,
        env = "AURORA_CONNECTOR_CATCHUP_WINDOW_SECS",
        default_value_t = 600
    )]
    catchup_window_secs: u64,
    /// Detached signature over the manifest digest (the core verifies it).
    #[arg(long, global = true, env = "AURORA_CONNECTOR_MANIFEST_SIGNATURE")]
    manifest_signature: Option<String>,
    /// Manifest publisher (must match the key that signed the manifest).
    #[arg(long, global = true, env = "AURORA_CONNECTOR_PUBLISHER")]
    publisher: Option<String>,
    /// Test profile: comma-separated capabilities to declare (default: all supported).
    #[arg(
        long,
        global = true,
        env = "AURORA_MM_CAPABILITIES",
        value_delimiter = ','
    )]
    capabilities: Option<Vec<String>>,
    /// Test profile: mount `/mm/test/*`.
    #[arg(long, global = true, env = "AURORA_MM_TEST_CONTROL")]
    test_control: bool,
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
    match serve(cli, &cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn hmac_key(cli: &Cli) -> Result<Vec<u8>, String> {
    match &cli.callback_hmac_key {
        Some(k) if k.len() >= 32 => Ok(k.as_bytes().to_vec()),
        Some(_) => Err("connector.callback_hmac_key must be at least 32 bytes".into()),
        None => {
            // Safe for a single replica only: buttons rendered by one process
            // verify in that process. Production installs set the key.
            tracing::warn!("no callback HMAC key configured: using a random per-process key");
            let a = uuid::Uuid::now_v7();
            let b = uuid::Uuid::now_v7();
            let mut raw = a.as_bytes().to_vec();
            raw.extend_from_slice(b.as_bytes());
            Ok(base64::engine::general_purpose::STANDARD
                .encode(raw)
                .into_bytes())
        }
    }
}

async fn serve(cli: Cli, cfg: &ConnectorConfig) -> std::io::Result<()> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    let core_url = cli
        .api_url
        .clone()
        .or_else(|| cli.core_url.clone())
        .unwrap_or_else(|| "http://127.0.0.1:4100".into());
    let key = hmac_key(&cli).map_err(invalid)?;
    let window = cli.catchup_window_secs.clamp(1, MAX_CATCHUP_SECS);
    let settings = Settings {
        public_url: cli
            .public_url
            .clone()
            .map(|u| u.trim_end_matches('/').to_owned())
            .filter(|u| !u.is_empty()),
        hmac_key: key,
        catchup_window: Duration::from_secs(window),
        token_check: Duration::from_secs(cli.token_check_secs.clamp(1, 3600)),
        api_url: Some(core_url.trim_end_matches('/').to_owned()),
        capabilities: cli.capabilities.clone(),
        test_control: cli.test_control,
        net: aurora_connector_mattermost::api::NetPolicy::from_env(),
    };
    if settings.public_url.is_none() {
        tracing::warn!("no public URL configured: buttons, menus and slash commands are disabled");
    }
    let mut connector = MattermostConnector::new(settings);
    if let Some(p) = &cli.publisher {
        connector = connector.with_publisher(p);
    }
    let connector = Arc::new(connector);

    let metrics = service::install_metrics(SERVICE);
    let ops = service::ops_router(metrics, vec![]);
    let (host, port) = cli
        .listen
        .rsplit_once(':')
        .ok_or_else(|| invalid("connector.listen must be host:port".into()))?;
    let port: u16 = port
        .parse()
        .map_err(|_| invalid("bad listen port".into()))?;
    let public = service::bind(host.trim_matches(['[', ']']), port).await?;
    let ops_listener = service::bind("0.0.0.0", cli.ops_port).await?;
    let callbacks = connector.router().unwrap_or_default();

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut rt = RuntimeConfig::new(&core_url, cfg.token.expose());
    if let Some(sig) = cli.manifest_signature.clone() {
        rt.manifest_signature = sig;
    }
    let runtime = tokio::spawn(aurora_connector_sdk::run(connector.clone(), rt, stop_rx));
    let served = service::serve_all(vec![(public, callbacks), (ops_listener, ops)]).await;
    let _ = stop_tx.send(true);
    match runtime.await {
        Ok(Err(e)) => tracing::error!(error = %e, "connector runtime stopped"),
        Err(e) => tracing::error!(error = %e, "connector runtime panicked"),
        Ok(Ok(())) => {}
    }
    served
}

//! aurora-supervisor binary: `run` (supervisor) and `mcp-bridge` (SUP-002).
#![forbid(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use aurora_supervisor::adapters;
use aurora_supervisor::boot::{self, BootDocument};
use aurora_supervisor::channel::{self, ChannelConfig};
use aurora_supervisor::harness::HarnessAdapter;
use aurora_supervisor::logging;
use aurora_supervisor::proxy::{Credential, GatewayUpstream, ProxyMode, spawn_proxy};
use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig, boot_refusal};
use clap::{Parser, Subcommand};

const MACHINE_PROXY_PORT: u16 = 3128;
const OPEN_PROXY_PORT: u16 = 3129;

#[derive(Parser)]
#[command(
    name = "aurora-supervisor",
    version,
    about = "Aurora in-sandbox supervisor"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the supervisor.
    Run,
    /// Run the MCP bridge.
    McpBridge {
        /// Conversation id the harness session belongs to.
        #[arg(long)]
        session: String,
        #[arg(long, default_value = aurora_supervisor::tools::DEFAULT_SOCKET)]
        socket: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    logging::init();
    match Cli::parse().command {
        Command::Run => match run().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                tracing::error!(error = %e, "supervisor failed");
                ExitCode::FAILURE
            }
        },
        Command::McpBridge { session, socket } => {
            aurora_supervisor::mcp_bridge::run(
                session,
                socket,
                tokio::io::stdin(),
                tokio::io::stdout(),
            )
            .await;
            ExitCode::SUCCESS
        }
    }
}

/// Sends `sandbox.status failed <code>` over a short-lived channel (the bootstrap token is still
/// unused: nothing connected yet), then lets it flush and close (D-153, FIX-025).
async fn report_boot_failure(doc: &BootDocument, code: &str, message: &str) {
    let chan = channel::spawn(ChannelConfig::new(
        &doc.runner_url,
        doc.bootstrap_token.clone(),
    ));
    let _ = chan.sender.send(boot_refusal(code, message)).await;
    // Dropping the sender lets the channel task flush and close.
    drop(chan.sender);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), chan.task).await;
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let raw = boot::read_boot_env()?;
    boot::make_non_dumpable()?;
    let doc = BootDocument::parse(&raw)?;
    drop(raw);
    let skipped = boot::write_boot_files(&doc, Path::new(boot::DEFAULT_CA_PATH))?;
    for p in skipped {
        tracing::info!(path = %p.display(), "boot file kept (exists without sentinel)");
    }

    // (P4-HAR, D-122, SEC-66) The harness is the boot document's, from the compile-time
    // registry. An unknown id or a runtime missing from the image refuses the boot before
    // `ready` (reason `harness_unsupported`): there is no fallback to another harness.
    let adapter: Arc<dyn HarnessAdapter> = match adapters::adapter_for(&doc.harness.id) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(harness = %doc.harness.id, code = %e.code, error = %e.message, "boot refused");
            report_boot_failure(&doc, &e.code, &e.message).await;
            return Err(format!("{}: {}", e.code, e.message).into());
        }
    };

    let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let boot_token = doc.gateway.boot_token.expose().to_owned();
    // (FIX-025, C-585) A boot step that fails before the channel exists is reported to the runner
    // as `sandbox.status failed supervisor_boot_failed` with the error, instead of a silent exit.
    let proxies = async {
        let upstream = GatewayUpstream::new(
            doc.gateway.proxy.clone(),
            &doc.ca_pem,
            &doc.gateway.verify_name(),
        )?;
        let machine = spawn_proxy(
            SocketAddr::new(lo, MACHINE_PROXY_PORT),
            upstream.clone(),
            Credential::new("x", boot_token.clone()),
            ProxyMode::Mitm,
        )
        .await?;
        // (RFE-003) The open proxy exists only for a sandbox booted with the `browser`
        // capability; otherwise nothing listens on 3129 (the gateway also refuses the lane).
        let open = if doc.has_capability("browser") {
            Some(
                spawn_proxy(
                    SocketAddr::new(lo, OPEN_PROXY_PORT),
                    upstream.clone(),
                    Credential::new("open", boot_token.clone()),
                    ProxyMode::ConnectOnly,
                )
                .await?,
            )
        } else {
            tracing::info!("no browser capability: open proxy not started");
            None
        };
        Ok::<_, Box<dyn std::error::Error>>((upstream, machine, open))
    }
    .await;
    let (upstream, _machine, _open) = match proxies {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("the supervisor could not start its gateway proxies: {e}");
            tracing::error!(error = %msg, "boot failed");
            report_boot_failure(&doc, "supervisor_boot_failed", &msg).await;
            return Err(msg.into());
        }
    };

    let chan = channel::spawn(ChannelConfig::new(
        &doc.runner_url,
        doc.bootstrap_token.clone(),
    ));
    let mut cfg = SupervisorConfig::new(upstream, boot_token);
    cfg.pg = Some(aurora_supervisor::pgrelay::RELAY_PORT);
    cfg.tools.socket = Some(PathBuf::from(aurora_supervisor::tools::DEFAULT_SOCKET));
    if let Ok(exe) = std::env::current_exe() {
        cfg.mcp_bridge_command = vec![exe.display().to_string(), "mcp-bridge".to_owned()];
    }
    let sup = Supervisor::new(cfg, chan.sender, adapter);
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        () = sup.run(chan.inbound) => Ok(()),
        r = chan.task => Ok(r??),
        _ = term.recv() => Ok(()),
    }
}

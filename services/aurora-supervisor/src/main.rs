//! aurora-supervisor binary: `run` (supervisor) and `mcp-bridge` (SUP-002).
#![forbid(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use aurora_supervisor::boot::{self, BootDocument};
use aurora_supervisor::channel::{self, ChannelConfig};
use aurora_supervisor::fake::FakeHarness;
use aurora_supervisor::harness::HarnessAdapter;
use aurora_supervisor::logging;
use aurora_supervisor::proxy::{Credential, ProxyMode, spawn_proxy};
use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig};
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
    McpBridge,
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
        Command::McpBridge => {
            eprintln!("aurora-supervisor mcp-bridge: not implemented in this milestone (SUP-002)");
            ExitCode::from(2)
        }
    }
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

    let adapter: Arc<dyn HarnessAdapter> = match doc.harness.id.as_str() {
        "fake" => Arc::new(FakeHarness),
        other => return Err(format!("harness `{other}` is not available in this build").into()),
    };

    let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let boot_token = doc.gateway.boot_token.expose().to_owned();
    let upstream = doc.gateway.proxy.clone();
    let _machine = spawn_proxy(
        SocketAddr::new(lo, MACHINE_PROXY_PORT),
        upstream.clone(),
        Credential::new("x", boot_token.clone()),
        ProxyMode::Mitm,
    )
    .await?;
    let _open = spawn_proxy(
        SocketAddr::new(lo, OPEN_PROXY_PORT),
        upstream.clone(),
        Credential::new("open", boot_token.clone()),
        ProxyMode::ConnectOnly,
    )
    .await?;

    let chan = channel::spawn(ChannelConfig::new(
        &doc.runner_url,
        doc.bootstrap_token.clone(),
    ));
    let sup = Supervisor::new(
        SupervisorConfig::new(upstream, boot_token),
        chan.sender,
        adapter,
    );
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        () = sup.run(chan.inbound) => Ok(()),
        r = chan.task => Ok(r??),
        _ = term.recv() => Ok(()),
    }
}

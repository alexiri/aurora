//! aurora-supervisor: skeleton; behaviour arrives in a later milestone.
#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use std::process::ExitCode;

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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let name = match cli.command {
        Command::Run => "run",
        Command::McpBridge => "mcp-bridge",
    };
    eprintln!("aurora-supervisor {name}: not implemented in this milestone");
    ExitCode::from(2)
}

//! aurora: operator CLI skeleton (`--version` only in this milestone).
#![forbid(unsafe_code)]

use clap::Parser;

#[derive(Parser)]
#[command(name = "aurora", version, about = "Aurora command-line client")]
struct Cli {}

fn main() {
    let _ = Cli::parse();
}

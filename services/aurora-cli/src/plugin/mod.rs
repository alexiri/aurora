//! `aurora plugin ...` (docs/09 §12): package development tooling. Everything here is
//! offline; validation, compilation and fixture runs reuse the registry and gateway code.

mod cmds;
mod hooktest;
mod pack;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

pub use cmds::Reserved;

#[derive(Subcommand)]
pub enum PluginCmd {
    /// Scaffold a package directory.
    New {
        id: String,
        /// Parent directory (default: current).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "example")]
        publisher: String,
    },
    /// Assemble `<id>-<version>.aurora-plugin` from a source directory.
    Build {
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Output file (default: `<id>-<version>.aurora-plugin` in the current directory).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Sign with this key file (see `keygen`).
        #[arg(long)]
        key: Option<PathBuf>,
    },
    /// Run the linter (signature, manifest rules, wildcard proofs, zones, hooks).
    Validate {
        /// Package archive or source directory.
        package: PathBuf,
        /// Trust this public key file for the package's publisher.
        #[arg(long)]
        pubkey: Vec<PathBuf>,
        /// Accept a missing signature (dev only).
        #[arg(long)]
        allow_unsigned: bool,
        /// Extra reserved host pattern (repeatable; `*.aurora.internal` is always reserved).
        #[arg(long)]
        reserved_host: Vec<String>,
        /// Instance external URL; its host is reserved.
        #[arg(long)]
        external_url: Option<String>,
        #[arg(long, default_value = cmds::CORE_VERSION)]
        core_version: String,
    },
    /// Print the registry snapshot this package contributes.
    Compile { package: PathBuf },
    /// Validate, compile and run `tests/*.yaml` through the check-snapshot runner.
    Test {
        /// Package archive or source directory.
        package: PathBuf,
        /// Trust this public key file for the package's publisher (repeatable).
        #[arg(long)]
        pubkey: Vec<PathBuf>,
        /// Extra reserved host pattern (repeatable; `*.aurora.internal` is always reserved).
        #[arg(long)]
        reserved_host: Vec<String>,
        /// Instance external URL; its host is reserved.
        #[arg(long)]
        external_url: Option<String>,
        #[arg(long, default_value = cmds::CORE_VERSION)]
        core_version: String,
    },
    /// Write `SIGNATURE` into an archive.
    Sign {
        package: PathBuf,
        #[arg(long)]
        key: PathBuf,
        /// Output (default: overwrite the input).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Create an ed25519 key pair (`<out>` private, `<out>.pub` public).
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Render the setup guide and permissions table.
    Docs {
        package: PathBuf,
        #[arg(long, default_value = "md", value_parser = ["md", "html"])]
        format: String,
    },
}

pub fn run(cmd: PluginCmd) -> Result<i32> {
    match cmd {
        PluginCmd::New { id, dir, publisher } => cmds::new(&id, &dir, &publisher),
        PluginCmd::Build { dir, out, key } => cmds::build(&dir, out, key),
        PluginCmd::Validate {
            package,
            pubkey,
            allow_unsigned,
            reserved_host,
            external_url,
            core_version,
        } => cmds::validate(
            &package,
            &pubkey,
            allow_unsigned,
            &Reserved::new(reserved_host, external_url),
            &core_version,
        ),
        PluginCmd::Compile { package } => cmds::compile(&package),
        PluginCmd::Test {
            package,
            pubkey,
            reserved_host,
            external_url,
            core_version,
        } => cmds::test(
            &package,
            &pubkey,
            &Reserved::new(reserved_host, external_url),
            &core_version,
        ),
        PluginCmd::Sign { package, key, out } => cmds::sign(&package, &key, out),
        PluginCmd::Keygen { out } => cmds::keygen(&out),
        PluginCmd::Docs { package, format } => cmds::docs(&package, &format),
    }
}

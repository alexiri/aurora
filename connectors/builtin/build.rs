//! Embeds the detached manifest signatures of the built-in connector types (docs/13 §3.1).
//!
//! The release image build signs each type's manifest with the project key
//! (`aurora-connector-host sign-manifests`, see `images/services/Containerfile`) and rebuilds with
//! `AURORA_CONNECTOR_SIGNATURES_DIR=<dir>` holding `<type_id>.sig`. A type without a file embeds
//! an empty signature: a core that verifies signatures refuses (and names) the type at `register`,
//! so only builds that were signed list their types.

use std::path::PathBuf;

/// Types the crate compiles in (kept in step with `src/lib.rs`).
const TYPES: &[&str] = &["mattermost", "fake"];

fn main() {
    println!("cargo:rerun-if-env-changed=AURORA_CONNECTOR_SIGNATURES_DIR");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    let dir = std::env::var_os("AURORA_CONNECTOR_SIGNATURES_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    for ty in TYPES {
        let mut text = String::new();
        if let Some(d) = &dir {
            let src = d.join(format!("{ty}.sig"));
            println!("cargo:rerun-if-changed={}", src.display());
            text = std::fs::read_to_string(&src).unwrap_or_default();
        }
        if let Err(e) = std::fs::write(out.join(format!("{ty}.sig")), text) {
            println!("cargo:warning=cannot write embedded signature for {ty}: {e}");
            std::process::exit(1);
        }
    }
}

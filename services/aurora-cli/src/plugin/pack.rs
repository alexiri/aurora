//! Source directory <-> package archive helpers and key files.

use anyhow::{Context, Result, anyhow, bail};
use aurora_registry::builder::PackageBuilder;
use aurora_registry::{Limits, Package};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{SigningKey, VerifyingKey};
use std::path::Path;

/// Files a source directory may contribute (docs/09 §2), relative paths.
fn source_files(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for f in [
        "plugin.yaml",
        "assets/icon.png",
        "assets/icon-dark.png",
        "docs/setup.md",
        "hooks.wasm",
    ] {
        if dir.join(f).is_file() {
            out.push(f.to_owned());
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir.join("tests")) {
        let mut t: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
            .map(|e| format!("tests/{}", e.file_name().to_string_lossy()))
            .collect();
        t.sort();
        out.extend(t);
    }
    Ok(out)
}

/// A builder holding exactly the files of `dir` (no defaults).
pub fn builder_from_dir(dir: &Path) -> Result<PackageBuilder> {
    let manifest = std::fs::read_to_string(dir.join("plugin.yaml"))
        .with_context(|| format!("{}/plugin.yaml", dir.display()))?;
    let mut b = PackageBuilder::new(&manifest);
    b.remove_file("assets/icon.png")
        .remove_file("docs/setup.md");
    for f in source_files(dir)? {
        let data = std::fs::read(dir.join(&f)).with_context(|| f.clone())?;
        b.set_file(&f, &data);
    }
    Ok(b)
}

#[derive(serde::Deserialize)]
struct Head {
    package: HeadPkg,
    #[serde(default)]
    permissions: HeadPerm,
}
#[derive(serde::Deserialize)]
struct HeadPkg {
    id: String,
    version: String,
}
#[derive(serde::Deserialize, Default)]
struct HeadPerm {
    #[serde(default)]
    hooks: Vec<serde_yaml::Value>,
}

pub struct Identity {
    pub id: String,
    pub version: String,
    pub has_hooks: bool,
}

pub fn identity(dir: &Path) -> Result<Identity> {
    let y = std::fs::read_to_string(dir.join("plugin.yaml"))
        .with_context(|| format!("{}/plugin.yaml", dir.display()))?;
    let h: Head =
        serde_yaml::from_str(&y).context("plugin.yaml: cannot read package id/version")?;
    Ok(Identity {
        id: h.package.id,
        version: h.package.version,
        has_hooks: !h.permissions.hooks.is_empty(),
    })
}

/// Compile the hook guest crate at `src` (a standalone cargo project whose `cdylib` implements
/// `aurora-plugin-sdk`'s hooks) to a WebAssembly component for `wasm32-wasip2` and return it.
/// `--locked` keeps the build reproducible: `Cargo.lock` is part of the source. The target
/// directory is cargo's default (`<src>/target`) or `CARGO_TARGET_DIR` when the caller sets it.
fn compile_hooks(src: &Path) -> Result<Vec<u8>> {
    use std::process::{Command, Stdio};
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(&cargo)
        .current_dir(src)
        .args([
            "build",
            "--release",
            "--locked",
            "--target",
            "wasm32-wasip2",
            "--message-format=json-render-diagnostics",
        ])
        // The host's link flags (lld, ...) are for native targets only.
        .env("RUSTFLAGS", "")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| {
            format!(
                "cannot run {} (is the Rust toolchain installed?)",
                cargo.to_string_lossy()
            )
        })?;
    if !out.status.success() {
        bail!(
            "compiling {} failed (needs the `wasm32-wasip2` target: `rustup target add wasm32-wasip2`, \
             and a committed Cargo.lock)",
            src.display()
        );
    }
    let mut wasm: Vec<std::path::PathBuf> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] != "compiler-artifact"
            || !v["target"]["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|k| k == "cdylib"))
        {
            continue;
        }
        for f in v["filenames"].as_array().into_iter().flatten() {
            if let Some(p) = f.as_str().filter(|p| p.ends_with(".wasm")) {
                wasm.push(p.into());
            }
        }
    }
    let [path] = wasm.as_slice() else {
        bail!(
            "{}: expected exactly one cdylib (`crate-type = [\"cdylib\"]`) producing a .wasm, found {}",
            src.display(),
            wasm.len()
        );
    };
    std::fs::read(path).with_context(|| path.display().to_string())
}

/// Build the archive bytes from `dir`. A package that declares hooks ships `hooks.wasm`: either
/// compiled from the source project in `<dir>/hooks/` (built-ins, docs/09 §12), or a prebuilt
/// `<dir>/hooks.wasm` (never both).
pub fn build_dir(dir: &Path, key: Option<&SigningKey>) -> Result<Vec<u8>> {
    let id = identity(dir)?;
    let src = dir.join("hooks");
    let compiled = if src.join("Cargo.toml").is_file() {
        if !id.has_hooks {
            bail!("hooks/ holds a hook guest crate but permissions.hooks is empty in plugin.yaml");
        }
        if dir.join("hooks.wasm").exists() {
            bail!(
                "both hooks/ (source) and hooks.wasm exist: keep only the source, the build compiles it"
            );
        }
        Some(compile_hooks(&src)?)
    } else {
        None
    };
    if id.has_hooks && compiled.is_none() && !dir.join("hooks.wasm").is_file() {
        bail!(
            "permissions.hooks is non-empty but there is no hook guest: put the crate in hooks/ \
             (Cargo.toml with crate-type cdylib, Cargo.lock) or a prebuilt hooks.wasm next to plugin.yaml"
        );
    }
    let mut b = builder_from_dir(dir)?;
    if let Some(w) = &compiled {
        b.set_file("hooks.wasm", w);
    }
    if let Some(k) = key {
        b.sign(k, &PackageBuilder::key_id(k));
    }
    Ok(b.build()?)
}

/// Read an archive, or build a (unsigned) one from a directory.
pub fn load(path: &Path) -> Result<Package> {
    let bytes = if path.is_dir() {
        build_dir(path, None)?
    } else {
        std::fs::read(path).with_context(|| path.display().to_string())?
    };
    Package::read(&bytes, &Limits::default()).map_err(|v| {
        anyhow!(
            "invalid package:\n{}",
            v.iter()
                .map(|x| format!("  {x}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

/// Rebuild an archive from `pkg`'s files (used by `sign`).
pub fn rebuild(pkg: &Package, key: &SigningKey) -> Result<Vec<u8>> {
    let manifest = pkg
        .manifest_yaml()
        .ok_or_else(|| anyhow!("package has no plugin.yaml"))?;
    let mut b = PackageBuilder::new(manifest);
    b.remove_file("assets/icon.png")
        .remove_file("docs/setup.md");
    for (p, d) in &pkg.files {
        b.set_file(p, d);
    }
    b.sign(key, &PackageBuilder::key_id(key));
    Ok(b.build()?)
}

pub fn read_signing_key(path: &Path) -> Result<SigningKey> {
    let t = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let raw = STANDARD
        .decode(t.trim())
        .context("key file is not base64")?;
    let seed: [u8; 32] = raw
        .try_into()
        .map_err(|_| anyhow!("key file must hold a 32-byte seed"))?;
    Ok(SigningKey::from_bytes(&seed))
}

pub fn read_public_key(path: &Path) -> Result<VerifyingKey> {
    let t = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    aurora_crypto::parse_public_key_b64(t.trim()).map_err(|e| anyhow!("{}: {e}", path.display()))
}

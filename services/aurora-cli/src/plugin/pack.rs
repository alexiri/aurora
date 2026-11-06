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

/// Build the archive bytes from `dir`; `hooks.wasm` is never compiled here (I-02).
pub fn build_dir(dir: &Path, key: Option<&SigningKey>) -> Result<Vec<u8>> {
    let id = identity(dir)?;
    if id.has_hooks && !dir.join("hooks.wasm").is_file() {
        bail!(
            "permissions.hooks is non-empty but hooks.wasm is missing: compile it with \
             `cargo component build --release --target wasm32-wasip2` and copy it next to plugin.yaml"
        );
    }
    let mut b = builder_from_dir(dir)?;
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

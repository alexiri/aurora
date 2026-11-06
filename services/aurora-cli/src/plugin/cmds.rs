use super::pack;
use anyhow::{Context, Result, anyhow, bail};
use aurora_registry::snapshot::{ActivePackage, compile as compile_snapshot};
use aurora_registry::validate::{ValidationContext, package as validate_package};
use aurora_registry::{Package, TrustStore, TrustedKey, Violation};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Snapshot contract this CLI compiles against (matches the gateway).
const CONTRACT: u32 = 1;

pub fn new(id: &str, dir: &Path, publisher: &str) -> Result<i32> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !ok {
        bail!("package id must match [a-z][a-z0-9_-]*");
    }
    let root = dir.join(id);
    if root.exists() {
        bail!("{} already exists", root.display());
    }
    std::fs::create_dir_all(root.join("assets"))?;
    std::fs::create_dir_all(root.join("docs"))?;
    std::fs::create_dir_all(root.join("tests"))?;
    let w = |p: &str, d: &[u8]| std::fs::write(root.join(p), d).with_context(|| p.to_owned());
    w(
        "plugin.yaml",
        format!(
            include_str!("templates/plugin.yaml"),
            id = id,
            publisher = publisher
        )
        .as_bytes(),
    )?;
    w(
        "assets/icon.png",
        &aurora_registry::builder::png_stub(64, 64),
    )?;
    w(
        "docs/setup.md",
        format!(
            "# {id}\n\nCreate an API token in the service settings and paste it when connecting.\n"
        )
        .as_bytes(),
    )?;
    w(
        "tests/basic.yaml",
        format!(include_str!("templates/basic.yaml"), id = id).as_bytes(),
    )?;
    println!("created {}", root.display());
    Ok(0)
}

pub fn build(dir: &Path, out: Option<PathBuf>, key: Option<PathBuf>) -> Result<i32> {
    let key = key.map(|k| pack::read_signing_key(&k)).transpose()?;
    let id = pack::identity(dir)?;
    let bytes = pack::build_dir(dir, key.as_ref())?;
    let out =
        out.unwrap_or_else(|| PathBuf::from(format!("{}-{}.aurora-plugin", id.id, id.version)));
    std::fs::write(&out, &bytes).with_context(|| out.display().to_string())?;
    println!(
        "wrote {} ({} bytes, {})",
        out.display(),
        bytes.len(),
        if key.is_some() { "signed" } else { "unsigned" }
    );
    Ok(0)
}

fn violations_json(v: &[Violation]) -> Value {
    Value::Array(
        v.iter()
            .map(|x| json!({ "code": x.code, "path": x.path, "message": x.message }))
            .collect(),
    )
}

fn lint(
    pkg: &Package,
    pubkeys: &[PathBuf],
    allow_unsigned: bool,
    core: &str,
) -> Result<Vec<Violation>> {
    let publisher = pkg
        .manifest(core)
        .map(|m| m.package.publisher)
        .unwrap_or_default();
    let mut trust = TrustStore::default();
    for p in pubkeys {
        trust.keys.push(TrustedKey {
            publisher: publisher.clone(),
            key: pack::read_public_key(p)?,
            revoked: false,
        });
    }
    let ctx = ValidationContext {
        running_version: core.to_owned(),
        ..Default::default()
    };
    Ok(validate_package(pkg, &ctx, &trust, allow_unsigned))
}

pub fn validate(path: &Path, pubkeys: &[PathBuf], allow_unsigned: bool, core: &str) -> Result<i32> {
    let pkg = pack::load(path)?;
    let v = lint(&pkg, pubkeys, allow_unsigned, core)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({ "ok": v.is_empty(), "violations": violations_json(&v) })
        )?
    );
    Ok(i32::from(!v.is_empty()))
}

fn active(pkg: &Package, core: &str) -> Result<ActivePackage> {
    let manifest = pkg.manifest(core).map_err(|v| {
        anyhow!(
            "manifest invalid: {}",
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    Ok(ActivePackage {
        digest: hex(&aurora_registry::digest::digest(pkg)),
        publisher: manifest.package.publisher.clone(),
        hooks_digest: pkg.file("hooks.wasm").map(|w| hex(&Sha256::digest(w))),
        manifest,
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn compiled(pkg: &Package, core: &str) -> Result<aurora_registry::snapshot::CompiledSnapshot> {
    compile_snapshot(&[active(pkg, core)?], &[], &[], CONTRACT).map_err(|e| anyhow!("compile: {e}"))
}

pub fn compile(path: &Path) -> Result<i32> {
    let pkg = pack::load(path)?;
    let c = compiled(&pkg, CORE_VERSION)?;
    let v: Value = serde_json::from_slice(&c.json)?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(0)
}

pub fn test(path: &Path, core: &str) -> Result<i32> {
    let pkg = pack::load(path)?;
    let v = lint(&pkg, &[], true, core)?;
    let mut ok = v.is_empty();
    let mut out = json!({ "validate": { "ok": v.is_empty(), "violations": violations_json(&v) } });
    if ok {
        let c = compiled(&pkg, core)?;
        let r = run_check_snapshot(&pkg, &c.json)?;
        ok = r["ok"].as_bool().unwrap_or(false);
        out["compile"] = json!({ "ok": true, "snapshot_hash": c.hash });
        out["fixtures"] = r;
    }
    out["ok"] = json!(ok);
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(i32::from(!ok))
}

pub fn sign(path: &Path, key: &Path, out: Option<PathBuf>) -> Result<i32> {
    let key = pack::read_signing_key(key)?;
    let pkg = pack::load(path)?;
    let bytes = pack::rebuild(&pkg, &key)?;
    let out = out.unwrap_or_else(|| path.to_path_buf());
    std::fs::write(&out, bytes).with_context(|| out.display().to_string())?;
    println!(
        "signed {} (key id {})",
        out.display(),
        aurora_registry::builder::PackageBuilder::key_id(&key)
    );
    Ok(0)
}

pub fn keygen(out: &Path) -> Result<i32> {
    use std::io::Write;
    let key = aurora_crypto::generate_signing_key().map_err(|e| anyhow!("{e}"))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts
        .open(out)
        .with_context(|| format!("{} (refusing to overwrite)", out.display()))?;
    writeln!(f, "{}", STANDARD.encode(key.to_bytes()))?;
    let public = STANDARD.encode(key.verifying_key().to_bytes());
    let pub_path = PathBuf::from(format!("{}.pub", out.display()));
    std::fs::write(&pub_path, format!("{public}\n"))?;
    println!(
        "private key: {}\npublic key:  {} ({public})",
        out.display(),
        pub_path.display()
    );
    Ok(0)
}

pub fn docs(path: &Path, format: &str) -> Result<i32> {
    let pkg = pack::load(path)?;
    let m = pkg.manifest(CORE_VERSION).map_err(|v| {
        anyhow!(
            "manifest invalid: {}",
            v.first().map(ToString::to_string).unwrap_or_default()
        )
    })?;
    let mut md = format!(
        "# {} {}\n\n{}\n\n",
        m.package.name, m.package.version, m.package.description
    );
    md.push_str(&String::from_utf8_lossy(
        pkg.file("docs/setup.md").unwrap_or_default(),
    ));
    md.push_str("\n\n## Permissions\n\n| Permission | Value |\n|---|---|\n");
    let hooks: Vec<String> = m
        .permissions
        .hooks
        .iter()
        .map(|h| {
            serde_json::to_value(h)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        })
        .collect();
    md.push_str(&format!("| hooks | {} |\n", or_none(&hooks)));
    md.push_str(&format!(
        "| context | {} |\n",
        or_none(&m.permissions.context)
    ));
    for (h, hosts) in &m.permissions.egress {
        let name = serde_json::to_value(h)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        md.push_str(&format!("| egress ({name}) | {} |\n", or_none(hosts)));
    }
    for a in &m.apps {
        let hosts: Vec<String> = a.hosts.iter().map(|h| h.matcher.host.clone()).collect();
        md.push_str(&format!(
            "| app `{}` hosts | {} |\n",
            a.about.id,
            or_none(&hosts)
        ));
    }
    if format == "html" {
        let mut html = String::new();
        let opts = pulldown_cmark::Options::ENABLE_TABLES;
        // Raw HTML in the guide is escaped, never passed through.
        let events = pulldown_cmark::Parser::new_ext(&md, opts).map(|e| match e {
            pulldown_cmark::Event::Html(t) | pulldown_cmark::Event::InlineHtml(t) => {
                pulldown_cmark::Event::Text(t)
            }
            e => e,
        });
        pulldown_cmark::html::push_html(&mut html, events);
        println!("{html}");
    } else {
        println!("{md}");
    }
    Ok(0)
}

fn or_none(v: &[String]) -> String {
    if v.is_empty() {
        "none".to_owned()
    } else {
        v.iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Locate `aurora-gateway`: `AURORA_GATEWAY_BIN`, next to this executable, else PATH.
fn gateway_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("AURORA_GATEWAY_BIN") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("aurora-gateway")))
        .filter(|p| p.is_file())
    {
        return p;
    }
    PathBuf::from("aurora-gateway")
}

/// Runs `aurora-gateway check-snapshot` over the package's `tests/*.yaml`.
fn run_check_snapshot(pkg: &Package, snapshot: &[u8]) -> Result<Value> {
    let dir = std::env::temp_dir().join(format!(
        "aurora-plugin-test-{}",
        uuid::Uuid::now_v7().simple()
    ));
    let fx = dir.join("fixtures");
    std::fs::create_dir_all(&fx)?;
    let result = (|| {
        std::fs::write(dir.join("snapshot.json"), snapshot)?;
        for (p, d) in &pkg.files {
            if let Some(n) = p.strip_prefix("tests/") {
                std::fs::write(fx.join(n), d)?;
            }
        }
        let bin = gateway_bin();
        let out = std::process::Command::new(&bin)
            .args(["check-snapshot", "--snapshot"])
            .arg(dir.join("snapshot.json"))
            .arg("--fixtures")
            .arg(&fx)
            .output()
            .with_context(|| format!("cannot run {} (set AURORA_GATEWAY_BIN)", bin.display()))?;
        // Exit 1 = fixture failures (JSON still printed); anything else is an error.
        if out.stdout.is_empty() || out.status.code().is_none_or(|c| c > 1) {
            bail!(
                "check-snapshot failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        serde_json::from_slice::<Value>(&out.stdout).context("check-snapshot output is not JSON")
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

//! `aurora run`: apply a container config to a child process (08 §10).
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use aurora_client::types::ContainerConfig;
use base64::Engine;

pub const PLACEHOLDER: &str = "<AGENT_TOKEN>";

fn write_private(path: &Path, content: &str) -> Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

/// Create a private (0700) temp directory for the CA files.
pub fn make_temp_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("aurora-run-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// System trust roots as PEM (used to build the combined bundle).
pub fn system_roots_pem() -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut out = String::new();
    for cert in rustls_native_certs::load_native_certs().certs {
        out.push_str("-----BEGIN CERTIFICATE-----\n");
        let enc = b64.encode(cert.as_ref());
        for chunk in enc.as_bytes().chunks(64) {
            out.push_str(&String::from_utf8_lossy(chunk));
            out.push('\n');
        }
        out.push_str("-----END CERTIFICATE-----\n");
    }
    out
}

/// Build the child's environment additions. Writes `ca.pem` (gateway CA) and
/// `bundle.pem` (system roots + gateway CA) into `dir`; every env value equal
/// to the config's `ca_path` points at the bundle; `<AGENT_TOKEN>` is replaced
/// with `token` (an error if it is still unresolved).
pub fn build_env(
    cfg: &ContainerConfig,
    token: Option<&str>,
    dir: &Path,
    system_pem: &str,
) -> Result<BTreeMap<String, String>> {
    let ca = dir.join("ca.pem");
    let bundle = dir.join("bundle.pem");
    write_private(&ca, &cfg.ca_pem)?;
    let mut combined = String::from(system_pem);
    if !combined.is_empty() && !combined.ends_with('\n') {
        combined.push('\n');
    }
    combined.push_str(&cfg.ca_pem);
    write_private(&bundle, &combined)?;
    let bundle_s = bundle.to_string_lossy().into_owned();

    let sub = |v: &str| -> Result<String> {
        let v = if !cfg.ca_path.is_empty() && v == cfg.ca_path {
            bundle_s.clone()
        } else {
            v.to_string()
        };
        if v.contains(PLACEHOLDER) {
            match token {
                Some(t) => Ok(v.replace(PLACEHOLDER, t)),
                None => bail!(
                    "this agent's token is not stored; run `aurora agents token <handle>` first"
                ),
            }
        } else {
            Ok(v)
        }
    };
    let mut env = BTreeMap::new();
    for (k, v) in cfg.env.iter().chain(cfg.llm_env.iter()) {
        env.insert(k.clone(), sub(v)?);
    }
    // Always provide the CA bundle even if the config omitted the variables.
    for k in [
        "SSL_CERT_FILE",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "GIT_SSL_CAINFO",
    ] {
        env.entry(k.to_string()).or_insert_with(|| bundle_s.clone());
    }
    let proxy = sub(&cfg.proxy_url)?;
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        env.entry(k.to_string()).or_insert_with(|| proxy.clone());
    }
    Ok(env)
}

/// Exit code passthrough (128+signal when killed).
pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ContainerConfig {
        serde_json::from_value(serde_json::json!({
            "proxy_url": "http://x:<AGENT_TOKEN>@gw:4102",
            "env": {"HTTPS_PROXY": "http://x:<AGENT_TOKEN>@gw:4102", "SSL_CERT_FILE": "/aurora/ca.pem",
                    "NO_PROXY": "localhost"},
            "ca_pem": "-----BEGIN CERTIFICATE-----\nGW\n-----END CERTIFICATE-----\n",
            "ca_path": "/aurora/ca.pem",
            "llm_env": {"ANTHROPIC_AUTH_TOKEN": "aurora-placeholder"},
            "stubs": []
        }))
        .unwrap()
    }

    #[test]
    fn substitutes_token_and_builds_bundle() {
        let dir = make_temp_dir().unwrap();
        let env = build_env(&cfg(), Some("aug_tok"), &dir, "-----SYS-----\n").unwrap();
        assert_eq!(env["HTTPS_PROXY"], "http://x:aug_tok@gw:4102");
        assert_eq!(env["HTTP_PROXY"], "http://x:aug_tok@gw:4102");
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "aurora-placeholder");
        let bundle = std::fs::read_to_string(&env["SSL_CERT_FILE"]).unwrap();
        assert!(bundle.starts_with("-----SYS-----") && bundle.contains("GW"));
        assert!(env["CURL_CA_BUNDLE"].ends_with("bundle.pem"));
        let mode = std::fs::metadata(dir.join("bundle.pem"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // No API key or other real secret appears in the env.
        assert!(env.values().all(|v| !v.contains("auk_")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_token_is_an_error() {
        let dir = make_temp_dir().unwrap();
        assert!(build_env(&cfg(), None, &dir, "").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

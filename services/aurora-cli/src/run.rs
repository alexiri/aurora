//! `aurora run`: apply a container config to a child process (08 §10).
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use aurora_client::types::ContainerConfig;
use base64::Engine;

pub const PLACEHOLDER: &str = "<AGENT_TOKEN>";
/// (RFE-025) Placeholder of an agent that requires context credentials.
pub const CONTEXT_PLACEHOLDER: &str = "<CONTEXT_TOKEN>";

/// Default and bounds of `--ttl` (docs/08 §7).
pub const DEFAULT_TTL_SECS: u32 = 3600;
pub const TTL_RANGE: std::ops::RangeInclusive<i64> = 60..=86400;

/// Label of the context credential minted for one run: `aurora run: <command line>`, control
/// characters stripped, truncated to the API's 128 characters.
pub fn run_label(command: &[String]) -> String {
    let line: String = format!("aurora run: {}", command.join(" "))
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    if line.chars().count() <= 128 {
        line.to_owned()
    } else {
        let mut s: String = line.chars().take(127).collect();
        s.push('…');
        s
    }
}

/// `true` when the agent must use context credentials for credentialed requests.
pub fn needs_context(cfg: &ContainerConfig) -> bool {
    cfg.byo.as_ref().is_some_and(|b| b.require_context)
        || cfg.proxy_url.contains(CONTEXT_PLACEHOLDER)
}

/// Warnings to print before the child starts (go to stderr).
pub fn warnings(cfg: &ContainerConfig) -> Vec<String> {
    let mut out = cfg.notes.clone();
    if let Some(b) = &cfg.byo {
        if b.posture == "disabled" {
            out.push(
                "BYO agents are disabled for this workspace; requests will be refused.".into(),
            );
        }
        if let Some(l) = &b.source_cidrs {
            out.push(format!(
                "This agent's credentials only work from: {}. Requests from other addresses are refused (source_not_permitted).",
                l.join(", ")
            ));
        }
    }
    out
}

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
        if v.contains(PLACEHOLDER) || v.contains(CONTEXT_PLACEHOLDER) {
            match token {
                Some(t) => Ok(v.replace(PLACEHOLDER, t).replace(CONTEXT_PLACEHOLDER, t)),
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
    for k in PROXY_VARS {
        env.entry(k.to_string()).or_insert_with(|| proxy.clone());
    }
    // (phase 1, RFE-002, SEC-40) The gateway proxy listener is TLS-only: never hand a child a
    // cleartext gateway URL for a non-loopback host.
    crate::forward::require_tls_url(&proxy)?;
    for k in PROXY_VARS {
        if let Some(v) = env.get(k) {
            crate::forward::require_tls_url(v)?;
        }
    }
    Ok(env)
}

pub const PROXY_VARS: [&str; 4] = ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"];

/// The gateway proxy URL with the agent token substituted.
pub fn resolved_proxy_url(cfg: &ContainerConfig, token: &str) -> String {
    cfg.proxy_url
        .replace(PLACEHOLDER, token)
        .replace(CONTEXT_PLACEHOLDER, token)
}

/// `--forward`: point every proxy variable at the loopback forwarder (plain HTTP on 127.0.0.1,
/// no credentials; the forwarder adds them and speaks TLS upstream).
pub fn apply_forwarder(env: &mut BTreeMap<String, String>, forwarder_url: &str) {
    for k in PROXY_VARS {
        env.insert(k.to_owned(), forwarder_url.to_owned());
    }
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
            "proxy_url": "https://x:<AGENT_TOKEN>@gw:4102",
            "env": {"HTTPS_PROXY": "https://x:<AGENT_TOKEN>@gw:4102", "SSL_CERT_FILE": "/aurora/ca.pem",
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
        assert_eq!(env["HTTPS_PROXY"], "https://x:aug_tok@gw:4102");
        assert_eq!(env["HTTP_PROXY"], "https://x:aug_tok@gw:4102");
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
    fn cleartext_gateway_url_is_refused() {
        let dir = make_temp_dir().unwrap();
        let mut c = cfg();
        c.proxy_url = "http://x:<AGENT_TOKEN>@gw:4102".into();
        c.env.clear();
        let e = build_env(&c, Some("aug_tok"), &dir, "")
            .unwrap_err()
            .to_string();
        assert!(e.contains("TLS-only") && !e.contains("aug_tok"), "{e}");
        let mut c = cfg();
        c.env
            .insert("HTTP_PROXY".into(), "http://x:<AGENT_TOKEN>@gw:4102".into());
        assert!(build_env(&c, Some("aug_tok"), &dir, "").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forwarder_replaces_the_proxy_variables_with_loopback() {
        let dir = make_temp_dir().unwrap();
        let mut env = build_env(&cfg(), Some("aug_tok"), &dir, "").unwrap();
        apply_forwarder(&mut env, "http://127.0.0.1:41000");
        for k in PROXY_VARS {
            assert_eq!(env[k], "http://127.0.0.1:41000");
        }
        assert!(env.values().all(|v| !v.contains("aug_tok")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn context_placeholder_takes_the_context_credential() {
        let dir = make_temp_dir().unwrap();
        let mut c = cfg();
        c.proxy_url = "https://x:<CONTEXT_TOKEN>@gw:4102".into();
        c.env.insert("HTTPS_PROXY".into(), c.proxy_url.clone());
        let env = build_env(&c, Some("auc_run"), &dir, "").unwrap();
        assert_eq!(env["HTTPS_PROXY"], "https://x:auc_run@gw:4102");
        assert_eq!(env["https_proxy"], "https://x:auc_run@gw:4102");
        assert!(needs_context(&c));
        assert_eq!(
            resolved_proxy_url(&c, "auc_run"),
            "https://x:auc_run@gw:4102"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_label_is_bounded_and_clean() {
        let cmd = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            run_label(&cmd("git push origin")),
            "aurora run: git push origin"
        );
        let long = run_label(&["x".repeat(500)]);
        assert_eq!(long.chars().count(), 128);
        assert!(long.starts_with("aurora run: xxx"));
        assert!(
            !run_label(&["a\nb\u{7}c".to_owned()])
                .chars()
                .any(char::is_control)
        );
    }

    #[test]
    fn warnings_cover_notes_posture_and_source_list() {
        let mut c = cfg();
        c.notes = vec!["note".into()];
        c.byo = Some(aurora_client::types::ContainerByo {
            posture: "disabled".into(),
            source_cidrs: Some(vec!["203.0.113.0/24".into()]),
            require_context: false,
        });
        let w = warnings(&c);
        assert!(w[0] == "note" && w[1].contains("disabled") && w[2].contains("203.0.113.0/24"));
        assert!(warnings(&cfg()).is_empty());
    }

    #[test]
    fn missing_token_is_an_error() {
        let dir = make_temp_dir().unwrap();
        assert!(build_env(&cfg(), None, &dir, "").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

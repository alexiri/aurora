//! `/run/aurora/harness.env`: what the web terminal's `claude` and `codex` wrappers need to
//! reach the LLM **through the gateway** exactly as an agent turn does (FIX-138 item 1b,
//! `docs/12 §Terminal`).
//!
//! The file is shell-sourced by the wrappers in `/opt/aurora/bin` and holds **no secret**: the
//! machine proxy's address (`127.0.0.1:3128`, which adds the boot token itself; the terminal
//! never has it), the CA bundle path, the placeholder credential the gateway replaces, the
//! harnesses the instance enables and, once a turn has chosen one, the agent's model and
//! endpoint (the same values the adapters put into a turn's environment). The wrappers add
//! their own state directories; nothing here points at the agent harness's.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aurora_protocol::runner::ModelChoice;

use crate::adapters::claude_code::config::PLACEHOLDER;

/// Where the file lives (tmpfs `/run/aurora`, owned by the supervisor, read-only to the agent).
pub const DEFAULT_PATH: &str = "/run/aurora/harness.env";

/// The fixed inputs of the file (from the boot document and the supervisor's own listeners).
#[derive(Debug, Clone)]
pub struct HarnessEnvConfig {
    pub path: PathBuf,
    /// Harness ids the instance enables (`hosted.enabled_harnesses`, from the boot document).
    pub enabled: Vec<String>,
    /// The machine proxy's URL (`http://127.0.0.1:3128`).
    pub proxy_url: String,
    /// The gateway CA bundle (`/run/aurora/ca.pem`).
    pub ca_path: String,
}

/// A single-quoted shell word; control characters are dropped.
fn q(v: &str) -> String {
    let clean: String = v.chars().filter(|c| !c.is_control()).collect();
    format!("'{}'", clean.replace('\'', "'\\''"))
}

/// The file's text. `model` is the agent's last turn's model choice (`None` before the first
/// turn: the CLIs then use their own defaults and the gateway's governance decides what is
/// allowed).
pub fn render(cfg: &HarnessEnvConfig, model: Option<&ModelChoice>) -> String {
    let mut out = String::from(
        "# Written by aurora-supervisor. No secret: the gateway adds the agent's real LLM key.\n\
         # Sourced by /opt/aurora/bin/claude and /opt/aurora/bin/codex (docs/12 §Terminal).\n",
    );
    let mut set = |k: &str, v: &str| out.push_str(&format!("{k}={}\n", q(v)));
    set("HARNESSES_ENABLED", &cfg.enabled.join(" "));
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        set(k, &cfg.proxy_url);
    }
    set("NO_PROXY", "localhost,127.0.0.1");
    for k in [
        "SSL_CERT_FILE",
        "NODE_EXTRA_CA_CERTS",
        "CODEX_CA_CERTIFICATE",
    ] {
        set(k, &cfg.ca_path);
    }
    set("DISABLE_AUTOUPDATER", "1");
    set("DISABLE_TELEMETRY", "1");
    set("DISABLE_ERROR_REPORTING", "1");
    set("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    set("OPENAI_API_KEY", PLACEHOLDER);
    match model {
        Some(m) if m.dialect == "anthropic_messages" && m.provider != "anthropic" => {
            // As `claude_code::config::build_env`: a non-Anthropic endpoint is reached by its base URL.
            set("ANTHROPIC_BASE_URL", &m.base_url);
            set("ANTHROPIC_AUTH_TOKEN", PLACEHOLDER);
            set("ANTHROPIC_MODEL", &m.model);
        }
        Some(m) if m.dialect == "anthropic_messages" => {
            set("ANTHROPIC_API_KEY", PLACEHOLDER);
            set("ANTHROPIC_MODEL", &m.model);
        }
        Some(m) if m.dialect == "openai_responses" => {
            // Codex reads the provider from its config, not the environment: the wrapper turns
            // these into `-c` overrides (`codex::config::render_config` has the same values).
            set("ANTHROPIC_API_KEY", PLACEHOLDER);
            set("AURORA_CODEX_MODEL", &m.model);
            set(
                "AURORA_CODEX_BASE_URL",
                &crate::adapters::codex::config::openai_base_url(m),
            );
        }
        _ => set("ANTHROPIC_API_KEY", PLACEHOLDER),
    }
    out
}

/// Writes the file atomically with mode 0644 (world-readable: it holds nothing secret).
pub fn write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("env.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.set_permissions(std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::rename(&tmp, path)
}

/// The wrappers' own state directories (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) under the home's
/// `.aurora/terminal`, owned by the agent user so the CLIs can write there although `.aurora`
/// itself belongs to the supervisor. A terminal harness never reads or changes the agent
/// harness's own state (FIX-095). Best effort: a failure is the caller's warning.
pub fn ensure_state_dirs(home: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    let base = home.join(".aurora/terminal");
    for d in [base.clone(), base.join("claude"), base.join("codex")] {
        std::fs::create_dir_all(&d)?;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755))?;
        // Not root (tests): the directories are already ours.
        let _ = std::os::unix::fs::chown(&d, Some(uid), Some(gid));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HarnessEnvConfig {
        HarnessEnvConfig {
            path: PathBuf::from(DEFAULT_PATH),
            enabled: vec!["claude-code".into(), "codex".into()],
            proxy_url: "http://127.0.0.1:3128".into(),
            ca_path: "/run/aurora/ca.pem".into(),
        }
    }

    fn choice(provider: &str, dialect: &str) -> ModelChoice {
        ModelChoice {
            provider: provider.into(),
            endpoint_id: "e".into(),
            base_url: "http://llm.example/v1".into(),
            dialect: dialect.into(),
            model: "m'1".into(),
            effort: String::new(),
            client_env: vec![],
        }
    }

    #[test]
    fn before_any_turn_it_has_the_proxy_the_ca_the_placeholder_and_no_model() {
        let t = render(&cfg(), None);
        assert!(t.contains("HTTPS_PROXY='http://127.0.0.1:3128'\n"), "{t}");
        assert!(t.contains("SSL_CERT_FILE='/run/aurora/ca.pem'\n"), "{t}");
        assert!(t.contains("HARNESSES_ENABLED='claude-code codex'\n"), "{t}");
        assert!(
            t.contains("ANTHROPIC_API_KEY='aurora-placeholder'\n"),
            "{t}"
        );
        assert!(!t.contains("ANTHROPIC_MODEL"), "{t}");
    }

    #[test]
    fn never_a_real_credential_only_the_placeholder() {
        for m in [
            None,
            Some(choice("anthropic", "anthropic_messages")),
            Some(choice("litellm", "anthropic_messages")),
            Some(choice("litellm", "openai_responses")),
        ] {
            let t = render(&cfg(), m.as_ref());
            for line in t.lines().filter(|l| {
                l.starts_with("ANTHROPIC_API_KEY")
                    || l.starts_with("ANTHROPIC_AUTH_TOKEN")
                    || l.starts_with("OPENAI_API_KEY")
            }) {
                assert!(line.ends_with("='aurora-placeholder'"), "{line}");
            }
            assert!(!t.contains("aub_") && !t.contains("aup_"), "{t}");
        }
    }

    #[test]
    fn a_non_anthropic_claude_endpoint_uses_its_base_url_and_a_quoted_model() {
        let t = render(&cfg(), Some(&choice("litellm", "anthropic_messages")));
        assert!(
            t.contains("ANTHROPIC_BASE_URL='http://llm.example/v1'\n"),
            "{t}"
        );
        assert!(
            t.contains("ANTHROPIC_AUTH_TOKEN='aurora-placeholder'\n"),
            "{t}"
        );
        assert!(t.contains("ANTHROPIC_MODEL='m'\\''1'\n"), "{t}");
        assert!(!t.contains("\nANTHROPIC_API_KEY"), "{t}");
    }

    #[test]
    fn a_responses_endpoint_hands_codex_its_model_and_base_url() {
        let t = render(&cfg(), Some(&choice("litellm", "openai_responses")));
        assert!(
            t.contains("AURORA_CODEX_BASE_URL='http://llm.example/v1'\n"),
            "{t}"
        );
        assert!(t.contains("AURORA_CODEX_MODEL="), "{t}");
    }

    #[test]
    fn state_directories_are_separate_per_harness_under_aurora_terminal() {
        let home = std::env::temp_dir().join(format!("aurora-harness-home-{}", std::process::id()));
        ensure_state_dirs(&home, 1000, 1000).unwrap();
        ensure_state_dirs(&home, 1000, 1000).unwrap();
        for d in ["claude", "codex"] {
            assert!(home.join(".aurora/terminal").join(d).is_dir());
        }
        // Never the agent harness's own `.claude` / `.codex` homes.
        assert!(!home.join(".claude").exists() && !home.join(".codex").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn written_world_readable_and_replaced_atomically() {
        let dir = std::env::temp_dir().join(format!("aurora-harness-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("harness.env");
        write(&p, "A='1'\n").unwrap();
        write(&p, "B='2'\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "B='2'\n");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(!dir.join("harness.env.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

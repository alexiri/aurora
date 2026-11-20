//! `/run/aurora/harness.env`: what the web terminal's `claude`, `codex`, `pi`, `goose` and
//! `opencode` wrappers need to reach the LLM **through the gateway** exactly as an agent turn
//! does (FIX-138 item 1b, P5-HARN C-1015, `docs/12 §Terminal`).
//!
//! The wrappers in `/opt/aurora/bin` **parse** the file as data (blank lines, `# …` and
//! `NAME='VALUE'` only, [`q`]'s escaping; any other line makes them refuse to start, FIX-155) — they
//! never source it. The directory is writable by agent code (same uid), so the file is not a
//! trust anchor: the terminal is the agent's own environment (`17` residual, "`/run/aurora` is the
//! agent's"). It holds **no secret**: the
//! machine proxy's address (`127.0.0.1:3128`, which adds the boot token itself; the terminal
//! never has it), the CA bundle path, the placeholder credential the gateway replaces, the
//! harnesses the instance enables and one section per harness with the agent's model and
//! endpoint (the same values the adapters put into a turn's environment) or, when the agent
//! has none for it, a fixed sentence the wrapper prints (C-750 (4)). The sections come from the
//! terminal open (`pty.open`) and from each turn's session start for the agent's own harness.
//! The wrappers add their own state directories; nothing here points at the agent harness's.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aurora_protocol::runner::{
    ModelChoice, TerminalModel, TerminalModelUnavailable, TerminalModels,
};

use crate::adapters::claude_code::config::PLACEHOLDER;

/// Where the file lives (tmpfs `/run/aurora`; agent-writable at one uid, so agent code can rewrite it — the
/// wrappers parse it as data and the terminal is the agent's environment, see `17`).
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

/// The file's fixed inputs plus the current per-harness state, shared by the PTY engine (which
/// replaces it from `pty.open`) and the supervisor (a turn's session start).
#[derive(Debug)]
pub struct HarnessEnv {
    cfg: HarnessEnvConfig,
    models: Mutex<TerminalModels>,
}

impl HarnessEnv {
    pub fn new(cfg: HarnessEnvConfig) -> Self {
        Self {
            cfg,
            models: Mutex::new(TerminalModels::default()),
        }
    }

    pub fn config(&self) -> &HarnessEnvConfig {
        &self.cfg
    }

    /// Writes the file for the current state (boot: no model, the CLIs' defaults).
    pub fn write_current(&self) -> std::io::Result<()> {
        let m = self.models.lock().unwrap_or_else(|e| e.into_inner());
        write(&self.cfg.path, &render(&self.cfg, &m))
    }

    /// `pty.open`: replaces the sections of the harnesses present in `models` and rewrites the
    /// file (the caller does this before the shell is spawned).
    pub fn apply_terminal(&self, models: &TerminalModels) -> std::io::Result<()> {
        let mut m = self.models.lock().unwrap_or_else(|e| e.into_inner());
        for h in TerminalModels::HARNESSES {
            if let Some(entry) = models.get(h) {
                m.set(h, Some(entry.clone()));
            }
        }
        write(&self.cfg.path, &render(&self.cfg, &m))
    }

    /// A turn's session start: replaces only the section of the agent's own harness.
    pub fn apply_turn(&self, harness: &str, choice: &ModelChoice) -> std::io::Result<()> {
        let entry = Some(TerminalModel {
            choice: Some(choice.clone()),
            unavailable: None,
        });
        let mut m = self.models.lock().unwrap_or_else(|e| e.into_inner());
        if !m.set(harness, entry) {
            return Ok(());
        }
        write(&self.cfg.path, &render(&self.cfg, &m))
    }
}

/// A single-quoted shell word; control characters are dropped.
fn q(v: &str) -> String {
    let clean: String = v.chars().filter(|c| !c.is_control()).collect();
    format!("'{}'", clean.replace('\'', "'\\''"))
}

/// Whether `k` can be a shell variable name (a `client_env` key is data, not trusted).
fn is_name(k: &str) -> bool {
    let mut c = k.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The fixed sentence a wrapper prints for a harness with no model (C-750 (4)).
pub fn unavailable_sentence(harness_name: &str, why: TerminalModelUnavailable) -> String {
    match why {
        TerminalModelUnavailable::NoKey => {
            format!("this agent has no usable LLM key for {harness_name}")
        }
        TerminalModelUnavailable::NoModel => {
            "no model is chosen for this agent; choose one on its Instructions page".to_owned()
        }
    }
}

/// The file's text: the fixed part plus one section per harness. A harness with no entry gets
/// no model variables (the CLI's own default and the gateway's governance, today's behaviour
/// before any turn).
pub fn render(cfg: &HarnessEnvConfig, models: &TerminalModels) -> String {
    let mut out = String::from(
        "# Written by aurora-supervisor. No secret: the gateway adds the agent's real LLM key.\n\
         # Parsed (never sourced) by the wrappers in /opt/aurora/bin (docs/12 §Terminal).\n",
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

    // claude-code: exactly what `claude_code::config::build_env` sets for a turn.
    match models.claude_code.as_ref() {
        // SEC-122: a choice whose values do not pass the supervisor's re-check never reaches the file.
        Some(TerminalModel {
            choice: Some(m), ..
        }) if crate::adapters::claude_code::config::check_model(m).is_err() => {
            set("ANTHROPIC_API_KEY", PLACEHOLDER);
        }
        Some(TerminalModel {
            choice: Some(m), ..
        }) => {
            for (k, v) in crate::adapters::claude_code::config::client_env(m) {
                if is_name(k) {
                    set(k, v);
                }
            }
            for (k, v) in crate::adapters::claude_code::config::model_env(m) {
                set(&k, &v);
            }
        }
        Some(TerminalModel {
            unavailable: Some(why),
            ..
        }) => set(
            "AURORA_CLAUDE_UNAVAILABLE",
            &unavailable_sentence("Claude Code", *why),
        ),
        _ => set("ANTHROPIC_API_KEY", PLACEHOLDER),
    }
    // codex reads the provider from its config, not the environment: the wrapper turns these
    // into `-c` overrides (`codex::config::render_config` has the same values).
    match models.codex.as_ref() {
        Some(TerminalModel {
            choice: Some(m), ..
        }) => {
            set("AURORA_CODEX_MODEL", &m.model);
            set(
                "AURORA_CODEX_BASE_URL",
                &crate::adapters::codex::config::openai_base_url(m),
            );
        }
        Some(TerminalModel {
            unavailable: Some(why),
            ..
        }) => set(
            "AURORA_CODEX_UNAVAILABLE",
            &unavailable_sentence("Codex", *why),
        ),
        _ => {}
    }
    // (phase 5, P5-HARN, C-1015) pi, goose and opencode: the wrappers configure the CLI from these
    // (the adapters' own renderers produce the values, so a terminal and a turn agree).
    for (id, display) in [("pi", "Pi"), ("goose", "goose"), ("opencode", "OpenCode")] {
        let var = |s: &str| format!("AURORA_{}_{s}", id.to_ascii_uppercase());
        match models.get(id) {
            Some(TerminalModel {
                choice: Some(m), ..
            }) => match terminal_values(id, m) {
                Ok(values) => {
                    for (k, v) in values {
                        set(&var(k), &v);
                    }
                }
                // SEC-122 (the id or role fails the re-check) or a dialect the harness cannot speak.
                Err(_) => set(
                    &var("UNAVAILABLE"),
                    &format!("this agent's model endpoint cannot be used with {display}"),
                ),
            },
            Some(TerminalModel {
                unavailable: Some(why),
                ..
            }) => set(&var("UNAVAILABLE"), &unavailable_sentence(display, *why)),
            _ => {}
        }
    }
    out
}

/// What the wrapper of `harness` needs, as `AURORA_<HARNESS>_<KEY>` values: the model and where
/// it is served, with the CLI's own configuration where it has no inline form.
fn terminal_values(
    harness: &str,
    m: &ModelChoice,
) -> Result<Vec<(&'static str, String)>, crate::harness::HarnessError> {
    use crate::adapters::{goose, opencode, pi};
    crate::adapters::claude_code::config::check_model(m)?;
    Ok(match harness {
        "pi" => vec![
            ("MODEL", m.model.clone()),
            ("MODELS_JSON", pi::config::terminal_models(m)?),
            ("KEY_VAR", pi::config::api_of(&m.dialect)?.1.to_owned()),
            // (FIX-190, D-404) The turn's settings (no project trust, no install telemetry, a
            // quiet start), so the terminal's Pi starts configured.
            ("SETTINGS_JSON", pi::config::terminal_settings()),
        ],
        "goose" => {
            let (provider, _) = goose::config::provider_of(&m.dialect)?;
            vec![
                ("MODEL", m.model.clone()),
                ("PROVIDER", provider.to_owned()),
                ("HOST", goose::config::terminal_host(m)),
            ]
        }
        _ => vec![
            ("MODEL", opencode::config::model_ref(&m.model)),
            ("CONFIG", opencode::config::terminal_config(m)?),
            (
                "KEY_VAR",
                opencode::config::provider_of(&m.dialect)?.1.to_owned(),
            ),
        ],
    })
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

/// Names of the per-harness state directories under `.aurora/terminal` (`/workspace/.aurora/terminal/<id>`).
pub const TERMINAL_STATE_DIRS: [&str; 5] = ["claude", "codex", "pi", "goose", "opencode"];

/// The wrappers' own state directories (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) under the home's
/// `.aurora/terminal`, owned by the agent user so the CLIs can write there although `.aurora`
/// itself belongs to the supervisor. A terminal harness never reads or changes the agent
/// harness's own state (FIX-095). Best effort: a failure is the caller's warning.
pub fn ensure_state_dirs(home: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    let base = home.join(".aurora/terminal");
    // One directory per terminal harness (`claude`, `codex`, `pi`, `goose`, `opencode`; C-1015).
    let dirs =
        std::iter::once(base.clone()).chain(TERMINAL_STATE_DIRS.iter().map(|d| base.join(d)));
    for d in dirs {
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
            model: "m.1".into(),
            effort: String::new(),
            client_env: vec![],
            roles: None,
            subagent_role: None,
            model_meta: None,
        }
    }

    fn claude(c: ModelChoice) -> TerminalModels {
        TerminalModels {
            claude_code: Some(TerminalModel {
                choice: Some(c),
                unavailable: None,
            }),
            ..Default::default()
        }
    }

    fn codex(c: ModelChoice) -> TerminalModels {
        TerminalModels {
            codex: Some(TerminalModel {
                choice: Some(c),
                unavailable: None,
            }),
            ..Default::default()
        }
    }

    fn unavailable(why: TerminalModelUnavailable) -> Option<TerminalModel> {
        Some(TerminalModel {
            choice: None,
            unavailable: Some(why),
        })
    }

    #[test]
    fn before_any_turn_it_has_the_proxy_the_ca_the_placeholder_and_no_model() {
        let t = render(&cfg(), &TerminalModels::default());
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
            TerminalModels::default(),
            claude(choice("anthropic", "anthropic_messages")),
            claude(choice("litellm", "anthropic_messages")),
            codex(choice("litellm", "openai_responses")),
        ] {
            let t = render(&cfg(), &m);
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

    /// (FIX-190, D-404) Pi's terminal section carries the turn's settings, rendered by the adapter.
    #[test]
    fn the_pi_section_carries_the_turns_settings() {
        let mut m = TerminalModels::default();
        m.set(
            "pi",
            Some(TerminalModel {
                choice: Some(choice("litellm", "anthropic_messages")),
                unavailable: None,
            }),
        );
        let t = render(&cfg(), &m);
        let want = format!(
            "AURORA_PI_SETTINGS_JSON={}\n",
            q(&crate::adapters::pi::config::terminal_settings())
        );
        assert!(t.contains(&want), "{t}");
    }

    #[test]
    fn a_non_anthropic_claude_endpoint_uses_its_base_url_and_a_quoted_model() {
        let t = render(&cfg(), &claude(choice("litellm", "anthropic_messages")));
        assert!(
            t.contains("ANTHROPIC_BASE_URL='http://llm.example/v1'\n"),
            "{t}"
        );
        assert!(
            t.contains("ANTHROPIC_AUTH_TOKEN='aurora-placeholder'\n"),
            "{t}"
        );
        assert!(t.contains("ANTHROPIC_MODEL='m.1'\n"), "{t}");
        assert!(!t.contains("\nANTHROPIC_API_KEY"), "{t}");
    }

    /// (C-881, SEC-122) A `harness.env` with roles is syntactically valid shell, and parsing it back as
    /// `NAME='VALUE'` data gives the values the turn's environment has. A choice that fails the supervisor's
    /// re-check writes no model variables at all.
    #[test]
    fn roles_are_written_single_quoted_and_source_back_to_the_same_values() {
        let mut m = choice("litellm", "anthropic_messages");
        m.roles = Some(aurora_protocol::runner::ModelRoleIds {
            large: Some("glm-4.6".into()),
            medium: None,
            small: Some("openai/glm-air:1@x".into()),
        });
        m.subagent_role = Some("small".into());
        m.model_meta = Some(vec![aurora_protocol::runner::ModelMeta {
            model: "m.1".into(),
            context_window: Some(131_072),
            capabilities: vec!["thinking".into()],
        }]);
        let t = render(&cfg(), &claude(m.clone()));
        for want in [
            "ANTHROPIC_DEFAULT_OPUS_MODEL='glm-4.6'\n",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL='openai/glm-air:1@x'\n",
            "CLAUDE_CODE_SUBAGENT_MODEL='haiku'\n",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS='131072'\n",
        ] {
            assert!(t.contains(want), "{want}\n{t}");
        }
        assert!(!t.contains("ANTHROPIC_DEFAULT_SONNET_MODEL"), "{t}");
        assert!(!t.contains("_FORCE"), "{t}");
        // `sh -n` accepts the file, and sourcing it yields the same values
        let dir = std::env::temp_dir().join(format!("aurora-henv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("harness.env");
        std::fs::write(&f, &t).unwrap();
        let ok = crate::spawn::command("sh", Vec::<(String, String)>::new())
            .arg("-n")
            .arg(&f)
            .status()
            .unwrap();
        assert!(ok.success());
        let out = crate::spawn::command("sh", Vec::<(String, String)>::new())
            .arg("-c")
            .arg(format!(
                ". {}; printf '%s|%s|%s' \"$ANTHROPIC_DEFAULT_OPUS_MODEL\" \"$ANTHROPIC_DEFAULT_HAIKU_MODEL\" \"$CLAUDE_CODE_SUBAGENT_MODEL\"",
                f.display()
            ))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "glm-4.6|openai/glm-air:1@x|haiku"
        );
        let _ = std::fs::remove_dir_all(&dir);
        // a violating id never reaches the file
        for bad in ["a'b", "$(id)", "a b", "a=b", "x\ny"] {
            let mut b = m.clone();
            b.roles.as_mut().unwrap().large = Some(bad.into());
            let t = render(&cfg(), &claude(b));
            assert!(
                !t.contains("ANTHROPIC_DEFAULT_") && !t.contains("ANTHROPIC_MODEL"),
                "{bad:?}\n{t}"
            );
        }
    }

    #[test]
    fn a_responses_endpoint_hands_codex_its_model_and_base_url() {
        let t = render(&cfg(), &codex(choice("litellm", "openai_responses")));
        assert!(
            t.contains("AURORA_CODEX_BASE_URL='http://llm.example/v1'\n"),
            "{t}"
        );
        assert!(t.contains("AURORA_CODEX_MODEL="), "{t}");
    }

    #[test]
    fn both_harnesses_render_both_sections() {
        let both = TerminalModels {
            claude_code: claude(choice("litellm", "anthropic_messages")).claude_code,
            codex: codex(choice("litellm", "openai_responses")).codex,
            ..Default::default()
        };
        let t = render(&cfg(), &both);
        assert!(t.contains("\nANTHROPIC_MODEL="), "{t}");
        assert!(t.contains("\nANTHROPIC_BASE_URL="), "{t}");
        assert!(t.contains("\nAURORA_CODEX_MODEL="), "{t}");
        assert!(!t.contains("UNAVAILABLE"), "{t}");
    }

    #[test]
    fn unavailable_renders_the_sentence_and_no_model_variables() {
        let t = render(
            &cfg(),
            &TerminalModels {
                claude_code: unavailable(TerminalModelUnavailable::NoKey),
                codex: unavailable(TerminalModelUnavailable::NoModel),
                ..Default::default()
            },
        );
        assert!(
            t.contains(
                "AURORA_CLAUDE_UNAVAILABLE='this agent has no usable LLM key for Claude Code'\n"
            ),
            "{t}"
        );
        assert!(
            t.contains("AURORA_CODEX_UNAVAILABLE='no model is chosen for this agent; choose one on its Instructions page'\n"),
            "{t}"
        );
        assert!(
            !t.contains("ANTHROPIC_MODEL") && !t.contains("AURORA_CODEX_MODEL"),
            "{t}"
        );
        assert!(!t.contains("ANTHROPIC_BASE_URL"), "{t}");
    }

    #[test]
    fn a_turn_replaces_only_its_own_section() {
        let dir = std::env::temp_dir().join(format!("aurora-harness-turn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = cfg();
        c.path = dir.join("harness.env");
        let env = HarnessEnv::new(c);
        env.apply_terminal(&TerminalModels {
            claude_code: unavailable(TerminalModelUnavailable::NoKey),
            codex: codex(choice("litellm", "openai_responses")).codex,
            ..Default::default()
        })
        .unwrap();
        let mut turn = choice("litellm", "anthropic_messages");
        turn.model = "turn-model".into();
        env.apply_turn("claude-code", &turn).unwrap();
        let t = std::fs::read_to_string(dir.join("harness.env")).unwrap();
        assert!(t.contains("ANTHROPIC_MODEL='turn-model'\n"), "{t}");
        assert!(!t.contains("AURORA_CLAUDE_UNAVAILABLE"), "{t}");
        assert!(t.contains("AURORA_CODEX_MODEL="), "codex kept: {t}");
        // A terminal open that only names codex keeps claude's section.
        env.apply_terminal(&TerminalModels {
            codex: unavailable(TerminalModelUnavailable::NoKey),
            ..Default::default()
        })
        .unwrap();
        let t = std::fs::read_to_string(dir.join("harness.env")).unwrap();
        assert!(t.contains("ANTHROPIC_MODEL='turn-model'\n"), "{t}");
        assert!(t.contains("AURORA_CODEX_UNAVAILABLE="), "{t}");
        assert!(!t.contains("AURORA_CODEX_MODEL"), "{t}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_claude_section_matches_what_a_turn_sets() {
        use crate::adapters::claude_code::config::build_env;
        use crate::harness::{McpBridgeSpec, SessionOptions};
        for provider in ["anthropic", "litellm"] {
            let mut m = choice(provider, "anthropic_messages");
            m.client_env = vec![
                ("LLM_EXTRA".into(), "1".into()),
                ("ANTHROPIC_API_KEY".into(), "sk-REAL".into()),
                ("bad name".into(), "x".into()),
            ];
            let turn = build_env(&SessionOptions {
                conversation_id: uuid::Uuid::nil(),
                working_dir: "/workspace".into(),
                resume_ref: None,
                model: m.clone(),
                proxy_url: "http://p".into(),
                mcp_bridge: McpBridgeSpec { command: vec![] },
                env: vec![],
            });
            let file = render(&cfg(), &claude(m));
            for k in [
                "ANTHROPIC_MODEL",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_AUTH_TOKEN",
                "LLM_EXTRA",
            ] {
                let in_turn = turn.iter().find(|(n, _)| n == k).map(|(_, v)| q(v));
                let in_file = file
                    .lines()
                    .find_map(|l| l.strip_prefix(&format!("{k}=")))
                    .map(str::to_owned);
                assert_eq!(in_turn, in_file, "{provider} {k}\n{file}");
            }
            assert!(
                !file.contains("sk-REAL") && !file.contains("bad name"),
                "{file}"
            );
        }
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

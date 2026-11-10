//! Pure construction of the Codex child's argv, environment and `config.toml`
//! (`docs/12-supervisor-harness.md` §Adapter codex).
//!
//! Nothing rendered here is a secret: the model credential is the placeholder
//! [`PLACEHOLDER`] and the gateway injects the real key; `auth.json` is never written.

use std::path::{Path, PathBuf};

use crate::harness::{HarnessError, McpBridgeSpec, SessionOptions};

pub use crate::adapters::claude_code::config::{PLACEHOLDER, render_turn};

/// The model provider block's table name in `config.toml`.
pub const PROVIDER: &str = "aurora";
/// Name of the MCP server the adapter configures.
pub const MCP_SERVER: &str = "aurora";

/// Everything needed to start one `codex exec` child.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Written to the child's stdin, which is then closed (the prompt is never an argument).
    pub stdin: String,
}

/// Version-pinned effort table (`10`: `low`/`medium`/`high`/`max`) for Codex 0.167:
/// `max` maps to the highest level the CLI accepts (`xhigh`). Unknown or empty = Codex's own default.
pub fn effort(effort: &str) -> Option<&'static str> {
    match effort {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("xhigh"),
        _ => None,
    }
}

/// A TOML basic string (JSON string escapes are valid TOML escapes).
fn s(v: &str) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into())
}

/// The wire API Codex speaks for an Aurora dialect (`openai_responses` only in 0.167: the CLI
/// refuses `wire_api = "chat"`).
pub fn wire_api(dialect: &str) -> Result<&'static str, HarnessError> {
    match dialect {
        "openai_responses" => Ok("responses"),
        other => Err(HarnessError::new(
            "harness_failed",
            format!("the codex harness does not speak the `{other}` dialect"),
        )),
    }
}

/// The endpoint base URL in OpenAI form: the provider's own `OPENAI_BASE_URL` (placeholder-only
/// client environment, e.g. LiteLLM's `<base>/v1`) when it supplied one, else `base_url`.
pub fn provider_base_url(opts: &SessionOptions) -> String {
    opts.model
        .client_env
        .iter()
        .find(|(k, _)| k == "OPENAI_BASE_URL")
        .map_or_else(
            || opts.model.base_url.clone(),
            |(_, v)| v.trim_end_matches('/').to_owned(),
        )
}

fn mcp_block(spec: &McpBridgeSpec) -> Result<String, HarnessError> {
    let (cmd, args) = spec
        .command
        .split_first()
        .ok_or_else(|| HarnessError::new("harness_failed", "no mcp-bridge command configured"))?;
    let args: Vec<String> = args.iter().map(|a| s(a)).collect();
    Ok(format!(
        "[mcp_servers.{MCP_SERVER}]\ncommand = {}\nargs = [{}]\nstartup_timeout_sec = 30\n",
        s(cmd),
        args.join(", ")
    ))
}

/// Features Aurora turns off: everything that reaches the network or a vendor service beyond
/// the model traffic, and everything that duplicates a platform capability (the Aurora sandbox
/// and gateway are the boundaries, `12 §Adapter codex`).
const FEATURES_OFF: [&str; 20] = [
    "apps",
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "code_mode_host",
    "computer_use",
    "daemon_auto_start",
    "goals",
    "image_generation",
    "in_app_browser",
    "multi_agent",
    "plugin_discovery_prefetch",
    "plugin_sharing",
    "plugins",
    "remote_plugin",
    "skill_mcp_dependency_install",
    "tool_suggest",
    "unbounded_connection_retries",
    "hooks",
    "shell_snapshot",
];

/// `CODEX_HOME/config.toml`, rendered for every session start (Aurora owns the file).
pub fn render_config(opts: &SessionOptions) -> Result<String, HarnessError> {
    let wire = wire_api(&opts.model.dialect)?;
    let mut t = String::from(
        "# Rendered by aurora-supervisor at every session start. Do not edit: it is overwritten.\n\
         # It holds no credential; the gateway adds the real model key.\n",
    );
    t.push_str(&format!("model = {}\n", s(&opts.model.model)));
    t.push_str(&format!("model_provider = {}\n", s(PROVIDER)));
    if let Some(e) = effort(&opts.model.effort) {
        t.push_str(&format!("model_reasoning_effort = {}\n", s(e)));
        t.push_str("model_reasoning_summary = \"auto\"\n");
    }
    // The Aurora sandbox and the gateway are the boundaries (docs/12): Codex's own sandbox
    // cannot nest in the container and its approval prompts have no one to answer them.
    t.push_str("approval_policy = \"never\"\n");
    t.push_str("sandbox_mode = \"danger-full-access\"\n");
    t.push_str("check_for_update_on_startup = false\n");
    t.push_str("web_search = \"disabled\"\n");
    t.push_str("forced_login_method = \"api\"\n");
    t.push_str("\n[analytics]\nenabled = false\n");
    t.push_str("\n[feedback]\nenabled = false\n");
    t.push_str("\n[features]\n");
    for f in FEATURES_OFF {
        t.push_str(&format!("{f} = false\n"));
    }
    t.push_str(&format!("\n[model_providers.{PROVIDER}]\n"));
    t.push_str("name = \"Aurora gateway\"\n");
    t.push_str(&format!("base_url = {}\n", s(&provider_base_url(opts))));
    t.push_str(&format!("wire_api = {}\n", s(wire)));
    t.push_str("env_key = \"OPENAI_API_KEY\"\n");
    t.push_str("requires_openai_auth = false\n");
    t.push_str("supports_websockets = false\n");
    t.push_str("request_max_retries = 2\nstream_max_retries = 2\n");
    t.push('\n');
    t.push_str(&mcp_block(&opts.mcp_bridge)?);
    Ok(t)
}

/// Environment variables that must never come from the caller: the provider's own client
/// environment (`OPENAI_BASE_URL` is rendered into the provider block only), Anthropic's, and
/// any Codex switch.
fn caller_owned(k: &str) -> bool {
    let u = k.to_ascii_uppercase();
    u.starts_with("OPENAI_")
        || u.starts_with("ANTHROPIC_")
        || u.starts_with("CODEX_")
        || matches!(u.as_str(), "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY")
}

/// Child environment: the caller's (supervisor + app) environment minus provider variables, the
/// per-session proxy, the placeholder credential, `CODEX_HOME` and the CA bundle. No secret.
pub fn build_env(opts: &SessionOptions, codex_home: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = opts
        .env
        .iter()
        .filter(|(k, _)| !caller_owned(k))
        .cloned()
        .collect();
    let ca = opts
        .env
        .iter()
        .find(|(k, _)| k == "SSL_CERT_FILE")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("SSL_CERT_FILE").ok());
    let mut set = |k: &str, v: &str| {
        env.retain(|(ek, _)| ek != k);
        env.push((k.to_owned(), v.to_owned()));
    };
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        set(k, &opts.proxy_url);
    }
    set("OPENAI_API_KEY", PLACEHOLDER);
    set("CODEX_HOME", &codex_home.display().to_string());
    if let Some(ca) = ca {
        set("CODEX_CA_CERTIFICATE", &ca);
        set("SSL_CERT_FILE", &ca);
    }
    env
}

/// Argv (without the program) for one turn; `resume` is the Codex thread id of the session.
/// The prompt arrives on stdin (`-`).
pub fn build_args(opts: &SessionOptions, resume: Option<&str>) -> Vec<String> {
    // `-C` is an option of `exec`, not of its `resume` subcommand: it goes first.
    let mut a: Vec<String> = vec!["exec".into(), "-C".into()];
    a.push(opts.working_dir.display().to_string());
    if let Some(r) = resume {
        a.extend(["resume".to_owned(), r.to_owned()]);
    }
    a.extend(
        ["--json", "--skip-git-repo-check", "-"]
            .iter()
            .map(|x| (*x).to_owned()),
    );
    a
}

pub fn launch_spec(
    opts: &SessionOptions,
    codex_home: &Path,
    resume: Option<&str>,
    prompt: &str,
) -> LaunchSpec {
    LaunchSpec {
        args: build_args(opts, resume),
        env: build_env(opts, codex_home),
        cwd: opts.working_dir.clone(),
        stdin: prompt.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use aurora_protocol::runner::ModelChoice;
    use uuid::Uuid;

    use super::*;

    pub(crate) fn opts(dialect: &str, effort: &str) -> SessionOptions {
        SessionOptions {
            conversation_id: Uuid::nil(),
            working_dir: "/workspace".into(),
            resume_ref: None,
            model: ModelChoice {
                provider: "litellm".into(),
                endpoint_id: "e1".into(),
                base_url: "https://llm.example".into(),
                dialect: dialect.into(),
                model: "gpt-5.1-codex".into(),
                effort: effort.into(),
                client_env: vec![
                    ("OPENAI_BASE_URL".into(), "https://llm.example/v1".into()),
                    ("OPENAI_API_KEY".into(), "sk-REAL-SECRET".into()),
                ],
            },
            proxy_url: "http://127.0.0.1:4100".into(),
            mcp_bridge: McpBridgeSpec {
                command: vec![
                    "/usr/local/bin/aurora-supervisor".into(),
                    "mcp-bridge".into(),
                    "--session".into(),
                    Uuid::nil().to_string(),
                ],
            },
            env: vec![
                ("TZ".into(), "UTC".into()),
                ("HTTPS_PROXY".into(), "http://evil".into()),
                ("OPENAI_BASE_URL".into(), "https://llm.example/v1".into()),
                ("OPENAI_API_KEY".into(), "sk-REAL-SECRET".into()),
                ("ANTHROPIC_API_KEY".into(), "sk-ant-SECRET".into()),
                ("CODEX_API_KEY".into(), "sk-codex-SECRET".into()),
                ("SSL_CERT_FILE".into(), "/run/aurora/ca.pem".into()),
            ],
        }
    }

    fn get<'a>(env: &'a [(String, String)], k: &str) -> Vec<&'a str> {
        env.iter()
            .filter(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[test]
    fn effort_table() {
        assert_eq!(effort("low"), Some("low"));
        assert_eq!(effort("medium"), Some("medium"));
        assert_eq!(effort("high"), Some("high"));
        assert_eq!(effort("max"), Some("xhigh"));
        assert_eq!(effort(""), None);
        assert_eq!(effort("ludicrous"), None);
    }

    #[test]
    fn only_the_responses_dialect_is_spoken() {
        assert_eq!(wire_api("openai_responses").unwrap(), "responses");
        assert!(wire_api("openai_chat").is_err());
        assert!(wire_api("anthropic_messages").is_err());
        assert!(render_config(&opts("openai_chat", "")).is_err());
    }

    #[test]
    fn env_has_placeholders_only_and_no_openai_base_url() {
        let env = build_env(
            &opts("openai_responses", "medium"),
            Path::new("/workspace/.codex"),
        );
        assert_eq!(get(&env, "OPENAI_API_KEY"), [PLACEHOLDER]);
        assert_eq!(get(&env, "HTTPS_PROXY"), ["http://127.0.0.1:4100"]);
        assert_eq!(get(&env, "HTTP_PROXY"), ["http://127.0.0.1:4100"]);
        assert_eq!(get(&env, "CODEX_HOME"), ["/workspace/.codex"]);
        assert_eq!(get(&env, "CODEX_CA_CERTIFICATE"), ["/run/aurora/ca.pem"]);
        assert_eq!(get(&env, "TZ"), ["UTC"]);
        assert!(get(&env, "OPENAI_BASE_URL").is_empty());
        assert!(get(&env, "ANTHROPIC_API_KEY").is_empty());
        assert!(get(&env, "CODEX_API_KEY").is_empty());
        assert!(
            !env.iter()
                .any(|(_, v)| v.contains("SECRET") || v.contains("evil"))
        );
    }

    #[test]
    fn args_exec_json_prompt_on_stdin_and_resume() {
        let o = opts("openai_responses", "");
        assert_eq!(
            build_args(&o, None),
            [
                "exec",
                "-C",
                "/workspace",
                "--json",
                "--skip-git-repo-check",
                "-"
            ]
        );
        let a = build_args(&o, Some("0199-thread"));
        // `-C` precedes `resume`: the subcommand does not accept it.
        assert_eq!(
            &a[..5],
            ["exec", "-C", "/workspace", "resume", "0199-thread"]
        );
        assert_eq!(a.last().map(String::as_str), Some("-"));
        // Never a flag that weakens or replaces the rendered configuration.
        assert!(
            !a.iter()
                .any(|x| x.starts_with("--dangerously") || x == "-c")
        );
    }

    #[test]
    fn mcp_bridge_is_required() {
        let mut o = opts("openai_responses", "");
        o.mcp_bridge = McpBridgeSpec::default();
        assert!(render_config(&o).is_err());
    }
}

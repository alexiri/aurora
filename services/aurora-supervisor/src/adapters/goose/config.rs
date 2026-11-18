//! Pure construction of the goose child's argv, environment and rendered files
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, SEC-133).
//!
//! goose keeps everything under one root, `GOOSE_PATH_ROOT`: `config/config.yaml` (rendered here),
//! `data/sessions/sessions.db` (its state) and `state/logs/`. The root is the conversation's own
//! directory (`<home>/.aurora/harness/goose/sessions/<ref>/`), so purging a session removes it whole.
//! Nothing rendered is a secret: the credential is the placeholder in an environment variable.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::harness::{HarnessError, McpBridgeSpec, SessionOptions};

pub use crate::adapters::claude_code::config::{PLACEHOLDER, render_turn};

/// Name of the MCP extension the adapter configures (`aurora__<tool>` in goose).
pub const MCP_SERVER: &str = "aurora";
/// Credential stores goose may write or an agent may plant: deleted before every start.
pub const CREDENTIAL_FILES: [&str; 1] = ["secrets.yaml"];

/// The dialects goose speaks (`12`): the Messages API through its `anthropic` provider and the
/// Chat Completions API through its `litellm` provider. The `openai` provider is **not** used: it
/// picks the Responses API by model name (`o*`, `gpt-5*`, `*codex*`), which a dialect cannot
/// promise, so `openai_responses` is not declared. Bedrock and Vertex sign with cloud credentials.
pub const DIALECTS: [&str; 2] = ["anthropic_messages", "openai_chat"];

/// Everything needed to start one `goose run` child.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Written to the child's stdin, which is then closed (the prompt is never an argument).
    pub stdin: String,
}

/// goose's provider and the variable that holds the placeholder key, for an Aurora dialect.
pub fn provider_of(dialect: &str) -> Result<(&'static str, &'static str), HarnessError> {
    match dialect {
        "anthropic_messages" => Ok(("anthropic", "ANTHROPIC_API_KEY")),
        "openai_chat" => Ok(("litellm", "LITELLM_API_KEY")),
        other => Err(HarnessError::new(
            "harness_failed",
            format!("the goose harness does not speak the `{other}` dialect"),
        )),
    }
}

/// Version-pinned effort table (`10`: `low`/`medium`/`high`/`max`) for `GOOSE_THINKING_EFFORT`
/// (*verified* in 1.59.2: thinking budgets 4000, 10000, 16000, 32000 tokens on a Claude model;
/// other values add no thinking). Unknown or empty = goose's own default.
pub fn effort(effort: &str) -> Option<&'static str> {
    match effort {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("max"),
        _ => None,
    }
}

fn strip_v1(url: &str) -> String {
    let b = url.trim_end_matches('/');
    b.strip_suffix("/v1").unwrap_or(b).to_owned()
}

/// The endpoint host in goose's form (no `/v1`: the provider appends its path).
pub fn host(opts: &SessionOptions) -> String {
    terminal_host(&opts.model)
}

/// [`host`] for a model choice (the web terminal's `goose` wrapper, C-1015).
pub fn terminal_host(m: &aurora_protocol::runner::ModelChoice) -> String {
    if m.dialect == "anthropic_messages" {
        strip_v1(&m.base_url)
    } else {
        strip_v1(&crate::adapters::codex::config::openai_base_url(m))
    }
}

/// The model of a role, if the choice resolved one.
fn role_model<'a>(m: &'a aurora_protocol::runner::ModelChoice, role: &str) -> Option<&'a str> {
    let r = m.roles.as_ref()?;
    match role {
        "large" => r.large.as_deref(),
        "medium" => r.medium.as_deref(),
        "small" => r.small.as_deref(),
        _ => None,
    }
}

/// The subagent's model: the model of the role `subagent_role` names (D-303); `None` = goose's
/// own rule (the subagent uses the main model).
pub fn subagent_model(opts: &SessionOptions) -> Option<&str> {
    let role = opts.model.subagent_role.as_deref()?;
    role_model(&opts.model, role)
}

fn context_window(opts: &SessionOptions, model: &str) -> Option<u32> {
    opts.model
        .model_meta
        .iter()
        .flatten()
        .find(|x| x.model == model)
        .and_then(|x| x.context_window)
}

fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `<root>/config/config.yaml`: the `developer` extension and the platform tools through the
/// bridge; the platform extensions that duplicate a platform feature (`apps`) or let the agent add
/// extensions at run time (`extensionmanager`) are off. Without `--no-profile` (which would ignore
/// this file) goose loads exactly these.
pub fn render_config(spec: &McpBridgeSpec) -> Result<String, HarnessError> {
    let (cmd, args) = spec
        .command
        .split_first()
        .ok_or_else(|| HarnessError::new("harness_failed", "no mcp-bridge command configured"))?;
    let args: Vec<String> = args.iter().map(|a| format!("      - {}", q(a))).collect();
    Ok(format!(
        "# Rendered by aurora-supervisor before every goose start. Do not edit: it is overwritten.\n\
         # It holds no credential; the gateway adds the real model key.\n\
         extensions:\n\
         \x20 developer:\n    enabled: true\n    type: builtin\n    name: developer\n    timeout: 300\n    bundled: true\n\
         \x20 apps:\n    enabled: false\n    type: platform\n    name: apps\n    description: off\n    bundled: true\n\
         \x20 extensionmanager:\n    enabled: false\n    type: platform\n    name: extensionmanager\n    description: off\n    bundled: true\n\
         \x20 {MCP_SERVER}:\n    enabled: true\n    type: stdio\n    name: {MCP_SERVER}\n    description: Aurora platform tools\n    cmd: {}\n    args:\n{}\n    envs: {{}}\n    timeout: 300\n",
        q(cmd),
        if args.is_empty() {
            "      []".to_owned()
        } else {
            args.join("\n")
        },
    ))
}

/// The model catalogue goose would fetch from `https://models.dev/api.json` at every start (it has
/// no switch for the fetch). goose skips the fetch when `data/model_catalog/models_dev_api.json`
/// and `.etag` exist and are fresh (*verified*: with both, a start makes no connection to
/// models.dev), so Aurora renders them: only the models of this session (the default and the
/// resolved roles), with their context window. The file is rendered again before every start, so
/// it is always fresh and a planted catalogue never survives.
pub fn render_catalog(opts: &SessionOptions) -> Result<(String, String), HarnessError> {
    let (provider, _) = provider_of(&opts.model.dialect)?;
    let mut ids: Vec<&str> = vec![&opts.model.model];
    for role in ["large", "medium", "small"] {
        if let Some(r) = role_model(&opts.model, role)
            && !ids.contains(&r)
        {
            ids.push(r);
        }
    }
    let mut models = serde_json::Map::new();
    for id in ids {
        let mut m = json!({"id": id, "name": id});
        if let Some(w) = context_window(opts, id) {
            m["limit"] = json!({"context": w, "output": w.min(32_000)});
        }
        models.insert(id.to_owned(), m);
    }
    let v = json!({provider: {"id": provider, "name": provider, "models": models}});
    // The etag file must end with a newline: without it goose treats the cache as stale and fetches
    // (*verified*: the same files with and without the newline).
    Ok((v.to_string(), "\"aurora\"\n".to_owned()))
}

/// The files of the root for a start: `(relative path, content)`.
pub fn render_files(opts: &SessionOptions) -> Result<Vec<(&'static str, String)>, HarnessError> {
    let (catalog, etag) = render_catalog(opts)?;
    Ok(vec![
        ("config/config.yaml", render_config(&opts.mcp_bridge)?),
        ("data/model_catalog/models_dev_api.json", catalog),
        ("data/model_catalog/models_dev_api.etag", etag),
    ])
}

/// Variables the caller never sets: goose's own, the providers' and the proxy.
fn caller_owned(k: &str) -> bool {
    let u = k.to_ascii_uppercase();
    u.starts_with("GOOSE_")
        || u.starts_with("OPENAI_")
        || u.starts_with("ANTHROPIC_")
        || u.starts_with("LITELLM_")
        || matches!(
            u.as_str(),
            "CONTEXT_FILE_NAMES" | "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY"
        )
}

/// The switches that keep goose off the vendor network and its own stores (*verified* for 1.59.2):
/// no telemetry, no keyring (its secret store), no session-naming request, approvals off (the Aurora
/// sandbox and the gateway are the boundaries), and only `AGENTS.md` as instructions.
pub const SWITCHES: [(&str, &str); 5] = [
    ("GOOSE_MODE", "auto"),
    ("GOOSE_TELEMETRY_ENABLED", "false"),
    ("GOOSE_DISABLE_KEYRING", "1"),
    ("GOOSE_DISABLE_SESSION_NAMING", "true"),
    ("CONTEXT_FILE_NAMES", "[\"AGENTS.md\"]"),
];

/// Child environment: the caller's environment minus provider variables, the per-session proxy,
/// the provider, model and endpoint, the placeholder credential, the switches, the CA bundle, the
/// context limit and the subagent model. No secret.
pub fn build_env(opts: &SessionOptions, root: &Path) -> Vec<(String, String)> {
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
    set("GOOSE_PATH_ROOT", &root.display().to_string());
    for (k, v) in SWITCHES {
        set(k, v);
    }
    if let Ok((provider, key_var)) = provider_of(&opts.model.dialect) {
        set("GOOSE_PROVIDER", provider);
        set(key_var, PLACEHOLDER);
        if provider == "anthropic" {
            set("ANTHROPIC_HOST", &host(opts));
        } else {
            set("LITELLM_HOST", &host(opts));
            set("LITELLM_BASE_PATH", "v1/chat/completions");
        }
        if let Some(sub) = subagent_model(opts) {
            set("GOOSE_SUBAGENT_PROVIDER", provider);
            set("GOOSE_SUBAGENT_MODEL", sub);
        }
    }
    set("GOOSE_MODEL", &opts.model.model);
    if let Some(w) = context_window(opts, &opts.model.model) {
        set("GOOSE_CONTEXT_LIMIT", &w.to_string());
    }
    if let Some(e) = effort(&opts.model.effort) {
        set("GOOSE_THINKING_EFFORT", e);
    }
    if let Some(ca) = ca {
        set("GOOSE_CA_CERT_PATH", &ca);
        set("SSL_CERT_FILE", &ca);
    }
    env
}

/// Argv (without the program) for one turn. `--provider`/`--model` are passed as well as set in
/// the environment: a resumed session keeps its stored model unless `--model` is given, so a model
/// change between turns needs the flag (*verified*). `resume` is true once the session exists:
/// `--resume` of a name that does not exist is an error. The prompt arrives on stdin (`-i -`).
pub fn build_args(opts: &SessionOptions, name: &str, resume: bool) -> Vec<String> {
    let provider = provider_of(&opts.model.dialect).map_or("anthropic", |p| p.0);
    let mut a: Vec<String> = ["run", "-i", "-", "--output-format", "stream-json", "--name"]
        .iter()
        .map(|x| (*x).to_owned())
        .collect();
    a.push(name.to_owned());
    if resume {
        a.push("--resume".into());
    }
    a.extend([
        "--provider".to_owned(),
        provider.to_owned(),
        "--model".to_owned(),
        opts.model.model.clone(),
    ]);
    a
}

pub fn launch_spec(
    opts: &SessionOptions,
    root: &Path,
    name: &str,
    resume: bool,
    prompt: &str,
) -> LaunchSpec {
    LaunchSpec {
        args: build_args(opts, name, resume),
        env: build_env(opts, root),
        cwd: opts.working_dir.clone(),
        stdin: prompt.to_owned(),
    }
}

/// The JSON value of the rendered catalogue (tests).
pub fn parse_catalog(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

#[cfg(test)]
pub(crate) mod tests {
    use aurora_protocol::runner::{ModelChoice, ModelMeta, ModelRoleIds};
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
                model: "claude-sonnet-4-5".into(),
                effort: effort.into(),
                client_env: vec![
                    ("OPENAI_BASE_URL".into(), "https://llm.example/v1".into()),
                    ("ANTHROPIC_API_KEY".into(), "sk-REAL-SECRET".into()),
                ],
                roles: Some(ModelRoleIds {
                    large: Some("big-model".into()),
                    medium: None,
                    small: Some("small-model".into()),
                }),
                subagent_role: Some("small".into()),
                model_meta: Some(vec![ModelMeta {
                    model: "claude-sonnet-4-5".into(),
                    context_window: Some(200_000),
                    capabilities: vec![],
                }]),
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
                ("PATH".into(), "/usr/bin".into()),
                ("HOME".into(), "/workspace".into()),
                ("SSL_CERT_FILE".into(), "/run/aurora/ca.pem".into()),
                ("OPENAI_API_KEY".into(), "sk-CALLER-SECRET".into()),
                ("GOOSE_PATH_ROOT".into(), "/evil".into()),
                ("GOOSE_MODE".into(), "smart_approve".into()),
                ("HTTPS_PROXY".into(), "http://evil".into()),
            ],
        }
    }

    fn get<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
        let all: Vec<_> = env.iter().filter(|(ek, _)| ek == k).collect();
        assert!(all.len() <= 1, "{k} set twice: {all:?}");
        all.first().map(|(_, v)| v.as_str())
    }

    #[test]
    fn environment_golden_for_the_anthropic_provider() {
        let env = build_env(&opts("anthropic_messages", "high"), Path::new("/r"));
        assert_eq!(get(&env, "GOOSE_PROVIDER"), Some("anthropic"));
        assert_eq!(get(&env, "ANTHROPIC_HOST"), Some("https://llm.example"));
        assert_eq!(get(&env, "ANTHROPIC_API_KEY"), Some(PLACEHOLDER));
        assert_eq!(get(&env, "LITELLM_HOST"), None);
        assert_eq!(get(&env, "GOOSE_MODEL"), Some("claude-sonnet-4-5"));
        assert_eq!(
            get(&env, "GOOSE_PATH_ROOT"),
            Some("/r"),
            "the caller's root is dropped"
        );
        assert_eq!(
            get(&env, "GOOSE_MODE"),
            Some("auto"),
            "the caller's mode is dropped"
        );
        assert_eq!(get(&env, "GOOSE_TELEMETRY_ENABLED"), Some("false"));
        assert_eq!(get(&env, "GOOSE_DISABLE_KEYRING"), Some("1"));
        assert_eq!(get(&env, "GOOSE_DISABLE_SESSION_NAMING"), Some("true"));
        assert_eq!(get(&env, "CONTEXT_FILE_NAMES"), Some("[\"AGENTS.md\"]"));
        assert_eq!(get(&env, "GOOSE_CONTEXT_LIMIT"), Some("200000"));
        assert_eq!(get(&env, "GOOSE_THINKING_EFFORT"), Some("high"));
        assert_eq!(get(&env, "GOOSE_CA_CERT_PATH"), Some("/run/aurora/ca.pem"));
        for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
            assert_eq!(get(&env, k), Some("http://127.0.0.1:4100"), "{k}");
        }
        assert_eq!(get(&env, "HOME"), Some("/workspace"));
        assert!(!format!("{env:?}").contains("SECRET"), "{env:?}");
        // The subagent's model is the model of the role the choice names.
        assert_eq!(get(&env, "GOOSE_SUBAGENT_PROVIDER"), Some("anthropic"));
        assert_eq!(get(&env, "GOOSE_SUBAGENT_MODEL"), Some("small-model"));
    }

    #[test]
    fn environment_golden_for_the_litellm_chat_provider() {
        let mut o = opts("openai_chat", "");
        o.model.subagent_role = None;
        let env = build_env(&o, Path::new("/r"));
        assert_eq!(get(&env, "GOOSE_PROVIDER"), Some("litellm"));
        assert_eq!(
            get(&env, "LITELLM_HOST"),
            Some("https://llm.example"),
            "no /v1: goose appends the path"
        );
        assert_eq!(get(&env, "LITELLM_BASE_PATH"), Some("v1/chat/completions"));
        assert_eq!(get(&env, "LITELLM_API_KEY"), Some(PLACEHOLDER));
        assert_eq!(
            get(&env, "OPENAI_API_KEY"),
            None,
            "the caller's key is dropped"
        );
        assert_eq!(get(&env, "ANTHROPIC_HOST"), None);
        assert_eq!(
            get(&env, "GOOSE_SUBAGENT_MODEL"),
            None,
            "no subagent role, no variable"
        );
        assert_eq!(get(&env, "GOOSE_THINKING_EFFORT"), None);
        assert!(!format!("{env:?}").contains("SECRET"));
    }

    #[test]
    fn dialects_goose_cannot_speak_are_refused_never_emulated() {
        for d in [
            "openai_responses",
            "anthropic_bedrock",
            "anthropic_vertex",
            "gemini",
            "",
        ] {
            assert!(provider_of(d).is_err(), "{d}");
            assert!(render_catalog(&opts(d, "")).is_err(), "{d}");
        }
    }

    #[test]
    fn config_yaml_golden_has_the_developer_extension_the_bridge_and_the_duplicates_off() {
        let y = render_config(&opts("anthropic_messages", "").mcp_bridge).unwrap();
        assert!(
            y.contains("developer:\n    enabled: true\n    type: builtin"),
            "{y}"
        );
        assert!(y.contains("apps:\n    enabled: false"), "{y}");
        assert!(y.contains("extensionmanager:\n    enabled: false"), "{y}");
        assert!(
            y.contains("cmd: \"/usr/local/bin/aurora-supervisor\""),
            "{y}"
        );
        assert!(y.contains("      - \"mcp-bridge\""), "{y}");
        assert!(y.contains("type: stdio"));
        assert!(!y.contains("secret") && !y.contains("sk-"), "{y}");
        assert!(render_config(&McpBridgeSpec::default()).is_err());
    }

    #[test]
    fn the_catalogue_is_seeded_for_the_session_models_with_their_context_window() {
        let (json, etag) = render_catalog(&opts("openai_chat", "")).unwrap();
        let v = parse_catalog(&json);
        let models = &v["litellm"]["models"];
        assert_eq!(models["claude-sonnet-4-5"]["limit"]["context"], 200_000);
        assert!(models.get("big-model").is_some() && models.get("small-model").is_some());
        assert!(models.get("medium-model").is_none());
        assert_eq!(etag, "\"aurora\"\n", "the newline matters to goose");
        let files: Vec<_> = render_files(&opts("anthropic_messages", ""))
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(
            files,
            [
                "config/config.yaml",
                "data/model_catalog/models_dev_api.json",
                "data/model_catalog/models_dev_api.etag"
            ]
        );
    }

    #[test]
    fn argv_golden_new_session_then_resume_with_the_model_always_passed() {
        let o = opts("anthropic_messages", "");
        assert_eq!(
            build_args(&o, "conv1", false),
            [
                "run",
                "-i",
                "-",
                "--output-format",
                "stream-json",
                "--name",
                "conv1",
                "--provider",
                "anthropic",
                "--model",
                "claude-sonnet-4-5"
            ]
        );
        let a = build_args(&opts("openai_chat", ""), "conv1", true);
        assert_eq!(&a[5..8], ["--name", "conv1", "--resume"]);
        assert_eq!(
            &a[8..],
            ["--provider", "litellm", "--model", "claude-sonnet-4-5"]
        );
        assert!(
            !a.contains(&"--no-profile".to_owned()),
            "--no-profile would ignore the rendered extensions"
        );
    }

    #[test]
    fn effort_table() {
        for (e, want) in [
            ("low", Some("low")),
            ("medium", Some("medium")),
            ("high", Some("high")),
            ("max", Some("max")),
            ("", None),
            ("x", None),
        ] {
            assert_eq!(effort(e), want, "{e}");
        }
    }
}

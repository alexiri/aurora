//! Pure construction of the Pi child's argv, environment and rendered files
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, SEC-133).
//!
//! Nothing rendered here is a secret: `models.json` refers to the **variable** that holds the
//! placeholder credential (`"apiKey": "$ANTHROPIC_API_KEY"`, Pi's environment interpolation), the
//! gateway injects the real key, and `auth.json` is removed before every start.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::harness::{HarnessError, McpBridgeSpec, SessionOptions, SteerInput};

pub use crate::adapters::claude_code::config::{PLACEHOLDER, render_turn};

/// The provider id in `models.json` and `--provider`.
pub const PROVIDER: &str = "aurora";
/// Name of the MCP server the adapter configures (`mcp__aurora__<tool>` in Pi).
pub const MCP_SERVER: &str = "aurora";
/// Credential stores Pi may write or an agent may plant: deleted before every start.
pub const CREDENTIAL_FILES: [&str; 1] = ["auth.json"];

/// The dialects Pi speaks, in preference order (`12`: Pi's `api` values `anthropic-messages`,
/// `openai-responses` and `openai-completions`; none of the cloud-signed dialects).
pub const DIALECTS: [&str; 3] = ["anthropic_messages", "openai_responses", "openai_chat"];

/// Everything needed to start one `pi --mode rpc` child.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// The conversation the child serves: the launcher registers the process for it (FIX-151).
    pub conversation_id: uuid::Uuid,
}

/// `models.json`'s `api` value and the placeholder variable for an Aurora dialect.
pub fn api_of(dialect: &str) -> Result<(&'static str, &'static str), HarnessError> {
    match dialect {
        "anthropic_messages" => Ok(("anthropic-messages", "ANTHROPIC_API_KEY")),
        "openai_responses" => Ok(("openai-responses", "OPENAI_API_KEY")),
        "openai_chat" => Ok(("openai-completions", "OPENAI_API_KEY")),
        other => Err(HarnessError::new(
            "harness_failed",
            format!("the pi harness does not speak the `{other}` dialect"),
        )),
    }
}

/// Version-pinned effort table (`10`: `low`/`medium`/`high`/`max`) for Pi 1.5.2's `--thinking`
/// (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`; a level the model cannot do is clamped
/// by Pi). Unknown or empty = Pi's own default.
pub fn effort(effort: &str) -> Option<&'static str> {
    match effort {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("max"),
        _ => None,
    }
}

/// The endpoint base URL in the form Pi's SDK for `dialect` wants: for `anthropic-messages` the host
/// **without** `/v1` (the SDK appends `/v1/messages`), for the OpenAI APIs the OpenAI form
/// (`…/v1`; the provider's own `OPENAI_BASE_URL` when it supplied one, as for `codex`).
pub fn base_url(opts: &SessionOptions) -> String {
    let m = &opts.model;
    if m.dialect == "anthropic_messages" {
        let b = m.base_url.trim_end_matches('/');
        b.strip_suffix("/v1").unwrap_or(b).to_owned()
    } else {
        crate::adapters::codex::config::openai_base_url(m)
    }
}

/// `<config>/models.json`: one provider, one model. `contextWindow` comes from `model_meta`
/// (D-303).
pub fn render_models(opts: &SessionOptions) -> Result<String, HarnessError> {
    let (api, key_var) = api_of(&opts.model.dialect)?;
    let mut model = json!({"id": opts.model.model, "name": opts.model.model});
    if let Some(w) = opts
        .model
        .model_meta
        .iter()
        .flatten()
        .find(|x| x.model == opts.model.model)
        .and_then(|x| x.context_window)
    {
        model["contextWindow"] = json!(w);
    }
    let v = json!({"providers": {PROVIDER: {
        "baseUrl": base_url(opts),
        "api": api,
        // Pi resolves `apiKey` as a literal, `$NAME` (environment interpolation) or `!command`:
        // the variable's reference, never its value, and never the bare name (which would be sent
        // as the key itself).
        "apiKey": format!("${key_var}"),
        "models": [model],
    }}});
    Ok(pretty(&v))
}

/// The web terminal's `pi` (C-1015): `models.json` for the agent's model, as one compact JSON
/// line that the wrapper writes into its own config directory (Pi has no inline config).
pub fn terminal_models(m: &aurora_protocol::runner::ModelChoice) -> Result<String, HarnessError> {
    let opts = SessionOptions {
        conversation_id: uuid::Uuid::nil(),
        working_dir: "/workspace".into(),
        resume_ref: None,
        model: m.clone(),
        proxy_url: String::new(),
        mcp_bridge: McpBridgeSpec::default(),
        env: vec![],
    };
    let v: Value = serde_json::from_str(&render_models(&opts)?).unwrap_or(Value::Null);
    Ok(v.to_string())
}

/// `<config>/mcp.json`: the platform tools through the bridge, **exposed directly** (Pi's default
/// `codemode` would hide every tool behind one tool and the on-demand index with it, D-297).
pub fn render_mcp(spec: &McpBridgeSpec) -> Result<String, HarnessError> {
    let (cmd, args) = spec
        .command
        .split_first()
        .ok_or_else(|| HarnessError::new("harness_failed", "no mcp-bridge command configured"))?;
    Ok(pretty(&json!({"mcpServers": {MCP_SERVER: {
        "command": cmd, "args": args, "env": {}, "exposure": "direct"
    }}})))
}

/// Retries of a refused model request: Pi's default (3 tries, 2 s, 4 s, 8 s) makes a refusal
/// arrive after about 14 s; two tries match `codex` (`request_max_retries = 2`) and arrive after
/// about 6 s.
pub const MAX_RETRIES: u32 = 2;

/// `<config>/settings.json`: the retry bound, no project trust, no install telemetry, a quiet start.
pub fn render_settings() -> String {
    pretty(&json!({
        "retry": {"enabled": true, "maxRetries": MAX_RETRIES, "baseDelayMs": 2000},
        "defaultProjectTrust": "never",
        "enableInstallTelemetry": false,
        "quietStartup": true,
    }))
}

/// (FIX-190, D-404) The web terminal's `settings.json`: a turn's [`render_settings`] plus
/// [`TERMINAL_ONLY_SETTINGS`], as one compact JSON line. The wrapper adds the keys a file in its
/// config directory lacks (Pi writes that file itself on its first start); a value already there is
/// kept.
pub fn terminal_settings() -> String {
    let mut v: Value = serde_json::from_str(&render_settings()).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        for (k, val) in TERMINAL_ONLY_SETTINGS {
            o.insert(k.to_owned(), val);
        }
    }
    v.to_string()
}

/// Settings only the interactive interface reads (an RPC turn never shows them): after an image
/// update that bumps Pi, one line instead of the full "What's New" changelog before the prompt.
pub const TERMINAL_ONLY_SETTINGS: [(&str, Value); 1] = [("collapseChangelog", Value::Bool(true))];

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    s
}

/// The files of `<config>` for a session start, rendered from `opts`.
pub fn render_files(opts: &SessionOptions) -> Result<Vec<(&'static str, String)>, HarnessError> {
    Ok(vec![
        ("models.json", render_models(opts)?),
        ("mcp.json", render_mcp(&opts.mcp_bridge)?),
        ("settings.json", render_settings()),
    ])
}

/// Variables the caller never sets: Pi's own switches, the providers' and the proxy.
fn caller_owned(k: &str) -> bool {
    let u = k.to_ascii_uppercase();
    u.starts_with("OPENAI_")
        || u.starts_with("ANTHROPIC_")
        || u.starts_with("PI_")
        || matches!(
            u.as_str(),
            "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY" | "NODE_OPTIONS" | "NODE_EXTRA_CA_CERTS"
        )
}

/// The switches that keep Pi off the vendor network (*verified* for 1.5.2: with these a turn makes
/// only the inference request): no update check, no telemetry, offline resources.
pub const VENDOR_OFF: [(&str, &str); 3] = [
    ("PI_OFFLINE", "1"),
    ("PI_TELEMETRY", "0"),
    ("PI_SKIP_VERSION_CHECK", "1"),
];

/// Child environment: the caller's (supervisor + app) environment minus provider variables, the
/// per-session proxy, the placeholder credential, `PI_CODING_AGENT_DIR`, the vendor-network switches
/// and the CA bundle. No secret.
pub fn build_env(opts: &SessionOptions, config_dir: &Path) -> Vec<(String, String)> {
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
    if let Ok((_, key_var)) = api_of(&opts.model.dialect) {
        set(key_var, PLACEHOLDER);
    }
    set("PI_CODING_AGENT_DIR", &config_dir.display().to_string());
    for (k, v) in VENDOR_OFF {
        set(k, v);
    }
    if let Some(ca) = ca {
        set("NODE_EXTRA_CA_CERTS", &ca);
        set("SSL_CERT_FILE", &ca);
    }
    env
}

/// Argv (without the program) for the long-lived RPC child of one session. `session_ref` is the Pi
/// session id (the conversation's), `sessions` its directory; `--no-approve` keeps project-local
/// resources (`.pi/`, project `.agents/skills`) from loading.
pub fn build_args(opts: &SessionOptions, sessions: &Path, session_ref: &str) -> Vec<String> {
    let mut a: Vec<String> = [
        "--mode",
        "rpc",
        "--provider",
        PROVIDER,
        "--model",
        &opts.model.model,
        "--session-dir",
        &sessions.display().to_string(),
        "--session-id",
        session_ref,
        "--no-approve",
    ]
    .iter()
    .map(|x| (*x).to_owned())
    .collect();
    if let Some(e) = effort(&opts.model.effort) {
        a.extend(["--thinking".to_owned(), e.to_owned()]);
    }
    a
}

pub fn launch_spec(
    opts: &SessionOptions,
    config_dir: &Path,
    sessions: &Path,
    session_ref: &str,
) -> LaunchSpec {
    LaunchSpec {
        args: build_args(opts, sessions, session_ref),
        env: build_env(opts, config_dir),
        cwd: opts.working_dir.clone(),
        conversation_id: opts.conversation_id,
    }
}

/// The RPC `prompt` command of a turn (sent while idle: the run starts).
pub fn prompt_line(text: &str) -> String {
    json!({"type": "prompt", "message": text}).to_string()
}

/// The RPC `prompt` command of a steer (delivered before the next model call of a live run).
pub fn steer_line(text: &str) -> String {
    json!({"type": "prompt", "message": text, "streamingBehavior": "steer"}).to_string()
}

/// The text of a steer as the model sees it (and as the join is matched on).
pub fn render_steer(s: &SteerInput) -> String {
    if s.speaker_label.is_empty() {
        s.text.clone()
    } else {
        format!("{}: {}", s.speaker_label, s.text)
    }
}

pub const ABORT_LINE: &str = r#"{"type":"abort"}"#;

#[cfg(test)]
pub(crate) mod tests {
    use aurora_protocol::runner::{ModelChoice, ModelMeta};
    use uuid::Uuid;

    use super::*;

    /// (P5-GUIDE-7) The session's own variables (`AURORA_SESSION`) reach the harness process.
    #[test]
    fn the_session_environment_reaches_the_harness_process() {
        let mut o = opts("anthropic_messages", "medium");
        o.env.push(("AURORA_SESSION".into(), "conv-1".into()));
        let env = build_env(&o, Path::new("/tmp/pi"));
        assert!(env.contains(&("AURORA_SESSION".to_owned(), "conv-1".to_owned())));
    }

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
                roles: None,
                subagent_role: None,
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
                ("PI_CODING_AGENT_DIR".into(), "/evil".into()),
                ("HTTPS_PROXY".into(), "http://evil".into()),
            ],
        }
    }

    #[test]
    fn models_json_golden_for_each_dialect_has_the_variable_name_and_no_secret() {
        for (dialect, api, key, base) in [
            (
                "anthropic_messages",
                "anthropic-messages",
                "ANTHROPIC_API_KEY",
                "https://llm.example",
            ),
            (
                "openai_responses",
                "openai-responses",
                "OPENAI_API_KEY",
                "https://llm.example/v1",
            ),
            (
                "openai_chat",
                "openai-completions",
                "OPENAI_API_KEY",
                "https://llm.example/v1",
            ),
        ] {
            let m = render_models(&opts(dialect, "")).unwrap();
            let v: Value = serde_json::from_str(&m).unwrap();
            let p = &v["providers"][PROVIDER];
            assert_eq!(p["api"], api);
            assert_eq!(
                p["apiKey"],
                format!("${key}"),
                "a `$NAME` reference, not a value"
            );
            assert_eq!(p["baseUrl"], base);
            assert_eq!(p["models"][0]["id"], "claude-sonnet-4-5");
            assert_eq!(p["models"][0]["contextWindow"], 200_000);
            assert!(
                !m.contains("sk-") && !m.contains("aurora-placeholder"),
                "{m}"
            );
        }
        // An Anthropic base that already ends in /v1 is not doubled.
        let mut o = opts("anthropic_messages", "");
        o.model.base_url = "https://llm.example/v1/".into();
        assert_eq!(base_url(&o), "https://llm.example");
    }

    #[test]
    fn a_dialect_pi_cannot_speak_is_refused_never_emulated() {
        for d in ["anthropic_bedrock", "anthropic_vertex", "gemini", ""] {
            let e = render_models(&opts(d, "")).unwrap_err();
            assert_eq!(e.code, "harness_failed", "{d}");
        }
    }

    #[test]
    fn mcp_json_exposes_the_tools_directly() {
        let v: Value =
            serde_json::from_str(&render_mcp(&opts("openai_chat", "").mcp_bridge).unwrap())
                .unwrap();
        let s = &v["mcpServers"][MCP_SERVER];
        assert_eq!(s["exposure"], "direct");
        assert_eq!(s["command"], "/usr/local/bin/aurora-supervisor");
        assert_eq!(s["args"][0], "mcp-bridge");
        assert!(render_mcp(&McpBridgeSpec::default()).is_err());
    }

    #[test]
    fn settings_bound_the_retries_and_turn_off_trust_and_telemetry() {
        let v: Value = serde_json::from_str(&render_settings()).unwrap();
        assert_eq!(v["retry"]["maxRetries"], MAX_RETRIES);
        assert_eq!(v["defaultProjectTrust"], "never");
        assert_eq!(v["enableInstallTelemetry"], false);
    }

    /// (FIX-190, D-404) A terminal and a turn agree: every key of the turn's settings with the turn's
    /// value, plus only the interface's own keys, on one line (harness.env holds one value per line),
    /// with nothing secret.
    #[test]
    fn terminal_settings_are_the_turns_settings_on_one_line() {
        let t = terminal_settings();
        assert!(!t.contains('\n'), "{t}");
        let term: Value = serde_json::from_str(&t).unwrap();
        let mut want: Value = serde_json::from_str(&render_settings()).unwrap();
        for (k, v) in TERMINAL_ONLY_SETTINGS {
            assert!(want.get(k).is_none(), "{k} is a turn setting too");
            want[k] = v;
        }
        assert_eq!(term, want);
        assert_eq!(term["collapseChangelog"], true);
        assert!(
            !t.contains("aurora-placeholder") && !t.contains("apiKey"),
            "{t}"
        );
    }

    #[test]
    fn environment_golden_placeholder_only_every_vendor_switch_off_caller_values_dropped() {
        let o = opts("anthropic_messages", "");
        let env = build_env(&o, Path::new("/workspace/.aurora/harness/pi/config"));
        let get = |k: &str| {
            let all: Vec<_> = env.iter().filter(|(ek, _)| ek == k).collect();
            assert!(all.len() <= 1, "{k} set twice: {all:?}");
            all.first().map(|(_, v)| v.as_str())
        };
        assert_eq!(get("ANTHROPIC_API_KEY"), Some(PLACEHOLDER));
        assert_eq!(
            get("OPENAI_API_KEY"),
            None,
            "caller's key dropped, none set for this dialect"
        );
        assert_eq!(
            get("PI_CODING_AGENT_DIR"),
            Some("/workspace/.aurora/harness/pi/config")
        );
        for (k, v) in VENDOR_OFF {
            assert_eq!(get(k), Some(v), "{k}");
        }
        for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
            assert_eq!(get(k), Some("http://127.0.0.1:4100"), "{k}");
        }
        assert_eq!(get("NODE_EXTRA_CA_CERTS"), Some("/run/aurora/ca.pem"));
        assert_eq!(get("HOME"), Some("/workspace"));
        assert!(!format!("{env:?}").contains("SECRET"), "{env:?}");
        // The OpenAI dialects set the OpenAI placeholder instead.
        let env = build_env(&opts("openai_chat", ""), Path::new("/c"));
        assert!(env.contains(&("OPENAI_API_KEY".to_owned(), PLACEHOLDER.to_owned())));
        assert!(!env.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn argv_golden_thinking_follows_the_effort_table_and_approval_is_never_asked() {
        let a = build_args(&opts("openai_chat", "max"), Path::new("/s"), "r1");
        assert_eq!(
            a,
            [
                "--mode",
                "rpc",
                "--provider",
                "aurora",
                "--model",
                "claude-sonnet-4-5",
                "--session-dir",
                "/s",
                "--session-id",
                "r1",
                "--no-approve",
                "--thinking",
                "max"
            ]
        );
        assert!(!a.iter().any(|x| x == "--approve"));
        for (e, want) in [
            ("low", Some("low")),
            ("medium", Some("medium")),
            ("high", Some("high")),
            ("max", Some("max")),
            ("", None),
            ("bogus", None),
        ] {
            assert_eq!(effort(e), want, "{e}");
        }
        assert!(
            !build_args(&opts("openai_chat", ""), Path::new("/s"), "r1")
                .contains(&"--thinking".to_owned())
        );
    }

    #[test]
    fn steer_text_is_the_speaker_and_the_text() {
        let s = SteerInput {
            followup_id: Uuid::nil(),
            text: "more".into(),
            speaker_label: "Ann".into(),
        };
        assert_eq!(render_steer(&s), "Ann: more");
        assert!(steer_line("x").contains("\"streamingBehavior\":\"steer\""));
        assert!(!prompt_line("x").contains("streamingBehavior"));
    }
}

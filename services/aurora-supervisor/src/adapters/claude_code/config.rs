//! Pure construction of the Claude Code child's argv, environment, MCP config and turn message.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::harness::{HarnessError, McpBridgeSpec, SessionOptions, TurnInput};

/// Placeholder credential; the real credential is added by the gateway/supervisor proxy.
pub const PLACEHOLDER: &str = "aurora-placeholder";

/// Everything needed to start the child.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// Version-pinned effort table (`10`: `low`/`medium`/`high`/`max`): `(CLI --effort value,
/// extra env)`. `low` also disables extended thinking. Unknown or empty efforts are dropped.
pub fn effort_flags(effort: &str) -> Option<(&'static str, Vec<(String, String)>)> {
    match effort {
        "low" => Some(("low", vec![("MAX_THINKING_TOKENS".into(), "0".into())])),
        "medium" => Some(("medium", vec![])),
        "high" => Some(("high", vec![])),
        "max" => Some(("max", vec![])),
        _ => None,
    }
}

/// Inline `--mcp-config` JSON: one stdio server `aurora`.
pub fn mcp_config(spec: &McpBridgeSpec) -> Result<Value, HarnessError> {
    let (cmd, args) = spec
        .command
        .split_first()
        .ok_or_else(|| HarnessError::new("harness_failed", "no mcp-bridge command configured"))?;
    Ok(json!({ "mcpServers": { super::stream::MCP_SERVER: {
        "type": "stdio", "command": cmd, "args": args, "env": {}
    } } }))
}

/// Child environment: caller env (minus credential-shaped keys), proxy, provider placeholders,
/// model, and disabled auto-update/telemetry/non-essential traffic. Never carries secrets.
pub fn build_env(opts: &SessionOptions) -> Vec<(String, String)> {
    const OWN: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "HTTPS_PROXY",
        "HTTP_PROXY",
    ];
    let mut env: Vec<(String, String)> = opts
        .env
        .iter()
        .chain(opts.model.client_env.iter())
        .filter(|(k, _)| !OWN.contains(&k.as_str()))
        .cloned()
        .collect();
    let mut set = |k: &str, v: &str| env.push((k.to_owned(), v.to_owned()));
    set("HTTPS_PROXY", &opts.proxy_url);
    set("HTTP_PROXY", &opts.proxy_url);
    if opts.model.provider == "anthropic" {
        set("ANTHROPIC_API_KEY", PLACEHOLDER);
    } else {
        set("ANTHROPIC_BASE_URL", &opts.model.base_url);
        set("ANTHROPIC_AUTH_TOKEN", PLACEHOLDER);
    }
    set("ANTHROPIC_MODEL", &opts.model.model);
    set("DISABLE_AUTOUPDATER", "1");
    set("DISABLE_TELEMETRY", "1");
    set("DISABLE_ERROR_REPORTING", "1");
    set("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    if let Some((_, extra)) = effort_flags(&opts.model.effort) {
        env.extend(extra);
    }
    env
}

/// Argv (without the program) for one live child; `resume` is the harness session id.
pub fn build_args(
    opts: &SessionOptions,
    resume: Option<&str>,
) -> Result<Vec<String>, HarnessError> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-mode",
        "bypassPermissions",
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .map(str::to_owned)
    .into();
    a.push(mcp_config(&opts.mcp_bridge)?.to_string());
    if let Some((level, _)) = effort_flags(&opts.model.effort) {
        a.extend(["--effort".to_owned(), level.to_owned()]);
    }
    if let Some(r) = resume {
        a.extend(["--resume".to_owned(), r.to_owned()]);
    }
    Ok(a)
}

pub fn launch_spec(
    opts: &SessionOptions,
    resume: Option<&str>,
) -> Result<LaunchSpec, HarnessError> {
    Ok(LaunchSpec {
        args: build_args(opts, resume)?,
        env: build_env(opts),
        cwd: opts.working_dir.clone(),
    })
}

/// Text of the user message for a turn: notices, recall (delivery-only), attachments, speaker, text.
pub fn render_turn(input: &TurnInput) -> String {
    let mut s = String::new();
    for n in &input.notices {
        s.push_str(&format!("[notice] {n}\n"));
    }
    if !input.recall.index.is_empty() || !input.recall.snippets.is_empty() {
        s.push_str("<recall>\n");
        if !input.recall.index.is_empty() {
            s.push_str(&input.recall.index);
            s.push('\n');
        }
        for sn in &input.recall.snippets {
            s.push_str(&format!("- {sn}\n"));
        }
        s.push_str("</recall>\n");
    }
    if !input.attachment_paths.is_empty() {
        s.push_str("Attachments:\n");
        for p in &input.attachment_paths {
            s.push_str(&format!("- {}\n", p.display()));
        }
    }
    if input.speaker_label.is_empty() {
        s.push_str(&input.text);
    } else {
        s.push_str(&format!(
            "{} ({}): {}",
            input.speaker_label, input.door, input.text
        ));
    }
    s
}

/// One stdin line (stream-json input) carrying the user message.
pub fn user_message_line(text: &str) -> String {
    json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })
        .to_string()
}

#[cfg(test)]
mod tests {
    use aurora_protocol::runner::{ModelChoice, Recall};
    use uuid::Uuid;

    use super::*;

    fn opts(provider: &str, effort: &str) -> SessionOptions {
        SessionOptions {
            conversation_id: Uuid::nil(),
            working_dir: "/workspace".into(),
            resume_ref: None,
            model: ModelChoice {
                provider: provider.into(),
                endpoint_id: "e1".into(),
                base_url: "https://llm.example/v1".into(),
                dialect: "anthropic_messages".into(),
                model: "claude-sonnet-4-5".into(),
                effort: effort.into(),
                client_env: vec![
                    ("ANTHROPIC_API_KEY".into(), "sk-REAL-SECRET".into()),
                    ("LLM_EXTRA".into(), "1".into()),
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
    fn litellm_env_has_placeholders_and_no_secrets() {
        let env = build_env(&opts("litellm", "medium"));
        assert_eq!(get(&env, "ANTHROPIC_BASE_URL"), ["https://llm.example/v1"]);
        assert_eq!(get(&env, "ANTHROPIC_AUTH_TOKEN"), [PLACEHOLDER]);
        assert!(get(&env, "ANTHROPIC_API_KEY").is_empty());
        assert_eq!(get(&env, "HTTPS_PROXY"), ["http://127.0.0.1:4100"]);
        assert_eq!(get(&env, "ANTHROPIC_MODEL"), ["claude-sonnet-4-5"]);
        assert_eq!(get(&env, "DISABLE_AUTOUPDATER"), ["1"]);
        assert_eq!(get(&env, "DISABLE_TELEMETRY"), ["1"]);
        assert_eq!(get(&env, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"), ["1"]);
        assert_eq!(get(&env, "TZ"), ["UTC"]);
        assert_eq!(get(&env, "LLM_EXTRA"), ["1"]);
        assert!(
            !env.iter()
                .any(|(_, v)| v.contains("SECRET") || v.contains("evil"))
        );
    }

    #[test]
    fn anthropic_env_uses_api_key_placeholder_only() {
        let env = build_env(&opts("anthropic", "low"));
        assert_eq!(get(&env, "ANTHROPIC_API_KEY"), [PLACEHOLDER]);
        assert!(get(&env, "ANTHROPIC_AUTH_TOKEN").is_empty());
        assert!(get(&env, "ANTHROPIC_BASE_URL").is_empty());
        assert_eq!(get(&env, "MAX_THINKING_TOKENS"), ["0"]);
    }

    #[test]
    fn effort_table() {
        assert_eq!(effort_flags("low").unwrap().0, "low");
        assert_eq!(effort_flags("max").unwrap().0, "max");
        assert!(effort_flags("").is_none());
        assert!(effort_flags("ludicrous").is_none());
        assert!(effort_flags("medium").unwrap().1.is_empty());
    }

    #[test]
    fn mcp_config_is_single_stdio_aurora_server() {
        let c = mcp_config(&opts("litellm", "").mcp_bridge).unwrap();
        let servers = c["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        let s = &servers["aurora"];
        assert_eq!(s["type"], "stdio");
        assert_eq!(s["command"], "/usr/local/bin/aurora-supervisor");
        assert_eq!(
            s["args"],
            json!(["mcp-bridge", "--session", Uuid::nil().to_string()])
        );
        assert!(mcp_config(&McpBridgeSpec::default()).is_err());
    }

    #[test]
    fn args_headless_no_prompts_resume_and_effort() {
        let o = opts("litellm", "high");
        let a = build_args(&o, Some("sess-1")).unwrap();
        let has = |x: &str| a.iter().any(|v| v == x);
        assert!(has("-p") && has("--include-partial-messages") && has("--strict-mcp-config"));
        let pos = |x: &str| a.iter().position(|v| v == x).unwrap();
        assert_eq!(a[pos("--permission-mode") + 1], "bypassPermissions");
        assert_eq!(a[pos("--input-format") + 1], "stream-json");
        assert_eq!(a[pos("--output-format") + 1], "stream-json");
        assert_eq!(a[pos("--resume") + 1], "sess-1");
        assert_eq!(a[pos("--effort") + 1], "high");
        let fresh = build_args(&opts("litellm", ""), None).unwrap();
        assert!(!fresh.iter().any(|v| v == "--resume" || v == "--effort"));
        assert_eq!(
            launch_spec(&o, None).unwrap().cwd,
            PathBuf::from("/workspace")
        );
    }

    #[test]
    fn turn_rendering_and_stdin_line() {
        let t = TurnInput {
            turn_id: Uuid::nil(),
            text: "hi".into(),
            speaker_label: "Ann".into(),
            door: "web".into(),
            recall: Recall {
                index: "idx".into(),
                snippets: vec!["s1".into()],
            },
            attachment_paths: vec!["/workspace/.aurora/inbox/t/a.txt".into()],
            notices: vec!["n1".into()],
        };
        let r = render_turn(&t);
        assert!(r.starts_with("[notice] n1\n<recall>\nidx\n- s1\n</recall>\n"));
        assert!(r.ends_with("Ann (web): hi"));
        assert!(r.contains("/workspace/.aurora/inbox/t/a.txt"));
        let line: Value = serde_json::from_str(&user_message_line("x\ny")).unwrap();
        assert_eq!(line["message"]["content"][0]["text"], "x\ny");
    }
}

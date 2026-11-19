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
    /// The conversation the child serves: the launcher registers the process for it (FIX-151).
    pub conversation_id: uuid::Uuid,
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

/// Variables the adapter owns: a caller or client environment never sets them.
const OWN_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "HTTPS_PROXY",
    "HTTP_PROXY",
];

/// (C-877, pinned with the CLI version) role → the CLI's alias and its per-alias variables.
const ROLE_ALIASES: [(&str, &str, &str, &str); 3] = [
    (
        "large",
        "opus",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
    ),
    (
        "medium",
        "sonnet",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
    ),
    (
        "small",
        "haiku",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
    ),
];

/// (SEC-122) The supervisor re-checks every model value of a `ModelChoice` before it enters a process
/// environment or `harness.env`: ids against `MODEL_ID_PATTERN`, roles and capabilities against their closed
/// vocabularies, the context window as an integer in range. A violation refuses the turn (`harness_failed`).
pub fn check_model(m: &aurora_protocol::runner::ModelChoice) -> Result<(), HarnessError> {
    use aurora_core::model_id::{
        CONTEXT_WINDOW_MAX, CONTEXT_WINDOW_MIN, MODEL_CAPABILITIES, MODEL_ROLES, validate_model_id,
    };
    let bad = |what: &str| {
        Err(HarnessError::new(
            "harness_failed",
            format!("the model choice is refused: {what} is not valid"),
        ))
    };
    if !validate_model_id(&m.model) {
        return bad("the model id");
    }
    if let Some(r) = &m.roles
        && r.set().any(|(_, id)| !validate_model_id(id))
    {
        return bad("a role's model id");
    }
    if m.subagent_role
        .as_deref()
        .is_some_and(|r| !MODEL_ROLES.contains(&r))
    {
        return bad("the subagent role");
    }
    for meta in m.model_meta.iter().flatten() {
        if !validate_model_id(&meta.model) {
            return bad("a model_meta id");
        }
        if meta
            .context_window
            .is_some_and(|w| !(CONTEXT_WINDOW_MIN..=CONTEXT_WINDOW_MAX).contains(&w))
        {
            return bad("a context window");
        }
        if meta
            .capabilities
            .iter()
            .any(|c| !MODEL_CAPABILITIES.contains(&c.as_str()))
        {
            return bad("a capability");
        }
    }
    Ok(())
}

/// The dialects Claude Code speaks (`12 §Adapter claude-code`, D-260(a)): the Messages API, and the
/// Bedrock and Vertex surfaces it reaches through its own "use provider" switches. The cloud ones
/// carry **no credential** in the sandbox: the CLI is told to skip auth and the gateway signs or
/// mints. (P4-LLM-5; the variable names below are pinned for the Claude Code version in the sandbox
/// image's lockfile and re-checked by the live suites.)
pub const DIALECTS: &[&str] = &[
    "anthropic_messages",
    "anthropic_bedrock",
    "anthropic_vertex",
];

/// The turn is refused (`harness_failed`) for a dialect Claude Code cannot speak.
pub fn check_dialect(dialect: &str) -> Result<(), HarnessError> {
    if DIALECTS.contains(&dialect) {
        Ok(())
    } else {
        Err(HarnessError::new(
            "harness_failed",
            format!("the claude-code harness does not speak the `{dialect}` dialect"),
        ))
    }
}

/// A variable that can carry (or point at) a cloud credential: never passed to the child, from a
/// caller or from `client_env`, on any dialect (`12 §Adapter claude-code`: no AWS key variables, no
/// Bedrock API key, no profile, no application-default credentials file). The region survives.
fn is_credential_env(k: &str) -> bool {
    (k.starts_with("AWS_") && !matches!(k, "AWS_REGION" | "AWS_DEFAULT_REGION"))
        || matches!(
            k,
            "GOOGLE_APPLICATION_CREDENTIALS"
                | "GOOGLE_API_KEY"
                | "GOOGLE_CLOUD_ACCESS_TOKEN"
                | "CLOUDSDK_AUTH_ACCESS_TOKEN"
                | "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE"
                | "OPENAI_API_KEY"
                | "AZURE_OPENAI_API_KEY"
        )
}

fn passes(k: &str) -> bool {
    !OWN_ENV.contains(&k) && !is_credential_env(k)
}

/// The provider placeholders and model a Claude Code child gets for `m` (shared by a turn and
/// the terminal's `harness.env`, so the two cannot drift). Never a real credential. The cloud
/// dialects set only the model: their switches and endpoint come from `client_env` (C-809).
pub fn model_env(m: &aurora_protocol::runner::ModelChoice) -> Vec<(String, String)> {
    let mut env = Vec::with_capacity(3);
    let mut set = |k: &str, v: &str| env.push((k.to_owned(), v.to_owned()));
    match (m.dialect.as_str(), m.provider.as_str()) {
        ("anthropic_bedrock" | "anthropic_vertex", _) => {}
        (_, "anthropic") => set("ANTHROPIC_API_KEY", PLACEHOLDER),
        _ => {
            set("ANTHROPIC_BASE_URL", &m.base_url);
            set("ANTHROPIC_AUTH_TOKEN", PLACEHOLDER);
        }
    }
    set("ANTHROPIC_MODEL", &m.model);
    // (C-877) Roles: an unset role sets nothing. Never `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` (a subagent
    // definition's own `model:` still wins) and never anything under `.claude/agents/` (D-302(d)).
    let meta_of = |model: &str| m.model_meta.iter().flatten().find(|x| x.model == model);
    if let Some(roles) = &m.roles {
        for (role, model) in roles.set() {
            let Some((_, _, var, caps_var)) = ROLE_ALIASES.iter().find(|a| a.0 == role) else {
                continue;
            };
            set(var, model);
            if let Some(meta) = meta_of(model).filter(|x| !x.capabilities.is_empty()) {
                set(caps_var, &meta.capabilities.join(","));
            }
        }
    }
    if let Some(alias) = m
        .subagent_role
        .as_deref()
        .and_then(|r| ROLE_ALIASES.iter().find(|a| a.0 == r))
        .map(|a| a.1)
    {
        set("CLAUDE_CODE_SUBAGENT_MODEL", alias);
    }
    if let Some(w) = meta_of(&m.model).and_then(|x| x.context_window) {
        set("CLAUDE_CODE_MAX_CONTEXT_TOKENS", &w.to_string());
    }
    env
}

/// The choice's `client_env` pairs a Claude Code child receives (those the adapter owns and
/// anything credential-shaped are dropped), as in [`build_env`].
pub fn client_env(
    m: &aurora_protocol::runner::ModelChoice,
) -> impl Iterator<Item = &(String, String)> {
    m.client_env.iter().filter(|(k, _)| passes(k))
}

/// Child environment: caller env (minus credential-shaped keys), proxy, provider placeholders,
/// model, and disabled auto-update/telemetry/non-essential traffic. Never carries secrets.
pub fn build_env(opts: &SessionOptions) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = opts
        .env
        .iter()
        .chain(opts.model.client_env.iter())
        .filter(|(k, _)| passes(k))
        .cloned()
        .collect();
    let mut set = |k: &str, v: &str| env.push((k.to_owned(), v.to_owned()));
    set("HTTPS_PROXY", &opts.proxy_url);
    set("HTTP_PROXY", &opts.proxy_url);
    env.extend(model_env(&opts.model));
    let mut set = |k: &str, v: &str| env.push((k.to_owned(), v.to_owned()));
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
    check_dialect(&opts.model.dialect)?;
    check_model(&opts.model)?;
    Ok(LaunchSpec {
        args: build_args(opts, resume)?,
        env: build_env(opts),
        cwd: opts.working_dir.clone(),
        conversation_id: opts.conversation_id,
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

    /// (P5-GUIDE-7) The session's own variables (`AURORA_SESSION`) reach the harness process.
    #[test]
    fn the_session_environment_reaches_the_harness_process() {
        let mut o = opts("anthropic", "medium");
        o.env.push(("AURORA_SESSION".into(), "conv-1".into()));
        let env = build_env(&o);
        assert!(env.contains(&("AURORA_SESSION".to_owned(), "conv-1".to_owned())));
    }

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
                roles: None,
                subagent_role: None,
                model_meta: None,
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

    /// The C-809 environment of the cloud dialects with credential variables smuggled in by the endpoint's
    /// `client_env` or by the caller.
    fn cloud_opts(provider: &str, dialect: &str, switches: &[(&str, &str)]) -> SessionOptions {
        let mut o = opts(provider, "high");
        o.model.dialect = dialect.into();
        o.model.client_env = switches
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .chain([
                ("AWS_SECRET_ACCESS_KEY".into(), "REAL-aws-secret".into()),
                ("AWS_ACCESS_KEY_ID".into(), "AKIAREAL".into()),
                ("AWS_BEARER_TOKEN_BEDROCK".into(), "REAL-bearer".into()),
                ("AWS_PROFILE".into(), "REAL-profile".into()),
                (
                    "GOOGLE_APPLICATION_CREDENTIALS".into(),
                    "/REAL/key.json".into(),
                ),
                ("ANTHROPIC_API_KEY".into(), "REAL-anthropic".into()),
            ])
            .collect();
        o.env
            .push(("AWS_SESSION_TOKEN".into(), "REAL-session".into()));
        o
    }

    fn assert_no_credential(env: &[(String, String)]) {
        for (k, v) in env {
            assert!(
                !v.contains("REAL") && !v.contains("AKIA"),
                "{k}={v} is credential-shaped"
            );
            assert!(
                !k.starts_with("AWS_") || k == "AWS_REGION",
                "{k} must not reach the child"
            );
        }
        for k in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
        ] {
            assert!(get(env, k).is_empty(), "{k} is not set on a cloud dialect");
        }
    }

    /// (P4-LLM-5, C-809, D-261) Bedrock: the "use Bedrock, skip auth" switches, the endpoint and the model, and nothing
    /// that could be a credential, whatever the auth mode (the mode never reaches the adapter) and whatever the
    /// endpoint's `client_env` or the caller smuggles in.
    #[test]
    fn bedrock_env_is_switches_only_and_credential_variables_are_dropped() {
        let o = cloud_opts(
            "bedrock",
            "anthropic_bedrock",
            &[
                ("CLAUDE_CODE_USE_BEDROCK", "1"),
                ("AWS_REGION", "us-east-1"),
                (
                    "ANTHROPIC_BEDROCK_BASE_URL",
                    "https://bedrock-runtime.us-east-1.amazonaws.com",
                ),
                ("CLAUDE_CODE_SKIP_BEDROCK_AUTH", "1"),
            ],
        );
        let env = build_env(&o);
        assert_no_credential(&env);
        assert_eq!(get(&env, "CLAUDE_CODE_USE_BEDROCK"), ["1"]);
        assert_eq!(get(&env, "CLAUDE_CODE_SKIP_BEDROCK_AUTH"), ["1"]);
        assert_eq!(get(&env, "AWS_REGION"), ["us-east-1"]);
        assert_eq!(
            get(&env, "ANTHROPIC_BEDROCK_BASE_URL"),
            ["https://bedrock-runtime.us-east-1.amazonaws.com"]
        );
        assert_eq!(get(&env, "ANTHROPIC_MODEL"), ["claude-sonnet-4-5"]);
        assert_eq!(get(&env, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"), ["1"]);
        assert_eq!(get(&env, "HTTPS_PROXY"), ["http://127.0.0.1:4100"]);
        // The terminal's `harness.env` takes the same two helpers.
        let t: Vec<_> = client_env(&o.model)
            .cloned()
            .chain(model_env(&o.model))
            .collect();
        assert_no_credential(&t);
        assert_eq!(get(&t, "CLAUDE_CODE_SKIP_BEDROCK_AUTH"), ["1"]);
    }

    #[test]
    fn vertex_env_is_switches_only_and_credential_variables_are_dropped() {
        let o = cloud_opts(
            "vertex",
            "anthropic_vertex",
            &[
                ("CLAUDE_CODE_USE_VERTEX", "1"),
                ("CLOUD_ML_REGION", "us-east5"),
                ("ANTHROPIC_VERTEX_PROJECT_ID", "proj-1"),
                (
                    "ANTHROPIC_VERTEX_BASE_URL",
                    "https://us-east5-aiplatform.googleapis.com/v1",
                ),
                ("CLAUDE_CODE_SKIP_VERTEX_AUTH", "1"),
            ],
        );
        let env = build_env(&o);
        assert_no_credential(&env);
        assert!(get(&env, "GOOGLE_APPLICATION_CREDENTIALS").is_empty());
        assert_eq!(get(&env, "CLAUDE_CODE_USE_VERTEX"), ["1"]);
        assert_eq!(get(&env, "CLAUDE_CODE_SKIP_VERTEX_AUTH"), ["1"]);
        assert_eq!(get(&env, "CLOUD_ML_REGION"), ["us-east5"]);
        assert_eq!(get(&env, "ANTHROPIC_VERTEX_PROJECT_ID"), ["proj-1"]);
        assert_eq!(get(&env, "ANTHROPIC_MODEL"), ["claude-sonnet-4-5"]);
    }

    /// (D-260(a)) A dialect Claude Code does not speak refuses the turn instead of guessing an environment.
    #[test]
    fn an_unknown_dialect_refuses_the_turn() {
        for d in ["openai_responses", "openai_chat", "", "anthropic_converse"] {
            let mut o = opts("litellm", "low");
            o.model.dialect = d.into();
            let e = launch_spec(&o, None).unwrap_err();
            assert_eq!(e.code, "harness_failed", "{d}");
        }
        for d in DIALECTS {
            let mut o = opts("litellm", "low");
            o.model.dialect = (*d).into();
            assert!(launch_spec(&o, None).is_ok(), "{d}");
        }
    }

    fn with_roles(
        roles: Option<(&str, &str, &str)>,
        subagent: Option<&str>,
        meta: Vec<aurora_protocol::runner::ModelMeta>,
    ) -> SessionOptions {
        let mut o = opts("litellm", "medium");
        o.model.roles = roles.map(|(l, m, s)| aurora_protocol::runner::ModelRoleIds {
            large: (!l.is_empty()).then(|| l.to_owned()),
            medium: (!m.is_empty()).then(|| m.to_owned()),
            small: (!s.is_empty()).then(|| s.to_owned()),
        });
        o.model.subagent_role = subagent.map(str::to_owned);
        o.model.model_meta = (!meta.is_empty()).then_some(meta);
        o
    }

    fn one(env: &[(String, String)], k: &str) -> Option<String> {
        let v = get(env, k);
        assert!(v.len() <= 1, "{k} set twice: {v:?}");
        v.first().map(|s| (*s).to_owned())
    }

    /// (C-877) Golden environments: no roles, all roles, a subagent default, capabilities and a context window.
    #[test]
    fn model_roles_map_to_the_cli_variables() {
        let role_vars = [
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
        ];
        // no roles: none of the variables
        let env = build_env(&with_roles(None, None, vec![]));
        for k in role_vars {
            assert_eq!(one(&env, k), None, "{k}");
        }
        // all roles + a subagent default
        let env = build_env(&with_roles(
            Some(("glm-big", "glm-mid", "glm-air")),
            Some("small"),
            vec![],
        ));
        assert_eq!(
            one(&env, "ANTHROPIC_DEFAULT_OPUS_MODEL").as_deref(),
            Some("glm-big")
        );
        assert_eq!(
            one(&env, "ANTHROPIC_DEFAULT_SONNET_MODEL").as_deref(),
            Some("glm-mid")
        );
        assert_eq!(
            one(&env, "ANTHROPIC_DEFAULT_HAIKU_MODEL").as_deref(),
            Some("glm-air")
        );
        assert_eq!(
            one(&env, "CLAUDE_CODE_SUBAGENT_MODEL").as_deref(),
            Some("haiku")
        );
        assert_eq!(
            one(&env, "ANTHROPIC_MODEL").as_deref(),
            Some("claude-sonnet-4-5")
        );
        // an unset role sets nothing; `_FORCE` is never set
        let env = build_env(&with_roles(
            Some(("", "", "glm-air")),
            Some("large"),
            vec![],
        ));
        assert_eq!(one(&env, "ANTHROPIC_DEFAULT_OPUS_MODEL"), None);
        assert_eq!(
            one(&env, "CLAUDE_CODE_SUBAGENT_MODEL").as_deref(),
            Some("opus")
        );
        assert_eq!(one(&env, "CLAUDE_CODE_SUBAGENT_MODEL_FORCE"), None);
        // capabilities of a role's model, and the default model's context window
        let meta = vec![
            aurora_protocol::runner::ModelMeta {
                model: "glm-air".into(),
                context_window: Some(128_000),
                capabilities: vec!["thinking".into(), "effort".into()],
            },
            aurora_protocol::runner::ModelMeta {
                model: "claude-sonnet-4-5".into(),
                context_window: Some(200_000),
                capabilities: vec!["effort".into()],
            },
        ];
        let env = build_env(&with_roles(Some(("", "", "glm-air")), None, meta));
        assert_eq!(
            one(&env, "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES").as_deref(),
            Some("thinking,effort")
        );
        assert_eq!(
            one(&env, "CLAUDE_CODE_MAX_CONTEXT_TOKENS").as_deref(),
            Some("200000")
        );
        // a caller or client environment can never set the adapter's own variables
        let mut o = with_roles(None, None, vec![]);
        o.env
            .push(("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), "evil".into()));
        o.env
            .push(("CLAUDE_CODE_SUBAGENT_MODEL_FORCE".into(), "evil".into()));
        o.model
            .client_env
            .push(("CLAUDE_CODE_SUBAGENT_MODEL".into(), "evil".into()));
        let env = build_env(&o);
        for k in role_vars {
            assert_eq!(one(&env, k), None, "{k}");
        }
    }

    /// (SEC-122) The supervisor refuses a choice whose values break the pattern, vocabularies or ranges.
    #[test]
    fn a_choice_with_a_bad_model_value_refuses_the_turn() {
        let bad_ids = [
            "a'b",
            "$(id)",
            "a b",
            "a\nb",
            "a=b",
            "x`y`",
            "sk-ant-api03-xyz",
            "",
        ];
        for id in bad_ids
            .iter()
            .map(|s| (*s).to_owned())
            .chain(["a".repeat(201)])
        {
            let mut o = with_roles(None, None, vec![]);
            o.model.model = id.clone();
            let e = launch_spec(&o, None).unwrap_err();
            assert_eq!(e.code, "harness_failed", "{id:?}");
            if !id.is_empty() {
                let mut o = with_roles(Some(("", &id, "")), None, vec![]);
                o.model.model = "ok".into();
                assert_eq!(
                    launch_spec(&o, None).unwrap_err().code,
                    "harness_failed",
                    "{id:?}"
                );
            }
        }
        let mut o = with_roles(None, Some("huge"), vec![]);
        assert!(launch_spec(&o, None).is_err());
        o = with_roles(
            None,
            None,
            vec![aurora_protocol::runner::ModelMeta {
                model: "m".into(),
                context_window: Some(5),
                capabilities: vec![],
            }],
        );
        assert!(launch_spec(&o, None).is_err());
        o = with_roles(
            None,
            None,
            vec![aurora_protocol::runner::ModelMeta {
                model: "m".into(),
                context_window: None,
                capabilities: vec!["telepathy".into()],
            }],
        );
        assert!(launch_spec(&o, None).is_err());
        assert!(
            launch_spec(
                &with_roles(Some(("a", "b", "c")), Some("large"), vec![]),
                None
            )
            .is_ok()
        );
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

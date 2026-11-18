//! Pure construction of the OpenCode child's argv, environment and rendered files
//! (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, SEC-133).
//!
//! OpenCode keeps its files under the XDG directories. All four point into the conversation's own
//! directory (`<home>/.aurora/harness/opencode/sessions/<ref>/`): `config/` (rendered here and
//! wiped before every start), `data/` (the session database), `cache/` and `state/`. Purging a
//! session removes the directory whole. Nothing rendered is a secret: the credential is the
//! placeholder in an environment variable, referenced as `{env:NAME}`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::harness::{HarnessError, SessionOptions};

pub use crate::adapters::claude_code::config::{PLACEHOLDER, render_turn};

/// The provider id in the rendered `opencode.json` (`<provider>/<model>` on the command line).
pub const PROVIDER: &str = "aurora";
/// Name of the MCP server the adapter configures (`aurora_<tool>` in OpenCode).
pub const MCP_SERVER: &str = "aurora";
/// Credential stores OpenCode may write or an agent may plant, relative to the root: deleted before
/// every start (the directories on the way are checked first, see `render_root`).
pub const CREDENTIAL_FILES: [&str; 2] = ["data/opencode/auth.json", "data/opencode/mcp-auth.json"];
/// The state directories of the root, each checked one by one before a start (a link planted at
/// `data` must not redirect the credential removal below it). `config/` is wiped and rendered.
pub const STATE_DIRS: [&str; 4] = ["data", "data/opencode", "cache", "state"];

/// The dialects OpenCode speaks (`12`): its three bundled provider packages.
pub const DIALECTS: [&str; 3] = ["anthropic_messages", "openai_responses", "openai_chat"];

/// What the pinned CLI needs for a dialect: the bundled provider package and the variable that holds
/// the placeholder key (`@ai-sdk/anthropic` posts `/v1/messages`, `@ai-sdk/openai` `/v1/responses`,
/// `@ai-sdk/openai-compatible` `/v1/chat/completions`; all are inside the binary, so nothing is
/// fetched from npm).
pub fn provider_of(dialect: &str) -> Result<(&'static str, &'static str), HarnessError> {
    match dialect {
        "anthropic_messages" => Ok(("@ai-sdk/anthropic", "ANTHROPIC_API_KEY")),
        "openai_responses" => Ok(("@ai-sdk/openai", "OPENAI_API_KEY")),
        "openai_chat" => Ok(("@ai-sdk/openai-compatible", "OPENAI_API_KEY")),
        other => Err(HarnessError::new(
            "harness_failed",
            format!("the opencode harness does not speak the `{other}` dialect"),
        )),
    }
}

/// Reasoning options of a variant (`--variant low|medium|high|max`), by provider package. Version
/// pinned: Anthropic gets a thinking budget (the table goose uses), the OpenAI packages a
/// `reasoningEffort` (`max` is the highest level the API accepts for every model: `high`). A variant
/// the config does not define is silently ignored by the CLI, so all four are always defined.
fn variant_options(npm: &str, level: &str) -> Value {
    if npm == "@ai-sdk/anthropic" {
        let budget = match level {
            "low" => 4_000,
            "medium" => 10_000,
            "high" => 16_000,
            _ => 32_000,
        };
        json!({"thinking": {"type": "enabled", "budgetTokens": budget}})
    } else {
        let e = if level == "max" { "high" } else { level };
        json!({"reasoningEffort": e})
    }
}

/// Version-pinned effort table (`10`: `low`/`medium`/`high`/`max`) for `--variant`. Unknown or
/// empty = OpenCode's own default (no variant).
pub fn effort(effort: &str) -> Option<&'static str> {
    match effort {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("max"),
        _ => None,
    }
}

/// The endpoint base URL in the form the provider package wants: `@ai-sdk/anthropic` takes the host
/// **with** `/v1` (it appends `/messages`), the OpenAI packages the OpenAI form (`…/v1`; the
/// provider's own `OPENAI_BASE_URL` when it supplied one, as for `codex`).
pub fn base_url(opts: &SessionOptions) -> String {
    let m = &opts.model;
    if m.dialect == "anthropic_messages" {
        let b = m.base_url.trim_end_matches('/');
        let b = b.strip_suffix("/v1").unwrap_or(b);
        format!("{b}/v1")
    } else {
        crate::adapters::codex::config::openai_base_url(m)
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

/// The subagent's model: the model of the role `subagent_role` names (D-303); `None` = OpenCode's
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

/// `<provider>/<model>` as OpenCode's `-m`, `model`, `small_model` and `agent.*.model` take it.
pub fn model_ref(model: &str) -> String {
    format!("{PROVIDER}/{model}")
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    s
}

/// `<root>/config/opencode.json` (v1 schema, *verified* against 1.19.6): no auto-update, no share
/// links, approvals off, no title request, the platform tools through the bridge, the provider for
/// the dialect with every model of the session (the default and the resolved roles), the small model
/// and the subagent model.
pub fn render_config(opts: &SessionOptions) -> Result<String, HarnessError> {
    let (cmd, args) =
        opts.mcp_bridge.command.split_first().ok_or_else(|| {
            HarnessError::new("harness_failed", "no mcp-bridge command configured")
        })?;
    let mut command = vec![json!(cmd)];
    command.extend(args.iter().map(|a| json!(a)));
    let mut v = config_value(opts)?;
    v["mcp"] = json!({MCP_SERVER: {
        "type": "local", "command": command, "enabled": true, "environment": {},
    }});
    Ok(pretty(&v))
}

/// The web terminal's `opencode` (C-1015): the same provider, models and switches without the
/// platform tools, as one compact JSON line the wrapper passes in `OPENCODE_CONFIG_CONTENT`.
pub fn terminal_config(m: &aurora_protocol::runner::ModelChoice) -> Result<String, HarnessError> {
    let opts = SessionOptions {
        conversation_id: uuid::Uuid::nil(),
        working_dir: "/workspace".into(),
        resume_ref: None,
        model: m.clone(),
        proxy_url: String::new(),
        mcp_bridge: crate::harness::McpBridgeSpec::default(),
        env: vec![],
    };
    Ok(config_value(&opts)?.to_string())
}

/// Everything of `opencode.json` but the MCP server.
fn config_value(opts: &SessionOptions) -> Result<Value, HarnessError> {
    let (npm, key_var) = provider_of(&opts.model.dialect)?;
    let m = &opts.model;
    let mut ids: Vec<&str> = vec![&m.model];
    for role in ["large", "medium", "small"] {
        if let Some(r) = role_model(m, role)
            && !ids.contains(&r)
        {
            ids.push(r);
        }
    }
    let variants: Value = ["low", "medium", "high", "max"]
        .iter()
        .map(|l| ((*l).to_owned(), variant_options(npm, l)))
        .collect::<serde_json::Map<_, _>>()
        .into();
    let mut models = serde_json::Map::new();
    for id in ids {
        let mut e = json!({"name": id, "variants": variants});
        if let Some(w) = context_window(opts, id) {
            e["limit"] = json!({"context": w, "output": w.min(32_000)});
        }
        models.insert(id.to_owned(), e);
    }
    let small = role_model(m, "small").unwrap_or(&m.model);
    let mut agent = json!({"title": {"disable": true}});
    if let Some(sub) = subagent_model(opts) {
        agent["general"] = json!({"model": model_ref(sub)});
    }
    Ok(json!({
        "$schema": "https://opencode.ai/config.json",
        "autoupdate": false,
        "share": "disabled",
        "permission": {"*": "allow"},
        "model": model_ref(&m.model),
        "small_model": model_ref(small),
        "agent": agent,
        // `OPENCODE_DISABLE_PROJECT_CONFIG` also stops the project's `AGENTS.md` discovery, so it
        // is named here.
        "instructions": [opts.working_dir.join("AGENTS.md").display().to_string()],
        "provider": {PROVIDER: {
            "npm": npm,
            "name": "Aurora",
            "options": {"baseURL": base_url(opts), "apiKey": format!("{{env:{key_var}}}")},
            "models": models,
        }},
    }))
}

/// The files that make OpenCode's background `npm install @opencode-ai/plugin` return at once
/// (*verified* for 1.19.6: with a `package.json`, a `package-lock.json` listing the dependency and
/// the package in `node_modules`, a start makes no request to the npm registry). They are rendered
/// before every start into the global configuration directory; no mode bit is involved, so the
/// result does not depend on the uid model. The agent can delete them; the next start renders
/// them again, and the session proxy refuses the registry meanwhile (the live suite asserts that no
/// request reaches it).
pub fn plugin_files() -> Vec<(&'static str, String)> {
    let v = super::PINNED_VERSION;
    vec![
        (
            "config/opencode/package.json",
            pretty(&json!({"dependencies": {"@opencode-ai/plugin": v}})),
        ),
        (
            "config/opencode/package-lock.json",
            pretty(&json!({
                "name": "opencode", "lockfileVersion": 3, "requires": true,
                "packages": {"": {"dependencies": {"@opencode-ai/plugin": v}}},
            })),
        ),
        (
            "config/opencode/node_modules/@opencode-ai/plugin/package.json",
            pretty(&json!({"name": "@opencode-ai/plugin", "version": v})),
        ),
    ]
}

/// The files of the root for a start: `(relative path, content)`.
pub fn render_files(opts: &SessionOptions) -> Result<Vec<(&'static str, String)>, HarnessError> {
    let mut files = vec![("config/opencode.json", render_config(opts)?)];
    files.extend(plugin_files());
    Ok(files)
}

/// Everything needed to start one `opencode` child.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Written to the child's stdin, which is then closed (the prompt is never an argument:
    /// `MAX_ARG_STRLEN` is 128 KiB, far below a turn's input).
    pub stdin: String,
}

/// Variables the caller never sets: OpenCode's own, the XDG directories, the providers' and the proxy.
fn caller_owned(k: &str) -> bool {
    let u = k.to_ascii_uppercase();
    u.starts_with("OPENCODE_")
        || u.starts_with("XDG_")
        || u.starts_with("OPENAI_")
        || u.starts_with("ANTHROPIC_")
        || matches!(
            u.as_str(),
            "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY" | "NODE_EXTRA_CA_CERTS"
        )
}

/// The switches that keep OpenCode off the vendor network and out of the workspace's own
/// configuration (*verified* for 1.19.6: with these, the rendered files and the proxy refusing
/// everything, a turn makes only the inference request): no model catalogue (models.opencode.ai),
/// no default plugins, no LSP download, no auto-update, no project `opencode.json` or `.opencode`
/// (a workspace file cannot replace the rendered configuration), no `CLAUDE.md` or `.claude` skills.
pub const SWITCHES: [(&str, &str); 6] = [
    ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
    ("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1"),
    ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1"),
    ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
    ("OPENCODE_DISABLE_PROJECT_CONFIG", "1"),
    ("OPENCODE_DISABLE_CLAUDE_CODE", "1"),
];

/// Child environment: the caller's environment minus provider and OpenCode variables, the per-session
/// proxy, the XDG directories of the root, the rendered config, the placeholder credential, the
/// switches and the CA bundle. No secret.
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
    let dir = |d: &str| root.join(d).display().to_string();
    set("XDG_CONFIG_HOME", &dir("config"));
    set("XDG_DATA_HOME", &dir("data"));
    set("XDG_CACHE_HOME", &dir("cache"));
    set("XDG_STATE_HOME", &dir("state"));
    set("OPENCODE_CONFIG", &dir("config/opencode.json"));
    for (k, v) in SWITCHES {
        set(k, v);
    }
    if let Ok((_, key_var)) = provider_of(&opts.model.dialect) {
        set(key_var, PLACEHOLDER);
    }
    if let Some(ca) = ca {
        set("SSL_CERT_FILE", &ca);
        set("NODE_EXTRA_CA_CERTS", &ca);
    }
    env
}

/// Argv (without the program) for one turn: `run --format json -m <provider>/<model>`, the session
/// once it exists and the effort variant. There is **no message argument**: the prompt arrives on
/// stdin (*verified* up to 400 KB, delivered whole).
pub fn build_args(opts: &SessionOptions, session: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = ["run", "--format", "json", "-m"]
        .iter()
        .map(|x| (*x).to_owned())
        .collect();
    a.push(model_ref(&opts.model.model));
    if let Some(s) = session {
        a.push("--session".into());
        a.push(s.to_owned());
    }
    if let Some(e) = effort(&opts.model.effort) {
        a.push("--variant".into());
        a.push(e.to_owned());
    }
    a
}

pub fn launch_spec(
    opts: &SessionOptions,
    root: &Path,
    session: Option<&str>,
    prompt: &str,
) -> LaunchSpec {
    LaunchSpec {
        args: build_args(opts, session),
        env: build_env(opts, root),
        cwd: opts.working_dir.clone(),
        stdin: prompt.to_owned(),
    }
}

/// True for an OpenCode session id (`ses_` and letters and digits): the only form that reaches
/// argv or SQL.
pub fn is_session_id(s: &str) -> bool {
    s.strip_prefix("ses_").is_some_and(|r| {
        !r.is_empty() && r.len() <= 64 && r.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// The query for the turn's per-model usage: the assistant messages created since `since_ms` in the
/// session and in every session below it (subagents), grouped by model.
pub fn usage_sql(session: &str, since_ms: u128) -> String {
    format!(
        "with recursive t(id) as (select '{session}' union select s.id from session s join t on s.parent_id = t.id) \
         select json_extract(data,'$.modelID') as model, \
         sum(coalesce(json_extract(data,'$.tokens.input'),0)) as input, \
         sum(coalesce(json_extract(data,'$.tokens.output'),0)) as output, \
         sum(coalesce(json_extract(data,'$.tokens.cache.read'),0)) as cache_read, \
         sum(coalesce(json_extract(data,'$.tokens.cache.write'),0)) as cache_write \
         from message where session_id in (select id from t) \
         and json_extract(data,'$.role') = 'assistant' \
         and json_extract(data,'$.modelID') is not null and time_created >= {since_ms} \
         group by model order by model"
    )
}

/// The query for the root session of a conversation (a resumed session in a new process).
pub const ROOT_SESSION_SQL: &str =
    "select id from session where parent_id is null order by time_created desc limit 1";

/// Argv of `opencode db "<sql>" --format json`.
pub fn db_args(sql: &str) -> Vec<String> {
    ["db", sql, "--format", "json"]
        .iter()
        .map(|x| (*x).to_owned())
        .collect()
}

/// The query's launch spec: the same environment as the turn (no network, the same directories).
pub fn db_spec(opts: &SessionOptions, root: &Path, sql: &str) -> LaunchSpec {
    LaunchSpec {
        args: db_args(sql),
        env: build_env(opts, root),
        cwd: opts.working_dir.clone(),
        stdin: String::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use aurora_protocol::runner::{ModelChoice, ModelMeta, ModelRoleIds};

    use crate::harness::McpBridgeSpec;
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
                subagent_role: Some("large".into()),
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
                ("OPENCODE_CONFIG_CONTENT".into(), "{\"evil\":1}".into()),
                ("OPENCODE_DISABLE_MODELS_FETCH".into(), "0".into()),
                ("XDG_CONFIG_HOME".into(), "/evil".into()),
                ("HTTPS_PROXY".into(), "http://evil".into()),
            ],
        }
    }

    fn get<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
        let all: Vec<_> = env.iter().filter(|(ek, _)| ek == k).collect();
        assert!(all.len() <= 1, "{k} set twice: {all:?}");
        all.first().map(|(_, v)| v.as_str())
    }

    fn cfg(dialect: &str, effort: &str) -> Value {
        serde_json::from_str(&render_config(&opts(dialect, effort)).unwrap()).unwrap()
    }

    #[test]
    fn environment_golden_every_vendor_switch_is_on_and_only_the_placeholder_is_a_credential() {
        let env = build_env(&opts("anthropic_messages", "high"), Path::new("/r"));
        for (k, v) in SWITCHES {
            assert_eq!(get(&env, k), Some(v), "{k} (the caller's value is dropped)");
        }
        assert_eq!(get(&env, "XDG_CONFIG_HOME"), Some("/r/config"));
        assert_eq!(get(&env, "XDG_DATA_HOME"), Some("/r/data"));
        assert_eq!(get(&env, "XDG_CACHE_HOME"), Some("/r/cache"));
        assert_eq!(get(&env, "XDG_STATE_HOME"), Some("/r/state"));
        assert_eq!(
            get(&env, "OPENCODE_CONFIG"),
            Some("/r/config/opencode.json")
        );
        assert_eq!(get(&env, "OPENCODE_CONFIG_CONTENT"), None);
        assert_eq!(get(&env, "ANTHROPIC_API_KEY"), Some(PLACEHOLDER));
        assert_eq!(
            get(&env, "OPENAI_API_KEY"),
            None,
            "the caller's key is dropped"
        );
        for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
            assert_eq!(get(&env, k), Some("http://127.0.0.1:4100"), "{k}");
        }
        assert_eq!(get(&env, "SSL_CERT_FILE"), Some("/run/aurora/ca.pem"));
        assert_eq!(get(&env, "NODE_EXTRA_CA_CERTS"), Some("/run/aurora/ca.pem"));
        assert_eq!(get(&env, "HOME"), Some("/workspace"));
        assert!(!format!("{env:?}").contains("SECRET"), "{env:?}");
        let env = build_env(&opts("openai_chat", ""), Path::new("/r"));
        assert_eq!(get(&env, "OPENAI_API_KEY"), Some(PLACEHOLDER));
        assert_eq!(get(&env, "ANTHROPIC_API_KEY"), None);
    }

    #[test]
    fn config_golden_has_no_secret_no_update_no_share_no_title_and_the_bridge() {
        for d in DIALECTS {
            let text = render_config(&opts(d, "")).unwrap();
            assert!(!text.contains("SECRET") && !text.contains("sk-"), "{text}");
            let v: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["autoupdate"], false);
            assert_eq!(v["share"], "disabled");
            assert_eq!(v["permission"]["*"], "allow");
            assert_eq!(v["agent"]["title"]["disable"], true);
            assert_eq!(v["model"], "aurora/claude-sonnet-4-5");
            assert_eq!(v["small_model"], "aurora/small-model");
            assert_eq!(v["agent"]["general"]["model"], "aurora/big-model");
            assert_eq!(v["instructions"][0], "/workspace/AGENTS.md");
            let mcp = &v["mcp"]["aurora"];
            assert_eq!(mcp["type"], "local");
            assert_eq!(mcp["enabled"], true);
            assert_eq!(mcp["command"][1], "mcp-bridge");
            let key = &v["provider"]["aurora"]["options"]["apiKey"];
            assert!(
                key == "{env:ANTHROPIC_API_KEY}" || key == "{env:OPENAI_API_KEY}",
                "a reference, never a literal: {key}"
            );
            let m = &v["provider"]["aurora"]["models"];
            assert_eq!(m["claude-sonnet-4-5"]["limit"]["context"], 200_000);
            assert!(m.get("big-model").is_some() && m.get("small-model").is_some());
            assert!(m.get("medium-model").is_none());
        }
        assert!(
            render_config(&{
                let mut o = opts("anthropic_messages", "");
                o.mcp_bridge = McpBridgeSpec::default();
                o
            })
            .is_err()
        );
    }

    #[test]
    fn each_dialect_gets_its_bundled_package_and_base_url() {
        let a = cfg("anthropic_messages", "");
        assert_eq!(a["provider"]["aurora"]["npm"], "@ai-sdk/anthropic");
        assert_eq!(
            a["provider"]["aurora"]["options"]["baseURL"], "https://llm.example/v1",
            "the Anthropic package wants /v1"
        );
        let r = cfg("openai_responses", "");
        assert_eq!(r["provider"]["aurora"]["npm"], "@ai-sdk/openai");
        let c = cfg("openai_chat", "");
        assert_eq!(c["provider"]["aurora"]["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(
            c["provider"]["aurora"]["options"]["baseURL"],
            "https://llm.example/v1"
        );
        let mut o = opts("anthropic_messages", "");
        o.model.base_url = "https://llm.example/v1/".into();
        assert_eq!(base_url(&o), "https://llm.example/v1", "no doubled /v1");
    }

    #[test]
    fn dialects_opencode_cannot_speak_are_refused_never_emulated() {
        for d in ["anthropic_bedrock", "anthropic_vertex", "gemini", ""] {
            assert!(provider_of(d).is_err(), "{d}");
            assert!(render_config(&opts(d, "")).is_err(), "{d}");
        }
    }

    #[test]
    fn effort_variants_are_defined_for_every_level_and_selected_by_the_flag() {
        let a = cfg("anthropic_messages", "");
        let v = &a["provider"]["aurora"]["models"]["claude-sonnet-4-5"]["variants"];
        assert_eq!(v["low"]["thinking"]["budgetTokens"], 4_000);
        assert_eq!(v["max"]["thinking"]["budgetTokens"], 32_000);
        let o = cfg("openai_chat", "");
        let v = &o["provider"]["aurora"]["models"]["claude-sonnet-4-5"]["variants"];
        assert_eq!(v["medium"]["reasoningEffort"], "medium");
        assert_eq!(v["max"]["reasoningEffort"], "high");
        for (e, want) in [
            ("low", Some("low")),
            ("medium", Some("medium")),
            ("high", Some("high")),
            ("max", Some("max")),
            ("", None),
            ("x", None),
        ] {
            assert_eq!(effort(e), want, "{e}");
            let args = build_args(&opts("anthropic_messages", e), None);
            assert_eq!(
                args.windows(2)
                    .any(|w| w == ["--variant", want.unwrap_or("-")]),
                want.is_some(),
                "{e}: {args:?}"
            );
        }
    }

    #[test]
    fn argv_golden_has_no_message_and_the_session_only_once_it_exists() {
        let o = opts("anthropic_messages", "");
        assert_eq!(
            build_args(&o, None),
            ["run", "--format", "json", "-m", "aurora/claude-sonnet-4-5"]
        );
        let a = build_args(&o, Some("ses_abc123"));
        assert_eq!(&a[5..], ["--session", "ses_abc123"]);
        assert!(
            !a.contains(&"--attach".to_owned()),
            "never a shared server (D-301)"
        );
        assert!(
            !a.contains(&"--auto".to_owned()),
            "the permission block does the job"
        );
        assert!(!a.contains(&"--thinking".to_owned()));
    }

    #[test]
    fn a_prompt_over_128_kib_is_never_an_argument() {
        let big = "x".repeat(300 * 1024);
        let s = launch_spec(&opts("anthropic_messages", ""), Path::new("/r"), None, &big);
        assert_eq!(s.stdin.len(), 300 * 1024);
        assert!(
            s.args.iter().all(|a| a.len() < 1024),
            "no argument carries the prompt"
        );
    }

    #[test]
    fn the_plugin_files_make_the_npm_install_a_no_op() {
        let f = plugin_files();
        let lock: Value = serde_json::from_str(
            &f.iter()
                .find(|(p, _)| p.ends_with("package-lock.json"))
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(
            lock["packages"][""]["dependencies"]["@opencode-ai/plugin"],
            "1.19.6"
        );
        assert!(
            f.iter()
                .any(|(p, _)| p.contains("node_modules/@opencode-ai/plugin"))
        );
    }

    #[test]
    fn session_ids_and_the_usage_query_are_injection_safe() {
        assert!(is_session_id("ses_e11d2b907ffeXt4EQ1jXlfE02D"));
        for bad in [
            "",
            "ses_",
            "x",
            "ses_a'b",
            "ses_a b",
            "ses_a;drop",
            "--session",
        ] {
            assert!(!is_session_id(bad), "{bad}");
        }
        let q = usage_sql("ses_abc", 5);
        assert!(q.contains("'ses_abc'") && q.contains("time_created >= 5"));
        assert_eq!(db_args("q"), ["db", "q", "--format", "json"]);
    }
}

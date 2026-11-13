//! Boot document and sandbox spec assembly (docs/11 §The boot document).

use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::repo::agents::AgentRow;
use aurora_protocol::common::SecretString;
use aurora_protocol::runner::{Pairs, RunnerCapabilities, SandboxResources, SandboxSpec};
use aurora_registry::snapshot::AppEntry;
use aurora_registry::validate::{env_name_reserved, env_name_valid, stub_path_ok};
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Sandbox-facing supervisor port of the runner (decision I-07).
pub const SUPERVISOR_PORT: u16 = 4105;

// Fallbacks for runners that do not send `defaults`: the docs/15 values of
// `runner.sandbox_cpus` / `sandbox_memory_mib` / `sandbox_pids` / `home_gib` (FIX-011, D-90).
const DEFAULT_CPU_MILLIS: u32 = 1000;
const DEFAULT_MEMORY_MIB: u32 = 2048;
const DEFAULT_PIDS: u32 = 512;
const DEFAULT_HOME_GIB: u32 = 10;

/// Resource profile: agent override > runner default (`caps.defaults`, else the docs/15
/// constants), clamped to the runner's bounds. (There is no org override in the code yet.)
pub fn profile(
    caps: Option<&RunnerCapabilities>,
    overrides: Option<&Value>,
) -> (SandboxResources, u32) {
    let d = caps.and_then(|c| c.defaults);
    let pick = |key: &str, default: u32, range: Option<&aurora_protocol::runner::Range>| {
        let v = overrides
            .and_then(|o| o.get(key))
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(default);
        range.map_or(v, |r| v.clamp(r.min, r.max.max(r.min)))
    };
    let r = caps.map(|c| &c.resources);
    (
        SandboxResources {
            cpu_millis: pick(
                "cpu_millis",
                d.map_or(DEFAULT_CPU_MILLIS, |d| d.cpu_millis),
                r.map(|r| &r.cpu),
            ),
            memory_mib: pick(
                "memory_mib",
                d.map_or(DEFAULT_MEMORY_MIB, |d| d.memory_mib),
                r.map(|r| &r.memory_mib),
            ),
            pids: pick(
                "pids",
                d.map_or(DEFAULT_PIDS, |d| d.pids),
                r.map(|r| &r.pids),
            ),
        },
        pick(
            "home_gib",
            d.map_or(DEFAULT_HOME_GIB, |d| d.home_gib),
            r.map(|r| &r.home_gib),
        ),
    )
}

/// `host:port` of the gateway proxy from the advertised URL (no scheme, no credentials).
fn proxy_hostport(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split('/').next().unwrap_or(rest);
    rest.rsplit('@').next().unwrap_or(rest).to_owned()
}

/// Host part of the advertised proxy URL (no port, brackets or credentials).
fn proxy_host(url: &str) -> String {
    let hp = proxy_hostport(url);
    match hp.strip_prefix('[') {
        Some(r) => r.split(']').next().unwrap_or(r).to_owned(),
        None => hp
            .rsplit_once(':')
            .map_or(hp.as_str(), |(h, _)| h)
            .to_owned(),
    }
}

pub struct BootInput<'a> {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub agent_id: Uuid,
    pub handle: &'a str,
    pub name: &'a str,
    pub harness: &'a str,
    pub replica_host: &'a str,
    pub bootstrap_token: &'a str,
    pub boot_token: &'a str,
    pub ca_pem: &'a str,
    /// App stub files (`files` of the boot document), from [`AppBoot::files`].
    pub files: &'a [Value],
    /// (RFE-003) Boot capabilities, see [`capabilities_for`].
    pub capabilities: &'a [String],
}

/// Total stub content cap: the boot document (64 KiB, also an env var) must stay small.
const STUB_BYTES_MAX: usize = 32 * 1024;

/// Env and files that the agent's granted apps contribute to a boot (09 §`agent`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppBoot {
    /// `[name, value]` pairs: `AURORA_AGENT_HANDLE` then the app entries, sorted by name.
    pub env: Pairs,
    /// Boot-document `files` entries.
    pub files: Vec<Value>,
    /// Hex SHA-256 over `env` and `files`; compared with the running boot's for `restart_to_update`.
    pub hash: String,
}

/// Pure assembly from the granted apps' manifests. Invalid or reserved env names and unsafe stub
/// paths or modes are dropped (the validator already refuses them; this is the defensive re-check).
/// On a duplicate, the later app (apps are in id order) wins, with a warning.
pub fn app_boot(handle: &str, apps: &[&AppEntry]) -> AppBoot {
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut stubs: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut bytes = 0usize;
    for app in apps {
        let Some(ag) = &app.agent else { continue };
        for e in &ag.env {
            if env_name_reserved(&e.name) || !env_name_valid(&e.name) {
                tracing::warn!(app = %app.app_id, name = %e.name, "app env entry dropped (reserved or invalid name)");
                continue;
            }
            if env.insert(e.name.clone(), e.value.clone()).is_some() {
                tracing::warn!(app = %app.app_id, name = %e.name, "duplicate app env entry; last app wins");
            }
        }
        for st in &ag.stubs {
            let mode = st.mode.clone().unwrap_or_else(|| "0644".to_owned());
            let mode_ok = !mode.is_empty()
                && mode.len() <= 4
                && u32::from_str_radix(&mode, 8).is_ok_and(|m| m <= 0o777);
            if !stub_path_ok(&st.path) || !mode_ok {
                tracing::warn!(app = %app.app_id, path = %st.path, "app stub dropped (unsafe path or mode)");
                continue;
            }
            let prev = stubs
                .get(&st.path)
                .map_or(0, |(_, c): &(String, String)| c.len());
            if bytes - prev + st.content.len() > STUB_BYTES_MAX {
                tracing::warn!(app = %app.app_id, path = %st.path, "app stub dropped (boot document size cap)");
                continue;
            }
            bytes = bytes - prev + st.content.len();
            if stubs
                .insert(st.path.clone(), (mode, st.content.clone()))
                .is_some()
            {
                tracing::warn!(app = %app.app_id, path = %st.path, "duplicate app stub path; last app wins");
            }
        }
    }
    // Request env cannot be overridden by apps: the handle is set last.
    env.insert("AURORA_AGENT_HANDLE".into(), handle.to_owned());
    let files: Vec<Value> = stubs
        .into_iter()
        .map(|(path, (mode, content))| {
            json!({
                "path": format!("/workspace/{path}"),
                "mode": mode,
                "content_b64": base64::engine::general_purpose::STANDARD.encode(content),
                "only_if_absent_or_sentinel": true,
            })
        })
        .collect();
    let env: Pairs = env.into_iter().collect();
    // (phase 1, RFE-002) The gateway transport is a boot input: a sandbox booted before the proxy
    // listener became TLS-only (its supervisor forwards plaintext) differs here, so the API flags
    // it `restart_to_update`. Bump the marker when the boot-time gateway contract changes again.
    let digest = Sha256::digest(
        json!({"env": env, "files": files, "gateway": {"tls": true}})
            .to_string()
            .as_bytes(),
    );
    AppBoot {
        env,
        files,
        hash: digest.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

/// The boot env/files for `agent`: apps with an effective (selected) connection grant, in the same
/// view as `GET /v1/agents/{agent}/effective` and available to the org (09 §App availability), with manifests from the active snapshot.
pub async fn app_boot_for(state: &AppState, agent: &AgentRow) -> Result<AppBoot, ApiError> {
    let (_, granted) = crate::domains::grants::effective::compute_with_apps(state, agent).await?;
    let snap = crate::domains::apps::service::snapshot(state).await?;
    let av =
        crate::domains::apps::service::availability_of(state, agent.org_id, agent.workspace_id)
            .await?;
    let apps: Vec<&AppEntry> = snap
        .as_ref()
        .map(|sn| {
            granted
                .iter()
                .filter(|id| av.is_available(id))
                .filter_map(|id| sn.app(id))
                .collect()
        })
        .unwrap_or_default();
    Ok(app_boot(&agent.handle, &apps))
}

/// (phase 1, RFE-003) The boot capabilities of an agent's sandbox: what the sandbox image
/// declares (`org.aurora.capabilities`, reported by the runner at registration) ∩ `browser`
/// unless the effective open-lane posture (strictest of instance, org, workspace) is
/// `disabled`. The boot credential is minted with the same set. (P4-TERM, C-623) `terminal`
/// rides the same array when the operator enabled the terminal **and** the runner advertised
/// it (FIX-092): the supervisor spawns PTYs only for a boot document that carries it
/// (`12 §Terminal`: `pty.open` → `pty.refused { disabled }` otherwise).
pub async fn capabilities_for(
    state: &AppState,
    agent: &AgentRow,
    caps: Option<&RunnerCapabilities>,
) -> Result<Vec<String>, ApiError> {
    use aurora_llm::governance::OpenLanePosture;
    let mut out: Vec<String> = Vec::new();
    let declared: &[String] = caps.map_or(&[], |c| c.image_capabilities.as_slice());
    if declared.iter().any(|c| c == "browser") {
        let org = crate::domains::secrets::gov::effective_for(state, agent.org_id).await?;
        let ws =
            aurora_db::repo::policy::published_open_lane_posture(&state.pool, agent.workspace_id)
                .await?;
        // An unparsable stored posture fails closed.
        let ws = OpenLanePosture::parse(&ws).unwrap_or(OpenLanePosture::Disabled);
        // Strictest of org and workspace; only `disabled` removes the capability.
        if org.open_lane.max(ws) != OpenLanePosture::Disabled {
            out.push("browser".to_owned());
        }
    }
    // Only when this runner advertised the terminal too (C-623, same gate as `register`): a
    // runner without the capability cannot serve the `pty.*` frames the boot would promise.
    if state.config.terminal.enabled && caps.is_some_and(|c| c.terminal) {
        out.push("terminal".to_owned());
    }
    Ok(out)
}

/// The `gateway` object of the boot document: the supervisor dials `proxy` over TLS and verifies
/// the proxy certificate against `server_name` (C-55).
fn gateway_boot(sandbox_proxy_url: &str, boot_token: &str) -> Value {
    json!({
        "proxy": proxy_hostport(sandbox_proxy_url),
        "tls": true,
        "server_name": proxy_host(sandbox_proxy_url),
        "boot_token": boot_token,
    })
}

/// base64url(JSON) `AURORA_BOOT` document.
pub fn boot_secret(s: &AppState, i: &BootInput<'_>) -> Result<SecretString, ApiError> {
    let host = if i.replica_host.contains(':') {
        i.replica_host.to_owned()
    } else {
        format!("{}:{SUPERVISOR_PORT}", i.replica_host)
    };
    let doc = json!({
        "v": 1,
        "sandbox_id": i.sandbox_id,
        "boot_id": i.boot_id,
        "agent": {"id": i.agent_id, "handle": i.handle, "name": i.name},
        "runner_url": format!("ws://{host}/v1/supervisor"),
        "bootstrap_token": i.bootstrap_token,
        // (phase 1, RFE-002) The supervisor always dials the gateway over TLS, verifying the
        // proxy certificate against `ca_pem` and `server_name` (the host of the proxy address,
        // which is in the certificate's SAN set, docs/06 §Proxy listener TLS).
        // (FIX-010, D-87) The address is `gateway.sandbox_proxy_url` (the in-cluster alias on
        // Kubernetes), not the advertised URL that BYO agents get.
        "gateway": gateway_boot(&s.config.sandbox_proxy_url, i.boot_token),
        "ca_pem": i.ca_pem,
        "files": i.files,
        // (phase 1, RFE-003) The supervisor starts the open proxy only with `browser`.
        "capabilities": i.capabilities,
        "harness": {"id": i.harness, "capabilities_expected": []},
        "timezone": s.config.hosted.default_timezone,
    });
    let bytes = serde_json::to_vec(&doc).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(SecretString::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
    ))
}

/// Image named in a start item when the runner did not advertise one (older runners). The
/// runner applies its own `runner.image` regardless (docs/11 §Sandbox image).
pub fn default_image() -> String {
    format!("aurora-sandbox:{}", env!("CARGO_PKG_VERSION"))
}

pub fn spec(
    image: String,
    env: Pairs,
    boot_secret: SecretString,
    home_ref: String,
    home_gib: u32,
    resources: SandboxResources,
    labels: BTreeMap<String, String>,
) -> SandboxSpec {
    SandboxSpec {
        image,
        env,
        boot_secret,
        home_ref,
        home_gib,
        resources,
        labels,
    }
}

/// The gateway CA certificate (`gateway.ca_dir/ca.pem`).
pub async fn read_ca(state: &AppState) -> Result<String, ApiError> {
    let path = std::path::Path::new(&state.config.gateway_ca_dir).join("ca.pem");
    tokio::fs::read_to_string(path).await.map_err(|e| {
        tracing::warn!(error = %e, "gateway CA unreadable");
        ApiError::unavailable("gateway_ca_unavailable", "The gateway CA is not available.")
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // FIX-010 (C-300): the boot document's gateway object is built from the sandbox proxy address
    // (config tests pin the per-backend defaults and that the advertised URL is untouched).
    #[test]
    fn boot_gateway_is_built_from_the_given_sandbox_address() {
        let g = gateway_boot("https://aurora-gateway:4102", "t");
        assert_eq!(g["proxy"], "aurora-gateway:4102");
        assert_eq!(g["server_name"], "aurora-gateway");
        assert_eq!(g["tls"], true);
        let g = gateway_boot("https://[::1]:4102", "t");
        assert_eq!(g["proxy"], "[::1]:4102");
        assert_eq!(g["server_name"], "::1");
    }

    // FIX-011 (C-192): agent override > runner defaults > docs/15 constants, then clamped.
    fn caps(defaults: Option<Value>) -> RunnerCapabilities {
        let mut v = json!({
            "backend": "kubernetes", "home_durability": "resident",
            "resources": {
                "cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 512, "max": 8192},
                "pids": {"min": 64, "max": 2048}, "home_gib": {"min": 1, "max": 100}
            },
            "max_sandboxes": 4, "harnesses": [], "max_attachment_bytes": 1024,
        });
        if let Some(d) = defaults {
            v["defaults"] = d;
        }
        serde_json::from_value(v).unwrap()
    }

    fn flat(r: &SandboxResources, home: u32) -> (u32, u32, u32, u32) {
        (r.cpu_millis, r.memory_mib, r.pids, home)
    }

    #[test]
    fn profile_agent_override_wins_over_runner_defaults() {
        let c = caps(Some(
            json!({"cpu_millis": 1000, "memory_mib": 2048, "pids": 512, "home_gib": 10}),
        ));
        let o = json!({"cpu_millis": 3000, "memory_mib": 4096, "pids": 800, "home_gib": 20});
        let (r, h) = profile(Some(&c), Some(&o));
        assert_eq!(flat(&r, h), (3000, 4096, 800, 20));
        // A partial override only replaces its own axis.
        let (r, h) = profile(Some(&c), Some(&json!({"memory_mib": 1024})));
        assert_eq!(flat(&r, h), (1000, 1024, 512, 10));
    }

    #[test]
    fn profile_uses_the_runner_defaults_without_an_override() {
        let c = caps(Some(
            json!({"cpu_millis": 2000, "memory_mib": 3072, "pids": 700, "home_gib": 15}),
        ));
        let (r, h) = profile(Some(&c), None);
        assert_eq!(flat(&r, h), (2000, 3072, 700, 15));
    }

    #[test]
    fn profile_falls_back_to_the_docs_15_values_when_defaults_are_absent() {
        let (r, h) = profile(Some(&caps(None)), None);
        assert_eq!(flat(&r, h), (1000, 2048, 512, 10));
        let (r, h) = profile(None, None);
        assert_eq!(flat(&r, h), (1000, 2048, 512, 10));
    }

    #[test]
    fn profile_still_clamps_to_the_runner_bounds() {
        let c = caps(Some(
            json!({"cpu_millis": 1000, "memory_mib": 2048, "pids": 512, "home_gib": 10}),
        ));
        let o = json!({"cpu_millis": 64000, "memory_mib": 1, "pids": 99999, "home_gib": 500});
        let (r, h) = profile(Some(&c), Some(&o));
        assert_eq!(flat(&r, h), (4000, 512, 2048, 100));
        // Runner defaults outside the bounds are clamped too.
        let c = caps(Some(
            json!({"cpu_millis": 16000, "memory_mib": 2048, "pids": 512, "home_gib": 10}),
        ));
        assert_eq!(profile(Some(&c), None).0.cpu_millis, 4000);
    }

    fn app(id: &str, env: Value, stubs: Value) -> AppEntry {
        serde_json::from_value(json!({
            "app_id": id, "package_id": "p",
            "about": {"id": id, "name": id, "api_spec": null, "connect_note": null, "label_hint": null},
            "credentials": [], "metadata": [], "connect": [], "token": null, "tools": null,
            "approvals": {}, "database": null,
            "agent": {"instructions": "", "env": env, "stubs": stubs, "default_blocklist": []},
        }))
        .unwrap()
    }

    fn get<'a>(b: &'a AppBoot, k: &str) -> Option<&'a str> {
        b.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn env_and_stubs_are_assembled_and_unsafe_entries_dropped() {
        let a = app(
            "a",
            json!([
                {"name": "A_BASE", "value": "1"},
                {"name": "PATH", "value": "/evil"},
                {"name": "AURORA_AGENT_HANDLE", "value": "evil"},
                {"name": "HTTPS_PROXY", "value": "evil"},
                {"name": "ANTHROPIC_API_KEY", "value": "evil"},
                {"name": "lower", "value": "x"},
            ]),
            json!([
                {"path": ".config/a.json", "mode": "0600", "content": "hi"},
                {"path": "../etc/passwd", "content": "x"},
                {"path": "/abs", "content": "x"},
                {"path": "ok/./dot", "content": "x"},
                {"path": "badmode", "mode": "4755", "content": "x"},
            ]),
        );
        let b = app(
            "b",
            json!([{"name": "A_BASE", "value": "2"}, {"name": "B_ONE", "value": "b"}]),
            json!([]),
        );
        let out = app_boot("daneel", &[&a, &b]);
        assert_eq!(get(&out, "AURORA_AGENT_HANDLE"), Some("daneel"));
        assert_eq!(get(&out, "A_BASE"), Some("2"), "last app wins");
        assert_eq!(get(&out, "B_ONE"), Some("b"));
        assert_eq!(out.env.len(), 3, "{:?}", out.env);
        let names: Vec<_> = out.env.iter().map(|(n, _)| n.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.files[0]["path"], "/workspace/.config/a.json");
        assert_eq!(out.files[0]["mode"], "0600");
        assert_eq!(out.files[0]["only_if_absent_or_sentinel"], true);
        assert_eq!(out.files[0]["content_b64"], "aGk=");
        assert_eq!(out, app_boot("daneel", &[&a, &b]), "deterministic");
        assert_ne!(out.hash, app_boot("daneel", &[&a]).hash);
        assert_eq!(
            app_boot("x", &[]).env,
            [("AURORA_AGENT_HANDLE".into(), "x".into())]
        );
    }
}

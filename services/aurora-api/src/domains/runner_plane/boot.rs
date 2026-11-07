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

const DEFAULT_CPU_MILLIS: u32 = 2000;
const DEFAULT_MEMORY_MIB: u32 = 4096;
const DEFAULT_PIDS: u32 = 1024;
const DEFAULT_HOME_GIB: u32 = 10;

/// Resource profile: agent override > runner default, clamped to the runner's bounds.
pub fn profile(
    caps: Option<&RunnerCapabilities>,
    overrides: Option<&Value>,
) -> (SandboxResources, u32) {
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
            cpu_millis: pick("cpu_millis", DEFAULT_CPU_MILLIS, r.map(|r| &r.cpu)),
            memory_mib: pick("memory_mib", DEFAULT_MEMORY_MIB, r.map(|r| &r.memory_mib)),
            pids: pick("pids", DEFAULT_PIDS, r.map(|r| &r.pids)),
        },
        pick("home_gib", DEFAULT_HOME_GIB, r.map(|r| &r.home_gib)),
    )
}

/// `host:port` of the gateway proxy from the advertised URL (no scheme, no credentials).
fn proxy_hostport(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split('/').next().unwrap_or(rest);
    rest.rsplit('@').next().unwrap_or(rest).to_owned()
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
    let digest = Sha256::digest(json!({"env": env, "files": files}).to_string().as_bytes());
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
        "gateway": {"proxy": proxy_hostport(&s.config.advertised_proxy_url), "boot_token": i.boot_token},
        "ca_pem": i.ca_pem,
        "files": i.files,
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

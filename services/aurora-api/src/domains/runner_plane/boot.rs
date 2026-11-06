//! Boot document and sandbox spec assembly (docs/11 §The boot document).

use crate::app::AppState;
use aurora_core::ApiError;
use aurora_protocol::common::SecretString;
use aurora_protocol::runner::{RunnerCapabilities, SandboxResources, SandboxSpec};
use base64::Engine;
use serde_json::{Value, json};
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
        "files": [],
        "harness": {"id": i.harness, "capabilities_expected": []},
        "timezone": s.config.hosted.default_timezone,
    });
    let bytes = serde_json::to_vec(&doc).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(SecretString::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
    ))
}

pub fn spec(
    boot_secret: SecretString,
    home_ref: String,
    home_gib: u32,
    resources: SandboxResources,
    labels: BTreeMap<String, String>,
) -> SandboxSpec {
    SandboxSpec {
        image: format!("aurora-sandbox:{}", env!("CARGO_PKG_VERSION")),
        env: Vec::new(),
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

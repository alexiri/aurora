//! Container state as podman reports it, read as strings.
//!
//! bollard's typed models reject any status they do not know (podman 4.x reports
//! `stopped`, which Docker never does), so listing/inspection bodies are parsed here
//! with a minimal schema whose `status` is a plain string. One function maps it.

use std::collections::HashMap;

use serde::Deserialize;

/// What a status string means for the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Stopping,
    Stopped,
    /// Not a status we know: not running, needs reconcile. Never treated as running.
    Unknown,
}

impl ContainerState {
    /// The one place a status string is interpreted (case-insensitive).
    pub fn classify(status: &str) -> Self {
        match status.trim().to_ascii_lowercase().as_str() {
            "created" | "configured" | "initialized" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "stopping" => Self::Stopping,
            "exited" | "stopped" | "dead" | "removing" => Self::Stopped,
            other => {
                tracing::warn!(
                    status = other,
                    "unknown container state from the runtime; treating it as not running"
                );
                Self::Unknown
            }
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Restarting)
    }
}

/// One row of `GET /containers/json`.
#[derive(Debug, Deserialize)]
pub struct ListedContainer {
    #[serde(rename = "Id", default)]
    pub id: Option<String>,
    #[serde(rename = "State", default)]
    pub state: Option<String>,
    #[serde(rename = "Created", default)]
    pub created: Option<i64>,
    #[serde(rename = "Labels", default)]
    pub labels: Option<HashMap<String, String>>,
}

impl ListedContainer {
    pub fn running(&self) -> bool {
        self.state
            .as_deref()
            .is_some_and(|s| ContainerState::classify(s).is_running())
    }
}

pub fn parse_list(body: &[u8]) -> Result<Vec<ListedContainer>, serde_json::Error> {
    // `null` is what some versions return for an empty list.
    Ok(serde_json::from_slice::<Option<Vec<ListedContainer>>>(body)?.unwrap_or_default())
}

#[derive(Debug, Default, Deserialize)]
struct RawState {
    #[serde(rename = "Status", default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(rename = "Labels", default)]
    labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
struct RawHostConfig {
    #[serde(rename = "NanoCpus", default)]
    nano_cpus: Option<i64>,
    #[serde(rename = "Memory", default)]
    memory: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawInspect {
    #[serde(rename = "State", default)]
    state: Option<RawState>,
    #[serde(rename = "Config", default)]
    config: Option<RawConfig>,
    #[serde(rename = "HostConfig", default)]
    host_config: Option<RawHostConfig>,
}

/// The parts of `GET /containers/{id}/json` the runner reads.
#[derive(Debug)]
pub struct Inspected {
    pub state: ContainerState,
    /// The status string as reported (`"unknown"` when absent).
    pub phase: String,
    pub labels: HashMap<String, String>,
    pub nano_cpus: i64,
    pub memory: i64,
}

pub fn parse_inspect(body: &[u8]) -> Result<Inspected, serde_json::Error> {
    let raw: RawInspect = serde_json::from_slice(body)?;
    let status = raw.state.and_then(|s| s.status);
    let state = status
        .as_deref()
        .map_or(ContainerState::Unknown, ContainerState::classify);
    let hc = raw.host_config.unwrap_or_default();
    Ok(Inspected {
        state,
        phase: status.unwrap_or_else(|| "unknown".into()),
        labels: raw.config.and_then(|c| c.labels).unwrap_or_default(),
        nano_cpus: hc.nano_cpus.unwrap_or(0),
        memory: hc.memory.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V4_INSPECT: &str = include_str!("../../tests/fixtures/podman-4.9-inspect-stopped.json");
    const V6_INSPECT: &str = include_str!("../../tests/fixtures/podman-6-inspect-exited.json");
    const UNKNOWN_INSPECT: &str =
        include_str!("../../tests/fixtures/podman-unknown-state-inspect.json");
    const V4_LIST: &str = include_str!("../../tests/fixtures/podman-4.9-list-stopped.json");

    #[test]
    fn podman_4_stopped_is_stopped() {
        let i = parse_inspect(V4_INSPECT.as_bytes()).unwrap();
        assert_eq!(i.state, ContainerState::Stopped);
        assert!(!i.state.is_running());
        assert_eq!(i.phase, "stopped");
        assert_eq!(i.labels.get("aurora.boot").map(String::as_str), Some("b1"));
    }

    #[test]
    fn podman_6_exited_is_stopped() {
        let i = parse_inspect(V6_INSPECT.as_bytes()).unwrap();
        assert_eq!(i.state, ContainerState::Stopped);
        assert_eq!(i.phase, "exited");
        assert_eq!(i.nano_cpus, 500_000_000);
        assert_eq!(i.memory, 512 * 1024 * 1024);
    }

    #[test]
    fn unknown_state_is_never_running() {
        let i = parse_inspect(UNKNOWN_INSPECT.as_bytes()).unwrap();
        assert_eq!(i.state, ContainerState::Unknown);
        assert!(!i.state.is_running());
        assert_eq!(i.phase, "frobnicating");
    }

    #[test]
    fn missing_status_is_never_running() {
        let i = parse_inspect(br#"{"State":{"Running":true}}"#).unwrap();
        assert!(!i.state.is_running());
        assert_eq!(i.phase, "unknown");
    }

    #[test]
    fn classification_table() {
        use ContainerState::*;
        for (s, want) in [
            ("created", Created),
            ("running", Running),
            ("paused", Paused),
            ("restarting", Restarting),
            ("stopping", Stopping),
            ("exited", Stopped),
            ("stopped", Stopped),
            ("dead", Stopped),
            ("removing", Stopped),
            ("", Unknown),
            ("whatever", Unknown),
        ] {
            assert_eq!(ContainerState::classify(s), want, "{s}");
        }
        assert!(Running.is_running() && Paused.is_running() && Restarting.is_running());
        assert!(!Created.is_running() && !Stopping.is_running() && !Stopped.is_running());
    }

    #[test]
    fn list_tolerates_stopped_and_unknown() {
        let l = parse_list(V4_LIST.as_bytes()).unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(
            l.iter().map(ListedContainer::running).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert!(parse_list(b"null").unwrap().is_empty());
    }
}

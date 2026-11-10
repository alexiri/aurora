//! The substrate seam (`11 §The backend seam`).

use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::{HarnessId, Observed, RunnerCapabilities, SandboxSpec};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// OCI label through which a sandbox image lists the harness runtimes it contains (`11 §Sandbox
/// image`).
pub const HARNESSES_LABEL: &str = "org.aurora.harnesses";

/// Comma list → de-duplicated harness ids. An absent label is `claude-code,fake`, so images built
/// before phase 4 keep working; a present but empty label lists nothing (fail closed).
pub fn parse_harnesses(label: Option<&str>) -> Vec<HarnessId> {
    let Some(label) = label else {
        return vec![HarnessId::ClaudeCode, HarnessId::Fake];
    };
    let mut out: Vec<HarnessId> = Vec::new();
    for t in label.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if let Ok(id) = HarnessId::from_wire(t)
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
    out
}

/// Label keys on every managed object.
pub const LABEL_MANAGED: &str = "aurora.managed";
pub const LABEL_INSTALLATION: &str = "aurora.installation";
pub const LABEL_RUNNER: &str = "aurora.runner";
pub const LABEL_SANDBOX: &str = "aurora.sandbox";
pub const LABEL_BOOT: &str = "aurora.boot";

/// Backend failure with the meaning the runner maps to report reason codes.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BackendError {
    #[error("image unavailable: {0}")]
    ImageUnavailable(String),
    #[error("no capacity: {0}")]
    Capacity(String),
    #[error("wake capacity wait expired")]
    WakeCapacityTimeout,
    /// (FIX-025, C-581) The substrate refused to create the container (Kubernetes admission:
    /// quota, LimitRange, Pod Security, RBAC, validation), with the API server's message.
    #[error("{0}")]
    Refused(String),
    /// (FIX-025, C-581) The container ended while starting (exit code, last state reason).
    #[error("{0}")]
    Terminated(String),
    /// (FIX-025, D-158) The whole start exceeded its bound; the message names the last phase.
    #[error("{0}")]
    StartTimeout(String),
    /// (FIX-025, C-581) The home volume claim cannot be bound (no matching PersistentVolume, no
    /// or an unknown storage class); waiting does not help.
    #[error("{0}")]
    HomeVolumeUnbound(String),
    #[error("{0}")]
    Other(String),
}

impl BackendError {
    /// Reason code carried by `sandbox.status failed`.
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::ImageUnavailable(_) => "image_unavailable",
            Self::Capacity(_) | Self::WakeCapacityTimeout => "at_capacity",
            Self::Refused(_) => "pod_refused",
            Self::Terminated(_) => "pod_failed",
            Self::StartTimeout(_) => "sandbox_start_failed",
            Self::HomeVolumeUnbound(_) => "home_volume_unbound",
            Self::Other(_) => "agent_start_failed",
        }
    }

    /// Message safe to put on the wire: one line, bounded, credentials masked (FIX-025).
    pub fn sanitized_message(&self) -> String {
        aurora_core::sanitize::reason(&self.to_string(), 500)
    }
}

/// (FIX-025) Bound and sanitise substrate warning events for a `sandbox.status` (C-580).
pub fn bound_events(events: impl IntoIterator<Item = String>) -> Vec<String> {
    events
        .into_iter()
        .map(|e| aurora_core::sanitize::reason(&e, aurora_protocol::runner::MAX_STATUS_EVENT_CHARS))
        .filter(|e| !e.is_empty())
        .take(aurora_protocol::runner::MAX_STATUS_EVENTS)
        .collect()
}

/// Opaque substrate handle of a running container.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContainerRef(pub String);

/// Everything a backend needs to start one boot.
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub spec: SandboxSpec,
}

/// Lets a backend report a held wake (`waiting_for_capacity`) and (FIX-025) start progress.
pub trait HoldCallback: Send + Sync {
    fn on_hold(&self);
    /// (FIX-025, C-580) Current start phase (`provisioning`, `scheduling`, `pulling`,
    /// `creating`, `pending`, …) and a one-line detail; the executor rate-limits the reports.
    fn progress(&self, _phase: &str, _detail: &str) {}
}

/// State of a sandbox's container as read from the substrate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub container: ContainerRef,
    pub running: bool,
    pub boot_id: Option<Uuid>,
    pub phase: String,
    pub observed: Option<Observed>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Container,
    Home,
    Secret,
}

/// An object bearing aurora labels (any runner, any installation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedObject {
    pub kind: ObjectKind,
    pub reference: String,
    pub sandbox_id: Option<Uuid>,
    pub boot_id: Option<Uuid>,
    /// `aurora.runner` label (runner id).
    pub runner: Option<String>,
    /// `aurora.installation` label (token fingerprint).
    pub installation: Option<String>,
    pub created_at: DateTime<Utc>,
    pub running: bool,
}

#[async_trait]
pub trait Backend: Send + Sync {
    fn id(&self) -> &'static str;
    /// Declared at registration; never probed later.
    fn capabilities(&self) -> RunnerCapabilities;
    /// (RFE-003) Capabilities the sandbox `image` declares in its `org.aurora.capabilities`
    /// label (comma list), read once when the runner registers. A backend that cannot read
    /// image labels, or an image without the label, reports none (fail closed).
    async fn image_capabilities(&self, _image: &str) -> Vec<String> {
        Vec::new()
    }
    /// (phase 4, P4-HAR, D-122) Harness runtimes the sandbox `image` declares in its
    /// `org.aurora.harnesses` label, read once when the runner registers; an image without the
    /// label is `claude-code,fake` ([`parse_harnesses`]). `None` = this backend cannot read
    /// image labels (Kubernetes) or the image cannot be inspected: the registration keeps the
    /// backend's declared list, or `runner.image_harnesses` when configured.
    async fn image_harnesses(&self, _image: &str) -> Option<Vec<HarnessId>> {
        None
    }
    /// Idempotent.
    async fn provision_home(
        &self,
        sandbox_id: Uuid,
        home_ref: &str,
        home_gib: u32,
    ) -> Result<(), BackendError>;
    /// Idempotent per `boot_id`; removes objects of other boots of the sandbox first.
    async fn start(
        &self,
        req: &StartRequest,
        hold: &dyn HoldCallback,
    ) -> Result<ContainerRef, BackendError>;
    /// (FIX-025, C-580) The substrate's latest warning events for this boot, newest first
    /// (Kubernetes: the pod's `Warning` events). Unbounded here; the executor bounds them.
    async fn start_events(&self, _req: &StartRequest) -> Vec<String> {
        Vec::new()
    }
    /// (FIX-025) Where the sandbox's supervisor must connect (`host (ip):port`), for the
    /// `supervisor_unreachable` hint. `None` when the backend cannot tell.
    async fn supervisor_endpoint(&self) -> Option<String> {
        None
    }
    /// Removes the container, keeps the home.
    async fn stop(&self, container: &ContainerRef, grace: Duration) -> Result<(), BackendError>;
    async fn inspect(&self, sandbox_id: Uuid) -> Result<Option<Observation>, BackendError>;
    async fn list_managed(&self) -> Result<Vec<ManagedObject>, BackendError>;
    async fn destroy(&self, obj: &ManagedObject) -> Result<(), BackendError>;
}

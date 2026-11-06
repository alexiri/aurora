//! The substrate seam (`11 §The backend seam`).

use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::{Observed, RunnerCapabilities, SandboxSpec};
use chrono::{DateTime, Utc};
use uuid::Uuid;

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
    #[error("{0}")]
    Other(String),
}

impl BackendError {
    /// Reason code carried by `sandbox.status failed`.
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::ImageUnavailable(_) => "image_unavailable",
            Self::Capacity(_) | Self::WakeCapacityTimeout => "at_capacity",
            Self::Other(_) => "agent_start_failed",
        }
    }

    /// Message safe to put on the wire: control characters removed, bounded.
    pub fn sanitized_message(&self) -> String {
        self.to_string()
            .chars()
            .filter(|c| !c.is_control())
            .take(500)
            .collect()
    }
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

/// Lets a backend report a held wake (`waiting_for_capacity`).
pub trait HoldCallback: Send + Sync {
    fn on_hold(&self);
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
    /// Removes the container, keeps the home.
    async fn stop(&self, container: &ContainerRef, grace: Duration) -> Result<(), BackendError>;
    async fn inspect(&self, sandbox_id: Uuid) -> Result<Option<Observation>, BackendError>;
    async fn list_managed(&self) -> Result<Vec<ManagedObject>, BackendError>;
    async fn destroy(&self, obj: &ManagedObject) -> Result<(), BackendError>;
}

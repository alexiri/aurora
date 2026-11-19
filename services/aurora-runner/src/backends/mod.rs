//! Backend registry: a new substrate is a module plus one line here.

use std::sync::Arc;

use aurora_config::{RunnerBackend, RunnerConfig};

use crate::backend::Backend;

pub mod fake;
pub mod kubernetes;
pub mod podman;
pub mod podman_state;

/// The sandbox seccomp profile (D-394, SEC-137): the container runtime's default plus a denial of
/// `ptrace`, `process_vm_readv` and `process_vm_writev` (EPERM). One file serves both backends: the
/// chart ships it to the nodes' kubelet seccomp directory (`Localhost` profile
/// [`SANDBOX_SECCOMP_LOCALHOST`]) and the podman backend hands it to podman as a file.
pub const SANDBOX_SECCOMP_PROFILE: &str =
    include_str!("../../../../deploy/helm/aurora/files/sandbox-seccomp.json");

/// The profile's path under the kubelet's seccomp directory (`/var/lib/kubelet/seccomp`), which the
/// chart's installer DaemonSet fills; must match `deploy/helm/aurora/templates/seccomp.yaml`.
pub const SANDBOX_SECCOMP_LOCALHOST: &str = "aurora/sandbox-seccomp.json";

pub async fn build(cfg: &RunnerConfig) -> Result<Arc<dyn Backend>, String> {
    match cfg.backend {
        RunnerBackend::Fake => Ok(Arc::new(fake::FakeBackend::new(cfg.runner.max_sandboxes))),
        RunnerBackend::Podman => {
            let settings = podman::PodmanSettings::from_config(cfg);
            let backend = podman::PodmanBackend::connect(settings).await?;
            Ok(Arc::new(backend))
        }
        RunnerBackend::Kubernetes => {
            let settings = kubernetes::KubernetesSettings::from_config(cfg)?;
            let backend = kubernetes::KubernetesBackend::connect(settings).await?;
            Ok(Arc::new(backend))
        }
    }
}

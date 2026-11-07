//! Backend registry: a new substrate is a module plus one line here.

use std::sync::Arc;

use aurora_config::{RunnerBackend, RunnerConfig};

use crate::backend::Backend;

pub mod fake;
pub mod kubernetes;
pub mod podman;
pub mod podman_state;

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

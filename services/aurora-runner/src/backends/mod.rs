//! Backend registry: a new substrate is a module plus one line here.

use std::sync::Arc;

use aurora_config::RunnerBackend;

use crate::backend::Backend;

pub mod fake;

pub fn build(kind: RunnerBackend, max_sandboxes: u32) -> Result<Arc<dyn Backend>, String> {
    match kind {
        RunnerBackend::Fake => Ok(Arc::new(fake::FakeBackend::new(max_sandboxes))),
        RunnerBackend::Podman => Err("backend `podman` is not implemented yet (RUN-003)".into()),
        RunnerBackend::Kubernetes => Err("backend `kubernetes` is not implemented yet (M7)".into()),
    }
}

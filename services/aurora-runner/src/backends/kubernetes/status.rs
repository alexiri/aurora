//! Pod status → backend meaning (`11 §Backend kubernetes`).

use k8s_openapi::api::core::v1::Pod;

/// Container waiting reasons that mean the image cannot be used.
const IMAGE_ERRORS: [&str; 4] = [
    "ErrImagePull",
    "ImagePullBackOff",
    "InvalidImageName",
    "ErrImageNeverPull",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PodState {
    /// All containers started (`phase: Running`).
    Running,
    /// Accepted and scheduled, still starting (pulling, creating).
    Pending,
    /// `PodScheduled=False`, reason `Unschedulable`: no node has capacity.
    Unschedulable(String),
    /// Image pull failure with the kubelet's message.
    ImageError(String),
    /// `Succeeded`, `Failed` or being deleted.
    Terminated(String),
}

pub fn classify(pod: &Pod) -> PodState {
    if pod.metadata.deletion_timestamp.is_some() {
        return PodState::Terminated("pod is being deleted".into());
    }
    let Some(status) = pod.status.as_ref() else {
        return PodState::Pending;
    };
    let phase = status.phase.as_deref().unwrap_or("Pending");
    if matches!(phase, "Succeeded" | "Failed") {
        let why = status
            .message
            .clone()
            .or_else(|| status.reason.clone())
            .unwrap_or_else(|| format!("pod {}", phase.to_lowercase()));
        return PodState::Terminated(why);
    }
    for cs in status.container_statuses.iter().flatten() {
        if let Some(w) = cs.state.as_ref().and_then(|s| s.waiting.as_ref())
            && let Some(reason) = w.reason.as_deref()
            && IMAGE_ERRORS.contains(&reason)
        {
            let msg = w.message.as_deref().unwrap_or(reason);
            return PodState::ImageError(format!("{reason}: {msg}"));
        }
    }
    if phase == "Running" {
        return PodState::Running;
    }
    for c in status.conditions.iter().flatten() {
        if c.type_ == "PodScheduled"
            && c.status == "False"
            && c.reason.as_deref() == Some("Unschedulable")
        {
            return PodState::Unschedulable(c.message.clone().unwrap_or_default());
        }
    }
    PodState::Pending
}

/// The Pod phase as shown in an `Observation`.
pub fn phase_label(pod: &Pod) -> String {
    if pod.metadata.deletion_timestamp.is_some() {
        return "terminating".into();
    }
    match classify(pod) {
        PodState::Unschedulable(_) => "unschedulable".into(),
        PodState::ImageError(_) => "image_error".into(),
        _ => pod
            .status
            .as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Pending".into())
            .to_lowercase(),
    }
}

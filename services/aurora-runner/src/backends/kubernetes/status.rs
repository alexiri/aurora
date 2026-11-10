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
        // (FIX-025, C-581) exit code and last container state reason first.
        let container = status.container_statuses.iter().flatten().find_map(|cs| {
            let t = cs.state.as_ref().and_then(|s| s.terminated.as_ref())?;
            let mut s = format!("exit code {}", t.exit_code);
            if let Some(r) = &t.reason {
                s.push_str(&format!(" ({r})"));
            }
            if let Some(m) = t.message.as_deref().filter(|m| !m.trim().is_empty()) {
                s.push_str(&format!(": {m}"));
            }
            Some(s)
        });
        let why = container
            .or_else(|| status.message.clone())
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

/// (FIX-025, C-580) Progress phase and detail of a `Pending` pod: `scheduling` until a node is
/// assigned, then `creating` with the container's waiting reason (image pull, container
/// creation).
pub fn pending_progress(pod: &Pod) -> (&'static str, String) {
    let Some(status) = pod.status.as_ref() else {
        return ("scheduling", "waiting for the scheduler".into());
    };
    let scheduled = status
        .conditions
        .iter()
        .flatten()
        .any(|c| c.type_ == "PodScheduled" && c.status == "True");
    if !scheduled {
        let why = status
            .conditions
            .iter()
            .flatten()
            .find(|c| c.type_ == "PodScheduled")
            .and_then(|c| c.message.clone())
            .unwrap_or_else(|| "waiting for the scheduler".into());
        return ("scheduling", why);
    }
    for cs in status.container_statuses.iter().flatten() {
        if let Some(w) = cs.state.as_ref().and_then(|s| s.waiting.as_ref())
            && let Some(reason) = w.reason.as_deref()
        {
            let detail = match w.message.as_deref() {
                Some(m) if !m.is_empty() => format!("{reason}: {m}"),
                _ => reason.to_owned(),
            };
            return ("creating", detail);
        }
    }
    (
        "creating",
        "the container is being created (image pull, volume mounts)".into(),
    )
}

/// (FIX-025) The scheduler's message says the pod waits for an unbound PersistentVolumeClaim.
pub fn waits_for_unbound_pvc(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("unbound") && m.contains("persistentvolumeclaim")
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

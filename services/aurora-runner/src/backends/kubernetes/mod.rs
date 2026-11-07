//! Kubernetes backend (`11 §Backend kubernetes`): Pods, per-boot Secrets and home PVCs in the
//! sandbox namespace, via `kube` (rustls). This module and its children are the only place
//! that names Kubernetes.

mod objects;
mod status;

use std::time::Duration;

use async_trait::async_trait;
use aurora_config::RunnerConfig;
use aurora_protocol::runner::{
    HarnessId, HomeDurability, Observed, Range, ResourceRanges, RunnerCapabilities,
};
use chrono::{DateTime, Utc};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod, Secret, Service, Toleration};
use kube::api::{DeleteParams, ListParams, PostParams};
use kube::{Api, Client, ResourceExt};
use uuid::Uuid;

use crate::backend::{
    Backend, BackendError, ContainerRef, HoldCallback, LABEL_BOOT, LABEL_INSTALLATION,
    LABEL_MANAGED, LABEL_RUNNER, LABEL_SANDBOX, ManagedObject, ObjectKind, Observation,
    StartRequest,
};
use crate::runner::installation_fingerprint;

pub use objects::{
    PodConfig, boot_labels, boot_secret, boot_short, container_env, home_pvc, is_dns_label,
    pod_name, pod_spec, sanitize_dns, secret_name,
};
pub use status::{PodState, classify};

use objects::{LABEL_CREATED, LABEL_POD};

const DEFAULT_NAMESPACE: &str = "aurora-sandboxes";
const DEFAULT_GATEWAY_SERVICE: &str = "aurora-gateway";
const DEFAULT_RUNNER_SERVICE: &str = "aurora-runner";
const SERVICE_ACCOUNT_NAMESPACE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/namespace";
/// Grace for Pods of superseded boots and for orphan removal.
const CLEANUP_GRACE_SECS: u32 = 5;
const STALE_WAIT: Duration = Duration::from_secs(60);

/// Namespace of the core services: configured, else the runner pod's own, else `default`.
pub fn resolve_core_namespace(
    configured: Option<&str>,
    sa_namespace_file: &std::path::Path,
) -> String {
    configured
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            std::fs::read_to_string(sa_namespace_file)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "default".into())
}

#[derive(Debug, Clone)]
pub struct KubernetesSettings {
    /// Sandbox namespace.
    pub namespace: String,
    /// Namespace of the gateway / runner Services.
    pub core_namespace: String,
    pub gateway_service: String,
    pub runner_service: String,
    pub pod: PodConfig,
    pub max_sandboxes: u32,
    pub capacity_wait: Duration,
    /// Bound on scheduling-independent startup (image pull, container creation).
    pub start_timeout: Duration,
    pub poll: Duration,
}

impl KubernetesSettings {
    pub fn from_config(cfg: &RunnerConfig) -> Result<Self, String> {
        let r = &cfg.runner;
        let k = &r.k8s;
        let tolerations = k
            .tolerations
            .iter()
            .enumerate()
            .map(|(i, t)| {
                parse_toleration(t).map_err(|e| format!("runner.k8s.tolerations[{i}]: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            namespace: k
                .namespace
                .clone()
                .unwrap_or_else(|| DEFAULT_NAMESPACE.into()),
            core_namespace: resolve_core_namespace(
                k.core_namespace.as_deref(),
                std::path::Path::new(SERVICE_ACCOUNT_NAMESPACE),
            ),
            gateway_service: k
                .gateway_service
                .clone()
                .unwrap_or_else(|| DEFAULT_GATEWAY_SERVICE.into()),
            runner_service: k
                .runner_service
                .clone()
                .unwrap_or_else(|| DEFAULT_RUNNER_SERVICE.into()),
            pod: PodConfig {
                installation: installation_fingerprint(cfg.token.expose()),
                storage_class: k.storage_class.clone(),
                runtime_class: k.runtime_class.clone(),
                node_selector: k.node_selector.clone(),
                tolerations,
            },
            max_sandboxes: r.max_sandboxes,
            capacity_wait: Duration::from_secs(r.capacity_wait_secs),
            start_timeout: Duration::from_secs(300),
            poll: Duration::from_secs(2),
        })
    }
}

/// A TOML toleration table (`key`, `operator`, `value`, `effect`, `toleration_seconds`).
fn parse_toleration(t: &impl serde::Serialize) -> Result<Toleration, String> {
    let mut v = serde_json::to_value(t).map_err(|e| e.to_string())?;
    if let Some(o) = v.as_object_mut()
        && let Some(secs) = o.remove("toleration_seconds")
    {
        o.insert("tolerationSeconds".into(), secs);
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

pub struct KubernetesBackend {
    client: Client,
    s: KubernetesSettings,
}

fn other(e: impl std::fmt::Display) -> BackendError {
    BackendError::Other(e.to_string())
}

fn code(e: &kube::Error) -> Option<u16> {
    match e {
        kube::Error::Api(s) => Some(s.code),
        _ => None,
    }
}

/// Admission refused the Pod because the namespace quota is used up.
fn is_quota(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(s) if s.code == 403 && s.message.contains("exceeded quota"))
}

/// `Ok(())` for success or "already gone".
fn ignore_missing<T>(r: Result<T, kube::Error>) -> Result<(), BackendError> {
    match r {
        Err(e) if code(&e) != Some(404) => Err(other(e)),
        _ => Ok(()),
    }
}

fn selector(pairs: &[(&str, String)]) -> ListParams {
    let s = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    ListParams::default().labels(&s)
}

fn label<'a>(meta: &'a impl ResourceExt, key: &str) -> Option<&'a String> {
    meta.labels().get(key)
}

fn created_at(meta: &impl ResourceExt) -> DateTime<Utc> {
    label(meta, LABEL_CREATED)
        .and_then(|v| v.parse::<i64>().ok())
        .or_else(|| meta.creation_timestamp().map(|t| t.0.as_second()))
        .and_then(|t| DateTime::from_timestamp(t, 0))
        .unwrap_or_else(Utc::now)
}

/// Tracks a capacity wait across the admission and scheduling phases of one start.
struct CapacityWait {
    wait: Duration,
    reported: bool,
    since: Option<tokio::time::Instant>,
}

impl CapacityWait {
    fn new(wait: Duration) -> Self {
        Self {
            wait,
            reported: false,
            since: None,
        }
    }

    /// Called while blocked on capacity: reports the hold once, fails after the wait.
    fn blocked(&mut self, hold: &dyn HoldCallback, why: &str) -> Result<(), BackendError> {
        if self.wait.is_zero() {
            return Err(BackendError::Capacity(why.to_owned()));
        }
        if !self.reported {
            self.reported = true;
            hold.on_hold();
        }
        let since = *self.since.get_or_insert_with(tokio::time::Instant::now);
        if tokio::time::Instant::now() >= since + self.wait {
            return Err(BackendError::WakeCapacityTimeout);
        }
        Ok(())
    }

    fn unblocked(&mut self) {
        self.since = None;
    }
}

impl KubernetesBackend {
    /// Builds the client from the in-cluster service account and checks the sandbox namespace.
    pub async fn connect(s: KubernetesSettings) -> Result<Self, String> {
        let cfg = kube::Config::incluster().map_err(|e| {
            format!(
                "kubernetes backend needs in-cluster configuration \
                 (KUBERNETES_SERVICE_HOST and the service-account token): {e}"
            )
        })?;
        let client =
            Client::try_from(cfg).map_err(|e| format!("kubernetes client cannot be built: {e}"))?;
        let b = Self::with_client(client, s);
        b.pods()
            .list(&ListParams::default().limit(1))
            .await
            .map_err(|e| {
                format!(
                    "kubernetes API: cannot list pods in namespace `{}` (RBAC or namespace missing?): {e}",
                    b.s.namespace
                )
            })?;
        Ok(b)
    }

    /// For tests and embedding: use an already-built client.
    pub fn with_client(client: Client, s: KubernetesSettings) -> Self {
        Self { client, s }
    }

    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.s.namespace)
    }
    fn secrets(&self) -> Api<Secret> {
        Api::namespaced(self.client.clone(), &self.s.namespace)
    }
    fn pvcs(&self) -> Api<PersistentVolumeClaim> {
        Api::namespaced(self.client.clone(), &self.s.namespace)
    }
    fn services(&self) -> Api<Service> {
        Api::namespaced(self.client.clone(), &self.s.core_namespace)
    }

    async fn cluster_ip(&self, name: &str) -> Result<String, BackendError> {
        let svc = self.services().get(name).await.map_err(|e| {
            other(format!(
                "service `{name}` in namespace `{}` cannot be read: {e}",
                self.s.core_namespace
            ))
        })?;
        svc.spec
            .and_then(|s| s.cluster_ip)
            .filter(|ip| !ip.is_empty() && ip != "None")
            .ok_or_else(|| other(format!("service `{name}` has no ClusterIP")))
    }

    async fn delete_pod(&self, name: &str, grace: u32) -> Result<(), BackendError> {
        let dp = DeleteParams {
            grace_period_seconds: Some(grace),
            ..DeleteParams::default()
        };
        ignore_missing(self.pods().delete(name, &dp).await)
    }

    async fn wait_pod_gone(&self, name: &str, max: Duration) -> Result<(), BackendError> {
        let deadline = tokio::time::Instant::now() + max;
        loop {
            if self.pods().get_opt(name).await.map_err(other)?.is_none() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(other(format!("pod `{name}` is still terminating")));
            }
            tokio::time::sleep(self.s.poll).await;
        }
    }

    /// Deletes the boot Secrets matching `params` except the one named `keep`.
    async fn delete_secrets(
        &self,
        params: &ListParams,
        keep: Option<&str>,
    ) -> Result<(), BackendError> {
        for sec in self.secrets().list(params).await.map_err(other)? {
            let name = sec.name_any();
            if keep != Some(name.as_str()) {
                ignore_missing(self.secrets().delete(&name, &DeleteParams::default()).await)?;
            }
        }
        Ok(())
    }

    /// Removes a Pod and its boot Secret (the PVC stays).
    async fn remove_boot(&self, pod: &str, grace: u32, wait: Duration) -> Result<(), BackendError> {
        self.delete_pod(pod, grace).await?;
        let r = self.wait_pod_gone(pod, wait).await;
        // The Secret goes even when the Pod lingers: the env was resolved at container start.
        self.delete_secrets(&selector(&[(LABEL_POD, pod.to_owned())]), None)
            .await?;
        r
    }

    async fn active_other_pods(&self, sandbox_id: Uuid) -> Result<usize, BackendError> {
        let own = sandbox_id.to_string();
        let params = selector(&[(LABEL_INSTALLATION, self.s.pod.installation.clone())]);
        Ok(self
            .pods()
            .list(&params)
            .await
            .map_err(other)?
            .iter()
            .filter(|p| label(*p, LABEL_SANDBOX).is_none_or(|s| *s != own))
            .filter(|p| !matches!(classify(p), PodState::Terminated(_)))
            .count())
    }

    async fn ensure_secret(&self, secret: &Secret) -> Result<(), BackendError> {
        match self.secrets().create(&PostParams::default(), secret).await {
            Ok(_) => Ok(()),
            Err(e) if code(&e) == Some(409) => {
                // A leftover of an earlier attempt of this boot: replace it.
                let name = secret.name_any();
                ignore_missing(self.secrets().delete(&name, &DeleteParams::default()).await)?;
                self.secrets()
                    .create(&PostParams::default(), secret)
                    .await
                    .map(|_| ())
                    .map_err(other)
            }
            Err(e) => Err(other(e)),
        }
    }

    /// Creates the Pod, waiting while the namespace is at its limits.
    async fn create_pod(
        &self,
        pod: &Pod,
        sandbox_id: Uuid,
        hold: &dyn HoldCallback,
        cap: &mut CapacityWait,
    ) -> Result<(), BackendError> {
        loop {
            if self.active_other_pods(sandbox_id).await? >= self.s.max_sandboxes as usize {
                cap.blocked(
                    hold,
                    &format!("{} sandboxes already running", self.s.max_sandboxes),
                )?;
            } else {
                match self.pods().create(&PostParams::default(), pod).await {
                    Ok(_) => {
                        cap.unblocked();
                        return Ok(());
                    }
                    Err(e) if is_quota(&e) => cap.blocked(hold, "namespace quota exceeded")?,
                    Err(e) => return Err(other(e)),
                }
            }
            tokio::time::sleep(self.s.poll).await;
        }
    }

    /// Waits until the Pod runs. Failures remove the Pod and its Secret.
    async fn await_running(
        &self,
        name: &str,
        hold: &dyn HoldCallback,
        cap: &mut CapacityWait,
    ) -> Result<ContainerRef, BackendError> {
        let started = tokio::time::Instant::now();
        let outcome = loop {
            let pod = match self.pods().get_opt(name).await {
                Ok(Some(p)) => p,
                Ok(None) => break Err(other(format!("pod `{name}` disappeared while starting"))),
                Err(e) => break Err(other(e)),
            };
            match classify(&pod) {
                PodState::Running => break Ok(()),
                PodState::ImageError(m) => break Err(BackendError::ImageUnavailable(m)),
                PodState::Terminated(m) => {
                    break Err(other(format!("pod ended while starting: {m}")));
                }
                PodState::Unschedulable(m) => {
                    if let Err(e) = cap.blocked(hold, &format!("pod unschedulable: {m}")) {
                        break Err(e);
                    }
                }
                PodState::Pending => {
                    cap.unblocked();
                    if started.elapsed() >= self.s.start_timeout {
                        break Err(other("pod did not start in time"));
                    }
                }
            }
            tokio::time::sleep(self.s.poll).await;
        };
        match outcome {
            Ok(()) => Ok(ContainerRef(name.to_owned())),
            Err(e) => {
                let _ = self.remove_boot(name, 0, Duration::from_secs(10)).await;
                Err(e)
            }
        }
    }
}

#[async_trait]
impl Backend for KubernetesBackend {
    fn id(&self) -> &'static str {
        "kubernetes"
    }

    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            backend: "kubernetes".into(),
            home_durability: HomeDurability::Resident,
            resources: ResourceRanges {
                cpu: Range {
                    min: 250,
                    max: 8000,
                },
                memory_mib: Range {
                    min: 256,
                    max: 32768,
                },
                // Declared for profile validation; Kubernetes limits PIDs per node (kubelet
                // `podPidsLimit`), not per Pod.
                pids: Range { min: 64, max: 8192 },
                home_gib: Range { min: 1, max: 1000 },
            },
            max_sandboxes: self.s.max_sandboxes,
            // Both ids, as podman (docs/16 `harnesses`): the image decides whether the CLI exists.
            harnesses: vec![HarnessId::ClaudeCode, HarnessId::Fake],
            max_attachment_bytes: 25 * 1024 * 1024,
            // Filled in by the runner from `runner.image` at registration.
            image: None,
        }
    }

    async fn provision_home(
        &self,
        sandbox_id: Uuid,
        home_ref: &str,
        home_gib: u32,
    ) -> Result<(), BackendError> {
        if !is_dns_label(home_ref) {
            return Err(other("invalid home reference"));
        }
        let pvc = home_pvc(&self.s.pod, sandbox_id, home_ref, home_gib, Utc::now());
        match self.pvcs().create(&PostParams::default(), &pvc).await {
            Ok(_) => Ok(()),
            Err(e) if code(&e) == Some(409) => Ok(()),
            Err(e) => Err(other(e)),
        }
    }

    async fn start(
        &self,
        req: &StartRequest,
        hold: &dyn HoldCallback,
    ) -> Result<ContainerRef, BackendError> {
        if !is_dns_label(&req.spec.home_ref) {
            return Err(other("invalid home reference"));
        }
        let boot = req.boot_id.to_string();
        let sandbox = selector(&[(LABEL_SANDBOX, req.sandbox_id.to_string())]);

        // Idempotent per boot; other boots of this sandbox are removed first.
        let mut existing: Option<String> = None;
        for p in self.pods().list(&sandbox).await.map_err(other)? {
            let name = p.name_any();
            let same_boot = label(&p, LABEL_BOOT).is_some_and(|b| *b == boot);
            let usable = matches!(
                classify(&p),
                PodState::Running | PodState::Pending | PodState::Unschedulable(_)
            );
            if same_boot && usable && existing.is_none() {
                existing = Some(name);
            } else {
                self.remove_boot(&name, CLEANUP_GRACE_SECS, STALE_WAIT)
                    .await?;
            }
        }
        // Boot Secrets of other boots (also those whose Pod is already gone).
        let keep = secret_name(req.boot_id);
        self.delete_secrets(&sandbox, Some(&keep)).await?;

        let mut cap = CapacityWait::new(self.s.capacity_wait);
        if let Some(name) = existing {
            return self.await_running(&name, hold, &mut cap).await;
        }

        self.provision_home(req.sandbox_id, &req.spec.home_ref, req.spec.home_gib)
            .await?;
        let gateway_ip = self.cluster_ip(&self.s.gateway_service).await?;
        let runner_ip = self.cluster_ip(&self.s.runner_service).await?;
        let now = Utc::now();
        let name = pod_name(&objects::handle_for(req), req.boot_id);
        let pod = pod_spec(&self.s.pod, req, &name, &gateway_ip, &runner_ip, now);
        self.ensure_secret(&boot_secret(&self.s.pod, req, &name, now))
            .await?;
        if let Err(e) = self.create_pod(&pod, req.sandbox_id, hold, &mut cap).await {
            let _ = self
                .delete_secrets(&selector(&[(LABEL_POD, name)]), None)
                .await;
            return Err(e);
        }
        self.await_running(&name, hold, &mut cap).await
    }

    async fn stop(&self, container: &ContainerRef, grace: Duration) -> Result<(), BackendError> {
        let secs = u32::try_from(grace.as_secs()).unwrap_or(u32::MAX);
        let wait = grace + Duration::from_secs(10);
        self.remove_boot(&container.0, secs, wait).await
    }

    async fn inspect(&self, sandbox_id: Uuid) -> Result<Option<Observation>, BackendError> {
        let params = selector(&[(LABEL_SANDBOX, sandbox_id.to_string())]);
        let pods = self.pods().list(&params).await.map_err(other)?;
        // Prefer a live Pod; among equals the newest.
        let Some(pod) = pods.into_iter().max_by_key(|p| {
            (
                !matches!(classify(p), PodState::Terminated(_)),
                p.creation_timestamp().map(|t| t.0.as_second()),
            )
        }) else {
            return Ok(None);
        };
        let running = matches!(classify(&pod), PodState::Running);
        let limits = pod
            .spec
            .as_ref()
            .and_then(|s| s.containers.first())
            .and_then(|c| c.resources.as_ref())
            .and_then(|r| r.limits.as_ref());
        // Enforced limits as the cluster reports them.
        let observed = limits.map(|l| Observed {
            cpu_millis: l
                .get("cpu")
                .and_then(|q| objects::cpu_millis(&q.0))
                .unwrap_or(0),
            memory_mib: l
                .get("memory")
                .and_then(|q| objects::memory_mib(&q.0))
                .unwrap_or(0),
        });
        Ok(Some(Observation {
            container: ContainerRef(pod.name_any()),
            running,
            boot_id: label(&pod, LABEL_BOOT).and_then(|b| b.parse().ok()),
            phase: status::phase_label(&pod),
            observed: observed.filter(|_| running),
        }))
    }

    async fn list_managed(&self) -> Result<Vec<ManagedObject>, BackendError> {
        let params = selector(&[(LABEL_MANAGED, "true".into())]);
        let mut out = Vec::new();
        let managed = |meta: &dyn Meta, kind, reference: String, running| ManagedObject {
            kind,
            reference,
            sandbox_id: meta.get(LABEL_SANDBOX).and_then(|v| v.parse().ok()),
            boot_id: meta.get(LABEL_BOOT).and_then(|v| v.parse().ok()),
            runner: meta.get(LABEL_RUNNER),
            installation: meta.get(LABEL_INSTALLATION),
            created_at: meta.created(),
            running,
        };
        for p in self.pods().list(&params).await.map_err(other)? {
            let running = matches!(classify(&p), PodState::Running);
            out.push(managed(&p, ObjectKind::Container, p.name_any(), running));
        }
        for v in self.pvcs().list(&params).await.map_err(other)? {
            out.push(managed(&v, ObjectKind::Home, v.name_any(), false));
        }
        for s in self.secrets().list(&params).await.map_err(other)? {
            out.push(managed(&s, ObjectKind::Secret, s.name_any(), false));
        }
        Ok(out)
    }

    async fn destroy(&self, obj: &ManagedObject) -> Result<(), BackendError> {
        match obj.kind {
            ObjectKind::Container => {
                self.remove_boot(&obj.reference, CLEANUP_GRACE_SECS, STALE_WAIT)
                    .await
            }
            // The PVC delete is the home delete; the Pods using it are separate objects.
            ObjectKind::Home => ignore_missing(
                self.pvcs()
                    .delete(&obj.reference, &DeleteParams::default())
                    .await,
            ),
            ObjectKind::Secret => ignore_missing(
                self.secrets()
                    .delete(&obj.reference, &DeleteParams::default())
                    .await,
            ),
        }
    }
}

/// Label/creation access shared by the object kinds `list_managed` maps.
trait Meta {
    fn get(&self, key: &str) -> Option<String>;
    fn created(&self) -> DateTime<Utc>;
}

impl<T: ResourceExt> Meta for T {
    fn get(&self, key: &str) -> Option<String> {
        label(self, key).cloned()
    }
    fn created(&self) -> DateTime<Utc> {
        created_at(self)
    }
}

#[cfg(test)]
mod tests;

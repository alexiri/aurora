//! Podman backend (`11 §Backend podman`): Docker-compatible API over the podman socket.
//!
//! This is the only module that names a container runtime.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use async_trait::async_trait;
use aurora_config::RunnerConfig;
use aurora_protocol::runner::{
    HarnessId, HomeDurability, Observed, Range, ResourceRanges, RunnerCapabilities,
};
use bollard::Docker;
use bollard::errors::Error as DockerError;
use bollard::models::{
    ContainerCreateBody, HostConfig, RestartPolicy, RestartPolicyNameEnum, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListVolumesOptionsBuilder,
    RemoveContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use uuid::Uuid;

use super::podman_state::{ListedContainer, parse_inspect, parse_list};
use crate::backend::{
    Backend, BackendError, ContainerRef, HoldCallback, LABEL_BOOT, LABEL_INSTALLATION,
    LABEL_MANAGED, LABEL_RUNNER, LABEL_SANDBOX, ManagedObject, ObjectKind, Observation,
    StartRequest,
};
use crate::runner::installation_fingerprint;

const API_TIMEOUT_SECS: u64 = 120;
const LABEL_CREATED: &str = "aurora.created";
const SANDBOX_UID_GID: &str = "1000:1000";
const TMP_TMPFS: &str = "rw,nosuid,nodev,size=512m";
const RUN_TMPFS: &str = "rw,nosuid,nodev,noexec,size=16m,mode=1777,U";
/// Where the sandbox image keeps the supervisor (images/sandbox/Containerfile); the dev
/// bind-mount shadows it.
const SUPERVISOR_IMAGE_PATH: &str = "/usr/local/bin/aurora-supervisor";
const CAPACITY_POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct PodmanSettings {
    pub socket: Option<String>,
    pub network: String,
    /// Name -> IP entries injected as `/etc/hosts` lines.
    pub hosts: BTreeMap<String, String>,
    pub installation: String,
    pub allow_external_network: bool,
    /// OPS-018 dev only (`runner.dev_supervisor_binary`, validated by aurora-config).
    pub dev_supervisor_binary: Option<String>,
    pub max_sandboxes: u32,
    pub capacity_wait: Duration,
}

impl PodmanSettings {
    pub fn from_config(cfg: &RunnerConfig) -> Self {
        let r = &cfg.runner;
        Self {
            socket: r.podman_socket.clone(),
            network: r.sandbox_network.clone(),
            hosts: r.sandbox_hosts.clone(),
            installation: installation_fingerprint(cfg.token.expose()),
            allow_external_network: r.dev_allow_external_network,
            dev_supervisor_binary: r.dev_supervisor_binary.clone(),
            max_sandboxes: r.max_sandboxes,
            capacity_wait: Duration::from_secs(r.capacity_wait_secs),
        }
    }
}

pub struct PodmanBackend {
    docker: Docker,
    socket: String,
    s: PodmanSettings,
}

fn other(e: impl std::fmt::Display) -> BackendError {
    BackendError::Other(e.to_string())
}

fn is_status(e: &DockerError, code: u16) -> bool {
    matches!(e, DockerError::DockerResponseServerError { status_code, .. } if *status_code == code)
}

/// `Ok(())` for success or "already gone".
fn ignore_missing(r: Result<(), DockerError>) -> Result<(), BackendError> {
    match r {
        Err(e) if !is_status(&e, 404) => Err(other(e)),
        _ => Ok(()),
    }
}

fn default_socket() -> String {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(d) if !d.is_empty() => format!("{d}/podman/podman.sock"),
        _ => "/run/podman/podman.sock".into(),
    }
}

fn filter(label: &str) -> HashMap<&'static str, Vec<String>> {
    HashMap::from([("label", vec![label.to_owned()])])
}

fn valid_volume_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
fn percent_encode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn parse_time(s: Option<&str>) -> DateTime<Utc> {
    s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map_or_else(Utc::now, |t| t.with_timezone(&Utc))
}

/// Refuses a sandbox network that is not `internal` unless the dev escape hatch is set.
pub async fn verify_network(
    docker: &Docker,
    name: &str,
    allow_external: bool,
) -> Result<(), String> {
    let net = docker
        .inspect_network(
            name,
            None::<bollard::query_parameters::InspectNetworkOptions>,
        )
        .await
        .map_err(|e| format!("sandbox network `{name}` cannot be inspected: {e}"))?;
    if net.internal == Some(true) {
        return Ok(());
    }
    if allow_external {
        tracing::warn!(
            network = name,
            "runner.dev_allow_external_network is set: sandbox network is NOT internal; \
             sandboxes can reach external hosts (development only)"
        );
        return Ok(());
    }
    Err(format!(
        "sandbox network `{name}` is not internal; create it with \
         `--internal --disable-dns` (SEC-02) or set runner.dev_allow_external_network for development"
    ))
}

impl PodmanBackend {
    /// Connects to the socket, pings it and verifies the sandbox network.
    pub async fn connect(s: PodmanSettings) -> Result<Self, String> {
        let path = s.socket.clone().unwrap_or_else(default_socket);
        let docker =
            Docker::connect_with_unix(&path, API_TIMEOUT_SECS, bollard::API_DEFAULT_VERSION)
                .map_err(|e| format!("podman socket `{path}`: {e}"))?;
        docker
            .ping()
            .await
            .map_err(|e| format!("podman socket `{path}` does not answer: {e}"))?;
        verify_network(&docker, &s.network, s.allow_external_network).await?;
        if let Some(bin) = &s.dev_supervisor_binary {
            if !std::fs::metadata(bin).is_ok_and(|m| m.is_file()) {
                return Err(format!(
                    "runner.dev_supervisor_binary `{bin}` is not a file (build it: cargo build -p aurora-supervisor)"
                ));
            }
            tracing::warn!(
                path = %bin,
                "runner.dev_supervisor_binary is set: sandboxes run a host-built supervisor (dev only, never production)"
            );
        }
        Ok(Self {
            docker,
            socket: path,
            s,
        })
    }

    fn container_name(sandbox_id: Uuid) -> String {
        format!("aurora-sbx-{sandbox_id}")
    }

    fn labels(&self, req: &StartRequest) -> HashMap<String, String> {
        let mut l: HashMap<String, String> = req
            .spec
            .labels
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        l.insert(LABEL_MANAGED.into(), "true".into());
        l.insert(LABEL_INSTALLATION.into(), self.s.installation.clone());
        l.insert(LABEL_SANDBOX.into(), req.sandbox_id.to_string());
        l.insert(LABEL_BOOT.into(), req.boot_id.to_string());
        l.insert(LABEL_CREATED.into(), Utc::now().to_rfc3339());
        l
    }

    /// Non-secret environment (`11 §The boot document`); request entries win.
    fn environment(req: &StartRequest) -> Vec<String> {
        let proxy = "http://127.0.0.1:3128";
        let ca = "/run/aurora/ca.pem";
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in [
            ("TZ", "UTC"),
            ("HOME", "/workspace"),
            ("LANG", "C.UTF-8"),
            ("HTTPS_PROXY", proxy),
            ("HTTP_PROXY", proxy),
            ("https_proxy", proxy),
            ("http_proxy", proxy),
            ("NO_PROXY", "localhost,127.0.0.1"),
            ("no_proxy", "localhost,127.0.0.1"),
            ("SSL_CERT_FILE", ca),
            ("NODE_EXTRA_CA_CERTS", ca),
            ("REQUESTS_CA_BUNDLE", ca),
            ("CURL_CA_BUNDLE", ca),
            ("GIT_SSL_CAINFO", ca),
        ] {
            env.insert(k.into(), v.into());
        }
        env.insert("AURORA_SANDBOX_ID".into(), req.sandbox_id.to_string());
        for (k, v) in &req.spec.env {
            env.insert(k.clone(), v.clone());
        }
        // The boot document is delivered only through the container env (I-23).
        env.insert(
            "AURORA_BOOT".into(),
            req.spec.boot_secret.expose().to_owned(),
        );
        env.into_iter().map(|(k, v)| format!("{k}={v}")).collect()
    }

    /// The home volume, plus (dev only) the host-built supervisor, read-only.
    fn binds(&self, req: &StartRequest) -> Vec<String> {
        let mut binds = vec![format!("{}:/workspace", req.spec.home_ref)];
        if let Some(bin) = &self.s.dev_supervisor_binary {
            binds.push(format!("{bin}:{SUPERVISOR_IMAGE_PATH}:ro"));
        }
        binds
    }

    fn host_config(&self, req: &StartRequest) -> HostConfig {
        let res = &req.spec.resources;
        let memory = i64::from(res.memory_mib) * 1024 * 1024;
        HostConfig {
            network_mode: Some(self.s.network.clone()),
            extra_hosts: Some(
                self.s
                    .hosts
                    .iter()
                    .map(|(n, ip)| format!("{n}:{ip}"))
                    .collect(),
            ),
            binds: Some(self.binds(req)),
            readonly_rootfs: Some(true),
            tmpfs: Some(HashMap::from([
                ("/tmp".to_owned(), TMP_TMPFS.to_owned()),
                ("/run/aurora".to_owned(), RUN_TMPFS.to_owned()),
            ])),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges".into()]),
            privileged: Some(false),
            init: Some(false),
            pids_limit: Some(i64::from(res.pids)),
            memory: Some(memory),
            memory_swap: Some(memory),
            nano_cpus: Some(i64::from(res.cpu_millis) * 1_000_000),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::NO),
                maximum_retry_count: None,
            }),
            ..Default::default()
        }
    }

    /// Raw `GET` over the podman socket. State-bearing endpoints go through here so the
    /// status stays a string (see `podman_state`); bollard's typed models reject values
    /// such as podman 4.x's `stopped`.
    async fn raw_get(&self, path_and_query: &str) -> Result<(u16, Bytes), BackendError> {
        let call = async {
            let stream = tokio::net::UnixStream::connect(&self.socket)
                .await
                .map_err(other)?;
            let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
                .await
                .map_err(other)?;
            tokio::spawn(async move {
                let _ = conn.await;
            });
            let req = http::Request::builder()
                .method(http::Method::GET)
                .uri(path_and_query)
                .header(http::header::HOST, "d")
                .body(Empty::<Bytes>::new())
                .map_err(other)?;
            let resp = sender.send_request(req).await.map_err(other)?;
            let status = resp.status().as_u16();
            let body = resp.into_body().collect().await.map_err(other)?.to_bytes();
            Ok::<_, BackendError>((status, body))
        };
        tokio::time::timeout(Duration::from_secs(API_TIMEOUT_SECS), call)
            .await
            .map_err(|_| other("podman socket request timed out"))?
    }

    async fn list_containers(&self, label: &str) -> Result<Vec<ListedContainer>, BackendError> {
        let filters = serde_json::json!({ "label": [label] }).to_string();
        let (status, body) = self
            .raw_get(&format!(
                "/containers/json?all=true&filters={}",
                percent_encode(&filters)
            ))
            .await?;
        if status != 200 {
            return Err(other(format!(
                "list containers: HTTP {status}: {}",
                String::from_utf8_lossy(&body)
            )));
        }
        parse_list(&body).map_err(|e| other(format!("list containers: {e}")))
    }

    async fn running_managed(&self) -> Result<Vec<ListedContainer>, BackendError> {
        let label = format!("{LABEL_INSTALLATION}={}", self.s.installation);
        let all = self.list_containers(&label).await?;
        Ok(all.into_iter().filter(ListedContainer::running).collect())
    }

    async fn remove_container(&self, id: &str) -> Result<(), BackendError> {
        let opts = RemoveContainerOptionsBuilder::default().force(true).build();
        ignore_missing(self.docker.remove_container(id, Some(opts)).await)
    }

    async fn ensure_image(&self, image: &str) -> Result<(), BackendError> {
        if self.docker.inspect_image(image).await.is_ok() {
            return Ok(());
        }
        let opts = CreateImageOptionsBuilder::default()
            .from_image(image)
            .build();
        self.docker
            .create_image(Some(opts), None, None)
            .try_collect::<Vec<_>>()
            .await
            .map(|_| ())
            .map_err(|e| BackendError::ImageUnavailable(format!("{image}: {e}")))
    }

    /// Waits for a free slot; reports a hold, then fails after `capacity_wait`.
    async fn wait_for_capacity(
        &self,
        sandbox_id: Uuid,
        hold: &dyn HoldCallback,
    ) -> Result<(), BackendError> {
        let own = sandbox_id.to_string();
        let deadline = tokio::time::Instant::now() + self.s.capacity_wait;
        let mut held = false;
        loop {
            let others = self
                .running_managed()
                .await?
                .iter()
                .filter(|c| {
                    c.labels
                        .as_ref()
                        .and_then(|l| l.get(LABEL_SANDBOX))
                        .is_none_or(|s| *s != own)
                })
                .count();
            if others < self.s.max_sandboxes as usize {
                return Ok(());
            }
            if self.s.capacity_wait.is_zero() {
                return Err(BackendError::Capacity(format!(
                    "{} sandboxes already running",
                    self.s.max_sandboxes
                )));
            }
            if !held {
                held = true;
                hold.on_hold();
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(BackendError::WakeCapacityTimeout);
            }
            tokio::time::sleep(CAPACITY_POLL).await;
        }
    }
}

/// OCI label through which a sandbox image declares its capabilities (docs/11).
const CAPABILITIES_LABEL: &str = "org.aurora.capabilities";

/// Comma list → trimmed, de-duplicated, non-empty tokens.
fn parse_capabilities(label: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in label.unwrap_or_default().split(',') {
        let t = t.trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
    }
    out
}

#[async_trait]
impl Backend for PodmanBackend {
    fn id(&self) -> &'static str {
        "podman"
    }

    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            backend: "podman".into(),
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
                pids: Range { min: 64, max: 8192 },
                // Size limits are best-effort on podman.
                home_gib: Range { min: 1, max: 100 },
            },
            max_sandboxes: self.s.max_sandboxes,
            // The sandbox image decides whether the CLI is present; docs/16 lists both ids.
            harnesses: vec![HarnessId::ClaudeCode, HarnessId::Fake],
            max_attachment_bytes: 25 * 1024 * 1024,
            // Filled in by the runner from `runner.image` at registration.
            image: None,
            held_awake_ceiling: None,
            defaults: None,
            image_capabilities: vec![],
            // Set by the runner at registration when the stream listener opened (P4-PRV-2).
            previews: false,
            // (P4-PRV-2, `16 §W18`) Whether the stream listener speaks TLS; set by the runner
            // at registration from `runner.stream_tls` (the API derives the dial scheme).
            stream_tls: false,
        }
    }

    async fn image_capabilities(&self, image: &str) -> Vec<String> {
        if let Err(e) = self.ensure_image(image).await {
            tracing::warn!(error = %e, image, "cannot read image capabilities; reporting none");
            return Vec::new();
        }
        match self.docker.inspect_image(image).await {
            Ok(i) => parse_capabilities(
                i.config
                    .and_then(|c| c.labels)
                    .and_then(|l| l.get(CAPABILITIES_LABEL).cloned())
                    .as_deref(),
            ),
            Err(e) => {
                tracing::warn!(error = %e, image, "cannot inspect image; reporting no capabilities");
                Vec::new()
            }
        }
    }

    async fn image_harnesses(&self, image: &str) -> Option<Vec<HarnessId>> {
        self.ensure_image(image).await.ok()?;
        let i = self.docker.inspect_image(image).await.ok()?;
        let label = i
            .config
            .and_then(|c| c.labels)
            .and_then(|l| l.get(crate::backend::HARNESSES_LABEL).cloned());
        Some(crate::backend::parse_harnesses(label.as_deref()))
    }

    async fn provision_home(
        &self,
        sandbox_id: Uuid,
        home_ref: &str,
        _home_gib: u32,
    ) -> Result<(), BackendError> {
        if !valid_volume_name(home_ref) {
            return Err(BackendError::Other("invalid home reference".into()));
        }
        if self.docker.inspect_volume(home_ref).await.is_ok() {
            return Ok(());
        }
        let labels = HashMap::from([
            (LABEL_MANAGED.to_owned(), "true".to_owned()),
            (LABEL_INSTALLATION.to_owned(), self.s.installation.clone()),
            (LABEL_SANDBOX.to_owned(), sandbox_id.to_string()),
            (LABEL_CREATED.to_owned(), Utc::now().to_rfc3339()),
        ]);
        self.docker
            .create_volume(VolumeCreateRequest {
                name: Some(home_ref.to_owned()),
                labels: Some(labels),
                ..Default::default()
            })
            .await
            .map(|_| ())
            .map_err(other)
    }

    async fn start(
        &self,
        req: &StartRequest,
        hold: &dyn HoldCallback,
    ) -> Result<ContainerRef, BackendError> {
        if !valid_volume_name(&req.spec.home_ref) {
            return Err(BackendError::Other("invalid home reference".into()));
        }
        // Idempotent per boot; other boots of this sandbox are removed first.
        let sandbox_label = format!("{LABEL_SANDBOX}={}", req.sandbox_id);
        for c in self.list_containers(&sandbox_label).await? {
            let Some(id) = c.id.as_deref() else { continue };
            let boot = c.labels.as_ref().and_then(|l| l.get(LABEL_BOOT));
            let running = c.running();
            if running && boot.is_some_and(|b| *b == req.boot_id.to_string()) {
                return Ok(ContainerRef(id.to_owned()));
            }
            self.remove_container(id).await?;
        }
        self.ensure_image(&req.spec.image).await?;
        self.provision_home(req.sandbox_id, &req.spec.home_ref, req.spec.home_gib)
            .await?;
        self.wait_for_capacity(req.sandbox_id, hold).await?;

        let body = ContainerCreateBody {
            image: Some(req.spec.image.clone()),
            user: Some(SANDBOX_UID_GID.into()),
            env: Some(Self::environment(req)),
            labels: Some(self.labels(req)),
            host_config: Some(self.host_config(req)),
            ..Default::default()
        };
        let opts = CreateContainerOptionsBuilder::default()
            .name(&Self::container_name(req.sandbox_id))
            .build();
        let created = self
            .docker
            .create_container(Some(opts), body)
            .await
            .map_err(other)?;
        if let Err(e) = self
            .docker
            .start_container(
                &created.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
        {
            let _ = self.remove_container(&created.id).await;
            return Err(other(e));
        }
        Ok(ContainerRef(created.id))
    }

    async fn stop(&self, container: &ContainerRef, grace: Duration) -> Result<(), BackendError> {
        let opts = StopContainerOptionsBuilder::default()
            .t(i32::try_from(grace.as_secs()).unwrap_or(i32::MAX))
            .build();
        match self.docker.stop_container(&container.0, Some(opts)).await {
            // 304: already stopped.
            Err(e) if !is_status(&e, 404) && !is_status(&e, 304) => return Err(other(e)),
            _ => {}
        }
        self.remove_container(&container.0).await
    }

    async fn inspect(&self, sandbox_id: Uuid) -> Result<Option<Observation>, BackendError> {
        let label = format!("{LABEL_SANDBOX}={sandbox_id}");
        let Some(id) = self
            .list_containers(&label)
            .await?
            .into_iter()
            .find_map(|c| c.id)
        else {
            return Ok(None);
        };
        let (status, body) = self.raw_get(&format!("/containers/{id}/json")).await?;
        match status {
            200 => {}
            404 => return Ok(None),
            _ => {
                return Err(other(format!(
                    "inspect container: HTTP {status}: {}",
                    String::from_utf8_lossy(&body)
                )));
            }
        }
        let c = parse_inspect(&body).map_err(|e| other(format!("inspect container: {e}")))?;
        let running = c.state.is_running();
        let boot_id = c.labels.get(LABEL_BOOT).and_then(|b| b.parse().ok());
        // Enforced limits as the substrate reports them.
        let observed = Observed {
            cpu_millis: u32::try_from(c.nano_cpus / 1_000_000).unwrap_or(u32::MAX),
            memory_mib: u32::try_from(c.memory / (1024 * 1024)).unwrap_or(u32::MAX),
        };
        Ok(Some(Observation {
            container: ContainerRef(id),
            running,
            boot_id,
            phase: c.phase,
            observed: running.then_some(observed),
        }))
    }

    async fn list_managed(&self) -> Result<Vec<ManagedObject>, BackendError> {
        let label = format!("{LABEL_MANAGED}=true");
        let mut out = Vec::new();
        for c in self.list_containers(&label).await? {
            let Some(id) = c.id.clone() else { continue };
            let running = c.running();
            let l = c.labels.unwrap_or_default();
            out.push(ManagedObject {
                kind: ObjectKind::Container,
                reference: id,
                sandbox_id: l.get(LABEL_SANDBOX).and_then(|v| v.parse().ok()),
                boot_id: l.get(LABEL_BOOT).and_then(|v| v.parse().ok()),
                runner: l.get(LABEL_RUNNER).cloned(),
                installation: l.get(LABEL_INSTALLATION).cloned(),
                created_at: c
                    .created
                    .and_then(|t| DateTime::from_timestamp(t, 0))
                    .unwrap_or_else(Utc::now),
                running,
            });
        }
        let opts = ListVolumesOptionsBuilder::new()
            .filters(&filter(&label))
            .build();
        let vols = self.docker.list_volumes(Some(opts)).await.map_err(other)?;
        for v in vols.volumes.unwrap_or_default() {
            let l = v.labels;
            out.push(ManagedObject {
                kind: ObjectKind::Home,
                reference: v.name,
                sandbox_id: l.get(LABEL_SANDBOX).and_then(|v| v.parse().ok()),
                boot_id: None,
                runner: l.get(LABEL_RUNNER).cloned(),
                installation: l.get(LABEL_INSTALLATION).cloned(),
                created_at: parse_time(v.created_at.as_deref()),
                running: false,
            });
        }
        Ok(out)
    }

    async fn destroy(&self, obj: &ManagedObject) -> Result<(), BackendError> {
        match obj.kind {
            ObjectKind::Container => self.remove_container(&obj.reference).await,
            ObjectKind::Home => {
                let opts = bollard::query_parameters::RemoveVolumeOptionsBuilder::new()
                    .force(true)
                    .build();
                ignore_missing(self.docker.remove_volume(&obj.reference, Some(opts)).await)
            }
            // Secrets are never materialised by this backend.
            ObjectKind::Secret => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_label_is_parsed_fail_closed() {
        assert!(parse_capabilities(None).is_empty());
        assert!(parse_capabilities(Some("")).is_empty());
        assert_eq!(parse_capabilities(Some("browser")), ["browser"]);
        assert_eq!(
            parse_capabilities(Some(" browser, ,browser,future ")),
            ["browser", "future"]
        );
    }

    #[test]
    fn harnesses_label_is_parsed_and_defaults_for_old_images() {
        use crate::backend::parse_harnesses;
        assert_eq!(
            parse_harnesses(None),
            [HarnessId::ClaudeCode, HarnessId::Fake]
        );
        assert!(parse_harnesses(Some("")).is_empty());
        assert_eq!(
            parse_harnesses(Some(" claude-code, codex ,codex,fake")),
            [HarnessId::ClaudeCode, HarnessId::Codex, HarnessId::Fake]
        );
    }

    #[test]
    fn volume_names_are_restricted() {
        assert!(valid_volume_name("aurora-home-0192f1c0"));
        assert!(!valid_volume_name(""));
        assert!(!valid_volume_name("a/b"));
        assert!(!valid_volume_name("a:b"));
        assert!(!valid_volume_name("../x"));
    }

    /// SEC-26 / SEC-34: the rendered container spec is hardened (no live podman needed).
    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn rendered_spec_is_hardened() {
        use aurora_protocol::common::SecretString;
        use aurora_protocol::runner::{SandboxResources, SandboxSpec};
        let s = PodmanSettings {
            socket: Some("/dev/null".into()),
            network: "aurora-sandbox".into(),
            hosts: BTreeMap::from([("aurora-gateway".into(), "10.89.250.2".into())]),
            installation: "t".into(),
            allow_external_network: false,
            dev_supervisor_binary: None,
            max_sandboxes: 4,
            capacity_wait: Duration::ZERO,
        };
        // Connecting is lazy: building the client needs no daemon.
        let docker =
            Docker::connect_with_unix("/dev/null", 5, bollard::API_DEFAULT_VERSION).unwrap();
        let b = PodmanBackend {
            docker,
            socket: "/dev/null".into(),
            s,
        };
        let req = StartRequest {
            sandbox_id: Uuid::nil(),
            boot_id: Uuid::nil(),
            spec: SandboxSpec {
                image: "img".into(),
                env: vec![("AURORA_AGENT_HANDLE".into(), "probe".into())],
                boot_secret: SecretString::new("BOOTSECRETVALUE"),
                home_ref: "aurora-home-x".into(),
                home_gib: 1,
                resources: SandboxResources {
                    cpu_millis: 500,
                    memory_mib: 512,
                    pids: 256,
                },
                labels: BTreeMap::new(),
            },
        };
        let h = b.host_config(&req);
        assert_eq!(h.cap_drop, Some(vec!["ALL".to_owned()]));
        assert!(h.cap_add.is_none());
        assert_eq!(h.security_opt, Some(vec!["no-new-privileges".to_owned()]));
        assert_eq!(h.privileged, Some(false));
        assert_eq!(h.readonly_rootfs, Some(true));
        assert_eq!(h.memory, Some(512 * 1024 * 1024));
        assert_eq!(h.memory_swap, h.memory);
        assert_eq!(h.pids_limit, Some(256));
        assert_eq!(h.nano_cpus, Some(500_000_000));
        assert_eq!(h.network_mode.as_deref(), Some("aurora-sandbox"));
        // No host mounts except the named home volume.
        assert_eq!(h.binds, Some(vec!["aurora-home-x:/workspace".to_owned()]));
        // Dev supervisor mount: absent by default, read-only and exact when configured.
        let mut dev = b.s.clone();
        dev.dev_supervisor_binary = Some("/h/target/debug/aurora-supervisor".into());
        let b2 = PodmanBackend {
            docker: b.docker.clone(),
            socket: b.socket.clone(),
            s: dev,
        };
        assert_eq!(
            b2.host_config(&req).binds,
            Some(vec![
                "aurora-home-x:/workspace".to_owned(),
                "/h/target/debug/aurora-supervisor:/usr/local/bin/aurora-supervisor:ro".to_owned(),
            ])
        );
        assert_ne!(h.pid_mode.as_deref(), Some("host"));
        assert_ne!(h.ipc_mode.as_deref(), Some("host"));
        assert_ne!(SANDBOX_UID_GID, "0:0");
        assert!(!SANDBOX_UID_GID.starts_with("0:") && !SANDBOX_UID_GID.starts_with("root"));
    }
}

//! Pure builders for the Kubernetes objects of one sandbox (`11 §Backend kubernetes`).
//!
//! Nothing here talks to the API server, so every hardening field is unit-testable.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use k8s_openapi::api::core::v1::{
    Capabilities, Container, EmptyDirVolumeSource, EnvVar, EnvVarSource, HostAlias,
    LocalObjectReference, PersistentVolumeClaim, PersistentVolumeClaimSpec,
    PersistentVolumeClaimVolumeSource, Pod, PodDNSConfig, PodSecurityContext, PodSpec,
    ResourceRequirements, SeccompProfile, Secret, SecretKeySelector, SecurityContext, Toleration,
    Volume, VolumeMount, VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use uuid::Uuid;

use crate::backend::{LABEL_BOOT, LABEL_INSTALLATION, LABEL_MANAGED, LABEL_SANDBOX, StartRequest};

/// `aurora.created` label value: unix seconds (label values cannot hold RFC 3339).
pub const LABEL_CREATED: &str = "aurora.created";
/// On a boot Secret: the name of the Pod it belongs to (found even after the Pod is gone).
pub const LABEL_POD: &str = "aurora.pod";
/// Secret key (and container env name) of the boot document.
pub const BOOT_KEY: &str = "AURORA_BOOT";
pub const SANDBOX_UID: i64 = 1000;
pub const CONTAINER_NAME: &str = "sandbox";
/// Hostnames the boot document uses for the platform services (`11 §The boot document`).
pub const GATEWAY_HOST: &str = "aurora-gateway";
pub const RUNNER_HOST: &str = "aurora-runner";
/// The runner's supervisor port the sandbox connects to (decision I-07).
pub const SUPERVISOR_PORT: u16 = 4105;
const TMP_SIZE: &str = "512Mi";
const RUN_SIZE: &str = "16Mi";
const DNS_LABEL_MAX: usize = 63;

/// Lowercases and maps everything outside `[a-z0-9]` to `-`; collapses and trims dashes.
pub fn sanitize_dns(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Whether `s` is a valid DNS-1123 label (what `home_ref` must be to name a PVC).
pub fn is_dns_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= DNS_LABEL_MAX
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Last 8 hex characters of the boot id (the leading ones of a UUIDv7 are a timestamp).
pub fn boot_short(boot: Uuid) -> String {
    let s = boot.simple().to_string();
    s[s.len() - 8..].to_owned()
}

/// `sbx-<handle>-<boot short id>`, always a valid DNS-1123 label of at most 63 chars.
pub fn pod_name(handle: &str, boot: Uuid) -> String {
    let short = boot_short(boot);
    let mut h = sanitize_dns(handle);
    if h.is_empty() {
        h = "agent".into();
    }
    // "sbx-" + handle + "-" + 8
    h.truncate(DNS_LABEL_MAX - "sbx-".len() - 1 - short.len());
    while h.ends_with('-') {
        h.pop();
    }
    if h.is_empty() {
        h = "agent".into();
    }
    format!("sbx-{h}-{short}")
}

pub fn secret_name(boot: Uuid) -> String {
    format!("boot-{boot}")
}

/// Handle used in the Pod name: `AURORA_AGENT_HANDLE` from the request env when present,
/// else the leading sandbox id characters.
pub fn handle_for(req: &StartRequest) -> String {
    req.spec
        .env
        .iter()
        .find(|(k, _)| k == "AURORA_AGENT_HANDLE")
        .map(|(_, v)| v.clone())
        .filter(|v| !sanitize_dns(v).is_empty())
        .unwrap_or_else(|| req.sandbox_id.simple().to_string()[..8].to_owned())
}

/// What the object builders need from the backend configuration.
#[derive(Debug, Clone)]
pub struct PodConfig {
    pub installation: String,
    pub storage_class: Option<String>,
    pub runtime_class: Option<String>,
    pub node_selector: BTreeMap<String, String>,
    pub tolerations: Vec<Toleration>,
    /// Secret names (in the sandbox namespace) for `spec.imagePullSecrets`. Never read here.
    pub image_pull_secrets: Vec<String>,
}

fn created_label(now: DateTime<Utc>) -> String {
    now.timestamp().max(0).to_string()
}

/// Labels on every object of a boot; the request's labels come first and cannot override ours.
pub fn boot_labels(
    cfg: &PodConfig,
    req: &StartRequest,
    now: DateTime<Utc>,
) -> BTreeMap<String, String> {
    let mut l: BTreeMap<String, String> = req.spec.labels.clone();
    l.insert(LABEL_MANAGED.into(), "true".into());
    l.insert(LABEL_INSTALLATION.into(), cfg.installation.clone());
    l.insert(LABEL_SANDBOX.into(), req.sandbox_id.to_string());
    l.insert(LABEL_BOOT.into(), req.boot_id.to_string());
    l.insert(LABEL_CREATED.into(), created_label(now));
    l
}

/// The home PVC (`home-<sandbox_id>`), `ReadWriteOnce`.
pub fn home_pvc(
    cfg: &PodConfig,
    sandbox_id: Uuid,
    name: &str,
    home_gib: u32,
    now: DateTime<Utc>,
) -> PersistentVolumeClaim {
    let labels = BTreeMap::from([
        (LABEL_MANAGED.to_owned(), "true".to_owned()),
        (LABEL_INSTALLATION.to_owned(), cfg.installation.clone()),
        (LABEL_SANDBOX.to_owned(), sandbox_id.to_string()),
        (LABEL_CREATED.to_owned(), created_label(now)),
    ]);
    PersistentVolumeClaim {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteOnce".into()]),
            storage_class_name: cfg.storage_class.clone(),
            resources: Some(VolumeResourceRequirements {
                requests: Some(BTreeMap::from([(
                    "storage".to_owned(),
                    Quantity(format!("{home_gib}Gi")),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The per-boot Secret holding the boot document. The only place the document is written.
pub fn boot_secret(cfg: &PodConfig, req: &StartRequest, pod: &str, now: DateTime<Utc>) -> Secret {
    let mut labels = boot_labels(cfg, req, now);
    labels.insert(LABEL_POD.into(), pod.to_owned());
    Secret {
        metadata: ObjectMeta {
            name: Some(secret_name(req.boot_id)),
            labels: Some(labels),
            ..Default::default()
        },
        type_: Some("Opaque".into()),
        string_data: Some(BTreeMap::from([(
            BOOT_KEY.to_owned(),
            req.spec.boot_secret.expose().to_owned(),
        )])),
        ..Default::default()
    }
}

fn env_var(name: &str, value: &str) -> EnvVar {
    EnvVar {
        name: name.to_owned(),
        value: Some(value.to_owned()),
        value_from: None,
    }
}

/// Non-secret container environment (`11 §The boot document`) plus `AURORA_BOOT` by reference.
/// Request entries override defaults, except `AURORA_BOOT`, which can never be a literal.
pub fn container_env(req: &StartRequest) -> Vec<EnvVar> {
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
    env.remove(BOOT_KEY);
    let mut out: Vec<EnvVar> = env.iter().map(|(k, v)| env_var(k, v)).collect();
    out.push(EnvVar {
        name: BOOT_KEY.into(),
        value: None,
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: secret_name(req.boot_id),
                key: BOOT_KEY.into(),
                optional: Some(false),
            }),
            ..Default::default()
        }),
    });
    out
}

fn memory_empty_dir(size: &str) -> EmptyDirVolumeSource {
    EmptyDirVolumeSource {
        medium: Some("Memory".into()),
        size_limit: Some(Quantity(size.into())),
    }
}

fn runtime_default_seccomp() -> SeccompProfile {
    SeccompProfile {
        type_: "RuntimeDefault".into(),
        localhost_profile: None,
    }
}

/// The sandbox Pod. `gateway_ip` / `runner_ip` are the Services' ClusterIPs, looked up at spawn.
pub fn pod_spec(
    cfg: &PodConfig,
    req: &StartRequest,
    name: &str,
    gateway_ip: &str,
    runner_ip: &str,
    now: DateTime<Utc>,
) -> Pod {
    let res = &req.spec.resources;
    let quantities = BTreeMap::from([
        ("cpu".to_owned(), Quantity(format!("{}m", res.cpu_millis))),
        (
            "memory".to_owned(),
            Quantity(format!("{}Mi", res.memory_mib)),
        ),
    ]);
    let container = Container {
        name: CONTAINER_NAME.into(),
        image: Some(req.spec.image.clone()),
        env: Some(container_env(req)),
        resources: Some(ResourceRequirements {
            requests: Some(quantities.clone()),
            limits: Some(quantities),
            ..Default::default()
        }),
        security_context: Some(SecurityContext {
            run_as_user: Some(SANDBOX_UID),
            run_as_group: Some(SANDBOX_UID),
            run_as_non_root: Some(true),
            allow_privilege_escalation: Some(false),
            privileged: Some(false),
            read_only_root_filesystem: Some(true),
            capabilities: Some(Capabilities {
                drop: Some(vec!["ALL".into()]),
                add: None,
            }),
            seccomp_profile: Some(runtime_default_seccomp()),
            ..Default::default()
        }),
        volume_mounts: Some(vec![
            VolumeMount {
                name: "home".into(),
                mount_path: "/workspace".into(),
                ..Default::default()
            },
            VolumeMount {
                name: "tmp".into(),
                mount_path: "/tmp".into(),
                ..Default::default()
            },
            VolumeMount {
                name: "run-aurora".into(),
                mount_path: "/run/aurora".into(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    Pod {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            labels: Some(boot_labels(cfg, req, now)),
            ..Default::default()
        },
        spec: Some(PodSpec {
            restart_policy: Some("Never".into()),
            automount_service_account_token: Some(false),
            enable_service_links: Some(false),
            dns_policy: Some("None".into()),
            dns_config: Some(PodDNSConfig {
                nameservers: Some(vec!["127.0.0.1".into()]),
                ..Default::default()
            }),
            host_aliases: Some(vec![
                HostAlias {
                    ip: gateway_ip.to_owned(),
                    hostnames: Some(vec![GATEWAY_HOST.into()]),
                },
                HostAlias {
                    ip: runner_ip.to_owned(),
                    hostnames: Some(vec![RUNNER_HOST.into()]),
                },
            ]),
            security_context: Some(PodSecurityContext {
                run_as_user: Some(SANDBOX_UID),
                run_as_group: Some(SANDBOX_UID),
                fs_group: Some(SANDBOX_UID),
                run_as_non_root: Some(true),
                seccomp_profile: Some(runtime_default_seccomp()),
                ..Default::default()
            }),
            runtime_class_name: cfg.runtime_class.clone(),
            node_selector: (!cfg.node_selector.is_empty()).then(|| cfg.node_selector.clone()),
            tolerations: (!cfg.tolerations.is_empty()).then(|| cfg.tolerations.clone()),
            image_pull_secrets: (!cfg.image_pull_secrets.is_empty()).then(|| {
                cfg.image_pull_secrets
                    .iter()
                    .map(|n| LocalObjectReference { name: n.clone() })
                    .collect()
            }),
            containers: vec![container],
            volumes: Some(vec![
                Volume {
                    name: "home".into(),
                    persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                        claim_name: req.spec.home_ref.clone(),
                        read_only: Some(false),
                    }),
                    ..Default::default()
                },
                Volume {
                    name: "tmp".into(),
                    empty_dir: Some(memory_empty_dir(TMP_SIZE)),
                    ..Default::default()
                },
                Volume {
                    name: "run-aurora".into(),
                    empty_dir: Some(memory_empty_dir(RUN_SIZE)),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }),
        status: None,
    }
}

/// Parses `Quantity` strings this backend writes or a cluster normalises them to.
/// Returns `None` for formats it does not understand.
pub fn cpu_millis(q: &str) -> Option<u32> {
    if let Some(m) = q.strip_suffix('m') {
        return m.parse::<u32>().ok();
    }
    q.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| (v * 1000.0).round().min(f64::from(u32::MAX)) as u32)
}

pub fn memory_mib(q: &str) -> Option<u32> {
    const UNITS: [(&str, u64); 4] = [
        ("Ki", 1 << 10),
        ("Mi", 1 << 20),
        ("Gi", 1 << 30),
        ("Ti", 1 << 40),
    ];
    let bytes = UNITS
        .iter()
        .find_map(|(suffix, mult)| {
            q.strip_suffix(suffix)
                .and_then(|n| n.parse::<u64>().ok())
                .map(|n| n.saturating_mul(*mult))
        })
        .or_else(|| q.parse::<u64>().ok())?;
    u32::try_from(bytes / (1 << 20)).ok()
}

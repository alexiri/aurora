#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use aurora_protocol::runner::{SandboxResources, SandboxSpec};
use chrono::Utc;
use k8s_openapi::api::core::v1::{Pod, PodCondition, PodStatus};
use serde_json::json;
use uuid::Uuid;

use super::objects::{self, *};
use super::status::{PodState, classify, phase_label};
use super::*;
use crate::backend::StartRequest;

pub(crate) const BOOT_DOC: &str = "eyJ2IjoxLCJzZWNyZXQiOiJTVVBFUi1TRUNSRVQifQ";

fn cfg() -> PodConfig {
    PodConfig {
        installation: "0123456789abcdef".into(),
        storage_class: Some("fast".into()),
        runtime_class: Some("gvisor".into()),
        node_selector: BTreeMap::from([("pool".into(), "sandbox".into())]),
        tolerations: vec![
            parse_toleration(&toml_table(
                r#"key = "sandbox"
operator = "Exists"
effect = "NoSchedule"
toleration_seconds = 30"#,
            ))
            .unwrap(),
        ],
    }
}

fn toml_table(s: &str) -> serde_json::Value {
    // Same shape `toml::Table` serialises to.
    let mut m = serde_json::Map::new();
    for line in s.lines() {
        let (k, v) = line.split_once(" = ").unwrap();
        let v = v.trim();
        let val = v
            .trim_matches('"')
            .parse::<i64>()
            .map_or_else(|_| json!(v.trim_matches('"')), |n| json!(n));
        m.insert(k.into(), val);
    }
    serde_json::Value::Object(m)
}

fn req() -> StartRequest {
    StartRequest {
        sandbox_id: Uuid::parse_str("0192f1c0-aaaa-7bbb-8ccc-0123456789ab").unwrap(),
        boot_id: Uuid::parse_str("0192f1c0-1111-7222-8333-abcdef012345").unwrap(),
        spec: SandboxSpec {
            image: "registry.example/aurora-sandbox:1".into(),
            env: vec![
                ("AURORA_AGENT_HANDLE".into(), "Daneel".into()),
                ("APP_FLAG".into(), "1".into()),
                // Must never become a literal.
                ("AURORA_BOOT".into(), "literal-must-be-dropped".into()),
            ],
            boot_secret: aurora_protocol::common::SecretString::new(BOOT_DOC),
            home_ref: "home-0192f1c0-aaaa-7bbb-8ccc-0123456789ab".into(),
            home_gib: 7,
            resources: SandboxResources {
                cpu_millis: 1500,
                memory_mib: 2048,
                pids: 512,
            },
            labels: BTreeMap::from([
                ("aurora.runner".into(), "r-1".into()),
                // A request cannot override the installation label.
                ("aurora.installation".into(), "forged".into()),
            ]),
        },
    }
}

fn pod() -> Pod {
    pod_spec(
        &cfg(),
        &req(),
        "sbx-daneel-abcdef01",
        "10.0.0.1",
        "10.0.0.2",
        Utc::now(),
    )
}

#[test]
fn pod_hardening_fields() {
    let p = pod();
    let spec = p.spec.as_ref().unwrap();
    assert_eq!(spec.restart_policy.as_deref(), Some("Never"));
    assert_eq!(spec.automount_service_account_token, Some(false));
    assert_eq!(spec.enable_service_links, Some(false));
    assert_eq!(spec.dns_policy.as_deref(), Some("None"));
    assert_eq!(
        spec.dns_config.as_ref().unwrap().nameservers,
        Some(vec!["127.0.0.1".to_owned()])
    );
    let aliases = spec.host_aliases.as_ref().unwrap();
    assert_eq!(aliases.len(), 2);
    assert_eq!(aliases[0].ip, "10.0.0.1");
    assert_eq!(
        aliases[0].hostnames,
        Some(vec!["aurora-gateway".to_owned()])
    );
    assert_eq!(aliases[1].ip, "10.0.0.2");
    assert_eq!(aliases[1].hostnames, Some(vec!["aurora-runner".to_owned()]));

    let psc = spec.security_context.as_ref().unwrap();
    assert_eq!(psc.run_as_user, Some(1000));
    assert_eq!(psc.run_as_group, Some(1000));
    assert_eq!(psc.fs_group, Some(1000));
    assert_eq!(psc.run_as_non_root, Some(true));
    assert_eq!(
        psc.seccomp_profile.as_ref().unwrap().type_,
        "RuntimeDefault"
    );

    assert_eq!(spec.containers.len(), 1);
    let c = &spec.containers[0];
    let sc = c.security_context.as_ref().unwrap();
    assert_eq!(sc.run_as_user, Some(1000));
    assert_eq!(sc.run_as_group, Some(1000));
    assert_eq!(sc.run_as_non_root, Some(true));
    assert_eq!(sc.allow_privilege_escalation, Some(false));
    assert_eq!(sc.privileged, Some(false));
    assert_eq!(sc.read_only_root_filesystem, Some(true));
    assert_eq!(
        sc.capabilities.as_ref().unwrap().drop,
        Some(vec!["ALL".to_owned()])
    );
    assert!(sc.capabilities.as_ref().unwrap().add.is_none());
    assert_eq!(sc.seccomp_profile.as_ref().unwrap().type_, "RuntimeDefault");

    // No host access of any kind.
    assert_ne!(spec.host_network, Some(true));
    assert_ne!(spec.host_pid, Some(true));
    assert_ne!(spec.host_ipc, Some(true));
    assert!(c.ports.is_none());
}

#[test]
fn pod_scheduling_and_resources() {
    let p = pod();
    let spec = p.spec.as_ref().unwrap();
    assert_eq!(spec.runtime_class_name.as_deref(), Some("gvisor"));
    assert_eq!(
        spec.node_selector
            .as_ref()
            .unwrap()
            .get("pool")
            .map(String::as_str),
        Some("sandbox")
    );
    let t = &spec.tolerations.as_ref().unwrap()[0];
    assert_eq!(t.key.as_deref(), Some("sandbox"));
    assert_eq!(t.operator.as_deref(), Some("Exists"));
    assert_eq!(t.effect.as_deref(), Some("NoSchedule"));
    assert_eq!(t.toleration_seconds, Some(30));

    let r = spec.containers[0].resources.as_ref().unwrap();
    assert_eq!(r.requests, r.limits, "requests = limits");
    let l = r.limits.as_ref().unwrap();
    assert_eq!(l["cpu"].0, "1500m");
    assert_eq!(l["memory"].0, "2048Mi");
    assert_eq!(
        spec.containers[0].image.as_deref(),
        Some("registry.example/aurora-sandbox:1")
    );
}

#[test]
fn unset_scheduling_options_are_omitted() {
    let bare = PodConfig {
        runtime_class: None,
        node_selector: BTreeMap::new(),
        tolerations: vec![],
        ..cfg()
    };
    let p = pod_spec(&bare, &req(), "n", "10.0.0.1", "10.0.0.2", Utc::now());
    let spec = p.spec.unwrap();
    assert!(spec.runtime_class_name.is_none());
    assert!(spec.node_selector.is_none());
    assert!(spec.tolerations.is_none());
}

#[test]
fn volumes_and_mounts() {
    let p = pod();
    let spec = p.spec.as_ref().unwrap();
    let vols = spec.volumes.as_ref().unwrap();
    let by = |n: &str| vols.iter().find(|v| v.name == n).unwrap();
    assert_eq!(
        by("home")
            .persistent_volume_claim
            .as_ref()
            .unwrap()
            .claim_name,
        "home-0192f1c0-aaaa-7bbb-8ccc-0123456789ab"
    );
    for n in ["tmp", "run-aurora"] {
        let e = by(n).empty_dir.as_ref().unwrap();
        assert_eq!(e.medium.as_deref(), Some("Memory"));
        assert!(e.size_limit.is_some());
    }
    assert_eq!(
        by("tmp")
            .empty_dir
            .as_ref()
            .unwrap()
            .size_limit
            .as_ref()
            .unwrap()
            .0,
        "512Mi"
    );
    assert_eq!(
        by("run-aurora")
            .empty_dir
            .as_ref()
            .unwrap()
            .size_limit
            .as_ref()
            .unwrap()
            .0,
        "16Mi"
    );
    assert_eq!(vols.len(), 3, "no other volumes (no hostPath, no SA token)");
    let mounts: BTreeMap<_, _> = spec.containers[0]
        .volume_mounts
        .as_ref()
        .unwrap()
        .iter()
        .map(|m| (m.mount_path.as_str(), m.name.as_str()))
        .collect();
    assert_eq!(
        mounts,
        BTreeMap::from([
            ("/workspace", "home"),
            ("/tmp", "tmp"),
            ("/run/aurora", "run-aurora")
        ])
    );
}

#[test]
fn boot_document_only_in_the_secret() {
    let p = pod();
    let c = &p.spec.as_ref().unwrap().containers[0];
    let boot_env: Vec<_> = c
        .env
        .as_ref()
        .unwrap()
        .iter()
        .filter(|e| e.name == "AURORA_BOOT")
        .collect();
    assert_eq!(boot_env.len(), 1);
    assert!(boot_env[0].value.is_none(), "never a literal value");
    let r = boot_env[0]
        .value_from
        .as_ref()
        .unwrap()
        .secret_key_ref
        .as_ref()
        .unwrap();
    assert_eq!(r.name, "boot-0192f1c0-1111-7222-8333-abcdef012345");
    assert_eq!(r.key, "AURORA_BOOT");
    assert_eq!(r.optional, Some(false));

    // The document appears nowhere in the serialised Pod (nor the request's literal).
    let pod_json = serde_json::to_string(&p).unwrap();
    assert!(!pod_json.contains(BOOT_DOC));
    assert!(!pod_json.contains("literal-must-be-dropped"));

    let s = boot_secret(&cfg(), &req(), "sbx-daneel-abcdef01", Utc::now());
    assert_eq!(
        s.metadata.name.as_deref(),
        Some("boot-0192f1c0-1111-7222-8333-abcdef012345")
    );
    assert_eq!(s.string_data.as_ref().unwrap()["AURORA_BOOT"], BOOT_DOC);
    assert_eq!(s.type_.as_deref(), Some("Opaque"));
    assert_eq!(
        s.metadata.labels.as_ref().unwrap()["aurora.pod"],
        "sbx-daneel-abcdef01"
    );
    // And the document stays out of every label/annotation.
    assert!(
        !serde_json::to_string(&s.metadata)
            .unwrap()
            .contains(BOOT_DOC)
    );
}

#[test]
fn environment_contents() {
    let env = container_env(&req());
    let get = |k: &str| {
        env.iter()
            .find(|e| e.name == k)
            .and_then(|e| e.value.clone())
    };
    assert_eq!(get("HOME").as_deref(), Some("/workspace"));
    assert_eq!(get("HTTPS_PROXY").as_deref(), Some("http://127.0.0.1:3128"));
    assert_eq!(get("https_proxy").as_deref(), Some("http://127.0.0.1:3128"));
    assert_eq!(get("NO_PROXY").as_deref(), Some("localhost,127.0.0.1"));
    for k in [
        "SSL_CERT_FILE",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "GIT_SSL_CAINFO",
    ] {
        assert_eq!(get(k).as_deref(), Some("/run/aurora/ca.pem"), "{k}");
    }
    assert_eq!(
        get("AURORA_SANDBOX_ID").as_deref(),
        Some("0192f1c0-aaaa-7bbb-8ccc-0123456789ab")
    );
    assert_eq!(get("APP_FLAG").as_deref(), Some("1"));
    assert!(
        get("HTTPS_PROXY").unwrap().find('@').is_none(),
        "no credentials in the proxy URL"
    );
}

#[test]
fn labels_cannot_be_forged() {
    let l = boot_labels(&cfg(), &req(), Utc::now());
    assert_eq!(l["aurora.managed"], "true");
    assert_eq!(l["aurora.installation"], "0123456789abcdef");
    assert_eq!(l["aurora.sandbox"], "0192f1c0-aaaa-7bbb-8ccc-0123456789ab");
    assert_eq!(l["aurora.boot"], "0192f1c0-1111-7222-8333-abcdef012345");
    assert_eq!(l["aurora.runner"], "r-1");
    assert!(l["aurora.created"].parse::<i64>().is_ok());
    let pod_labels = pod().metadata.labels.unwrap();
    for k in [
        "aurora.managed",
        "aurora.installation",
        "aurora.sandbox",
        "aurora.boot",
        "aurora.runner",
    ] {
        assert_eq!(pod_labels[k], l[k], "{k}");
    }
}

#[test]
fn pvc_shape() {
    let r = req();
    let pvc = home_pvc(&cfg(), r.sandbox_id, &r.spec.home_ref, 7, Utc::now());
    assert_eq!(
        pvc.metadata.name.as_deref(),
        Some("home-0192f1c0-aaaa-7bbb-8ccc-0123456789ab")
    );
    let spec = pvc.spec.unwrap();
    assert_eq!(spec.access_modes, Some(vec!["ReadWriteOnce".to_owned()]));
    assert_eq!(spec.storage_class_name.as_deref(), Some("fast"));
    assert_eq!(
        spec.resources.unwrap().requests.unwrap()["storage"].0,
        "7Gi"
    );
    let l = pvc.metadata.labels.unwrap();
    assert_eq!(l["aurora.managed"], "true");
    assert_eq!(l["aurora.sandbox"], r.sandbox_id.to_string());
    assert_eq!(l["aurora.installation"], "0123456789abcdef");
}

#[test]
fn names_are_dns_labels() {
    let boot = Uuid::parse_str("0192f1c0-1111-7222-8333-abcdef012345").unwrap();
    assert_eq!(boot_short(boot), "ef012345");
    assert_eq!(pod_name("Daneel", boot), "sbx-daneel-ef012345");
    assert_eq!(pod_name("Dr. Who_2!", boot), "sbx-dr-who-2-ef012345");
    assert_eq!(pod_name("", boot), "sbx-agent-ef012345");
    assert_eq!(pod_name("---", boot), "sbx-agent-ef012345");
    assert_eq!(pod_name("ÄÖ", boot), "sbx-agent-ef012345");
    let long = "a-".repeat(80);
    let n = pod_name(&long, boot);
    assert!(n.len() <= 63, "{n}");
    assert!(is_dns_label(&n), "{n}");
    assert!(n.ends_with("-ef012345"));
    assert!(is_dns_label(&secret_name(boot)));
    assert!(is_dns_label("home-0192f1c0-aaaa-7bbb-8ccc-0123456789ab"));
    for bad in ["", "-a", "a-", "A", "a_b", "a/b", "a.b", &"a".repeat(64)] {
        assert!(!is_dns_label(bad), "{bad}");
    }
}

#[test]
fn pod_name_falls_back_to_sandbox_prefix() {
    let mut r = req();
    r.spec.env.clear();
    assert_eq!(objects::handle_for(&r), "0192f1c0");
    r.spec.env = vec![("AURORA_AGENT_HANDLE".into(), "!!!".into())];
    assert_eq!(objects::handle_for(&r), "0192f1c0");
}

fn status_pod(phase: &str, conditions: Vec<PodCondition>, waiting: Option<(&str, &str)>) -> Pod {
    let mut v = json!({"metadata": {"name": "p"}, "status": {"phase": phase}});
    if let Some((reason, msg)) = waiting {
        v["status"]["containerStatuses"] = json!([{
            "name": "sandbox", "image": "i", "imageID": "", "ready": false, "restartCount": 0,
            "state": {"waiting": {"reason": reason, "message": msg}}
        }]);
    }
    let mut p: Pod = serde_json::from_value(v).unwrap();
    p.status = Some(PodStatus {
        conditions: Some(conditions),
        ..p.status.unwrap()
    });
    p
}

fn scheduled(status: &str, reason: Option<&str>) -> PodCondition {
    PodCondition {
        type_: "PodScheduled".into(),
        status: status.into(),
        reason: reason.map(Into::into),
        message: Some("0/3 nodes are available".into()),
        ..Default::default()
    }
}

#[test]
fn status_mapping() {
    assert_eq!(classify(&Pod::default()), PodState::Pending);
    assert_eq!(
        classify(&status_pod("Running", vec![], None)),
        PodState::Running
    );
    assert_eq!(
        classify(&status_pod(
            "Pending",
            vec![scheduled("False", Some("Unschedulable"))],
            None
        )),
        PodState::Unschedulable("0/3 nodes are available".into())
    );
    // Scheduled=False for another reason is not a capacity hold.
    assert_eq!(
        classify(&status_pod(
            "Pending",
            vec![scheduled("False", Some("SchedulerError"))],
            None
        )),
        PodState::Pending
    );
    assert_eq!(
        classify(&status_pod("Pending", vec![scheduled("True", None)], None)),
        PodState::Pending
    );
    for reason in ["ErrImagePull", "ImagePullBackOff", "InvalidImageName"] {
        match classify(&status_pod(
            "Pending",
            vec![scheduled("True", None)],
            Some((reason, "pull failed")),
        )) {
            PodState::ImageError(m) => assert!(m.contains(reason) && m.contains("pull failed")),
            other => panic!("{reason}: {other:?}"),
        }
    }
    // ContainerCreating is just pending.
    assert_eq!(
        classify(&status_pod(
            "Pending",
            vec![],
            Some(("ContainerCreating", ""))
        )),
        PodState::Pending
    );
    assert!(matches!(
        classify(&status_pod("Failed", vec![], None)),
        PodState::Terminated(_)
    ));
    assert!(matches!(
        classify(&status_pod("Succeeded", vec![], None)),
        PodState::Terminated(_)
    ));
    let mut deleting = status_pod("Running", vec![], None);
    deleting.metadata.deletion_timestamp =
        Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
            k8s_openapi::jiff::Timestamp::UNIX_EPOCH,
        ));
    assert!(matches!(classify(&deleting), PodState::Terminated(_)));
    assert_eq!(phase_label(&deleting), "terminating");
    assert_eq!(phase_label(&status_pod("Running", vec![], None)), "running");
    assert_eq!(
        phase_label(&status_pod(
            "Pending",
            vec![scheduled("False", Some("Unschedulable"))],
            None
        )),
        "unschedulable"
    );
}

#[test]
fn quantities_round_trip() {
    assert_eq!(cpu_millis("1500m"), Some(1500));
    assert_eq!(cpu_millis("2"), Some(2000));
    assert_eq!(cpu_millis("0.5"), Some(500));
    assert_eq!(cpu_millis("x"), None);
    assert_eq!(memory_mib("2048Mi"), Some(2048));
    assert_eq!(memory_mib("2Gi"), Some(2048));
    assert_eq!(memory_mib("2097152Ki"), Some(2048));
    assert_eq!(memory_mib("2147483648"), Some(2048));
    assert_eq!(memory_mib("lots"), None);
}

#[test]
fn core_namespace_resolution() {
    let dir = std::env::temp_dir().join(format!("aurora-k8s-ns-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("namespace");
    std::fs::write(&f, "from-sa\n").unwrap();
    assert_eq!(resolve_core_namespace(Some("cfg"), &f), "cfg");
    assert_eq!(resolve_core_namespace(None, &f), "from-sa");
    assert_eq!(resolve_core_namespace(Some("  "), &f), "from-sa");
    assert_eq!(
        resolve_core_namespace(None, &dir.join("missing")),
        "default"
    );
}

#[test]
fn invalid_toleration_is_a_startup_error() {
    assert!(parse_toleration(&json!({"operator": 5})).is_err());
}

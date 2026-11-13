//! Podman backend integration tests. Skipped unless `AURORA_TEST_PODMAN=1`.
//!
//! Needs a podman socket (`runner.podman_socket` default), the internal
//! `aurora-sandbox` network (`AURORA_TEST_NETWORK`) and the `sandbox-min` image
//! (`scripts/images/build-sandbox-min.sh`; override with `AURORA_TEST_IMAGE`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::time::Duration;

use aurora_protocol::common::SecretString;
use aurora_protocol::runner::{SandboxResources, SandboxSpec};
use aurora_runner::backend::{Backend, HoldCallback, ObjectKind, StartRequest};
use aurora_runner::backends::podman::{
    PodmanBackend, PodmanSettings, verify_network, verify_network_dns,
};
use base64::Engine;
use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecResults};
use futures::StreamExt;
use uuid::Uuid;

/// A valid (never trusted for anything) CA: the supervisor builds its gateway TLS config from it.
const PROBE_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIBnzCCAUSgAwIBAgIUM9r7jwTt+H50+gskkfbctBQg0sowCgYIKoZIzj0EAwIw\nGjEYMBYGA1UEAwwPQXVyb3JhIHByb2JlIENBMCAXDTI2MTEwNzEzMjMyMloYDzIx\nMjYxMDE0MTMyMzIyWjAaMRgwFgYDVQQDDA9BdXJvcmEgcHJvYmUgQ0EwWTATBgcq\nhkjOPQIBBggqhkjOPQMBBwNCAASdiPf1x/r5vS0uY0nK0X3wfuEftek+6V7nsozQ\nwCJKGRDKzjU59cfBr+an8uoDuPo0DJl0CkHxqrD643MFTD2Lo2YwZDAdBgNVHQ4E\nFgQUIYO3pRSPN1VCDAVE+sMSFUz/urIwHwYDVR0jBBgwFoAUIYO3pRSPN1VCDAVE\n+sMSFUz/urIwEgYDVR0TAQH/BAgwBgEB/wIBADAOBgNVHQ8BAf8EBAMCAgQwCgYI\nKoZIzj0EAwIDSQAwRgIhAJYDTz1V0xJaySFLV71ajMjhmwEh3OGF1s9z5yaY8sM0\nAiEA8s1UeriAsGDKFM2OfjSCULwfCM5Q4Z8puur8GY2njI0=\n-----END CERTIFICATE-----\n";
const BOOT_MARKER: &str = "aub_probe_marker_7f3a";

fn enabled() -> bool {
    std::env::var("AURORA_TEST_PODMAN").is_ok_and(|v| v == "1")
}

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_owned())
}

struct NoHold;
impl HoldCallback for NoHold {
    fn on_hold(&self) {}
}

fn settings() -> PodmanSettings {
    PodmanSettings {
        socket: std::env::var("AURORA_TEST_PODMAN_SOCKET").ok(),
        network: env_or("AURORA_TEST_NETWORK", "aurora-sandbox"),
        hosts: BTreeMap::from([
            ("aurora-gateway".into(), "10.89.250.2".into()),
            ("aurora-runner".into(), "10.89.250.3".into()),
        ]),
        installation: "podmantest0000000".into(),
        allow_external_network: false,
        dev_supervisor_binary: None,
        max_sandboxes: 4,
        capacity_wait: Duration::ZERO,
    }
}

fn docker() -> Docker {
    let path = settings().socket.unwrap_or_else(|| {
        format!(
            "{}/podman/podman.sock",
            std::env::var("XDG_RUNTIME_DIR").unwrap()
        )
    });
    Docker::connect_with_unix(&path, 60, bollard::API_DEFAULT_VERSION).unwrap()
}

fn boot_secret(sandbox: Uuid, boot: Uuid) -> String {
    let doc = serde_json::json!({
        "v": 1, "sandbox_id": sandbox, "boot_id": boot,
        "agent": {"id": Uuid::nil(), "handle": "probe", "name": "Probe"},
        "runner_url": "ws://aurora-runner:4105/v1/supervisor",
        "bootstrap_token": "aup_probe",
        "gateway": {"proxy": "aurora-gateway:4102", "tls": true, "server_name": "aurora-gateway", "boot_token": BOOT_MARKER},
        "ca_pem": PROBE_CA_PEM,
        "files": [],
        "harness": {"id": "fake", "capabilities_expected": []},
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(doc.to_string())
}

fn request(sandbox: Uuid, boot: Uuid) -> StartRequest {
    StartRequest {
        sandbox_id: sandbox,
        boot_id: boot,
        spec: SandboxSpec {
            image: env_or("AURORA_TEST_IMAGE", "localhost/aurora-sandbox-min:dev"),
            env: vec![("AURORA_AGENT_HANDLE".into(), "probe".into())],
            boot_secret: SecretString::new(boot_secret(sandbox, boot)),
            home_ref: format!("aurora-home-{sandbox}"),
            home_gib: 1,
            resources: SandboxResources {
                cpu_millis: 500,
                memory_mib: 512,
                pids: 256,
            },
            labels: BTreeMap::new(),
        },
    }
}

/// Runs `sh -c script` in the container; returns (exit code, stdout+stderr).
async fn exec(d: &Docker, id: &str, script: &str) -> (i64, String) {
    let e = d
        .create_exec(
            id,
            CreateExecOptions {
                cmd: Some(vec!["bash", "-c", script]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut text = String::new();
    if let StartExecResults::Attached { mut output, .. } = d.start_exec(&e.id, None).await.unwrap()
    {
        while let Some(chunk) = output.next().await {
            text.push_str(&chunk.unwrap().to_string());
        }
    }
    let code = d.inspect_exec(&e.id).await.unwrap().exit_code.unwrap();
    (code, text)
}

async fn wait_running(b: &PodmanBackend, sandbox: Uuid) {
    for _ in 0..50 {
        if b.inspect(sandbox).await.unwrap().is_some_and(|o| o.running) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("container did not start");
}

#[tokio::test]
async fn sandbox_lifecycle_and_hardening() {
    if !enabled() {
        eprintln!("skipped: AURORA_TEST_PODMAN != 1");
        return;
    }
    let d = docker();
    let b = PodmanBackend::connect(settings()).await.unwrap();
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    let req = request(sandbox, boot);

    b.provision_home(sandbox, &req.spec.home_ref, 1)
        .await
        .unwrap();
    b.provision_home(sandbox, &req.spec.home_ref, 1)
        .await
        .unwrap(); // idempotent
    let c = b.start(&req, &NoHold).await.unwrap();
    let again = b.start(&req, &NoHold).await.unwrap(); // idempotent per boot
    assert_eq!(c, again);
    wait_running(&b, sandbox).await;
    let id = &c.0;

    let obs = b.inspect(sandbox).await.unwrap().unwrap();
    assert!(obs.running);
    assert_eq!(obs.boot_id, Some(boot));
    let o = obs.observed.unwrap();
    assert_eq!((o.cpu_millis, o.memory_mib), (500, 512));

    // Hardening probes (SEC-26).
    let (_, out) = exec(&d, id, "id -u; id -g").await;
    assert_eq!(out.trim(), "1000\n1000");
    let (_, out) = exec(&d, id, "grep CapEff /proc/self/status").await;
    assert!(out.contains("0000000000000000"), "caps: {out}");
    let (_, out) = exec(&d, id, "grep NoNewPrivs /proc/self/status").await;
    assert!(out.contains('1'), "nnp: {out}");
    let (code, _) = exec(&d, id, "touch /usr/rootfs-probe").await;
    assert_ne!(code, 0, "root filesystem must be read-only");
    let (code, _) = exec(&d, id, "touch /tmp/ok /workspace/ok").await;
    assert_eq!(code, 0, "tmp and home must be writable");
    let (_, out) = exec(&d, id, "df -m /tmp | tail -1").await;
    assert!(out.contains("512"), "tmpfs size: {out}");

    // Network isolation (SEC-02): no DNS, no arbitrary IP.
    let (code, _) = exec(&d, id, "getent hosts example.org").await;
    assert_ne!(code, 0, "external names must not resolve");
    let (code, _) = exec(&d, id, "timeout 3 bash -c '</dev/tcp/1.1.1.1/53'").await;
    assert_ne!(code, 0, "arbitrary IP must be unreachable");
    let (_, hosts) = exec(&d, id, "cat /etc/hosts").await;
    assert!(
        hosts.contains("10.89.250.2") && hosts.contains("aurora-gateway"),
        "{hosts}"
    );
    assert!(
        hosts.contains("10.89.250.3") && hosts.contains("aurora-runner"),
        "{hosts}"
    );

    // SEC-34 / I-23: the boot document is not readable by the agent uid.
    let (code, out) = exec(&d, id, "cat /proc/1/environ").await;
    assert!(
        code != 0 || !out.contains("AURORA_BOOT"),
        "boot env readable: {out}"
    );
    assert!(!out.contains(BOOT_MARKER));
    // `podman exec` inherits the container env by design (operator path, not an agent
    // path: agent processes are spawned by the supervisor with a scrubbed env, I-23).
    let (code, _) = exec(&d, id, "cat /proc/1/mem >/dev/null").await;
    assert_ne!(code, 0, "supervisor memory must not be readable");
    let (_, out) = exec(
        &d,
        id,
        "grep -i dumpable /proc/1/status; ls -ld /proc/1/environ",
    )
    .await;
    eprintln!("probe: {out}");

    // Non-secret env.
    let (_, out) = exec(&d, id, "echo $HOME $AURORA_AGENT_HANDLE $HTTPS_PROXY").await;
    assert_eq!(out.trim(), "/workspace probe http://127.0.0.1:3128");

    // Home persists across stop/start; stop parks (container removed, volume kept).
    exec(&d, id, "echo persisted > /workspace/marker").await;
    b.stop(&c, Duration::from_secs(5)).await.unwrap();
    assert!(b.inspect(sandbox).await.unwrap().is_none());
    let managed = b.list_managed().await.unwrap();
    assert!(
        managed
            .iter()
            .any(|m| m.kind == ObjectKind::Home && m.sandbox_id == Some(sandbox))
    );
    assert!(
        !managed
            .iter()
            .any(|m| m.kind == ObjectKind::Container && m.sandbox_id == Some(sandbox))
    );

    let boot2 = Uuid::new_v4();
    let c2 = b.start(&request(sandbox, boot2), &NoHold).await.unwrap();
    wait_running(&b, sandbox).await;
    let (_, out) = exec(&d, &c2.0, "cat /workspace/marker").await;
    assert_eq!(out.trim(), "persisted");

    // A new boot replaces the running one.
    let boot3 = Uuid::new_v4();
    let c3 = b.start(&request(sandbox, boot3), &NoHold).await.unwrap();
    assert_ne!(c2, c3);
    assert_eq!(
        b.inspect(sandbox).await.unwrap().unwrap().boot_id,
        Some(boot3)
    );

    // Destroy removes container and home.
    for m in b.list_managed().await.unwrap() {
        if m.sandbox_id == Some(sandbox) {
            assert_eq!(m.installation.as_deref(), Some("podmantest0000000"));
            b.destroy(&m).await.unwrap();
        }
    }
    assert!(b.inspect(sandbox).await.unwrap().is_none());
    assert!(d.inspect_volume(&req.spec.home_ref).await.is_err());
    assert!(
        b.list_managed()
            .await
            .unwrap()
            .iter()
            .all(|m| m.sandbox_id != Some(sandbox))
    );
}

#[tokio::test]
async fn refuses_non_internal_network() {
    if !enabled() {
        eprintln!("skipped: AURORA_TEST_PODMAN != 1");
        return;
    }
    let d = docker();
    let name = format!("aurora-test-ext-{}", Uuid::new_v4().simple());
    d.create_network(bollard::models::NetworkCreateRequest {
        name: name.clone(),
        internal: Some(false),
        ..Default::default()
    })
    .await
    .unwrap();
    let refused = verify_network(&d, &name, false).await;
    let allowed = verify_network(&d, &name, true).await;
    d.remove_network(&name).await.unwrap();
    assert!(refused.unwrap_err().contains("not internal"));
    allowed.unwrap();
    verify_network(&d, &settings().network, false)
        .await
        .unwrap();
}

/// FIX-094 (SEC-02): an `--internal` network that still has DNS enabled is refused (the runner
/// used to check `internal` only); the configured sandbox network, created `--disable-dns`,
/// passes.
#[tokio::test]
async fn refuses_internal_network_with_dns_enabled() {
    if !enabled() {
        eprintln!("skipped: AURORA_TEST_PODMAN != 1");
        return;
    }
    let socket = settings().socket.unwrap_or_else(|| {
        format!(
            "{}/podman/podman.sock",
            std::env::var("XDG_RUNTIME_DIR").unwrap()
        )
    });
    let d = docker();
    let name = format!("aurora-test-dns-{}", Uuid::new_v4().simple());
    d.create_network(bollard::models::NetworkCreateRequest {
        name: name.clone(),
        internal: Some(true),
        ..Default::default()
    })
    .await
    .unwrap();
    let refused = verify_network_dns(&socket, &name, false).await;
    let allowed = verify_network_dns(&socket, &name, true).await;
    d.remove_network(&name).await.unwrap();
    assert!(
        refused.unwrap_err().contains("DNS"),
        "DNS left on is refused"
    );
    allowed.unwrap();
    verify_network_dns(&socket, &settings().network, false)
        .await
        .unwrap();
}

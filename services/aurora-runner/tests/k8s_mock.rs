//! Kubernetes backend against an in-memory fake API server (no cluster needed).
//!
//! The fake implements just what the backend uses: get/list/create/delete of Pods, Secrets and
//! PVCs in a namespace, label selectors, and `get Service`. Pod statuses are scripted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aurora_protocol::common::SecretString;
use aurora_protocol::runner::{SandboxResources, SandboxSpec};
use aurora_runner::backend::{Backend, BackendError, HoldCallback, ObjectKind, StartRequest};
use aurora_runner::backends::kubernetes::{KubernetesBackend, KubernetesSettings, PodConfig};
use http::{Method, Request, Response};
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};
use uuid::Uuid;

const NS: &str = "sbx";
const CORE: &str = "core";
const BOOT_DOC: &str = "c2VjcmV0LWJvb3QtZG9jdW1lbnQ";

/// How the fake cluster treats a newly created Pod.
#[derive(Clone, Debug)]
enum Behaviour {
    Run,
    /// Unschedulable for the first `n` reads of the Pod, then running.
    UnschedulableFor(usize),
    ImagePull,
    /// Admission says "exceeded quota" for the first `n` creates.
    QuotaFor(usize),
}

#[derive(Default)]
struct State {
    objects: BTreeMap<(String, String), Value>, // (kind, name) -> object
    behaviour: Option<Behaviour>,
    reads: BTreeMap<String, usize>,
    quota_hits: usize,
    deletes: Vec<(String, String, Option<u64>)>,
    creates: Vec<(String, String)>,
    secret_bodies: Vec<Value>,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

fn status_body(code: u16, reason: &str, message: &str) -> Response<Body> {
    let v = json!({"kind": "Status", "apiVersion": "v1", "status": "Failure",
                   "code": code, "reason": reason, "message": message});
    Response::builder()
        .status(code)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&v).unwrap()))
        .unwrap()
}

fn json_body(code: u16, v: &Value) -> Response<Body> {
    Response::builder()
        .status(code)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(v).unwrap()))
        .unwrap()
}

fn matches_selector(obj: &Value, selector: &str) -> bool {
    selector.split(',').filter(|s| !s.is_empty()).all(|term| {
        let (k, v) = term.split_once('=').unwrap();
        obj["metadata"]["labels"][k].as_str() == Some(v)
    })
}

impl Fake {
    fn with(b: Behaviour) -> Self {
        let f = Self::default();
        f.0.lock().unwrap().behaviour = Some(b);
        f.put("services", "aurora-gateway", core_service("10.1.0.1"));
        f.put("services", "aurora-runner", core_service("10.1.0.2"));
        f
    }

    fn put(&self, kind: &str, name: &str, v: Value) {
        self.0
            .lock()
            .unwrap()
            .objects
            .insert((kind.into(), name.into()), v);
    }

    fn names(&self, kind: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .objects
            .keys()
            .filter(|(k, _)| k == kind)
            .map(|(_, n)| n.clone())
            .collect()
    }

    fn get(&self, kind: &str, name: &str) -> Option<Value> {
        self.0
            .lock()
            .unwrap()
            .objects
            .get(&(kind.into(), name.into()))
            .cloned()
    }

    fn handle(&self, method: Method, path: &str, query: &str, body: Vec<u8>) -> Response<Body> {
        let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
        // /api/v1/namespaces/{ns}/{kind}[/{name}]
        let ["api", "v1", "namespaces", ns, kind, rest @ ..] = segs.as_slice() else {
            return status_body(404, "NotFound", "unknown path");
        };
        if !matches!(
            *kind,
            "pods" | "secrets" | "persistentvolumeclaims" | "services"
        ) {
            return status_body(404, "NotFound", "unknown kind");
        }
        let expected_ns = if *kind == "services" { CORE } else { NS };
        assert_eq!(*ns, expected_ns, "{kind} must live in {expected_ns}");
        let mut st = self.0.lock().unwrap();
        match (method, rest) {
            (Method::GET, []) => {
                let sel = query
                    .split('&')
                    .find_map(|p| p.strip_prefix("labelSelector="))
                    .map(|s| s.replace("%3D", "=").replace("%2C", ","))
                    .unwrap_or_default();
                let mut items = Vec::new();
                let names: Vec<_> = st
                    .objects
                    .keys()
                    .filter(|(k, _)| k == kind)
                    .map(|(_, n)| n.clone())
                    .collect();
                for n in names {
                    let o = pod_view(&mut st, kind, &n);
                    if matches_selector(&o, &sel) {
                        items.push(o);
                    }
                }
                json_body(
                    200,
                    &json!({"apiVersion": "v1", "kind": "List", "metadata": {}, "items": items}),
                )
            }
            (Method::GET, [name]) => {
                if !st
                    .objects
                    .contains_key(&(kind.to_string(), name.to_string()))
                {
                    return status_body(404, "NotFound", &format!("{kind} \"{name}\" not found"));
                }
                json_body(200, &pod_view(&mut st, kind, name))
            }
            (Method::POST, []) => {
                let mut o: Value = serde_json::from_slice(&body).unwrap();
                let name = o["metadata"]["name"].as_str().unwrap().to_owned();
                if *kind == "pods" {
                    let n = match st.behaviour {
                        Some(Behaviour::QuotaFor(n)) => n,
                        _ => 0,
                    };
                    if st.quota_hits < n {
                        st.quota_hits += 1;
                        return status_body(
                            403,
                            "Forbidden",
                            "pods \"x\" is forbidden: exceeded quota: q, requested: pods=1",
                        );
                    }
                }
                let key = (kind.to_string(), name.clone());
                if st.objects.contains_key(&key) {
                    return status_body(
                        409,
                        "AlreadyExists",
                        &format!("{kind} \"{name}\" already exists"),
                    );
                }
                if *kind == "secrets" {
                    st.secret_bodies.push(o.clone());
                }
                o["metadata"]["creationTimestamp"] = json!("2026-01-01T00:00:00Z");
                o["metadata"]["uid"] = json!(Uuid::new_v4().to_string());
                st.creates.push((kind.to_string(), name));
                st.objects.insert(key, o.clone());
                json_body(201, &o)
            }
            (Method::DELETE, [name]) => {
                let grace = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|v| v["gracePeriodSeconds"].as_u64());
                match st.objects.remove(&(kind.to_string(), name.to_string())) {
                    Some(o) => {
                        st.deletes.push((kind.to_string(), name.to_string(), grace));
                        json_body(200, &o)
                    }
                    None => status_body(404, "NotFound", "not found"),
                }
            }
            _ => status_body(405, "MethodNotAllowed", "unsupported"),
        }
    }

    fn client(&self) -> Client {
        let fake = self.clone();
        let svc = tower::service_fn(move |req: Request<Body>| {
            let fake = fake.clone();
            async move {
                let (parts, body) = req.into_parts();
                let bytes = http_body_util_collect(body).await;
                let resp = fake.handle(
                    parts.method,
                    parts.uri.path(),
                    parts.uri.query().unwrap_or(""),
                    bytes,
                );
                Ok::<_, Infallible>(resp)
            }
        });
        Client::new(svc, NS)
    }
}

async fn http_body_util_collect(body: Body) -> Vec<u8> {
    use http_body_util::BodyExt;
    body.collect().await.unwrap().to_bytes().to_vec()
}

fn core_service(ip: &str) -> Value {
    json!({"apiVersion": "v1", "kind": "Service", "metadata": {"name": "svc"}, "spec": {"clusterIP": ip}})
}

/// Pods get their scripted status when read.
fn pod_view(st: &mut State, kind: &str, name: &str) -> Value {
    let mut o = st.objects[&(kind.to_string(), name.to_string())].clone();
    if kind != "pods" {
        return o;
    }
    let reads = st.reads.entry(name.to_owned()).or_insert(0);
    *reads += 1;
    let reads = *reads;
    let status = match st.behaviour.clone().unwrap_or(Behaviour::Run) {
        Behaviour::UnschedulableFor(n) if reads <= n => json!({
            "phase": "Pending",
            "conditions": [{"type": "PodScheduled", "status": "False", "reason": "Unschedulable",
                            "message": "0/3 nodes are available: insufficient cpu"}]
        }),
        Behaviour::ImagePull => json!({
            "phase": "Pending",
            "conditions": [{"type": "PodScheduled", "status": "True"}],
            "containerStatuses": [{"name": "sandbox", "image": "i", "imageID": "", "ready": false,
                "restartCount": 0, "state": {"waiting": {"reason": "ImagePullBackOff", "message": "Back-off pulling image"}}}]
        }),
        _ => json!({"phase": "Running"}),
    };
    o["status"] = status;
    o
}

#[derive(Default)]
struct Holds(AtomicUsize);
impl HoldCallback for Holds {
    fn on_hold(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl Holds {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

fn settings(max: u32, wait: Duration) -> KubernetesSettings {
    KubernetesSettings {
        namespace: NS.into(),
        core_namespace: CORE.into(),
        gateway_service: "aurora-gateway".into(),
        runner_service: "aurora-runner".into(),
        pod: PodConfig {
            installation: "0123456789abcdef".into(),
            storage_class: None,
            runtime_class: None,
            node_selector: BTreeMap::new(),
            tolerations: vec![],
        },
        max_sandboxes: max,
        capacity_wait: wait,
        start_timeout: Duration::from_secs(5),
        poll: Duration::from_millis(5),
    }
}

fn backend(fake: &Fake, max: u32, wait: Duration) -> KubernetesBackend {
    KubernetesBackend::with_client(fake.client(), settings(max, wait))
}

fn request(sandbox: Uuid, boot: Uuid) -> StartRequest {
    StartRequest {
        sandbox_id: sandbox,
        boot_id: boot,
        spec: SandboxSpec {
            image: "registry.example/aurora-sandbox:1".into(),
            env: vec![("AURORA_AGENT_HANDLE".into(), "daneel".into())],
            boot_secret: SecretString::new(BOOT_DOC),
            home_ref: format!("home-{sandbox}"),
            home_gib: 5,
            resources: SandboxResources {
                cpu_millis: 500,
                memory_mib: 1024,
                pids: 256,
            },
            labels: BTreeMap::from([("aurora.runner".into(), Uuid::nil().to_string())]),
        },
    }
}

#[tokio::test]
async fn start_creates_home_secret_and_pod() {
    let fake = Fake::with(Behaviour::Run);
    let b = backend(&fake, 4, Duration::ZERO);
    let (sbx, boot) = (Uuid::now_v7(), Uuid::now_v7());
    let holds = Holds::default();
    let c = b.start(&request(sbx, boot), &holds).await.unwrap();
    assert!(c.0.starts_with("sbx-daneel-"), "{}", c.0);
    assert_eq!(holds.count(), 0);

    assert_eq!(
        fake.names("persistentvolumeclaims"),
        vec![format!("home-{sbx}")]
    );
    assert_eq!(fake.names("pods"), vec![c.0.clone()]);
    assert_eq!(fake.names("secrets"), vec![format!("boot-{boot}")]);

    let pod = fake.get("pods", &c.0).unwrap();
    let pod_text = serde_json::to_string(&pod).unwrap();
    assert!(
        !pod_text.contains(BOOT_DOC),
        "boot document leaked into the Pod"
    );
    let envs = pod["spec"]["containers"][0]["env"].as_array().unwrap();
    let boot_env = envs.iter().find(|e| e["name"] == "AURORA_BOOT").unwrap();
    assert!(boot_env.get("value").is_none());
    assert_eq!(
        boot_env["valueFrom"]["secretKeyRef"]["name"],
        format!("boot-{boot}")
    );
    // Host aliases come from the Service lookup in the core namespace.
    let aliases = pod["spec"]["hostAliases"].as_array().unwrap();
    assert_eq!(aliases[0]["ip"], "10.1.0.1");
    assert_eq!(aliases[1]["ip"], "10.1.0.2");

    let secret = fake.get("secrets", &format!("boot-{boot}")).unwrap();
    assert_eq!(secret["stringData"]["AURORA_BOOT"], BOOT_DOC);
    assert_eq!(secret["metadata"]["labels"]["aurora.pod"], c.0);
}

#[tokio::test]
async fn start_is_idempotent_per_boot() {
    let fake = Fake::with(Behaviour::Run);
    let b = backend(&fake, 4, Duration::ZERO);
    let (sbx, boot) = (Uuid::now_v7(), Uuid::now_v7());
    let req = request(sbx, boot);
    let first = b.start(&req, &Holds::default()).await.unwrap();
    let pod_creates = |f: &Fake| {
        f.0.lock()
            .unwrap()
            .creates
            .iter()
            .filter(|(k, _)| k == "pods")
            .count()
    };
    assert_eq!(pod_creates(&fake), 1);
    let again = b.start(&req, &Holds::default()).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(pod_creates(&fake), 1, "no second Pod");
    assert!(
        fake.0
            .lock()
            .unwrap()
            .deletes
            .iter()
            .all(|(k, ..)| k != "pods"),
        "the running Pod of the same boot is left alone"
    );
    assert_eq!(fake.names("secrets").len(), 1);
}

#[tokio::test]
async fn start_removes_other_boots_first() {
    let fake = Fake::with(Behaviour::Run);
    let b = backend(&fake, 4, Duration::ZERO);
    let sbx = Uuid::now_v7();
    let (old_boot, new_boot) = (Uuid::now_v7(), Uuid::now_v7());
    let old = b
        .start(&request(sbx, old_boot), &Holds::default())
        .await
        .unwrap();

    // An unrelated sandbox must not be touched.
    let other_sbx = Uuid::now_v7();
    let other_boot = Uuid::now_v7();
    let other = b
        .start(&request(other_sbx, other_boot), &Holds::default())
        .await
        .unwrap();

    // A leftover Secret whose Pod is already gone.
    fake.put("secrets", "boot-stale", json!({
        "apiVersion": "v1", "kind": "Secret",
        "metadata": {"name": "boot-stale", "labels": {"aurora.sandbox": sbx.to_string(), "aurora.boot": Uuid::now_v7().to_string()}}
    }));

    let new = b
        .start(&request(sbx, new_boot), &Holds::default())
        .await
        .unwrap();
    assert_ne!(old, new);
    let pods = fake.names("pods");
    assert!(!pods.contains(&old.0), "old boot's Pod removed");
    assert!(pods.contains(&new.0) && pods.contains(&other.0));
    let secrets = fake.names("secrets");
    assert!(!secrets.contains(&format!("boot-{old_boot}")));
    assert!(!secrets.contains(&"boot-stale".to_string()));
    assert!(secrets.contains(&format!("boot-{new_boot}")));
    assert!(secrets.contains(&format!("boot-{other_boot}")));
    // The home is shared across boots.
    assert_eq!(
        fake.names("persistentvolumeclaims")
            .iter()
            .filter(|n| **n == format!("home-{sbx}"))
            .count(),
        1
    );
}

#[tokio::test]
async fn unschedulable_reports_hold_then_runs() {
    let fake = Fake::with(Behaviour::UnschedulableFor(3));
    let b = backend(&fake, 4, Duration::from_secs(5));
    let holds = Holds::default();
    let c = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &holds)
        .await
        .unwrap();
    assert_eq!(holds.count(), 1, "hold reported exactly once");
    assert!(fake.names("pods").contains(&c.0));
}

#[tokio::test]
async fn unschedulable_times_out_and_cleans_up() {
    let fake = Fake::with(Behaviour::UnschedulableFor(usize::MAX));
    let b = backend(&fake, 4, Duration::from_millis(60));
    let holds = Holds::default();
    let err = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &holds)
        .await
        .unwrap_err();
    assert!(matches!(err, BackendError::WakeCapacityTimeout), "{err:?}");
    assert_eq!(err.reason_code(), "at_capacity");
    assert_eq!(holds.count(), 1);
    assert!(fake.names("pods").is_empty());
    assert!(fake.names("secrets").is_empty(), "boot Secret removed");
    assert_eq!(
        fake.names("persistentvolumeclaims").len(),
        1,
        "the home stays"
    );
}

#[tokio::test]
async fn unschedulable_without_wait_is_immediate_capacity_error() {
    let fake = Fake::with(Behaviour::UnschedulableFor(usize::MAX));
    let b = backend(&fake, 4, Duration::ZERO);
    let holds = Holds::default();
    let err = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &holds)
        .await
        .unwrap_err();
    assert!(matches!(err, BackendError::Capacity(_)), "{err:?}");
    assert_eq!(holds.count(), 0);
    assert!(fake.names("pods").is_empty());
}

#[tokio::test]
async fn quota_exceeded_holds_then_succeeds() {
    let fake = Fake::with(Behaviour::QuotaFor(2));
    let b = backend(&fake, 4, Duration::from_secs(5));
    let holds = Holds::default();
    b.start(&request(Uuid::now_v7(), Uuid::now_v7()), &holds)
        .await
        .unwrap();
    assert_eq!(holds.count(), 1);
    assert_eq!(fake.names("pods").len(), 1);
}

#[tokio::test]
async fn max_sandboxes_is_enforced_like_podman() {
    let fake = Fake::with(Behaviour::Run);
    let b = backend(&fake, 1, Duration::ZERO);
    b.start(&request(Uuid::now_v7(), Uuid::now_v7()), &Holds::default())
        .await
        .unwrap();
    let err = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &Holds::default())
        .await
        .unwrap_err();
    assert!(matches!(err, BackendError::Capacity(_)), "{err:?}");
    assert_eq!(fake.names("pods").len(), 1);
}

#[tokio::test]
async fn image_pull_failure_maps_to_image_unavailable() {
    let fake = Fake::with(Behaviour::ImagePull);
    let b = backend(&fake, 4, Duration::ZERO);
    let err = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &Holds::default())
        .await
        .unwrap_err();
    assert!(matches!(err, BackendError::ImageUnavailable(_)), "{err:?}");
    assert_eq!(err.reason_code(), "image_unavailable");
    assert!(fake.names("pods").is_empty());
    assert!(fake.names("secrets").is_empty());
}

#[tokio::test]
async fn missing_service_is_a_clear_error() {
    let fake = Fake::with(Behaviour::Run);
    fake.0
        .lock()
        .unwrap()
        .objects
        .remove(&("services".to_string(), "aurora-gateway".to_string()));
    let b = backend(&fake, 4, Duration::ZERO);
    let err = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &Holds::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("aurora-gateway"), "{err}");
    assert!(fake.names("pods").is_empty());
}

#[tokio::test]
async fn stop_removes_pod_and_secret_but_keeps_the_pvc() {
    let fake = Fake::with(Behaviour::Run);
    let b = backend(&fake, 4, Duration::ZERO);
    let sbx = Uuid::now_v7();
    let c = b
        .start(&request(sbx, Uuid::now_v7()), &Holds::default())
        .await
        .unwrap();
    b.stop(&c, Duration::from_secs(20)).await.unwrap();
    assert!(fake.names("pods").is_empty());
    assert!(fake.names("secrets").is_empty());
    assert_eq!(
        fake.names("persistentvolumeclaims"),
        vec![format!("home-{sbx}")]
    );
    {
        let st = fake.0.lock().unwrap();
        let pod_delete = st.deletes.iter().find(|(k, ..)| k == "pods").unwrap();
        assert_eq!(pod_delete.2, Some(20), "stop passes the grace period");
        assert!(
            st.deletes
                .iter()
                .all(|(k, ..)| k != "persistentvolumeclaims")
        );
    }
    // Stopping again is fine.
    b.stop(&c, Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn inspect_list_and_destroy() {
    let fake = Fake::with(Behaviour::Run);
    let b = backend(&fake, 4, Duration::ZERO);
    let (sbx, boot) = (Uuid::now_v7(), Uuid::now_v7());
    assert!(b.inspect(sbx).await.unwrap().is_none());
    let c = b
        .start(&request(sbx, boot), &Holds::default())
        .await
        .unwrap();

    let o = b.inspect(sbx).await.unwrap().unwrap();
    assert_eq!(o.container, c);
    assert!(o.running);
    assert_eq!(o.boot_id, Some(boot));
    assert_eq!(o.phase, "running");
    let observed = o.observed.unwrap();
    assert_eq!((observed.cpu_millis, observed.memory_mib), (500, 1024));

    let objs = b.list_managed().await.unwrap();
    let kinds: Vec<_> = objs.iter().map(|o| o.kind).collect();
    assert!(kinds.contains(&ObjectKind::Container));
    assert!(kinds.contains(&ObjectKind::Home));
    assert!(kinds.contains(&ObjectKind::Secret));
    let pod = objs
        .iter()
        .find(|o| o.kind == ObjectKind::Container)
        .unwrap();
    assert_eq!(pod.sandbox_id, Some(sbx));
    assert_eq!(pod.boot_id, Some(boot));
    assert_eq!(
        pod.runner.as_deref(),
        Some(Uuid::nil().to_string().as_str())
    );
    assert_eq!(pod.installation.as_deref(), Some("0123456789abcdef"));
    assert!(pod.running);

    // Destroying a home deletes the PVC; destroying the container removes Pod + Secret.
    let home = objs.iter().find(|o| o.kind == ObjectKind::Home).unwrap();
    b.destroy(pod).await.unwrap();
    assert!(fake.names("pods").is_empty() && fake.names("secrets").is_empty());
    assert_eq!(fake.names("persistentvolumeclaims").len(), 1);
    b.destroy(home).await.unwrap();
    assert!(fake.names("persistentvolumeclaims").is_empty());
    b.destroy(home).await.unwrap(); // already gone
}

#[tokio::test]
async fn provision_home_is_idempotent_and_validates_the_name() {
    let fake = Fake::with(Behaviour::Run);
    let b = backend(&fake, 4, Duration::ZERO);
    let sbx = Uuid::now_v7();
    let name = format!("home-{sbx}");
    b.provision_home(sbx, &name, 3).await.unwrap();
    b.provision_home(sbx, &name, 3).await.unwrap();
    assert_eq!(fake.names("persistentvolumeclaims"), vec![name.clone()]);
    let pvc = fake.get("persistentvolumeclaims", &name).unwrap();
    assert_eq!(pvc["spec"]["resources"]["requests"]["storage"], "3Gi");
    assert_eq!(pvc["spec"]["accessModes"][0], "ReadWriteOnce");
    assert!(b.provision_home(sbx, "../etc", 1).await.is_err());
    assert!(b.provision_home(sbx, "Home_X", 1).await.is_err());
}

fn runner_config(k8s_toml: &str) -> aurora_config::RunnerConfig {
    const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let dir = std::env::temp_dir().join(format!(
        "aurora-k8s-cfg-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("aurora.toml");
    std::fs::write(
        &path,
        format!(
            "[server]\nexternal_url = \"https://aurora.example.org\"\n\
             [runner]\nbackend = \"kubernetes\"\napi_url = \"http://api:4101\"\ncapacity_wait_secs = 7\n\
             [runner.k8s]\n{k8s_toml}\n"
        ),
    )
    .unwrap();
    let env = [
        ("AURORA_RUNNER_TOKEN", "aur_abc"),
        ("AURORA_RUNNER_SESSION_KEY", KEY),
    ]
    .map(|(a, b)| (a.to_owned(), b.to_owned()));
    aurora_config::load(Some(&path), env.into_iter())
        .unwrap()
        .runner_config()
        .unwrap()
}

#[test]
fn settings_come_from_config() {
    let cfg = runner_config(
        "namespace = \"sbx\"\ncore_namespace = \"core\"\nstorage_class = \"fast\"\n\
         runtime_class = \"gvisor\"\ngateway_service = \"gw\"\nrunner_service = \"rn\"\n\
         node_selector = { pool = \"sandbox\" }\n\
         tolerations = [{ key = \"sandbox\", operator = \"Exists\", effect = \"NoSchedule\" }]",
    );
    let s = KubernetesSettings::from_config(&cfg).unwrap();
    assert_eq!(
        (s.namespace.as_str(), s.core_namespace.as_str()),
        ("sbx", "core")
    );
    assert_eq!(
        (s.gateway_service.as_str(), s.runner_service.as_str()),
        ("gw", "rn")
    );
    assert_eq!(s.pod.storage_class.as_deref(), Some("fast"));
    assert_eq!(s.pod.runtime_class.as_deref(), Some("gvisor"));
    assert_eq!(s.pod.node_selector["pool"], "sandbox");
    assert_eq!(s.pod.tolerations.len(), 1);
    assert_eq!(s.pod.tolerations[0].key.as_deref(), Some("sandbox"));
    assert_eq!(s.capacity_wait, Duration::from_secs(7));
    assert_eq!(s.pod.installation.len(), 16);

    // Defaults.
    let s = KubernetesSettings::from_config(&runner_config("")).unwrap();
    assert_eq!(s.namespace, "aurora-sandboxes");
    assert_eq!(s.gateway_service, "aurora-gateway");
    assert_eq!(s.runner_service, "aurora-runner");

    // A malformed toleration stops startup with a pointer to the key.
    let bad = runner_config("tolerations = [{ key = 5 }]");
    let err = KubernetesSettings::from_config(&bad).unwrap_err();
    assert!(err.contains("runner.k8s.tolerations[0]"), "{err}");
}

#[tokio::test]
async fn missing_in_cluster_config_is_a_clear_startup_error() {
    if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
        return; // running inside a cluster: nothing to assert
    }
    let cfg = runner_config("");
    let err = match aurora_runner::backends::build(&cfg).await {
        Ok(_) => panic!("must not start without in-cluster configuration"),
        Err(e) => e,
    };
    assert!(err.contains("in-cluster"), "{err}");
}

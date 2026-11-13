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
    /// (FIX-025) Admission refuses every pod (Pod Security).
    Refused,
    /// (FIX-025) The pod fails right after scheduling (exit code 1).
    Terminated,
    /// (FIX-025) The pod waits for its unbound home PVC (no storage class).
    PvcUnbound,
    /// (FIX-049) The pod waits for its home PVC for the first `n` reads, then runs (the
    /// provisioner caught up).
    PvcUnboundFor(usize),
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
            "pods" | "secrets" | "persistentvolumeclaims" | "services" | "events"
        ) {
            return status_body(404, "NotFound", "unknown kind");
        }
        let expected_ns = if *kind == "services" { CORE } else { NS };
        assert_eq!(*ns, expected_ns, "{kind} must live in {expected_ns}");
        let mut st = self.0.lock().unwrap();
        // (FIX-025) events: list filtered by the field selector's involvedObject terms.
        if *kind == "events" {
            let fields = query
                .split('&')
                .find_map(|p| p.strip_prefix("fieldSelector="))
                .map(|s| s.replace("%3D", "=").replace("%2C", ","))
                .unwrap_or_default();
            let items: Vec<Value> = st
                .objects
                .iter()
                .filter(|((k, _), _)| k == "events")
                .map(|(_, v)| v.clone())
                .filter(|e| {
                    fields.split(',').filter(|t| !t.is_empty()).all(|t| {
                        let (k, v) = t.split_once('=').unwrap();
                        match k {
                            "involvedObject.kind" => e["involvedObject"]["kind"] == v,
                            "involvedObject.name" => e["involvedObject"]["name"] == v,
                            "type" => e["type"] == v,
                            _ => true,
                        }
                    })
                })
                .collect();
            return json_body(
                200,
                &json!({"apiVersion": "v1", "kind": "EventList", "metadata": {}, "items": items}),
            );
        }
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
                    if matches!(st.behaviour, Some(Behaviour::Refused)) {
                        return status_body(
                            403,
                            "Forbidden",
                            "pods \"x\" is forbidden: violates PodSecurity \"restricted:latest\": seccompProfile",
                        );
                    }
                }
                if *kind == "persistentvolumeclaims"
                    && matches!(
                        st.behaviour,
                        Some(Behaviour::PvcUnbound | Behaviour::PvcUnboundFor(_))
                    )
                {
                    o["status"] = json!({"phase": "Pending"});
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
        Behaviour::Terminated => json!({
            "phase": "Failed",
            "containerStatuses": [{"name": "sandbox", "image": "i", "imageID": "", "ready": false,
                "restartCount": 0, "state": {"terminated": {"exitCode": 1, "reason": "Error",
                "message": "supervisor: cannot write /run/aurora"}}}]
        }),
        Behaviour::PvcUnbound => json!({
            "phase": "Pending",
            "conditions": [{"type": "PodScheduled", "status": "False", "reason": "Unschedulable",
                "message": "0/3 nodes are available: pod has unbound immediate PersistentVolumeClaims. preemption: 0/3 nodes are available"}]
        }),
        Behaviour::PvcUnboundFor(n) if reads <= n => json!({
            "phase": "Pending",
            "conditions": [{"type": "PodScheduled", "status": "False", "reason": "Unschedulable",
                "message": "0/3 nodes are available: pod has unbound immediate PersistentVolumeClaims. preemption: 0/3 nodes are available"}]
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
struct Holds(AtomicUsize, Mutex<Vec<(String, String)>>);
impl HoldCallback for Holds {
    fn on_hold(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn progress(&self, phase: &str, detail: &str) {
        self.1.lock().unwrap().push((phase.into(), detail.into()));
    }
}
impl Holds {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
    /// (FIX-049) Detail of every progress report of `phase`.
    fn progress_of(&self, phase: &str) -> Vec<String> {
        self.1
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == phase)
            .map(|(_, d)| d.clone())
            .collect()
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
            image_pull_secrets: vec![],
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

fn warning(kind: &str, name: &str, reason: &str, message: &str, at: &str) -> Value {
    json!({"apiVersion": "v1", "kind": "Event",
           "metadata": {"name": format!("{name}.{reason}.{at}"), "namespace": NS},
           "involvedObject": {"kind": kind, "name": name, "namespace": NS},
           "reason": reason, "message": message, "type": "Warning",
           "lastTimestamp": at})
}

/// (FIX-025, coordinator: owner's production cluster) A pod waiting for an unbound home PVC
/// whose claim has a `FailedBinding` warning fails fast with `home_volume_unbound` and the
/// claim's message, without a capacity hold and without waiting the capacity wait out.
#[tokio::test]
async fn unbound_home_volume_fails_fast_with_its_own_reason() {
    let fake = Fake::with(Behaviour::PvcUnbound);
    let req = request(Uuid::now_v7(), Uuid::now_v7());
    fake.put(
        "events",
        "e1",
        warning(
            "PersistentVolumeClaim",
            &req.spec.home_ref,
            "FailedBinding",
            "no persistent volumes available for this claim and no storage class is set",
            "2026-11-10T10:00:00Z",
        ),
    );
    // A capacity wait far longer than the test: failing fast must not wait it out.
    let b = backend(&fake, 4, Duration::from_secs(600));
    let holds = Holds::default();
    let started = std::time::Instant::now();
    let err = b.start(&req, &holds).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(matches!(err, BackendError::HomeVolumeUnbound(_)), "{err:?}");
    assert_eq!(err.reason_code(), "home_volume_unbound");
    let m = err.sanitized_message();
    assert!(m.contains("no storage class is set"), "{m}");
    assert!(m.contains("runner.k8s.storageClass"), "{m}");
    assert_eq!(holds.count(), 0, "not a capacity hold");
    assert!(fake.names("pods").is_empty(), "the pod is removed");
    let ev = b.start_events(&req).await;
    assert!(ev.iter().any(|e| e.starts_with("FailedBinding:")), "{ev:?}");
}

fn normal(kind: &str, name: &str, reason: &str, message: &str, at: &str) -> Value {
    let mut e = warning(kind, name, reason, message, at);
    e["type"] = json!("Normal");
    e
}

const RACE: &str = "Error saving claim: Operation cannot be fulfilled on persistentvolumeclaims: \
    the object has been modified; please apply your changes to the latest version and try again";

/// (FIX-049) The owner's manila CephFS sequence: the claim pins a class, the in-tree controller
/// loses the annotation race (`ProvisioningFailed`), then the external provisioner works.
fn seed_manila_claim(fake: &Fake, home: &str) {
    fake.put(
        "persistentvolumeclaims",
        home,
        json!({"apiVersion": "v1", "kind": "PersistentVolumeClaim",
               "metadata": {"name": home, "labels": {"aurora.managed": "true"}},
               "spec": {"accessModes": ["ReadWriteOnce"],
                        "storageClassName": "prevessin-cephfs-ssd-a-delete",
                        "resources": {"requests": {"storage": "5Gi"}}},
               "status": {"phase": "Pending"}}),
    );
}

fn rfc3339(ago_secs: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(ago_secs))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// (FIX-049, a) `ProvisioningFailed` followed by the provisioner's own `Provisioning` /
/// `ProvisioningSucceeded` is slow provisioning, not a failure: the start waits and proceeds,
/// the pod is kept. Fails without the fix (the old code failed on the warning alone).
#[tokio::test]
async fn provisioning_failed_superseded_by_the_provisioner_does_not_fail_the_start() {
    let fake = Fake::with(Behaviour::PvcUnboundFor(3));
    let req = request(Uuid::now_v7(), Uuid::now_v7());
    let home = req.spec.home_ref.clone();
    let pvc = "PersistentVolumeClaim";
    // Old enough that only the supersession keeps the start alive.
    fake.put(
        "events",
        "e1",
        warning(pvc, &home, "ProvisioningFailed", RACE, &rfc3339(600)),
    );
    fake.put(
        "events",
        "e2",
        normal(
            pvc,
            &home,
            "Provisioning",
            "External provisioner is provisioning volume",
            &rfc3339(590),
        ),
    );
    fake.put(
        "events",
        "e3",
        normal(
            pvc,
            &home,
            "ProvisioningSucceeded",
            "Successfully provisioned volume pvc-8e931fc3",
            &rfc3339(580),
        ),
    );
    let b = backend(&fake, 4, Duration::from_secs(600));
    let holds = Holds::default();
    let c = b.start(&req, &holds).await.unwrap();
    assert!(fake.names("pods").contains(&c.0), "pod kept");
    assert_eq!(holds.count(), 0, "not a capacity hold");
    assert!(
        !holds.progress_of("provisioning").is_empty()
            || !holds.progress_of("scheduling").is_empty(),
        "waited for the volume"
    );
}

/// (FIX-049, b) A `ProvisioningFailed` that is the newest event and older than the grace fails
/// fast with the event's message; the claim pins a class, so no storage-class hint but the
/// class name.
#[tokio::test]
async fn stale_unrecovered_provisioning_failed_fails_fast_without_the_class_hint() {
    let fake = Fake::with(Behaviour::PvcUnbound);
    let req = request(Uuid::now_v7(), Uuid::now_v7());
    let home = req.spec.home_ref.clone();
    seed_manila_claim(&fake, &home);
    fake.put(
        "events",
        "e1",
        warning(
            "PersistentVolumeClaim",
            &home,
            "ProvisioningFailed",
            RACE,
            &rfc3339(300),
        ),
    );
    let b = backend(&fake, 4, Duration::from_secs(600));
    let holds = Holds::default();
    let err = b.start(&req, &holds).await.unwrap_err();
    assert!(matches!(err, BackendError::HomeVolumeUnbound(_)), "{err:?}");
    assert_eq!(err.reason_code(), "home_volume_unbound");
    let m = err.sanitized_message();
    assert!(m.contains("the object has been modified"), "{m}");
    assert!(m.contains("`prevessin-cephfs-ssd-a-delete`"), "{m}");
    assert!(!m.contains("runner.k8s.storageClass"), "{m}");
    assert!(fake.names("pods").is_empty(), "the pod is removed");
}

/// (FIX-049, c) A fresh `ProvisioningFailed` (within the grace) keeps the start waiting and
/// reports `provisioning` progress with the warning; the pod is kept.
#[tokio::test]
async fn fresh_provisioning_failed_keeps_waiting_with_provisioning_progress() {
    let fake = Fake::with(Behaviour::PvcUnboundFor(3));
    let req = request(Uuid::now_v7(), Uuid::now_v7());
    let home = req.spec.home_ref.clone();
    seed_manila_claim(&fake, &home);
    fake.put(
        "events",
        "e1",
        warning(
            "PersistentVolumeClaim",
            &home,
            "ProvisioningFailed",
            RACE,
            &rfc3339(5),
        ),
    );
    let b = backend(&fake, 4, Duration::from_secs(600));
    let holds = Holds::default();
    let c = b.start(&req, &holds).await.unwrap();
    assert!(fake.names("pods").contains(&c.0), "pod kept");
    let p = holds.progress_of("provisioning");
    assert!(!p.is_empty(), "provisioning progress reported");
    assert!(p[0].contains("the object has been modified"), "{p:?}");
}

/// (FIX-025) After `runner.k8s.storageClass` is set, a never-bound home claim with another
/// class is recreated with the configured one; a bound claim is never touched.
#[tokio::test]
async fn never_bound_home_claim_is_recreated_with_the_configured_class() {
    let fake = Fake::with(Behaviour::Run);
    let mut s = settings(4, Duration::ZERO);
    s.pod.storage_class = Some("fast".into());
    let b = KubernetesBackend::with_client(fake.client(), s);
    let (sb, sb2) = (Uuid::now_v7(), Uuid::now_v7());
    let pvc = |name: &str, status: Value, volume: Option<&str>| {
        json!({"apiVersion": "v1", "kind": "PersistentVolumeClaim",
               "metadata": {"name": name, "labels": {"aurora.managed": "true"}},
               "spec": {"accessModes": ["ReadWriteOnce"], "volumeName": volume,
                        "resources": {"requests": {"storage": "5Gi"}}},
               "status": status})
    };
    let pending = format!("home-{sb}");
    let bound = format!("home-{sb2}");
    fake.put(
        "persistentvolumeclaims",
        &pending,
        pvc(&pending, json!({"phase": "Pending"}), None),
    );
    fake.put(
        "persistentvolumeclaims",
        &bound,
        pvc(&bound, json!({"phase": "Bound"}), Some("pv-1")),
    );
    b.provision_home(sb, &pending, 5).await.unwrap();
    b.provision_home(sb2, &bound, 5).await.unwrap();
    let now = fake.get("persistentvolumeclaims", &pending).unwrap();
    assert_eq!(now["spec"]["storageClassName"], "fast", "recreated");
    let kept = fake.get("persistentvolumeclaims", &bound).unwrap();
    assert!(
        kept["spec"]["storageClassName"].is_null(),
        "bound claim untouched"
    );
    let deletes = fake.0.lock().unwrap().deletes.clone();
    assert!(
        deletes
            .iter()
            .any(|(k, n, _)| k == "persistentvolumeclaims" && *n == pending)
    );
    assert!(!deletes.iter().any(|(_, n, _)| *n == bound));
}

/// (FIX-025, C-581) Admission refusal is `pod_refused` with the API server's message.
#[tokio::test]
async fn refused_pod_creation_is_pod_refused_with_the_server_message() {
    let fake = Fake::with(Behaviour::Refused);
    let b = backend(&fake, 4, Duration::from_secs(60));
    let err = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &Holds::default())
        .await
        .unwrap_err();
    assert_eq!(err.reason_code(), "pod_refused", "{err:?}");
    assert!(err.sanitized_message().contains("violates PodSecurity"));
    assert!(fake.names("pods").is_empty());
    assert!(fake.names("secrets").is_empty(), "boot Secret removed");
}

/// (FIX-025, C-581) A pod that ends while starting is `pod_failed` with exit code and reason.
#[tokio::test]
async fn pod_ending_while_starting_is_pod_failed_with_exit_code() {
    let fake = Fake::with(Behaviour::Terminated);
    let b = backend(&fake, 4, Duration::ZERO);
    let err = b
        .start(&request(Uuid::now_v7(), Uuid::now_v7()), &Holds::default())
        .await
        .unwrap_err();
    assert_eq!(err.reason_code(), "pod_failed", "{err:?}");
    let m = err.sanitized_message();
    assert!(m.contains("exit code 1 (Error)"), "{m}");
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

// FIX-011 (C-191): `runner.sandbox_*` / `runner.home_gib` become the advertised defaults
// (docs/15: 1 CPU, 2048 MiB, 512 pids, 10 GiB).
#[test]
fn runner_settings_advertise_the_configured_sandbox_defaults() {
    let s = aurora_runner::runner::RunnerSettings::from_config(&runner_config(""));
    let d = s.defaults.unwrap();
    assert_eq!(
        (d.cpu_millis, d.memory_mib, d.pids, d.home_gib),
        (1000, 2048, 512, 10)
    );
}

#[test]
fn settings_come_from_config() {
    let cfg = runner_config(
        "namespace = \"sbx\"\ncore_namespace = \"core\"\nstorage_class = \"fast\"\n\
         runtime_class = \"gvisor\"\ngateway_service = \"gw\"\nrunner_service = \"rn\"\n\
         node_selector = { pool = \"sandbox\" }\n\
         image_pull_secrets = [\"regcred\", \"second\"]\n\
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
    assert_eq!(s.pod.image_pull_secrets, ["regcred", "second"]);
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

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! OPS-011: hosted e2e against a real Kubernetes cluster (kind) running the Helm chart with the
//! `kubernetes` runner backend and NetworkPolicy enforcement. Black box: only HTTP to the API
//! (via `kubectl port-forward`) and `kubectl`. Gated by AURORA_TEST_KIND=1; run via
//! `scripts/e2e/kind.sh`.
//!
//! Env (set by kind.sh): AURORA_KIND_API (base URL), AURORA_KIND_ORIGIN (= server.external_url),
//! AURORA_KIND_CONTEXT, AURORA_KIND_NS (release namespace), AURORA_KIND_SBX_NS, AURORA_KIND_FULL
//! (chart fullname).
mod common;

use common::Api;
use serde_json::{Value, json};
use std::process::Command;
use std::time::{Duration, Instant};

fn enabled() -> bool {
    let on = std::env::var("AURORA_TEST_KIND").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: AURORA_TEST_KIND != 1");
    }
    on
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} not set: run scripts/e2e/kind.sh"))
}

fn kubectl(args: &[&str]) -> (i32, String) {
    let o = Command::new("kubectl")
        .arg("--context")
        .arg(env("AURORA_KIND_CONTEXT"))
        .args(args)
        .output()
        .unwrap();
    let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    (o.status.code().unwrap_or(-1), s)
}

fn sbx_ns() -> String {
    env("AURORA_KIND_SBX_NS")
}

async fn connect() -> Api {
    let email = format!("o{}@e2e.test", uuid::Uuid::now_v7().simple());
    Api::connect(&env("AURORA_KIND_API"), &env("AURORA_KIND_ORIGIN"), &email).await
}

fn turn_id(v: &Value) -> String {
    v["turn"]["id"].as_str().unwrap().to_owned()
}

/// Running pods of one boot (label `aurora.boot`).
fn boot_pods(boot: &str) -> Vec<String> {
    let (_, out) = kubectl(&[
        "-n",
        &sbx_ns(),
        "get",
        "pods",
        "-l",
        &format!("aurora.boot={boot}"),
        "-o",
        "jsonpath={range .items[*]}{.metadata.name}{\" \"}{.status.phase}{\"\\n\"}{end}",
    ]);
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (n, ph) = (it.next()?, it.next()?);
            (ph == "Running").then(|| n.to_owned())
        })
        .collect()
}

/// All pods in the sandboxes namespace, any phase, for diagnostics.
fn all_pods() -> String {
    kubectl(&[
        "-n",
        &sbx_ns(),
        "get",
        "pods",
        "-o",
        "wide",
        "--show-labels",
    ])
    .1
}

/// Home PVC names of the sandbox with this id (`home-<sandbox_id>`).
fn pvc_exists(sandbox_id: &str) -> bool {
    kubectl(&["-n", &sbx_ns(), "get", "pvc", &format!("home-{sandbox_id}")]).0 == 0
}

/// Wait for a sandbox boot and its single Running pod; returns (boot_id, pod name).
async fn wait_boot_pod(api: &Api, secs: u64) -> (String, String) {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let sb = api.sandbox().await;
        if let Some(boot) = sb["boot_id"].as_str() {
            let p = boot_pods(boot);
            if p.len() == 1 {
                return (boot.to_owned(), p[0].clone());
            }
        }
        assert!(
            Instant::now() < end,
            "no running sandbox pod: {sb}\n{}",
            all_pods()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn sandbox_id_of(pod: &str) -> String {
    kubectl(&[
        "-n",
        &sbx_ns(),
        "get",
        "pod",
        pod,
        "-o",
        "jsonpath={.metadata.labels.aurora\\.sandbox}",
    ])
    .1
    .trim()
    .to_owned()
}

async fn wait_pod_gone(boot: &str, secs: u64) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !boot_pods(boot).is_empty() {
        assert!(
            Instant::now() < end,
            "pod of boot {boot} still there\n{}",
            all_pods()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// `kubectl exec` into the sandbox pod (runs as the pod user, uid 1000).
fn exec_in(pod: &str, script: &str) -> (i32, String) {
    kubectl(&[
        "-n",
        &sbx_ns(),
        "exec",
        pod,
        "-c",
        "sandbox",
        "--",
        "bash",
        "-c",
        script,
    ])
}

fn can_connect(pod: &str, host: &str, port: u16) -> bool {
    exec_in(pod, &format!("timeout 4 bash -c '</dev/tcp/{host}/{port}'")).0 == 0
}

async fn say_and_finish(api: &Api, conv: &str, text: &str) -> String {
    let v = api.say(conv, text).await;
    let turn = turn_id(&v);
    let st = api.wait_finished(conv, &turn, 180).await;
    assert_eq!(st, "done", "turn {turn}");
    turn
}

fn transcript(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn k1_chat_turn_answered_in_sandbox_pod() {
    if !enabled() {
        return;
    }
    let api = connect().await;
    let conv = api.direct().await;
    say_and_finish(&api, &conv, "[[say: hello from daneel on k8s]]").await;
    let text = transcript(&api.events(&conv).await);
    assert!(text.contains("hello from daneel on k8s"), "{text}");
    let (boot, pod) = wait_boot_pod(&api, 30).await;
    eprintln!("[k1] boot {boot} runs in pod {pod}\n{}", all_pods());
    assert!(pvc_exists(&sandbox_id_of(&pod)), "home PVC");
}

#[tokio::test]
async fn k2_idle_park_keeps_home_and_next_message_wakes() {
    if !enabled() {
        return;
    }
    let api = connect().await;
    let conv = api.direct().await;
    let v = api.say(&conv, "[[sleep: 15s]] [[say: one]]").await;
    let turn = turn_id(&v);
    let (boot1, pod) = wait_boot_pod(&api, 180).await;
    let sbx = sandbox_id_of(&pod);
    let (rc, out) = exec_in(&pod, "echo persisted > /workspace/marker");
    assert_eq!(rc, 0, "{out}");
    assert_eq!(api.wait_finished(&conv, &turn, 120).await, "done");
    // idle_stop_secs is short in values-e2e.yaml
    api.wait_sandbox_state("stopped", 180).await;
    wait_pod_gone(&boot1, 60).await;
    assert!(pvc_exists(&sbx), "home PVC must survive the park");
    let v = api.say(&conv, "[[sleep: 15s]] [[say: two]]").await;
    let turn = turn_id(&v);
    let end = Instant::now() + Duration::from_secs(180);
    let (boot2, pod2) = loop {
        let (b, p) = wait_boot_pod(&api, 180).await;
        if b != boot1 {
            break (b, p);
        }
        assert!(Instant::now() < end, "no new boot");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert_ne!(boot1, boot2, "wake mints a new boot");
    let (rc, out) = exec_in(&pod2, "cat /workspace/marker");
    assert_eq!((rc, out.trim()), (0, "persisted"));
    assert_eq!(api.wait_finished(&conv, &turn, 120).await, "done");
}

#[tokio::test]
async fn k3_restart_fails_active_turn_and_boots_anew() {
    if !enabled() {
        return;
    }
    let api = connect().await;
    let conv = api.direct().await;
    say_and_finish(&api, &conv, "[[say: warm]]").await;
    let (boot1, _) = wait_boot_pod(&api, 30).await;
    let v = api.say(&conv, "[[sleep: 120s]]").await;
    let turn = turn_id(&v);
    // wait until the sleep turn is really running in the pod
    tokio::time::sleep(Duration::from_secs(5)).await;
    let (s, b) = api
        .post(&format!("/v1/agents/{}/restart", api.daneel), json!({}))
        .await;
    assert_eq!(s, 202, "{b}");
    assert_eq!(
        api.wait_finished(&conv, &turn, 60).await,
        "failed:agent_restarted"
    );
    say_and_finish(&api, &conv, "[[say: back]]").await;
    let (boot2, _) = wait_boot_pod(&api, 60).await;
    assert_ne!(boot1, boot2);
    wait_pod_gone(&boot1, 60).await;
}

fn pod_ip(ns: &str, selector: &str) -> String {
    let (_, out) = kubectl(&[
        "-n",
        ns,
        "get",
        "pod",
        "-l",
        selector,
        "-o",
        "jsonpath={.items[0].status.podIP}",
    ]);
    assert!(!out.trim().is_empty(), "no pod for {selector}");
    out.trim().to_owned()
}

fn svc_ip(ns: &str, name: &str) -> String {
    let (_, out) = kubectl(&[
        "-n",
        ns,
        "get",
        "svc",
        name,
        "-o",
        "jsonpath={.spec.clusterIP}",
    ]);
    out.trim().to_owned()
}

/// SEC-02 / SEC-09 from inside a sandbox Pod. Positive controls (the two allowed ports) prove
/// the probe method works, so a blocked connection is evidence of enforcement.
#[tokio::test]
async fn k4_sec02_sec09_probes_from_sandbox_pod() {
    if !enabled() {
        return;
    }
    let ns = env("AURORA_KIND_NS");
    let full = env("AURORA_KIND_FULL");
    let api = connect().await;
    let conv = api.direct().await;
    let v = api.say(&conv, "[[sleep: 150s]]").await;
    let turn = turn_id(&v);
    let (_boot, pod) = wait_boot_pod(&api, 180).await;
    let pod = pod.as_str();
    let comp =
        |c: &str| format!("app.kubernetes.io/instance=aurora,app.kubernetes.io/component={c}");
    let gw = pod_ip(&ns, &comp("gateway"));
    let runner = pod_ip(&ns, &comp("runner"));
    let apip = pod_ip(&ns, &comp("api"));
    let pg = pod_ip(&ns, &comp("postgres"));
    let hook = pod_ip(&ns, &comp("hookhost"));
    let gw_svc = svc_ip(&ns, &format!("{full}-gateway-proxy"));
    let dns_svc = svc_ip("kube-system", "kube-dns");
    let kube_svc = svc_ip("default", "kubernetes");
    let (_, node_ip) = kubectl(&[
        "get",
        "nodes",
        "-o",
        "jsonpath={.items[0].status.addresses[?(@.type==\"InternalIP\")].address}",
    ]);
    let node_ip = node_ip.trim().to_owned();

    let mut fails: Vec<String> = vec![];
    let mut check = |name: &str, ok: bool, detail: String| {
        eprintln!("[probe] {name}: {}", if ok { "ok" } else { "FAIL" });
        if !ok {
            fails.push(format!("{name}: {detail}"));
        }
    };

    // Positive controls: exactly gateway:4102 and runner:4105 are reachable.
    check(
        "allowed gateway pod :4102",
        can_connect(pod, &gw, 4102),
        gw.clone(),
    );
    check(
        "allowed gateway service :4102",
        can_connect(pod, &gw_svc, 4102),
        gw_svc.clone(),
    );
    check(
        "allowed runner pod :4105",
        can_connect(pod, &runner, 4105),
        runner.clone(),
    );
    // Blocked: everything else (negative controls).
    for (name, host, port) in [
        ("gateway :4103 (SEC-09)", &gw, 4103),
        ("runner :4106 (SEC-09)", &runner, 4106),
        ("api :4101", &apip, 4101),
        ("api :4111", &apip, 4111),
        ("postgres :5432", &pg, 5432),
        ("hookhost :4104", &hook, 4104),
        ("kube-dns tcp :53", &dns_svc, 53),
        ("kubernetes service :443", &kube_svc, 443),
        ("kube-apiserver node :6443", &node_ip, 6443),
        ("external 1.1.1.1 :443", &"1.1.1.1".to_owned(), 443),
    ] {
        check(
            &format!("blocked {name}"),
            !can_connect(pod, host, port),
            format!("{host}:{port} reachable"),
        );
    }
    // No DNS.
    let (rc, out) = exec_in(
        pod,
        "getent hosts kubernetes.default.svc.cluster.local example.org",
    );
    check("no DNS resolution", rc != 0, out);
    // No service account token, no API credentials.
    let (rc, out) = exec_in(
        pod,
        "ls /var/run/secrets/kubernetes.io/serviceaccount/ 2>&1; test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token",
    );
    check("no service account token", rc == 0, out);
    // Non-root, read-only rootfs, writable home.
    let (rc, out) = exec_in(pod, "test \"$(id -u)\" != 0 && id -u");
    check("non-root", rc == 0, out);
    let (rc, out) = exec_in(pod, "touch /etc/aurora-probe 2>&1");
    check("read-only rootfs", rc != 0, out);
    let (rc, out) = exec_in(pod, "touch /workspace/ok && touch /tmp/ok 2>&1");
    check("workspace and tmp writable", rc == 0, out);
    // The Pod spec says the same (restricted PSA, no token, no DNS).
    let (_, spec) = kubectl(&["-n", &sbx_ns(), "get", "pod", pod, "-o", "json"]);
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let ps = &spec["spec"];
    let cs = &ps["containers"][0]["securityContext"];
    check(
        "spec automountServiceAccountToken=false",
        ps["automountServiceAccountToken"] == false,
        ps["automountServiceAccountToken"].to_string(),
    );
    check(
        "spec dnsPolicy=None",
        ps["dnsPolicy"] == "None",
        ps["dnsPolicy"].to_string(),
    );
    check(
        "spec readOnlyRootFilesystem",
        cs["readOnlyRootFilesystem"] == true,
        cs.to_string(),
    );
    check(
        "spec runAsNonRoot",
        cs["runAsNonRoot"] == true,
        cs.to_string(),
    );
    check(
        "spec caps dropped",
        cs["capabilities"]["drop"] == json!(["ALL"]),
        cs.to_string(),
    );
    check(
        "spec not privileged",
        cs["privileged"] == false,
        cs.to_string(),
    );
    let projected = ps["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.get("projected").is_some());
    check(
        "spec has no projected token volume",
        !projected,
        ps["volumes"].to_string(),
    );
    // Gateway-less internal ops: API from the in-cluster side stays closed to the sandbox ns.
    let (_, np) = kubectl(&["-n", &sbx_ns(), "get", "networkpolicy", "-o", "name"]);
    check(
        "sandbox NetworkPolicies present",
        np.contains("default-deny") && np.contains("allow-gateway-runner"),
        np,
    );
    drop(check);

    // Let the long turn end so later tests start clean.
    let _ = api
        .post(&format!("/v1/turns/{turn}/abort"), json!({}))
        .await;
    assert!(
        fails.is_empty(),
        "probe failures:\n{}",
        fails.join("\n---\n")
    );
}

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
use reqwest::Method;
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

// ---------------------------------------------------------------------------------------------
// P4-PRV-2 (docs/18 §Port previews, E2E): a preview round trip through the deployed relay —
// share with a start command, the supervisor's on-demand `preview.start`, the grant exchange,
// HTTP and WebSocket through the stream listener :4108. Only runs when kind.sh exported
// AURORA_KIND_RELAY_URL (AURORA_E2E_PREVIEWS_DOMAIN); the port-forwarded relay is addressed
// with an explicit Host header (the suffix is synthetic, nothing resolves it).
// ---------------------------------------------------------------------------------------------

/// Relay edge base (http://127.0.0.1:<port>) and the preview DNS suffix; None = stock run.
fn previews_target() -> Option<(String, String)> {
    match (
        std::env::var("AURORA_KIND_RELAY_URL"),
        std::env::var("AURORA_KIND_PREVIEWS_DOMAIN"),
    ) {
        (Ok(u), Ok(d)) if !u.is_empty() && !d.is_empty() => Some((u, d)),
        _ => {
            eprintln!(
                "skipped: AURORA_KIND_RELAY_URL/AURORA_KIND_PREVIEWS_DOMAIN not set (previews off)"
            );
            None
        }
    }
}

/// One raw request to the port-forwarded relay edge with an explicit `Host`; returns
/// (status, headers, body-bytes).
async fn relay_get(
    base: &str,
    host: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut tcp = tokio::net::TcpStream::connect(
        base.trim_start_matches("http://")
            .split_once(':')
            .map(|(h, p)| (h.to_owned(), p.parse::<u16>().unwrap()))
            .unwrap(),
    )
    .await
    .unwrap();
    let mut req = format!("GET {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n");
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    tokio::io::AsyncWriteExt::write_all(&mut tcp, req.as_bytes())
        .await
        .unwrap();
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::io::AsyncReadExt::read_to_end(&mut tcp, &mut raw),
    )
    .await
    .unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(raw.len());
    let text = String::from_utf8_lossy(&raw[..split]);
    let mut lines = text.split("\r\n");
    let status: u16 = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect();
    (status, headers, raw[split + 4..].to_vec())
}

#[tokio::test]
async fn k6_preview_round_trip_http_and_websocket() {
    let Some((relay_base, domain)) = previews_target() else {
        return;
    };
    let api = connect().await;
    // A first turn boots Daneel's sandbox; the share below needs a running sandbox for the
    // on-demand `preview.start` (a parked one would also wake, but don't race the park).
    say_and_finish(&api, &api.direct().await, "[[say: k6 boot]]").await;

    // The org switch for previews (the instance switch is chart-level; this one is per org).
    let (s, v) = api
        .patch(
            &format!("/v1/orgs/{}", api.org),
            json!({"settings": {"previews_enabled": true}}),
        )
        .await;
    assert_eq!(s, 200, "enable previews on the org: {v}");

    // The share on Daneel (running sandbox): port 3000 and the on-demand start command.
    let (s, share) = api
        .post(
            &format!("/v1/agents/{}/previews", api.daneel),
            // `cwd: "."` = /workspace (exists in the sandbox image); a subdir like `app` would
            // make the spawn fail (no such directory) and the share would stay not_listening.
            json!({"port": 3000, "start_command": "preview-echo 3000", "cwd": "."}),
        )
        .await;
    assert_eq!(s, 201, "create share: {v}");
    let label = share["label"].as_str().unwrap().to_owned();
    let share_id = share["id"].as_str().unwrap().to_owned();
    let host = format!("{label}.{domain}");
    eprintln!("[k6] share {share_id} label {label} host {host}");

    // Start the app (C-544): wakes the sandbox if parked and makes arm 5 enqueue the
    // supervisor's on-demand `preview.start`; management `create` never starts anything.
    let (s, v) = api
        .post(&format!("/v1/previews/{share_id}/start"), json!({}))
        .await;
    assert_eq!(s, 202, "start share: {v}");

    // Wait for the derived `listening` status (arm 5's `preview.start` + the supervisor's
    // in-sandbox exec, the home part applies within a tick).
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        let (s, v) = api.get(&format!("/v1/previews/{share_id}")).await;
        assert_eq!(s, 200, "{v}");
        let status = v["status"].as_str().unwrap_or_default().to_owned();
        if status == "listening" {
            break;
        }
        assert!(
            Instant::now() < end,
            "share never reached `listening` (last: {status}): {v}\n{}",
            all_pods()
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    eprintln!("[k6] share listening");

    // ---- leg-1 reachability diagnostics ----------------------------------------------------
    // A 504 ("The sandbox timed out.") is the relay's 10 s dial budget to the runner's stream
    // listener. Reproduce the relay→runner path from a throwaway pod with the relay's exact
    // labels (the NetworkPolicy peer match): probe the stream port through the Service and
    // the pod IP, plus an api:4101 positive control. CONNECT = the path is open (the app is
    // at fault), REFUSED = closed port, HANG (rc=1/124) = dropped SYN (a policy drop).
    {
        let comp =
            |c: &str| format!("app.kubernetes.io/instance=aurora,app.kubernetes.io/component={c}");
        let ns = env("AURORA_KIND_NS");
        // The relay policy's podSelector.matchLabels is all three labels (name+instance+
        // component) — the probe pod must carry the exact set or the policy does not select
        // it and the probe proves nothing.
        let (rc, out) = kubectl(&[
            "-n",
            &ns,
            "run",
            "k6-probe",
            "--labels",
            "app.kubernetes.io/name=aurora,app.kubernetes.io/instance=aurora,app.kubernetes.io/component=relay",
            "--image",
            "registry.k8s.io/e2e-test-images/agnhost:2.53",
            "--command",
            "--",
            "sh",
            "-c",
            "sleep 60",
            "-o",
            "name",
        ]);
        if rc == 0 && out.trim().starts_with("pod/") {
            let _ = kubectl(&[
                "-n",
                &ns,
                "wait",
                "--for=condition=Ready",
                "pod/k6-probe",
                "--timeout=60s",
            ]);
            for (name, host, port) in [
                ("runner svc :4108", "aurora-runner".to_owned(), 4108),
                ("runner pod :4108", pod_ip(&ns, &comp("runner")), 4108),
                ("api svc :4101 (control)", "aurora-api".to_owned(), 4101),
            ] {
                // `curl http://…` completes the TCP connect for any answer (even a protocol
                // error: rc 0/8/56); refused is rc 7, a dropped SYN times out with rc 28.
                let (_, out) = kubectl(&[
                    "-n",
                    &ns,
                    "exec",
                    "pod/k6-probe",
                    "--",
                    "sh",
                    "-c",
                    &format!(
                        "curl -s -m 4 -o /dev/null http://{host}:{port}/v1/preview-stream; echo rc=$?"
                    ),
                ]);
                let rc = out.trim().trim_start_matches("rc=").to_owned();
                let verdict = match rc.as_str() {
                    "7" => "REFUSED (closed port)",
                    "28" => "HANG (dropped SYN / policy)",
                    _ => "CONNECT",
                };
                eprintln!("[k6] relay-path probe {name}: {verdict} (rc={rc})");
            }
            let _ = kubectl(&["-n", &ns, "delete", "pod/k6-probe", "--wait=false"]);
        } else {
            eprintln!("[k6] relay-path probe pod could not be created: {out}");
        }
    }

    // ---- the grant exchange --------------------------------------------------------------
    // Top-level navigation without a credential: the relay's login redirect sets the state
    // cookie and points at the dashboard's authorize.
    let (status, headers, _) = relay_get(&relay_base, &host, "/", &[]).await;
    assert_eq!(status, 302, "expected the login redirect");
    let pstate = headers
        .iter()
        .find(|(k, v)| k == "set-cookie" && v.starts_with("__Host-aurora_pstate="))
        .map(|(_, v)| v.split(';').next().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no pstate cookie: {headers:?}"));
    let loc = headers
        .iter()
        .find(|(k, _)| k == "location")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        loc.contains("/v1/previews/authorize?host="),
        "authorize redirect: {loc}"
    );
    let nonce = loc
        .split("state=")
        .nth(1)
        .unwrap_or_default()
        .split('&')
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(!nonce.is_empty(), "no state nonce in {loc}");

    // The dashboard's authorize (with the session cookie): a 302 to the preview host's
    // `/_aurora/grant?g=…`.
    let (status, headers, _) = api
        .no_redirect(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state={nonce}"),
        )
        .await;
    assert_eq!(status, 302, "authorize must redirect");
    let loc = headers
        .iter()
        .find(|(k, _)| k == "location")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let grant_prefix = format!("https://{host}/_aurora/grant?g=");
    assert!(loc.starts_with(&grant_prefix), "authorize location: {loc}");
    let grant = loc[grant_prefix.len()..].to_owned();

    // Redeem through the relay edge with the pstate cookie: the preview session cookie.
    let (status, headers, body) = relay_get(
        &relay_base,
        &host,
        &format!("/_aurora/grant?g={grant}"),
        &[("cookie", &pstate)],
    )
    .await;
    assert_eq!(
        status,
        302,
        "grant redeem: {}",
        String::from_utf8_lossy(&body)
    );
    let preview_cookie = headers
        .iter()
        .find(|(k, v)| k == "set-cookie" && v.starts_with("__Host-aurora_preview="))
        .map(|(_, v)| v.split(';').next().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no preview cookie: {headers:?}"));

    // ---- the HTTP round trip --------------------------------------------------------------
    // A plain GET reaches `preview-echo` inside the sandbox through :4112 → runner :4108 →
    // the supervisor's stream engine → 127.0.0.1:3000. A POST with a body echoes through too.
    for (path, body_text) in [("/", ""), ("/hello?x=1", "round-trip")] {
        let mut tcp = tokio::net::TcpStream::connect(
            relay_base
                .trim_start_matches("http://")
                .split_once(':')
                .map(|(h, p)| (h.to_owned(), p.parse::<u16>().unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
        let req = format!(
            "GET {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\ncookie: {preview_cookie}\r\ncontent-length: {}\r\n\r\n{body_text}",
            body_text.len()
        );
        tokio::io::AsyncWriteExt::write_all(&mut tcp, req.as_bytes())
            .await
            .unwrap();
        let mut raw = Vec::new();
        let _ = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::io::AsyncReadExt::read_to_end(&mut tcp, &mut raw),
        )
        .await
        .unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.starts_with("HTTP/1.1 200 OK\r\n"),
            "round trip {path}: {text}"
        );
        let want_path = format!(r#""path":"{path}""#);
        assert!(
            text.contains(&want_path) && text.contains(body_text),
            "echo missing in {path}: {text}"
        );
        // Header hygiene through the real stack: the app never sees the Aurora cookie.
        assert!(
            !text.contains("__Host-aurora_preview"),
            "preview cookie leaked to the app: {text}"
        );
    }
    eprintln!("[k6] http round trip ok");

    // ---- the WebSocket round trip ----------------------------------------------------------
    // The browser handshake (Origin = the preview's own) → 101 → unmasked app echo frames.
    let (status, _, body) = ws_round_trip(&relay_base, &host, &preview_cookie).await;
    assert_eq!(status, 101, "ws upgrade: {body}");
    eprintln!("[k6] websocket round trip ok");

    // Fetch Metadata: a same-site fetch subrequest is refused before the app.
    let (status, _, _) = relay_get(
        &relay_base,
        &host,
        "/",
        &[("cookie", &preview_cookie), ("sec-fetch-site", "same-site")],
    )
    .await;
    assert_eq!(status, 403, "same-site fetch must be refused");

    // Clean up: the share row goes away; the sandbox idles out as usual.
    let (s, v) = api.delete(&format!("/v1/previews/{share_id}")).await;
    assert_eq!(s, 204, "{v}");
}

/// A browser-style WebSocket round trip against the relay edge: raw TCP, a masked handshake,
/// then a masked text frame whose unmasked echo comes back from the app.
async fn ws_round_trip(
    base: &str,
    host: &str,
    cookie: &str,
) -> (u16, Vec<(String, String)>, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = base
        .trim_start_matches("http://")
        .split_once(':')
        .map(|(h, p)| (h.to_owned(), p.parse::<u16>().unwrap()))
        .unwrap();
    let mut tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /ws HTTP/1.1\r\nhost: {host}\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\norigin: https://{host}\r\ncookie: {cookie}\r\n\r\n"
    );
    tcp.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if tcp.read_exact(&mut byte).await.is_err() {
            break;
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head_text = String::from_utf8_lossy(&head).to_string();
    let status: u16 = head_text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = head_text
        .split("\r\n")
        .skip(1)
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect();
    if status != 101 {
        // Read the error page for diagnostics.
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut rest)).await;
        return (status, headers, String::from_utf8_lossy(&rest).to_string());
    }
    // Masked text frame "ws-echo" (mask 01020304), then the unmasked echo.
    let mask = [1u8, 2, 3, 4];
    let payload: Vec<u8> = b"ws-echo"
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect();
    let mut frame = vec![0x81u8, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend_from_slice(&payload);
    tcp.write_all(&frame).await.unwrap();
    // A single `read()` may return only the 2-byte header (the app writes header and payload
    // separately, and the relay forwards them as it gets them — AUDIT-004 saw `[129, 7]`):
    // read the header, then exactly the payload length (FIX-060). The echo is a short
    // unmasked server frame, so the 7-bit length form is all that is needed.
    let mut head = [0u8; 2];
    let mut body = Vec::new();
    let got = tokio::time::timeout(Duration::from_secs(15), async {
        tcp.read_exact(&mut head).await?;
        body = vec![0u8; (head[1] & 0x7f) as usize];
        tcp.read_exact(&mut body).await
    })
    .await;
    let echoed =
        matches!(got, Ok(Ok(_))) && head[0] == 0x81 && head[1] & 0x7f < 126 && body == b"ws-echo";
    let _ = tcp.write_all(&[0x88, 0x00]).await; // close
    assert!(
        echoed,
        "no ws echo from the app (read {got:?}, header {head:?}, payload {:?})",
        &body[..body.len().min(32)]
    );
    (status, headers, String::new())
}

/// `kubectl port-forward` to the connector host's ops port (it has no Service: the test control
/// API `/test/…` is for tests only and is enabled by `connectorHost.testTypes`).
struct HostOps {
    child: std::process::Child,
    base: String,
    http: reqwest::Client,
}

impl HostOps {
    async fn open() -> Self {
        let port = std::env::var("AURORA_KIND_HOST_OPS_PORT").unwrap_or_else(|_| "14192".into());
        let child = Command::new("kubectl")
            .args(["--context", &env("AURORA_KIND_CONTEXT")])
            .args(["-n", &env("AURORA_KIND_NS")])
            .arg("port-forward")
            .arg(format!("deploy/{}-connector-host", env("AURORA_KIND_FULL")))
            .arg(format!("{port}:4192"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("kubectl port-forward");
        let me = Self {
            child,
            base: format!("http://127.0.0.1:{port}"),
            http: reqwest::Client::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if me.get("/healthz").await.0 == 200 {
                return me;
            }
            assert!(
                Instant::now() < deadline,
                "connector host ops port unreachable"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn get(&self, p: &str) -> (u16, Value) {
        match self
            .http
            .get(format!("{}{p}", self.base))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(r) => (r.status().as_u16(), r.json().await.unwrap_or(Value::Null)),
            Err(_) => (0, Value::Null),
        }
    }

    async fn post(&self, p: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}{p}", self.base))
            .json(&body)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
}

impl Drop for HostOps {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs SQL in the evaluation Postgres Pod (the chart's `postgres.enabled`).
fn psql(sql: &str) {
    let pod = std::env::var("AURORA_KIND_PG_POD")
        .unwrap_or_else(|_| format!("{}-postgres-0", env("AURORA_KIND_FULL")));
    let (rc, out) = kubectl(&[
        "-n",
        &env("AURORA_KIND_NS"),
        "exec",
        &pod,
        "--",
        "psql",
        "-U",
        "aurora",
        "-d",
        "aurora",
        "-v",
        "ON_ERROR_STOP=1",
        "-c",
        sql,
    ]);
    assert_eq!(rc, 0, "psql failed: {out}");
}

async fn eventually<F, Fut>(what: &str, secs: u64, mut f: F) -> Value
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<Value>>,
{
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
}

#[tokio::test]
async fn k5_managed_fake_connector_round_trip_and_disable() {
    if !enabled() {
        return;
    }
    let owner = format!("m{}@e2e.test", uuid::Uuid::now_v7().simple());
    let me = Api::connect(&env("AURORA_KIND_API"), &env("AURORA_KIND_ORIGIN"), &owner).await;
    psql(&format!(
        "UPDATE users SET is_instance_admin = true, email_verified_at = now() WHERE email = '{owner}'"
    ));
    // The session cache holds the flag as of sign-in: sign in again.
    let admin = me.relogin(&owner).await;
    let host = HostOps::open().await;

    // The host signs the `fake` test type with an ephemeral key; trust it for its publisher, as
    // an admin would trust any third-party publisher (the project key never signs a test type).
    let (s, key) = host.get("/test/signing-key").await;
    assert_eq!(s, 200, "{key}");
    let (s, v) = admin
        .post(
            "/v1/admin/trusted-keys",
            json!({"publisher": key["publisher"], "name": "e2e-fake-host",
                   "public_key": key["key_id"]}),
        )
        .await;
    assert_eq!(s, 201, "trusted key: {v}");

    // The catalogue: the type appears at the host's next heartbeat (30 s).
    let ty = eventually("the fake type in the catalogue", 120, || async {
        let (s, v) = admin.get("/v1/admin/connector-types").await;
        assert_eq!(s, 200, "{v}");
        if v["host_running"] != true {
            return None;
        }
        v["items"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["type_id"] == "fake").cloned())
    })
    .await;
    assert_eq!(ty["added"], false);
    let fields: Vec<&str> = ty["instance_setup"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["type"].as_str())
        .collect();
    assert!(
        fields.contains(&"secret") && fields.contains(&"generated_secret"),
        "{fields:?}"
    );

    // Add it as a managed connector: the reviewed digest is approved in the same request and no
    // token comes back.
    let (s, v) = admin
        .post(
            "/v1/admin/connectors",
            json!({"id": "fake", "managed": true,
                   "approve_manifest_digest": ty["manifest_digest"],
                   "settings": {"nickname": "e2e"},
                   "secrets": {"api_key": "e2e-api-key-value"}}),
        )
        .await;
    assert_eq!(s, 201, "add managed connector: {v}");
    assert!(v.get("token").is_none_or(Value::is_null), "no token: {v}");
    assert!(
        !v.to_string().contains("e2e-api-key-value"),
        "no secret value in the response: {v}"
    );
    eventually("the connector active and running", 120, || async {
        let (s, v) = admin.get("/v1/admin/connectors").await;
        assert_eq!(s, 200, "{v}");
        let c = v["items"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["id"] == "fake").cloned())?;
        let running = c["host_status"]
            .as_array()
            .is_some_and(|h| h.iter().any(|x| x["state"] == "running"));
        (c["status"] == "active" && running).then_some(c)
    })
    .await;

    // An integration through the managed connector, one message round trip.
    let (s, v) = admin
        .post(
            &format!("/v1/orgs/{}/channel-integrations", me.org),
            json!({"connector_id": "fake", "config": {"verified_email_autolink": true},
                   "credentials": {"token": "e2e-token", "shared_bot": true}}),
        )
        .await;
    assert_eq!(s, 201, "create integration: {v}");
    assert_eq!(v["status"], "active", "{v}");
    let integ = v["id"].as_str().unwrap().to_owned();
    let (s, v) = admin
        .post(
            &format!("/v1/agents/{}/presences", me.daneel),
            json!({"integration_id": integ, "mode": "shared"}),
        )
        .await;
    assert_eq!(s, 201, "attach presence: {v}");
    let _conv = admin.direct().await;
    eventually(
        "the managed connector serving the integration",
        120,
        || async {
            let (_, st) = host.get("/test/fake/fake/status").await;
            st["integrations"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
                .then_some(st)
        },
    )
    .await;
    for (r, email, verified, bot) in [
        ("B1", None, false, true),
        ("U1", Some(owner.as_str()), true, false),
    ] {
        let (s, v) = host
            .post(
                "/test/fake/fake/users",
                json!({"ref": r, "display_name": r, "email": email,
                       "email_verified": verified, "is_bot": bot}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
    let (s, v) = host
        .post(
            "/test/fake/fake/messages",
            json!({"author_ref": "U1", "text": "[[say: hello through the managed connector]]",
                   "event_id": format!("ev-{}", uuid::Uuid::now_v7()),
                   "message_ref": format!("x-{}", uuid::Uuid::now_v7())}),
        )
        .await;
    assert_eq!((s, v["ack"].as_str()), (200, Some("accepted")), "{v}");
    eventually("the answer posted on the platform", 180, || async {
        let (_, p) = host.get("/test/fake/fake/posted?space_ref=dm-U1").await;
        p["messages"]
            .as_array()?
            .iter()
            .find(|m| {
                m["author_ref"]
                    .as_str()
                    .is_some_and(|a| a == "shared-bot" || a.starts_with('B'))
                    && m["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("hello through the managed connector"))
            })
            .cloned()
    })
    .await;

    // Disable: the host stops the instance (its callbacks and test control vanish) and the
    // connector is `disabled` for the core.
    let (s, v) = admin
        .patch("/v1/admin/connectors/fake", json!({"enabled": false}))
        .await;
    assert_eq!(s, 200, "disable: {v}");
    assert_eq!(v["status"], "disabled", "{v}");
    eventually("the host to stop the instance", 90, || async {
        (host.get("/test/fake/fake/status").await.0 == 404).then_some(Value::Null)
    })
    .await;
}

/// (P5-CERN-ADDONS) The real `aurora-addons-cern` repository end to end: register it as an addon
/// source (the manifest and both signed packages are fetched from GitHub), approve both packages,
/// connect them, grant them to the agent, then from inside the sandbox Pod one phonebook search
/// through the gateway LDAP lane (against xldap.cern.ch, whose CERN root the chart trusts through
/// `extraCaBundle`: `AURORA_E2E_EXTRA_CA_FILE`) and one CERN Maps route. Needs the internet; gated by
/// AURORA_E2E_CERN_ADDONS=1.
#[tokio::test]
async fn k7_cern_addons_source_to_phonebook_search_and_route() {
    if !enabled() {
        return;
    }
    if !std::env::var("AURORA_E2E_CERN_ADDONS").is_ok_and(|v| v == "1") {
        eprintln!("skipped: AURORA_E2E_CERN_ADDONS != 1");
        return;
    }
    let owner = format!("c{}@e2e.test", uuid::Uuid::now_v7().simple());
    let me = Api::connect(&env("AURORA_KIND_API"), &env("AURORA_KIND_ORIGIN"), &owner).await;
    psql(&format!(
        "UPDATE users SET is_instance_admin = true, email_verified_at = now() WHERE email = '{owner}'"
    ));
    let admin = me.relogin(&owner).await;

    // Register the repository: TOFU review, then one audited add that ingests the newest versions.
    let url = "https://github.com/alexiri/aurora-addons-cern";
    let (s, pv) = admin
        .post("/v1/admin/addon-sources/preview", json!({ "url": url }))
        .await;
    assert_eq!(s, 200, "preview: {pv}");
    assert_eq!(pv["publisher"], "cern", "{pv}");
    assert_eq!(pv["addons"].as_array().map(Vec::len), Some(2), "{pv}");
    let (s, src) = admin
        .post("/v1/admin/addon-sources", json!({ "url": url }))
        .await;
    assert_eq!(s, 201, "register: {src}");
    assert_eq!(src["key_fingerprint"], pv["key_fingerprint"], "{src}");

    // Both packages arrive `pending_approval` (an addon is never activated on first sight).
    let mut ids = std::collections::HashMap::new();
    for pkg in ["cern-phonebook", "cern-maps"] {
        let p = eventually(&format!("{pkg} ingested"), 60, || async {
            let (s, v) = admin.get("/v1/admin/plugins").await;
            assert_eq!(s, 200, "{v}");
            v["items"]
                .as_array()?
                .iter()
                .find(|p| p["package_id"] == pkg)
                .cloned()
        })
        .await;
        assert_eq!(p["source"], "addon", "{p}");
        assert_eq!(p["status"], "pending_approval", "{p}");
        assert_eq!(p["signer"]["kind"], "trusted_key", "{p}");
        let id = p["id"].as_str().unwrap().to_owned();
        let (s, a) = admin
            .post(&format!("/v1/admin/plugins/{id}/approve"), json!({}))
            .await;
        assert_eq!(s, 200, "approve {pkg}: {a}");
        assert_eq!(a["status"], "active", "{a}");
        ids.insert(pkg, id);
    }

    // Connect both (nothing to enter) and grant them to the agent.
    let mut conns = vec![];
    for app in ["cern-phonebook", "cern-maps"] {
        let (s, c) = admin
            .post(
                &format!("/v1/apps/{app}/connect"),
                json!({"scope": "workspace", "workspace_id": admin.ws, "method_id": "anonymous",
                       "fields": {}}),
            )
            .await;
        assert_eq!(s, 200, "connect {app}: {c}");
        assert_eq!(c["kind"], "connected", "{c}");
        conns.push(c["connection"]["id"].as_str().unwrap().to_owned());
    }
    for c in &conns {
        let (s, g) = admin
            .post(
                &format!("/v1/connections/{c}/grants"),
                json!({"agent_ids": [admin.daneel], "mode": "full"}),
            )
            .await;
        assert_eq!(s, 200, "grant: {g}");
    }

    // From the agent's own turns (a turn's credential carries the grants; a bare exec into the Pod
    // is the boot identity, which has none), through the gateway. The fake harness's `[[http: ...]]`
    // directive makes the request and puts the status and the first 200 characters in the transcript.
    let conv = admin.direct().await;
    let run = |directive: String| {
        let (admin, conv) = (&admin, conv.clone());
        async move {
            let before = transcript(&admin.events(&conv).await).len();
            say_and_finish(admin, &conv, &directive).await;
            transcript(&admin.events(&conv).await)[before..].to_owned()
        }
    };
    let t = run(
        r#"[[http: POST https://cern-phonebook.ldap.aurora.internal/search {"filter":"(|(displayName=Fabiola Gianotti*)(cn=fabiola gianotti*))","attributes":["mail"],"limit":3}]]"#
            .into(),
    )
    .await;
    assert!(
        t.contains("http 200") && t.contains("@cern.ch"),
        "phonebook search: {t}"
    );
    // An attribute the package does not list is refused by the lane (and never read).
    let t = run(
        r#"[[http: POST https://cern-phonebook.ldap.aurora.internal/search {"filter":"(uid=*)","attributes":["userPassword"]}]]"#
            .into(),
    )
    .await;
    assert!(t.contains("ldap_request_invalid"), "{t}");
    let t =
        run("[[http: GET https://maps.cern.ch/api/search?query=513&resultRecordCount=2]]".into())
            .await;
    assert!(
        t.contains("http 200") && t.contains("categories"),
        "maps search: {t}"
    );
    let t = run(
        "[[http: GET https://maps.cern.ch/arcgis/rest/services/Routing/NetworkAnalysis/NAServer/Route/solve?stops=6.0459,46.2325;6.0526,46.2330&inSR=4326&returnRoutes=true&f=json]]"
            .into(),
    )
    .await;
    assert!(
        t.contains("http 200") && t.contains("checksum"),
        "route: {t}"
    );
    // A path the package does not catalogue stays blocked.
    let t =
        run("[[http: GET https://maps.cern.ch/arcgis/rest/admin/services?f=json]]".into()).await;
    assert!(
        !t.contains("http 200"),
        "uncatalogued ArcGIS path was served: {t}"
    );
}

// ---------------------------------------------------------------------------------------------- P5-FILES-4

fn sha_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// `n` deterministic pseudo-random bytes (xorshift): incompressible enough that a truncated or reordered transfer
/// cannot match the digest by accident.
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(n);
    v
}

/// One Files request. A parked or restarting sandbox answers 503 (`sandbox_waking`, `files_unavailable`) with
/// `retry-after`; unless `once`, the call retries for up to 120 s like the dashboard does.
async fn fx(
    api: &Api,
    once: bool,
    m: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> (u16, reqwest::header::HeaderMap, Vec<u8>) {
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        let r = api.bytes(m.clone(), path, headers, body.clone()).await;
        let waking = r.0 == 503
            && serde_json::from_slice::<Value>(&r.2).is_ok_and(|v| {
                matches!(
                    v["error"]["code"].as_str(),
                    Some("sandbox_waking" | "files_unavailable")
                )
            });
        if once || !waking {
            return r;
        }
        assert!(Instant::now() < end, "files route still waking: {path}");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn code_of(b: &[u8]) -> String {
    serde_json::from_slice::<Value>(b)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("(no error code: {})", String::from_utf8_lossy(b)))
}

/// SQL to a value: the first column of the first row (`psql -t -A`).
fn psql_value(sql: &str) -> String {
    let pod = std::env::var("AURORA_KIND_PG_POD")
        .unwrap_or_else(|_| format!("{}-postgres-0", env("AURORA_KIND_FULL")));
    let (rc, out) = kubectl(&[
        "-n",
        &env("AURORA_KIND_NS"),
        "exec",
        &pod,
        "--",
        "psql",
        "-U",
        "aurora",
        "-d",
        "aurora",
        "-t",
        "-A",
        "-v",
        "ON_ERROR_STOP=1",
        "-c",
        sql,
    ]);
    assert_eq!(rc, 0, "psql failed: {out}");
    out.trim().to_owned()
}

/// P5-FILES-4 (D-316, C-907, SEC-125/126): the Files feature end to end on a real cluster — the API's SFTP client
/// over the runner and supervisor into the sandbox Pod, with the instance's `terminal.enabled` on
/// (`AURORA_E2E_TERMINAL=1`, which `scripts/e2e/files.sh` sets). With it off the routes answer 404
/// `terminal_disabled` and the scenario prints `SKIPPED:` and passes.
#[tokio::test]
async fn k8_files_journey() {
    if !enabled() {
        return;
    }
    let api = connect().await;
    let agent = api.daneel.clone();
    let files = format!("/v1/agents/{agent}/files");
    let content = |p: &str| format!("{files}/content?path={p}");
    let text = |p: &str| format!("{files}/text?path={p}");
    let ops = format!("{files}/ops");
    let json_body = |v: Value| Some(serde_json::to_vec(&v).unwrap());
    let jhdr = [("content-type", "application/json")];

    // The agent's sandbox must be running for a hosted agent's files: one turn boots it.
    let conv = api.direct().await;
    say_and_finish(&api, &conv, "[[say: files journey]]").await;
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &format!("{files}?path=/workspace"),
        &[],
        None,
    )
    .await;
    if s == 404 && code_of(&b) == "terminal_disabled" {
        eprintln!(
            "SKIPPED: terminal.enabled is off in this cluster (the Files routes answer 404 terminal_disabled); run scripts/e2e/files.sh, which turns it on"
        );
        return;
    }
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));

    // 1. A folder of three files: mkdir with parents, then each file with If-None-Match: *.
    let (s, _, b) = fx(
        &api,
        false,
        Method::POST,
        &ops,
        &jhdr,
        json_body(json!({"op":"mkdir","path":"/workspace/journey/src","parents":true})),
    )
    .await;
    assert!(
        s == 200 || s == 201,
        "mkdir: {s} {}",
        String::from_utf8_lossy(&b)
    );
    let canary = format!("AURORA-CANARY-{}", uuid::Uuid::now_v7().simple());
    let blob = noise(4096, 7);
    let a_text = b"alpha\n".to_vec();
    let c_text = format!("config = \"{canary}\"\n").into_bytes();
    for (name, bytes) in [("a.txt", &a_text), ("b.bin", &blob), ("c.txt", &c_text)] {
        let (s, _, b) = fx(
            &api,
            false,
            Method::PUT,
            &content(&format!("/workspace/journey/{name}")),
            &[("if-none-match", "*")],
            Some(bytes.clone()),
        )
        .await;
        assert_eq!(s, 201, "upload {name}: {}", String::from_utf8_lossy(&b));
    }
    // A second create of an existing name is refused (never a silent overwrite).
    let (s, _, b) = fx(
        &api,
        false,
        Method::PUT,
        &content("/workspace/journey/a.txt"),
        &[("if-none-match", "*")],
        Some(b"x".to_vec()),
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (409, "file_exists"));

    // 2. List it.
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &format!("{files}?path=/workspace/journey"),
        &[],
        None,
    )
    .await;
    assert_eq!(s, 200);
    let list: Value = serde_json::from_slice(&b).unwrap();
    let names: Vec<&str> = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["src", "a.txt", "b.bin", "c.txt"], "{list}");
    let size_of = |n: &str| {
        list["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == n)
            .unwrap()["size"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(size_of("b.bin"), 4096);

    // 3. Download one: the bytes match, and the safety headers of C-903 are there.
    let (s, h, b) = fx(
        &api,
        false,
        Method::GET,
        &content("/workspace/journey/b.bin"),
        &[],
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(sha_hex(&b), sha_hex(&blob), "downloaded bytes differ");
    let hv = |k: &str| {
        h.get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    assert_eq!(hv("content-type"), "application/octet-stream");
    assert!(
        hv("content-disposition").starts_with("attachment;"),
        "{h:?}"
    );
    assert_eq!(hv("x-content-type-options"), "nosniff");

    // 4. Text read, save with If-Match, then a stale If-Match is a 409 file_changed with the current etag.
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &text("/workspace/journey/a.txt"),
        &[],
        None,
    )
    .await;
    assert_eq!(s, 200);
    let t: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(t["content"], "alpha\n");
    let etag1 = t["etag"].as_str().unwrap().to_owned();
    assert_eq!(etag1, format!("sha256:{}", sha_hex(&a_text)));
    let (s, _, b) = fx(
        &api,
        false,
        Method::PUT,
        &content("/workspace/journey/a.txt"),
        &[("if-match", &format!("\"{etag1}\""))],
        Some(b"alpha\nbravo\n".to_vec()),
    )
    .await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let saved: Value = serde_json::from_slice(&b).unwrap();
    let etag2 = saved["etag"].as_str().unwrap().to_owned();
    assert_eq!(etag2, format!("sha256:{}", sha_hex(b"alpha\nbravo\n")));
    // The "terminal" changes the file behind the editor: a write with the old etag is refused and says what it is now.
    let (s, _, b) = fx(
        &api,
        false,
        Method::PUT,
        &content("/workspace/journey/a.txt"),
        &[("if-match", &format!("\"{etag1}\""))],
        Some(b"mine\n".to_vec()),
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (409, "file_changed"));
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["error"]["details"]["current_etag"], etag2.as_str());
    // Overwrite (no precondition) works.
    let (s, _, _) = fx(
        &api,
        false,
        Method::PUT,
        &content("/workspace/journey/a.txt"),
        &[],
        Some(b"mine\n".to_vec()),
    )
    .await;
    assert_eq!(s, 200);

    // 5. Rename (and an existing target is refused).
    let (s, _, b) = fx(&api, false, Method::POST, &ops, &jhdr, json_body(json!({"op":"rename","path":"/workspace/journey/a.txt","to":"/workspace/journey/a2.txt"}))).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let (s, _, b) = fx(&api, false, Method::POST, &ops, &jhdr, json_body(json!({"op":"rename","path":"/workspace/journey/a2.txt","to":"/workspace/journey/b.bin"}))).await;
    assert_eq!((s, code_of(&b).as_str()), (409, "file_exists"));

    // 6. `.aurora` is read-only (C-899).
    let (s, _, b) = fx(
        &api,
        false,
        Method::PUT,
        &content("/workspace/.aurora/journey.txt"),
        &[],
        Some(b"x".to_vec()),
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (403, "path_read_only"));

    // 7. A symlink to /etc, made inside the sandbox, is refused: the resolved path leaves /workspace.
    let (_, pod) = wait_boot_pod(&api, 60).await;
    let (rc, out) = exec_in(&pod, "ln -s /etc /workspace/journey/etclink");
    assert_eq!(rc, 0, "{out}");
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &content("/workspace/journey/etclink/hostname"),
        &[],
        None,
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (403, "path_outside_workspace"));
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &format!("{files}?path=/workspace/journey/etclink"),
        &[],
        None,
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (403, "path_outside_workspace"));
    // The link itself is listed (as a link whose target is outside), and can be removed as a link.
    let (_, _, b) = fx(
        &api,
        false,
        Method::GET,
        &format!("{files}?path=/workspace/journey"),
        &[],
        None,
    )
    .await;
    let list: Value = serde_json::from_slice(&b).unwrap();
    let link = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "etclink")
        .unwrap_or_else(|| panic!("{list}"));
    assert_eq!(
        (
            link["kind"].as_str(),
            link["target"]["outside_workspace"].as_bool()
        ),
        (Some("symlink"), Some(true))
    );

    // 8. 20 MiB up and down with matching digests.
    let big = noise(20 * 1024 * 1024, 42);
    let (s, _, b) = fx(
        &api,
        false,
        Method::PUT,
        &content("/workspace/journey/big.bin"),
        &[("if-none-match", "*")],
        Some(big.clone()),
    )
    .await;
    assert_eq!(s, 201, "{}", String::from_utf8_lossy(&b));
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &content("/workspace/journey/big.bin"),
        &[],
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(b.len(), big.len(), "downloaded length");
    assert_eq!(sha_hex(&b), sha_hex(&big), "20 MiB round trip differs");

    // 9. Recursive delete; the folder is gone and the link target is untouched.
    let (s, _, b) = fx(
        &api,
        false,
        Method::POST,
        &ops,
        &jhdr,
        json_body(json!({"op":"delete","path":"/workspace/journey","recursive":false})),
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (409, "directory_not_empty"));
    let (s, _, b) = fx(
        &api,
        false,
        Method::POST,
        &ops,
        &jhdr,
        json_body(json!({"op":"delete","path":"/workspace/journey","recursive":true})),
    )
    .await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &format!("{files}?path=/workspace/journey"),
        &[],
        None,
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (404, "path_not_found"));
    let (rc, out) = exec_in(&pod, "test -d /etc && echo etc-intact");
    assert_eq!((rc, out.trim()), (0, "etc-intact"));

    // 10. Frozen → 423 `frozen` on every files route; unfrozen → they work again.
    let (s, v) = api
        .post(
            &format!("/v1/agents/{agent}/freeze"),
            json!({"reason": "files journey"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _, b) = fx(
        &api,
        true,
        Method::GET,
        &format!("{files}?path=/workspace"),
        &[],
        None,
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (423, "frozen"));
    let (s, _, b) = fx(
        &api,
        true,
        Method::PUT,
        &content("/workspace/frozen.txt"),
        &[],
        Some(b"x".to_vec()),
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (423, "frozen"));
    let (s, v) = api
        .post(&format!("/v1/agents/{agent}/unfreeze"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &format!("{files}?path=/workspace"),
        &[],
        None,
    )
    .await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));

    // 11. A parked sandbox: the next request answers 503 `sandbox_waking` (with retry-after) at once, and succeeds once
    // the sandbox is back. (The files keep-awake grace is 120 s, then idle_stop_secs applies: values-e2e.yaml.)
    api.wait_sandbox_state("stopped", 420).await;
    let (s, h, b) = fx(
        &api,
        true,
        Method::GET,
        &format!("{files}?path=/workspace"),
        &[],
        None,
    )
    .await;
    assert_eq!((s, code_of(&b).as_str()), (503, "sandbox_waking"), "{h:?}");
    assert!(h.contains_key("retry-after"), "{h:?}");
    let (s, _, b) = fx(
        &api,
        false,
        Method::GET,
        &format!("{files}?path=/workspace"),
        &[],
        None,
    )
    .await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));

    // 12. The secret-grep (SEC-125/126): the canary, uploaded inside a file, appears in no API, runner or sandbox-side
    // log, in no audit row and in no request-log row. (Contents are never logged, stored or audited: C-906.)
    let mut hits = vec![];
    for ns in [env("AURORA_KIND_NS"), sbx_ns()] {
        let (_, pods) = kubectl(&["-n", &ns, "get", "pods", "-o", "name"]);
        for p in pods.lines().filter(|l| l.starts_with("pod/")) {
            let (_, logs) = kubectl(&["-n", &ns, "logs", p, "--all-containers", "--tail=-1"]);
            if logs.contains(&canary) {
                hits.push(format!("{ns}/{p}"));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the canary appeared in the logs of: {hits:?}"
    );
    assert_eq!(
        psql_value(&format!(
            "SELECT count(*) FROM audit_log WHERE audit_log::text LIKE '%{canary}%'"
        )),
        "0",
        "the canary is in audit_log"
    );
    assert_eq!(
        psql_value(&format!(
            "SELECT count(*) FROM request_log WHERE request_log::text LIKE '%{canary}%'"
        )),
        "0",
        "the canary is in request_log"
    );
    // ...while the audit rows themselves exist (the feature is audited, without the contents).
    let audited = psql_value("SELECT count(*) FROM audit_log WHERE action LIKE 'agent.files.%'");
    assert!(
        audited.parse::<u64>().unwrap() > 0,
        "no agent.files.* audit rows: {audited}"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// P4-SSH-2 (D-239, SEC-92..SEC-96, C-799): SSH access end to end — the real `ssh`, `sftp` and `scp` clients of this
// machine against the chart's terminator (reached through a port-forward), the API's session plane, the runner's
// `auz_` legs and the supervisor's exec engine, in a sandbox Pod. Needs the chart installed with `ssh.enabled`
// (`scripts/e2e/ssh.sh`, which sets AURORA_KIND_SSH=127.0.0.1:<port> and AURORA_E2E_CLI=<the aurora binary>).
// ---------------------------------------------------------------------------------------------------------------

/// One person's SSH identity for one agent: a key registered through the API and a certificate minted for it.
struct SshId {
    dir: std::path::PathBuf,
    port: u16,
    agent: String,
}

impl SshId {
    fn path(&self, n: &str) -> String {
        self.dir.join(n).display().to_string()
    }

    /// The options every client gets: the identity and certificate files, our own `known_hosts` (strict), the port.
    fn opts(&self) -> Vec<String> {
        let mut v = vec!["-F".into(), "/dev/null".into()];
        for o in [
            format!("CertificateFile={}", self.path("id-cert.pub")),
            "IdentitiesOnly=yes".into(),
            "BatchMode=yes".into(),
            "StrictHostKeyChecking=yes".into(),
            format!("UserKnownHostsFile={}", self.path("known_hosts")),
            format!("Port={}", self.port),
            "ConnectTimeout=20".into(),
        ] {
            v.push("-o".into());
            v.push(o);
        }
        v.push("-i".into());
        v.push(self.path("id"));
        v
    }

    fn dest(&self) -> String {
        format!("{}@127.0.0.1", self.agent)
    }

    fn command(&self, prog: &str, pre: &[&str], post: &[&str]) -> Command {
        let mut c = Command::new(prog);
        c.args(self.opts()).args(pre).arg(self.dest()).args(post);
        c
    }
}

/// Runs a client to the end (stdin from `input`), at most `secs`: (exit code, stdout, stderr).
async fn run_client(mut c: Command, input: &[u8], secs: u64) -> (i32, String, String) {
    use std::io::Write;
    use std::process::Stdio;
    let input = input.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut ch = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut si = ch.stdin.take().unwrap();
        let w = std::thread::spawn(move || {
            let _ = si.write_all(&input);
        });
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if ch.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() > end {
                let _ = ch.kill();
                panic!("a client ran longer than {secs} s");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = w.join();
        let o = ch.wait_with_output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    })
    .await
    .unwrap()
}

/// Registers a fresh ed25519 key for the signed-in person and mints a certificate for `agent`; writes `id`, `id.pub`,
/// `id-cert.pub` and the strict `known_hosts` (the host key the API publishes) into a new directory.
async fn ssh_identity(api: &Api, agent: &str, port: u16, host_key: &str) -> SshId {
    let dir =
        std::env::temp_dir().join(format!("aurora-e2e-ssh-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let st = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "aurora-e2e", "-f"])
        .arg(dir.join("id"))
        .status()
        .unwrap();
    assert!(st.success(), "ssh-keygen");
    let public = std::fs::read_to_string(dir.join("id.pub")).unwrap();
    let (s, v) = api
        .post(
            "/v1/me/ssh-keys",
            json!({"public_key": public.trim(), "name": "e2e"}),
        )
        .await;
    assert_eq!(s, 201, "register key: {v}");
    let (s, v) = api
        .post(
            &format!("/v1/agents/{agent}/ssh-certificate"),
            json!({"public_key": public.trim()}),
        )
        .await;
    assert_eq!(s, 201, "mint: {v}");
    std::fs::write(
        dir.join("id-cert.pub"),
        format!("{}\n", v["certificate"].as_str().unwrap().trim()),
    )
    .unwrap();
    std::fs::write(
        dir.join("known_hosts"),
        format!("[127.0.0.1]:{port} {host_key}\n"),
    )
    .unwrap();
    SshId {
        dir,
        port,
        agent: agent.to_owned(),
    }
}

/// Waits for the person's open sessions on the agent to number `n`; returns them.
async fn open_sessions(api: &Api, n: usize, secs: u64) -> Vec<Value> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let (s, v) = api.get("/v1/me/ssh-sessions").await;
        assert_eq!(s, 200, "{v}");
        let items = v["sessions"].as_array().cloned().unwrap_or_default();
        if items.len() == n {
            return items;
        }
        assert!(
            Instant::now() < end,
            "expected {n} open SSH sessions, have {}: {v}",
            items.len()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// A long-lived client (`sleep 300` in the sandbox): the process, to be waited on or killed.
fn spawn_sleeper(id: &SshId) -> std::process::Child {
    use std::process::Stdio;
    id.command("ssh", &[], &["sleep 300"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Waits until `child` exits (at most `secs`), returns (exit code, seconds it took).
async fn wait_child(child: &mut std::process::Child, secs: u64) -> (i32, u64) {
    let t0 = Instant::now();
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            return (st.code().unwrap_or(-1), t0.elapsed().as_secs());
        }
        if t0.elapsed() > Duration::from_secs(secs) {
            let _ = child.kill();
            panic!("the SSH client was still connected after {secs} s");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn k9_ssh_journey() {
    if !enabled() {
        return;
    }
    let Ok(target) = std::env::var("AURORA_KIND_SSH") else {
        eprintln!(
            "SKIPPED: ssh.enabled is off in this cluster (no AURORA_KIND_SSH); run scripts/e2e/ssh.sh, which turns it on"
        );
        return;
    };
    let port: u16 = target.rsplit(':').next().unwrap().parse().unwrap();
    let api = connect().await;
    let agent = api.daneel.clone();
    let canary = format!("AURORA-CANARY-{}", uuid::Uuid::now_v7().simple());

    // The terminator has registered and published its host key; the instance says SSH is on.
    let (_, inst) = api.get("/v1/instance").await;
    assert_eq!(inst["features"]["ssh"], true, "features.ssh: {inst}");
    let facts = eventually("a live terminator in GET /v1/ssh", 120, || async {
        let (s, v) = api.get("/v1/ssh").await;
        (s == 200 && v["terminators_live"].as_u64().unwrap_or(0) > 0).then_some(v)
    })
    .await;
    let host_key = facts["host_keys"][0]["public_key"]
        .as_str()
        .unwrap()
        .to_owned();

    // The agent's sandbox is running (one turn boots it); the person has a key and a certificate.
    let conv = api.direct().await;
    say_and_finish(&api, &conv, "[[say: ssh journey]]").await;
    let id = ssh_identity(&api, &agent, port, &host_key).await;

    // 1. A remote command; stdout and stderr stay separate; the exit status propagates.
    let (rc, out, err) = run_client(id.command("ssh", &[], &["echo aurora-ssh"]), b"", 60).await;
    assert_eq!((rc, out.trim()), (0, "aurora-ssh"), "stderr: {err}");
    let (rc, out, err) = run_client(
        id.command("ssh", &[], &["echo to-out; echo to-err >&2; exit 7"]),
        b"",
        60,
    )
    .await;
    assert_eq!(rc, 7, "exit status: {err}");
    assert_eq!(out.trim(), "to-out");
    assert!(
        err.contains("to-err") && !out.contains("to-err"),
        "{out} / {err}"
    );

    // 2. A shell (no tty and with one): the canary typed into it; a command carrying it.
    let (rc, out, err) = run_client(
        id.command("ssh", &["-T"], &[]),
        format!("echo typed-{canary}\nexit 3\n").as_bytes(),
        60,
    )
    .await;
    assert_eq!(rc, 3, "{err}");
    assert!(out.contains(&format!("typed-{canary}")), "{out}");
    let (rc, out, err) = run_client(
        id.command("ssh", &["-tt"], &[]),
        b"tty -s && echo has-tty; stty size; exit 0\n",
        60,
    )
    .await;
    assert_eq!(rc, 0, "{err}");
    assert!(out.contains("has-tty"), "{out}");
    let (rc, out, err) = run_client(
        id.command("ssh", &[], &[&format!("echo cmd-{canary}")]),
        b"",
        60,
    )
    .await;
    assert_eq!(
        (rc, out.trim()),
        (0, format!("cmd-{canary}").as_str()),
        "{err}"
    );

    // 3. sftp put/get and scp (SFTP mode and `-O`) of a 5 MiB file: the digests match.
    let blob = noise(5 * 1024 * 1024, 99);
    let want = sha_hex(&blob);
    std::fs::write(id.dir.join("up.bin"), &blob).unwrap();
    std::fs::write(
        id.dir.join("batch"),
        format!(
            "put {up} /workspace/ssh-up.bin\nget /workspace/ssh-up.bin {down}\n",
            up = id.path("up.bin"),
            down = id.path("down-sftp.bin")
        ),
    )
    .unwrap();
    let mut c = Command::new("sftp");
    c.args(id.opts())
        .arg("-b")
        .arg(id.path("batch"))
        .arg(id.dest());
    let (rc, out, err) = run_client(c, b"", 180).await;
    assert_eq!(rc, 0, "sftp: {out} {err}");
    assert_eq!(
        sha_hex(&std::fs::read(id.dir.join("down-sftp.bin")).unwrap()),
        want,
        "sftp round trip differs"
    );
    for (flag, name) in [(None, "scp"), (Some("-O"), "scp-O")] {
        let remote = format!("/workspace/{name}.bin");
        let mut up = Command::new("scp");
        up.args(id.opts());
        if let Some(f) = flag {
            up.arg(f);
        }
        up.arg(id.path("up.bin"))
            .arg(format!("{}:{remote}", id.dest()));
        let (rc, out, err) = run_client(up, b"", 180).await;
        assert_eq!(rc, 0, "{name} up: {out} {err}");
        let mut down = Command::new("scp");
        down.args(id.opts());
        if let Some(f) = flag {
            down.arg(f);
        }
        down.arg(format!("{}:{remote}", id.dest()))
            .arg(id.path(&format!("down-{name}.bin")));
        let (rc, out, err) = run_client(down, b"", 180).await;
        assert_eq!(rc, 0, "{name} down: {out} {err}");
        assert_eq!(
            sha_hex(&std::fs::read(id.dir.join(format!("down-{name}.bin"))).unwrap()),
            want,
            "{name} round trip differs"
        );
    }

    // 4. Forwarding is refused while the shell works: -R (setup fails), -L (the channel is refused), -A and -X
    // (nothing reaches the sandbox).
    let (rc, _, err) = run_client(
        id.command(
            "ssh",
            &["-o", "ExitOnForwardFailure=yes", "-R", "0:127.0.0.1:22"],
            &["echo should-not-run"],
        ),
        b"",
        60,
    )
    .await;
    assert_ne!(rc, 0, "-R must fail when forwarding is refused: {err}");
    let lport = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut lc = id.command(
        "ssh",
        &["-L", &format!("{lport}:127.0.0.1:22")],
        &["sleep 6; echo l-done"],
    );
    let lhandle = tokio::task::spawn_blocking(move || {
        use std::process::Stdio;
        let ch = lc
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        ch.wait_with_output().unwrap()
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", lport)) {
        use std::io::Read;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut buf = [0u8; 8];
        let _ = s.read(&mut buf); // the refused channel closes the connection
    }
    let o = lhandle.await.unwrap();
    let (lout, lerr) = (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    );
    assert!(
        lout.contains("l-done"),
        "the shell must keep working: {lout} {lerr}"
    );
    assert!(
        lerr.contains("open failed") || lerr.contains("administratively prohibited"),
        "-L must be refused: {lerr}"
    );
    // -A: a local agent is offered; the sandbox sees no agent socket.
    let agent_sock = id.dir.join("agent.sock");
    let mut agent_proc = Command::new("ssh-agent")
        .arg("-D")
        .arg("-a")
        .arg(&agent_sock)
        .spawn()
        .unwrap();
    for _ in 0..50 {
        if agent_sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut ac = id.command("ssh", &["-A"], &["echo agent=${SSH_AUTH_SOCK:-none}"]);
    ac.env("SSH_AUTH_SOCK", &agent_sock);
    let (rc, out, err) = run_client(ac, b"", 60).await;
    let _ = agent_proc.kill();
    let _ = agent_proc.wait();
    assert_eq!((rc, out.trim()), (0, "agent=none"), "-A: {err}");
    let mut xc = id.command("ssh", &["-X"], &["echo display=${DISPLAY:-none}"]);
    xc.env("DISPLAY", "localhost:99.0");
    let (rc, out, err) = run_client(xc, b"", 60).await;
    assert_eq!((rc, out.trim()), (0, "display=none"), "-X: {err}");

    // 5. A wrong pinned host key is refused by the client; a plain key (no certificate) gets the hint-free refusal.
    let other = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(id.dir.join("other"))
        .status()
        .unwrap();
    assert!(other.success());
    let other_pub = std::fs::read_to_string(id.dir.join("other.pub")).unwrap();
    std::fs::write(
        id.dir.join("known_hosts-wrong"),
        format!("[127.0.0.1]:{port} {}\n", other_pub.trim()),
    )
    .unwrap();
    let (rc, _, err) = run_client(
        id.command(
            "ssh",
            &[
                "-o",
                &format!("UserKnownHostsFile={}", id.path("known_hosts-wrong")),
            ],
            &["echo not-here"],
        ),
        b"",
        60,
    )
    .await;
    assert_eq!(rc, 255, "{err}");
    assert!(
        err.contains("Host key verification failed")
            || err.contains("REMOTE HOST IDENTIFICATION HAS CHANGED"),
        "a wrong host key must be refused by the client: {err}"
    );
    let (rc, out, err) = run_client(
        id.command(
            "ssh",
            &[
                "-o",
                "CertificateFile=/nonexistent",
                "-o",
                "PreferredAuthentications=publickey",
            ],
            &["echo not-here"],
        ),
        b"",
        60,
    )
    .await;
    assert_eq!(rc, 255, "{out} {err}");
    assert!(err.contains("Permission denied (publickey)"), "{err}");
    assert!(!out.contains("not-here"));

    // 6. `aurora ssh <agent> -- echo aurora-ssh` (C-798): a workspace API key in a config directory of its own.
    if let Ok(cli) = std::env::var("AURORA_E2E_CLI") {
        let (s, k) = api
            .post(
                &format!("/v1/workspaces/{}/api-keys", api.ws),
                json!({"name": "e2e ssh"}),
            )
            .await;
        assert_eq!(s, 201, "{k}");
        let (_, me) = api.get("/v1/me").await;
        let m = &me["memberships"][0];
        let sv = |v: &Value, f: &str| v[f].as_str().unwrap_or("x").to_owned();
        let cfg_dir = id.dir.join("cli");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg = format!(
            "url = \"{}\"\napi_key = \"{}\"\n\n[workspace]\nid = \"{}\"\nslug = \"{}\"\nname = \"{}\"\norg_id = \"{}\"\norg_slug = \"{}\"\n",
            env("AURORA_KIND_API"),
            k["key"].as_str().unwrap(),
            api.ws,
            sv(&m["workspaces"][0], "slug"),
            sv(&m["workspaces"][0], "name"),
            api.org,
            sv(&m["org"], "slug"),
        );
        std::fs::write(cfg_dir.join("config.toml"), cfg).unwrap();
        let mut c = Command::new(cli);
        c.env("AURORA_CONFIG_DIR", &cfg_dir)
            .arg("ssh")
            .arg(&agent)
            .arg("--key")
            .arg(id.path("id.pub"))
            .arg("--")
            .arg("echo aurora-ssh");
        let (rc, out, err) = run_client(c, b"", 90).await;
        assert_eq!((rc, out.trim()), (0, "aurora-ssh"), "aurora ssh: {err}");
    } else {
        eprintln!("note: AURORA_E2E_CLI not set; the `aurora ssh` leg was not run");
    }

    // 7. Sessions are listed and a person's disconnect ends one within a heartbeat (≤ 35 s).
    let mut s1 = spawn_sleeper(&id);
    let list = open_sessions(&api, 1, 60).await;
    assert_eq!(list[0]["agent"]["id"], agent.as_str(), "{list:?}");
    let sid = list[0]["id"].as_str().unwrap().to_owned();
    let (s, v) = api.delete(&format!("/v1/ssh-sessions/{sid}")).await;
    assert_eq!(s, 202, "{v}");
    let (_, took) = wait_child(&mut s1, 60).await;
    assert!(took <= 40, "disconnect took {took} s");
    open_sessions(&api, 0, 40).await;

    // 8. Freeze closes an open session within 35 s and refuses a new one (423 -> the hint-free failure); unfreeze
    // lets the next connect through.
    let mut s2 = spawn_sleeper(&id);
    open_sessions(&api, 1, 60).await;
    let (s, v) = api
        .post(
            &format!("/v1/agents/{agent}/freeze"),
            json!({"reason": "ssh journey"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, took) = wait_child(&mut s2, 60).await;
    assert!(took <= 40, "freeze took {took} s to close the session");
    open_sessions(&api, 0, 40).await;
    let (rc, _, err) = run_client(id.command("ssh", &[], &["echo frozen"]), b"", 60).await;
    assert_eq!(rc, 255, "{err}");
    assert!(err.contains("Permission denied (publickey)"), "{err}");
    let (s, v) = api
        .post(&format!("/v1/agents/{agent}/unfreeze"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
    let (rc, out, err) = run_client(id.command("ssh", &[], &["echo unfrozen"]), b"", 60).await;
    assert_eq!((rc, out.trim()), (0, "unfrozen"), "{err}");

    // 9. Deleting the key ends the trust of its certificate at once (SEC-94): the same certificate is refused.
    let (s, keys) = api.get("/v1/me/ssh-keys").await;
    assert_eq!(s, 200, "{keys}");
    let kid = keys["keys"][0]["id"].as_str().unwrap().to_owned();
    let (s, v) = api.delete(&format!("/v1/me/ssh-keys/{kid}")).await;
    assert_eq!(s, 204, "{v}");
    let (rc, _, err) = run_client(id.command("ssh", &[], &["echo deleted-key"]), b"", 60).await;
    assert_eq!(rc, 255, "{err}");
    assert!(err.contains("Permission denied (publickey)"), "{err}");

    // 10. A parked sandbox wakes on connect: wait for the park, then connect and run a command.
    let id2 = ssh_identity(&api, &agent, port, &host_key).await;
    api.wait_sandbox_state("stopped", 420).await;
    let end = Instant::now() + Duration::from_secs(180);
    let woke = loop {
        let (rc, out, err) = run_client(id2.command("ssh", &[], &["echo woke"]), b"", 150).await;
        if rc == 0 {
            break out;
        }
        assert!(
            Instant::now() < end,
            "the parked sandbox did not wake on connect: {out} {err}"
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    assert_eq!(woke.trim(), "woke");

    // 11. The secret-grep: the canary (typed in a shell and passed as a command) is in no API, terminator, runner or
    // sandbox log, in no audit row and in no request-log row; the sessions themselves are audited.
    let mut hits = vec![];
    for ns in [env("AURORA_KIND_NS"), sbx_ns()] {
        let (_, pods) = kubectl(&["-n", &ns, "get", "pods", "-o", "name"]);
        for p in pods.lines().filter(|l| l.starts_with("pod/")) {
            let (_, logs) = kubectl(&["-n", &ns, "logs", p, "--all-containers", "--tail=-1"]);
            if logs.contains(&canary) {
                hits.push(format!("{ns}/{p}"));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the canary appeared in the logs of: {hits:?}"
    );
    for t in ["audit_log", "request_log"] {
        assert_eq!(
            psql_value(&format!(
                "SELECT count(*) FROM {t} WHERE {t}::text LIKE '%{canary}%'"
            )),
            "0",
            "the canary is in {t}"
        );
    }
    for a in ["agent.ssh.open", "agent.ssh.close"] {
        let n = psql_value(&format!(
            "SELECT count(*) FROM audit_log WHERE action = '{a}'"
        ));
        assert!(n.parse::<u64>().unwrap() > 0, "no {a} audit rows");
    }
    let _ = std::fs::remove_dir_all(&id.dir);
    let _ = std::fs::remove_dir_all(&id2.dir);
}

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
    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(15), tcp.read(&mut buf))
        .await
        .unwrap_or(Ok(0))
        .unwrap_or(0);
    let echoed = n > 2 && buf[0] == 0x81 && &buf[2..2 + (buf[1] & 0x7f) as usize] == b"ws-echo";
    let _ = tcp.write_all(&[0x88, 0x00]).await; // close
    assert!(
        echoed,
        "no ws echo from the app ({} bytes: {:?})",
        n,
        &buf[..n.min(32)]
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

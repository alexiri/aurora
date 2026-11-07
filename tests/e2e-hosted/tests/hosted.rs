#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! TEST-013: M4 acceptance, black box. Gated by AURORA_TEST_PODMAN=1; run via scripts/e2e/hosted.sh.
mod common;

use aurora_testkit::wiremock::matchers::method;
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use common::*;
use futures::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;

/// Sign up, start the stack; returns (stack, api).
async fn fx(o: Opts) -> (Stack, Api) {
    Stack::wipe();
    let st = Stack::start(o).await;
    let api = st.signup("owner@e2e.test").await;
    (st, api)
}

fn turn_id(v: &Value) -> String {
    v["turn"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn s1_chat_with_daneel_fake_harness() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let conv = api.direct().await;
    // SSE first, then the message.
    let resp = api
        .raw(&format!("/v1/conversations/{conv}/stream"))
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sse = tokio::spawn(async move {
        let mut buf = String::new();
        let mut s = resp.bytes_stream();
        while let Some(Ok(b)) = s.next().await {
            buf.push_str(&String::from_utf8_lossy(&b));
            if buf.contains("hello from daneel") && buf.contains("turn.finished") {
                break;
            }
        }
        buf
    });
    let v = api.say(&conv, "[[say: hello from daneel]]").await;
    let turn = turn_id(&v);
    let (status, err) = st.wait_turn(&turn, &["done", "failed"], 90).await;
    assert_eq!((status.as_str(), err), ("done", None), "{}", st.log_tails());
    let ev = api.events(&conv).await;
    let text = transcript_text(&ev);
    assert!(text.contains("hello from daneel"), "{text}");
    let sse = tokio::time::timeout(Duration::from_secs(20), sse)
        .await
        .expect("sse timed out")
        .unwrap();
    assert!(sse.contains("hello from daneel"), "{sse}");
    // one container, one home volume
    assert_eq!(containers().len(), 1);
    assert_eq!(volumes().len(), 1);
    let _ = json!({});
    st.finish().await;
}

pub const SECRET: &str = "sk-e2e-hosted-SECRET-value-123";

async fn upstream() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("upstream-ok"))
        .mount(&s)
        .await;
    s
}

/// Grant Daneel a generic secret for the mock upstream.
async fn grant_secret(api: &Api, up: &MockServer) -> String {
    let id = api
        .ws_secret(json!({
            "name": "mock-api", "kind": "generic", "host_pattern": up.address().to_string(),
            "injection": {"kind": "header", "name": "x-api-key", "format": "{value}"}, "value": SECRET
        }))
        .await;
    api.grant(&api.daneel, &id).await;
    id
}

async fn log_rows(
    st: &Stack,
    host: &str,
) -> Vec<(Option<String>, Option<String>, Option<String>, i32)> {
    let end = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(Option<String>, Option<String>, Option<String>, i32)> = sqlx::query_as(
            "SELECT conversation_id::text, turn_id::text, secret_id::text, status FROM request_log WHERE host LIKE $1 AND path <> ''",
        )
        .bind(format!("{}%", host.split(':').next().unwrap()))
        .fetch_all(st.pool())
        .await
        .unwrap();
        if !rows.is_empty() || std::time::Instant::now() > end {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[tokio::test]
async fn s2_http_through_gateway_secret_injected_and_attributed() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let up = upstream().await;
    let secret = grant_secret(&api, &up).await;
    api.set_policy("allow", json!([])).await;
    let conv = api.direct().await;
    let host = up.address().to_string();
    let v = api
        .say(&conv, &format!("[[http: GET http://{host}/ping]]"))
        .await;
    let turn = turn_id(&v);
    let (s, e) = st.wait_turn(&turn, &["done", "failed"], 120).await;
    assert_eq!((s.as_str(), e), ("done", None), "{}", st.log_tails());
    let text = transcript_text(&api.events(&conv).await);
    assert!(text.contains("http 200: upstream-ok"), "{text}");
    // The mock saw the injected secret; the agent side never did.
    let got = up.received_requests().await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].headers.get("x-api-key").unwrap().to_str().unwrap(),
        SECRET
    );
    assert!(!text.contains(SECRET));
    // Attribution in the request log.
    let rows = log_rows(&st, &host).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0.as_deref(), Some(conv.as_str()));
    assert_eq!(rows[0].1.as_deref(), Some(turn.as_str()));
    assert_eq!(rows[0].2.as_deref(), Some(secret.as_str()));
    assert_eq!(rows[0].3, 200);
    st.finish().await;
}

#[tokio::test]
async fn s3_require_approval_holds_request_with_turn_attribution() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let up = upstream().await;
    grant_secret(&api, &up).await;
    let host = up.address().to_string();
    api.set_policy(
        "block",
        json!([{"name": "ask", "action": "allow", "targets": [{"type": "network", "host": host}],
                "require_approval": true}]),
    )
    .await;
    let conv = api.direct().await;
    let v = api
        .say(
            &conv,
            &format!("[[http: GET http://{host}/needs-approval]]"),
        )
        .await;
    let turn = turn_id(&v);
    let a = api.wait_pending().await;
    assert_eq!(a["status"], "pending");
    assert_eq!(a["path"], "/needs-approval");
    assert_eq!(a["conversation_id"].as_str(), Some(conv.as_str()));
    assert_eq!(a["turn_id"].as_str(), Some(turn.as_str()));
    assert_eq!(
        up.received_requests().await.unwrap().len(),
        0,
        "held before approval"
    );
    let (s, d) = api
        .post(
            &format!("/v1/approvals/{}/decision", a["id"].as_str().unwrap()),
            json!({"decision": "approve"}),
        )
        .await;
    assert_eq!(s, 200, "{d}");
    let (s, e) = st.wait_turn(&turn, &["done", "failed"], 90).await;
    assert_eq!((s.as_str(), e), ("done", None), "{}", st.log_tails());
    assert!(transcript_text(&api.events(&conv).await).contains("http 200"));
    st.finish().await;
}

async fn run_turn(st: &Stack, api: &Api, conv: &str, text: &str) -> String {
    let v = api.say(conv, text).await;
    let turn = turn_id(&v);
    let (s, e) = st
        .wait_turn(&turn, &["done", "failed", "aborted"], 120)
        .await;
    assert_eq!((s.as_str(), e), ("done", None), "{}", st.log_tails());
    turn
}

#[tokio::test]
async fn s4_idle_park_keeps_home_and_next_message_wakes() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts {
        idle_stop_secs: Some(10),
    })
    .await;
    let conv = api.direct().await;
    // The marker file proves the home volume survives the park; write it while the turn runs
    // (the boot itself takes longer than the idle window).
    let v = api.say(&conv, "[[sleep: 20s]] [[say: one]]").await;
    let turn = turn_id(&v);
    st.wait_turn(&turn, &["running"], 120).await;
    let c = containers();
    assert_eq!(c.len(), 1, "{c:?}");
    let (rc, out) = exec_in(&c[0], "echo persisted > /workspace/marker");
    assert_eq!(rc, 0, "{out}");
    st.wait_turn(&turn, &["done"], 60).await;
    let boot1 = api.sandbox().await["boot_id"].clone();
    assert!(!boot1.is_null());
    let sb = st.wait_sandbox(&api, "stopped", 120).await;
    let _ = sb;
    let reason: Option<String> = sqlx::query_scalar("SELECT stop_reason FROM sandboxes")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(reason.as_deref(), Some("idle"));
    assert!(containers().is_empty(), "container must be removed on park");
    assert_eq!(volumes().len(), 1, "home volume must be kept");
    // Next message wakes a fresh boot on the same home.
    let v = api.say(&conv, "[[sleep: 20s]] [[say: two]]").await;
    let turn = turn_id(&v);
    st.wait_turn(&turn, &["running"], 120).await;
    let boot2 = api.sandbox().await["boot_id"].clone();
    assert_ne!(boot1, boot2, "wake mints a new boot");
    let c = containers();
    assert_eq!(c.len(), 1);
    let (rc, out) = exec_in(&c[0], "cat /workspace/marker");
    assert_eq!((rc, out.trim()), (0, "persisted"));
    st.wait_turn(&turn, &["done"], 60).await;
    st.finish().await;
}

#[tokio::test]
async fn s5_restart_fails_active_turn_and_boots_anew() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "[[say: warm]]").await;
    let boot1 = api.sandbox().await["boot_id"].clone();
    let v = api.say(&conv, "[[sleep: 120s]]").await;
    let turn = turn_id(&v);
    st.wait_turn(&turn, &["running"], 60).await;
    let (s, b) = api
        .post(&format!("/v1/agents/{}/restart", api.daneel), json!({}))
        .await;
    assert_eq!(s, 202, "{b}");
    let (s, e) = st.wait_turn(&turn, &["failed"], 30).await;
    assert_eq!(
        (s.as_str(), e.as_deref()),
        ("failed", Some("agent_restarted"))
    );
    // The next message starts a fresh boot.
    run_turn(&st, &api, &conv, "[[say: back]]").await;
    let boot2 = api.sandbox().await["boot_id"].clone();
    assert_ne!(boot1, boot2);
    assert_eq!(containers().len(), 1, "old container removed");
    st.finish().await;
}

async fn followup_state(st: &Stack, id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM turns WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(st.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn s6_followup_joins_or_is_promoted() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "[[say: warm]]").await;
    // Joined: the fake harness accepts the steer.
    let v = api.say(&conv, "[[say: first]] [[steer-accept]]").await;
    let turn = turn_id(&v);
    st.wait_turn(&turn, &["running"], 60).await;
    let f = api.say(&conv, "second-word").await;
    let fid = f["followup"]["id"].as_str().expect("followup").to_owned();
    st.wait_turn(&turn, &["done", "failed"], 60).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while followup_state(&st, &fid).await == "joining" && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(followup_state(&st, &fid).await, "joined");
    assert!(transcript_text(&api.events(&conv).await).contains("steered: second-word"));
    // Promoted: no steer-accept, so the follow-up is promoted to its own turn.
    let v = api.say(&conv, "[[sleep: 4s]] [[say: slow]]").await;
    let turn2 = turn_id(&v);
    st.wait_turn(&turn2, &["running"], 60).await;
    let f = api.say(&conv, "[[say: promoted-reply]]").await;
    let fid2 = f["followup"]["id"].as_str().expect("followup").to_owned();
    st.wait_turn(&turn2, &["done"], 60).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let st_f = followup_state(&st, &fid2).await;
        let text = transcript_text(&api.events(&conv).await);
        if st_f == "promoted" && text.contains("promoted-reply") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "followup stuck at {st_f}\n{}",
            st.log_tails()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    st.finish().await;
}

#[tokio::test]
async fn s7_never_started_revived_once_then_failed() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "[[say: warm]]").await;
    let c = containers();
    assert_eq!(c.len(), 1);
    // Freeze the supervisor so it can never acknowledge the dispatched turn.
    let (rc, out) = podman_rc(&["pause", &c[0]]);
    assert_eq!(rc, 0, "{out}");
    let v = api.say(&conv, "[[say: never]]").await;
    let turn = turn_id(&v);
    let (mut revived, mut ages) = (0, 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(150);
    let result = loop {
        let (s, e): (String, Option<String>) =
            sqlx::query_as("SELECT status, error_code FROM turns WHERE id = $1::uuid")
                .bind(&turn)
                .fetch_one(st.pool())
                .await
                .unwrap();
        match s.as_str() {
            "dispatched" => {
                let n = sqlx::query(
                    "UPDATE turns SET dispatched_at = now() - interval '200 seconds' WHERE id = $1::uuid AND dispatched_at > now() - interval '100 seconds'",
                )
                .bind(&turn)
                .execute(st.pool())
                .await
                .unwrap()
                .rows_affected();
                ages += n;
            }
            "queued" if ages > 0 => revived += 1,
            "failed" | "done" | "aborted" => break (s, e),
            _ => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "turn stuck at {s} ({ages} ages)\n{}",
            st.log_tails()
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let _ = podman_rc(&["unpause", &c[0]]);
    assert_eq!(
        (result.0.as_str(), result.1.as_deref()),
        ("failed", Some("never_started")),
        "{}",
        st.log_tails()
    );
    let _ = revived;
    assert_eq!(ages, 2, "dispatched exactly twice: original + one revival");
    st.finish().await;
}

/// Sends one newline-delimited JSON request to the supervisor tools socket as uid 1000.
fn tools_sock(container: &str, req: &Value) -> String {
    let script = "import socket,sys\ns=socket.socket(socket.AF_UNIX);s.settimeout(20)\ns.connect('/run/aurora/tools.sock');s.sendall((sys.argv[1]+'\\n').encode());print(s.makefile().readline())";
    podman_rc(&[
        "exec",
        "--user",
        "1000:1000",
        container,
        "python3",
        "-c",
        script,
        &req.to_string(),
    ])
    .1
}

/// TCP connect probe from inside the sandbox: true when connected.
fn can_connect(c: &str, host: &str, port: u16) -> bool {
    exec_in(c, &format!("timeout 3 bash -c '</dev/tcp/{host}/{port}'")).0 == 0
}

fn boot_doc(c: &str) -> Value {
    let env = podman(&["inspect", c, "--format", "{{json .Config.Env}}"]);
    let env: Vec<String> = serde_json::from_str(env.trim()).unwrap();
    let b = env
        .iter()
        .find_map(|e| e.strip_prefix("AURORA_BOOT="))
        .expect("AURORA_BOOT is set on the container (operator view)");
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b)
        .unwrap();
    serde_json::from_slice(&raw).unwrap()
}

#[tokio::test]
async fn s8_security_probes() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let up = upstream().await;
    grant_secret(&api, &up).await;
    api.set_policy("allow", json!([])).await;
    let host = up.address().to_string();
    let conv = api.direct().await;
    // A turn that uses the secret, then a long one so the sandbox stays up for probing.
    run_turn(&st, &api, &conv, &format!("[[http: GET http://{host}/x]]")).await;
    let v = api.say(&conv, "[[sleep: 240s]]").await;
    let turn = turn_id(&v);
    st.wait_turn(&turn, &["running"], 60).await;
    let cs = containers();
    assert_eq!(cs.len(), 1);
    let c = cs[0].as_str();
    let mut fails: Vec<String> = vec![];
    let mut check = |name: &str, ok: bool, detail: String| {
        eprintln!("[probe] {name}: {}", if ok { "ok" } else { "FAIL" });
        if !ok {
            fails.push(format!("{name}: {detail}"));
        }
    };

    // --- SEC-01: no real secret anywhere in the sandbox or the transcript.
    let (_, out) = exec_in(
        c,
        &format!(
            "grep -rIl --exclude-dir=proc '{SECRET}' /workspace /tmp /run /etc /home 2>/dev/null; cat /proc/[0-9]*/environ 2>/dev/null | tr '\\0' '\\n' | grep -c '{SECRET}'; env | grep -c '{SECRET}'"
        ),
    );
    let lines: Vec<&str> = out.lines().collect();
    check(
        "SEC-01 sandbox",
        lines.iter().all(|l| *l == "0"),
        out.clone(),
    );
    let t = transcript_text(&api.events(&conv).await);
    check("SEC-01 transcript", !t.contains(SECRET), "leak".into());
    let doc = boot_doc(c);
    check(
        "SEC-01 boot doc has no secret",
        !doc.to_string().contains(SECRET),
        "leak".into(),
    );

    // --- SEC-26: container hardening (inside + inspect).
    let (_, out) = exec_in(
        c,
        "id -u; id -g; grep CapEff /proc/self/status; grep NoNewPrivs /proc/self/status; touch /usr/x 2>&1 | head -1; touch /workspace/ok && echo home-writable",
    );
    check(
        "SEC-26 inside",
        out.contains("1000\n1000")
            && out.contains("0000000000000000")
            && out.contains("NoNewPrivs:\t1")
            && out.contains("Read-only")
            && out.contains("home-writable"),
        out,
    );
    let insp: Value = serde_json::from_str(&podman(&["inspect", c])).unwrap();
    let h = &insp[0]["HostConfig"];
    let mounts = insp[0]["Mounts"].as_array().cloned().unwrap_or_default();
    let host_binds = mounts.iter().filter(|m| m["Type"] == "bind").count();
    check(
        "SEC-26 inspect",
        h["Privileged"] == false
            && h["ReadonlyRootfs"] == true
            // podman 6 reports `--cap-drop=ALL` expanded; the effective set is checked inside (CapEff=0).
            && (h["CapDrop"].to_string().to_uppercase().contains("\"ALL\"")
                || h["CapDrop"].as_array().is_some_and(|a| a.len() >= 11))
            && h["CapAdd"].as_array().is_none_or(Vec::is_empty)
            && h["SecurityOpt"].to_string().contains("no-new-privileges")
            && h["PidsLimit"].as_i64().unwrap_or(0) > 0
            && h["Memory"].as_i64().unwrap_or(0) > 0
            && host_binds == 0
            && mounts.iter().filter(|m| m["Destination"] == "/workspace").count() == 1
            && h["Devices"].as_array().is_none_or(Vec::is_empty),
        format!("{h}\n{mounts:?}"),
    );

    // --- SEC-02: only gateway proxy + runner channel reachable; no DNS.
    check(
        "SEC-02 dns",
        exec_in(c, "getent hosts example.org").0 != 0,
        "resolved".into(),
    );
    check(
        "SEC-02 internet",
        !can_connect(c, "1.1.1.1", 53),
        "reachable".into(),
    );
    check(
        "SEC-02 postgres",
        !can_connect(c, "10.89.252.5", 5432),
        "reachable".into(),
    );
    check(
        "SEC-02 anchor/other",
        !can_connect(c, "10.89.251.2", 22),
        "reachable".into(),
    );
    check(
        "SEC-02 gateway proxy reachable",
        can_connect(c, "aurora-gateway", 4102),
        "control".into(),
    );
    check(
        "SEC-02 runner channel reachable",
        can_connect(c, "aurora-runner", 4105),
        "control".into(),
    );

    // --- SEC-09: internal ports not reachable from the sandbox.
    for (n, port) in [
        ("api public", 4101),
        ("api internal", 4111),
        ("hookhost", 4104),
    ] {
        check(
            &format!("SEC-09 {n}"),
            !can_connect(c, BRIDGE, port),
            "reachable".into(),
        );
    }

    // --- SEC-34 / I-23: supervisor environ unreadable, no AURORA_BOOT in children.
    let (rc, out) = exec_in(
        c,
        "cat /proc/1/environ | tr '\\0' '\\n' | grep -c AURORA_BOOT",
    );
    check(
        "SEC-34 /proc/1/environ",
        rc != 0 || out.trim() == "0",
        out.clone(),
    );
    let (rc, _) = exec_in(c, "cat /proc/1/mem >/dev/null");
    check("SEC-34 /proc/1/mem", rc != 0, "readable".into());
    // Child-process env (I-23): no supervisor-spawned child path is reachable with the fake harness
    // (process_* tools are not in the API tool registry); reported as a defect by inspection.

    // --- SEC-24: forged identity on the tools socket is fenced / ignored.
    let other = uuid::Uuid::now_v7().to_string();
    let r = tools_sock(
        c,
        &json!({"id": 2, "session": other, "method": "call", "tool": "memory_save",
        "arguments": {"text": "x", "agent_id": other, "conversation_id": other, "sandbox_id": other}}),
    );
    check(
        "SEC-24 forged session",
        r.contains("\"ok\":false") && r.contains("not_found"),
        r.clone(),
    );

    // --- SEC-25: the bootstrap token is single use (already spent by the supervisor).
    let tok = doc["bootstrap_token"].as_str().unwrap().to_owned();
    let used: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT bootstrap_used_at FROM sandboxes")
            .fetch_one(st.pool())
            .await
            .unwrap();
    check(
        "SEC-25 bootstrap_used_at stamped",
        used.is_some(),
        "null".into(),
    );
    let url = format!("ws://{BRIDGE}:4105/v1/supervisor");
    let welcomed = match tokio_tungstenite::connect_async(&url).await {
        Err(e) => format!("connect error {e}"),
        Ok((mut ws, _)) => {
            use futures::SinkExt;
            ws.send(tokio_tungstenite::tungstenite::Message::text(
                json!({"hello": {"protocol": 1, "bootstrap_token": tok}}).to_string(),
            ))
            .await
            .unwrap();
            match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
                Ok(Some(Ok(m))) => format!("{m:?}"),
                other => format!("closed: {other:?}"),
            }
        }
    };
    check(
        "SEC-25 replayed bootstrap rejected",
        !welcomed.contains("welcome"),
        welcomed.clone(),
    );
    eprintln!("[probe] replay -> {welcomed}");

    // Boot token dies with its boot: restart, then try it through the gateway.
    let boot_token = doc["gateway"]["boot_token"].as_str().unwrap().to_owned();
    let (s, _) = api
        .post(&format!("/v1/agents/{}/restart", api.daneel), json!({}))
        .await;
    assert_eq!(s, 202);
    st.wait_sandbox(&api, "stopped", 90).await;
    let proxy = reqwest::Proxy::all(format!("https://{BRIDGE}:4102"))
        .unwrap()
        .basic_auth("agent", &boot_token);
    let cl = reqwest::Client::builder()
        .proxy(proxy)
        .add_root_certificate(reqwest::Certificate::from_pem(st.ca_pem.as_bytes()).unwrap())
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let before = up.received_requests().await.unwrap().len();
    let r = cl.get(format!("http://{host}/after-stop")).send().await;
    let reached = up.received_requests().await.unwrap().len() > before;
    check(
        "SEC-25 boot token dead after boot",
        !reached && r.as_ref().map(|r| !r.status().is_success()).unwrap_or(true),
        format!("{r:?}"),
    );
    drop(check);
    st.finish().await;
    assert!(
        fails.is_empty(),
        "probe failures:\n{}",
        fails.join("\n---\n")
    );
}

#[tokio::test]
async fn s8b_gateway_and_runner_ops_ports_unreachable_from_sandbox() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let conv = api.direct().await;
    let v = api.say(&conv, "[[sleep: 120s]]").await;
    st.wait_turn(&turn_id(&v), &["running"], 120).await;
    let cs = containers();
    let c = cs[0].as_str();
    let g = can_connect(c, BRIDGE, 4103);
    let r = can_connect(c, BRIDGE, 4106);
    st.finish().await;
    assert!(
        !g,
        "SEC-02/09: gateway ops port :4103 reachable from the sandbox"
    );
    assert!(
        !r,
        "SEC-09: runner ops port :4106 reachable from the sandbox"
    );
}

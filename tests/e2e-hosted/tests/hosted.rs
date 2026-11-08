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
        ..Opts::default()
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

    // --- SEC-41 (RFE-003): an image without the `org.aurora.capabilities` label (the e2e
    // sandbox-min image) boots with no capability: nothing listens on the open proxy 3129,
    // while the machine proxy 3128 does.
    check(
        "SEC-41 no open-proxy listener without the capability",
        !can_connect(c, "127.0.0.1", 3129),
        "3129 is listening".into(),
    );
    check(
        "SEC-41 machine proxy still listening",
        can_connect(c, "127.0.0.1", 3128),
        "3128 is not listening".into(),
    );
    check(
        "SEC-41 boot document carries no capability",
        doc["capabilities"].as_array().is_none_or(Vec::is_empty),
        doc["capabilities"].to_string(),
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
        !can_connect(c, &pg_ip(), 5432),
        "reachable".into(),
    );
    check(
        "SEC-02 anchor/other",
        !can_connect(c, &sbx_other_ip(), 22),
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
            !can_connect(c, &bridge(), port),
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
    let url = format!("ws://{}:4105/v1/supervisor", bridge());
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
    let proxy = reqwest::Proxy::all(format!("https://{}:4102", bridge()))
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
    let g = can_connect(c, &bridge(), 4103);
    let r = can_connect(c, &bridge(), 4106);
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

/// Runs `command` as a background process through the supervisor's `process_start` tool (over the
/// tools socket, as the agent would) and returns what it wrote to `/workspace/<name>.out` once it
/// printed the `done` marker. Fails after `secs`.
async fn process_output(c: &str, conv: &str, name: &str, command: &str, secs: u64) -> String {
    let cmd = format!(
        "{{ {command}; }} > /workspace/{name}.out 2>&1; echo done-marker >> /workspace/{name}.out"
    );
    let r = tools_sock(
        c,
        &json!({"id": 1, "session": conv, "method": "call", "tool": "process_start",
                "arguments": {"name": name, "command": cmd}}),
    );
    assert!(r.contains("\"result\""), "process_start: {r}");
    let mut out = String::new();
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        out = exec_in(c, &format!("cat /workspace/{name}.out 2>/dev/null")).1;
        if out.contains("done-marker") {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("process {name} did not finish within {secs}s: {out}");
}

/// Lines of an environment dump (`KEY=value` per line) that carry boot material: an `AURORA_BOOT*`
/// variable, the bootstrap token, the gateway boot token, or any `aub_`/`aup_`/`auc_` credential
/// value (the prefix at a word start followed by a token-like run of 8 or more characters).
fn boot_material_in(dump: &str, bootstrap: &str, boot_token: &str) -> Vec<String> {
    let mut found = vec![];
    for line in dump.lines() {
        let key = line.split('=').next().unwrap_or("");
        if key.starts_with("AURORA_BOOT") {
            found.push(format!("AURORA_BOOT variable: {key}"));
        }
        if line.contains(bootstrap) {
            found.push("the bootstrap token".into());
        }
        if line.contains(boot_token) {
            found.push("the gateway boot token".into());
        }
        for prefix in ["aub_", "aup_", "auc_"] {
            let mut rest = line;
            while let Some(i) = rest.find(prefix) {
                let after = &rest[i + prefix.len()..];
                let run = after
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
                    .count();
                let before_ok = rest[..i]
                    .chars()
                    .next_back()
                    .is_none_or(|ch| !ch.is_ascii_alphanumeric());
                if before_ok && run >= 8 {
                    found.push(format!("a `{prefix}` credential value in `{key}`"));
                }
                rest = after;
            }
        }
    }
    found
}

/// Shell snippet: sets `pid` to the PID of the `sleep` process (the image has no `pgrep`).
const FIND_SLEEP: &str = "pid=; for d in /proc/[0-9]*; do [ \"$(cat $d/comm 2>/dev/null)\" = sleep ] && pid=${d#/proc/}; done";

/// SEC-34 / I-23 (TEST-013 follow-up, P2-TEST-1): a process started through `process_start` (the
/// supervisor's own child, the same path every agent command takes) must not carry boot material in
/// its environment: no `AURORA_BOOT`, no `aub_`/`aup_`/`auc_` credential, not the bootstrap token.
#[tokio::test]
async fn s8c_process_start_children_have_no_boot_material_in_their_environment() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "[[say: warm]]").await;
    let cs = containers();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let c = cs[0].as_str();
    // What the operator view holds, so the probe knows the values it must not find.
    let doc = boot_doc(c);
    let bootstrap = doc["bootstrap_token"].as_str().unwrap().to_owned();
    let boot_token = doc["gateway"]["boot_token"].as_str().unwrap().to_owned();
    assert!(boot_token.starts_with("aub_"), "{boot_token}");

    // The child's own environment (`/proc/self/environ` of the shell the supervisor spawned),
    // plus its parent's (the shell's parent is the supervisor on some layouts; PID 1 must stay
    // unreadable, as s8 checks).
    let out = process_output(
        c,
        &conv,
        "envprobe",
        "echo pid=$$; echo '--- environ'; tr '\\0' '\\n' < /proc/$$/environ; echo '--- env'; env",
        60,
    )
    .await;
    // The probe ran with a real environment (otherwise an empty file would pass vacuously).
    assert!(
        out.contains("pid=") && out.contains("--- environ") && out.contains("PATH="),
        "the probe did not capture an environment: {out}"
    );
    // Negative control: the same matcher must flag an environment that does carry boot material.
    // A process started by `podman exec` gets the container's configured environment (which holds
    // `AURORA_BOOT`), so its `env` is such an environment; if the matcher missed it, the probe
    // could not fail.
    let (_, exec_env) = exec_in(c, "env");
    assert!(
        !boot_material_in(&exec_env, &bootstrap, &boot_token).is_empty(),
        "negative control: the matcher found no boot material in the `podman exec` environment"
    );
    let mut fails: Vec<String> = boot_material_in(&out, &bootstrap, &boot_token);
    // The same from outside: a long-running process the supervisor started, whose
    // `/proc/<pid>/environ` another agent-uid process reads. (Not a sweep over every PID: processes
    // started by `podman exec` get the container's configured environment, `AURORA_BOOT` included,
    // by podman's design; only supervisor-spawned processes are the agent-visible ones.)
    let r = tools_sock(
        c,
        &json!({"id": 2, "session": conv, "method": "call", "tool": "process_start",
                "arguments": {"name": "sleeper", "command": "exec sleep 301"}}),
    );
    assert!(r.contains("\"result\""), "process_start: {r}");
    let cn = c.to_owned();
    let found = eventually(30, move || {
        exec_in(&cn, &format!("{FIND_SLEEP}; test -n \"$pid\"")).0 == 0
    })
    .await;
    assert!(found, "the started process is not running");
    let (rc, env2) = exec_in(
        c,
        &format!("{FIND_SLEEP}; tr '\\0' '\\n' < /proc/$pid/environ"),
    );
    assert!(
        rc == 0 && env2.contains("PATH="),
        "could not read the started process's environment: {env2}"
    );
    fails.extend(boot_material_in(&env2, &bootstrap, &boot_token));
    st.finish().await;
    assert!(
        fails.is_empty(),
        "SEC-34 probe failures:\n{}",
        fails.join("\n")
    );
}

/// A TLS page server on `127.0.0.1` (self-signed; the browser runs with `--ignore-certificate-errors`
/// because the open lane is a blind tunnel). Returns its port and the request heads it has seen.
async fn https_page_server(
    body: &'static str,
) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use rcgen::{CertificateParams, KeyPair};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(cfg));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else {
                return;
            };
            let (acceptor, seen) = (acceptor.clone(), seen2.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let mut n = 0;
                while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => n += k,
                    }
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (port, seen)
}

/// SEC-41 / FU-P1-003b (P2-TEST-1): the Chromium wrapper on the full sandbox image (the one whose
/// `org.aurora.capabilities` label carries `browser`). The sandbox boots with the capability, the
/// open proxy listens on 3129, the wrapper starts Chromium behind it, and a page fetch goes through
/// the gateway's open lane: a host on the open-lane allow-list is fetched, another is refused.
///
/// Needs the full image: run `AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh` (otherwise SKIPPED).
// Multi-threaded runtime: `exec_in` blocks its thread while Chromium runs, and the in-process page
// server must keep answering meanwhile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s8d_chromium_wrapper_on_the_full_image_fetches_through_the_open_proxy() {
    if !enabled() {
        return;
    }
    let Ok(image) = std::env::var("AURORA_E2EH_FULL_IMAGE_TAG") else {
        eprintln!(
            "SKIPPED: AURORA_E2EH_FULL_IMAGE_TAG is not set; run AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh"
        );
        return;
    };
    const MARKER: &str = "aurora-open-lane-page-9f3c1e";
    let (port, seen) =
        https_page_server("<html><body><p id=\"m\">aurora-open-lane-page-9f3c1e</p></body></html>")
            .await;
    let (st, api) = fx(Opts {
        image: Some(image),
        ..Opts::default()
    })
    .await;
    // Open lane under `allowlist_only`: the page server is admitted by its IP literal; the same
    // server under another name (`localhost`) is not on the list.
    api.set_policy_with(
        "none",
        json!([]),
        json!({"open_lane_posture": "allowlist_only", "uncredentialed_allowlist": [{"host": "127.0.0.1"}]}),
    )
    .await;
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "[[say: warm]]").await;
    let cs = containers();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let c = cs[0].as_str();

    // The sandbox booted with the `browser` capability and the supervisor opened the open proxy.
    let doc = boot_doc(c);
    assert_eq!(
        doc["capabilities"],
        json!(["browser"]),
        "boot capabilities\n{}",
        st.log_tails()
    );
    assert!(
        eventually(30, || can_connect(c, "127.0.0.1", 3129)).await,
        "the open proxy is not listening on 3129"
    );
    assert!(can_connect(c, "127.0.0.1", 3128), "machine proxy");

    let chromium = |url: &str| {
        exec_in(
            c,
            &format!(
                "HOME=/workspace timeout 120 chromium-browser --headless --no-sandbox --disable-gpu \
                 --disable-dev-shm-usage --ignore-certificate-errors --user-data-dir=/workspace/.chromium-profile \
                 --virtual-time-budget=10000 --dump-dom {url} 2>&1; echo rc=$?"
            ),
        )
    };
    // Allowed host: the page is fetched through the gateway and rendered.
    let (_, out) = chromium(&format!("https://127.0.0.1:{port}/allowed"));
    assert!(
        !out.contains("browsing is disabled") && !out.contains("not installed"),
        "the wrapper did not start Chromium behind the open proxy: {out}"
    );
    assert!(
        out.contains(MARKER),
        "allowed host: the page was not fetched: {out}\n{}",
        st.log_tails()
    );
    let got = seen.lock().unwrap().clone();
    assert!(
        got.iter().any(|r| r.starts_with("GET /allowed")),
        "the page server never saw the allowed request: {got:?}"
    );

    // Denied host: the gateway refuses the tunnel; the server sees nothing and no page is rendered.
    let before = seen.lock().unwrap().len();
    let (_, out) = chromium(&format!("https://localhost:{port}/denied"));
    assert!(
        !out.contains(MARKER),
        "denied host: the page was fetched: {out}"
    );
    assert!(
        out.contains("ERR_TUNNEL_CONNECTION_FAILED") || out.contains("ERR_"),
        "denied host: Chromium reported no network error: {out}"
    );
    // What the open proxy answers for the denied host: the gateway's egress-policy refusal (the same
    // CONNECT Chromium sent; `--noproxy ''` because the sandbox's `no_proxy` lists loopback names).
    let (_, tunnel) = exec_in(
        c,
        &format!(
            "exec 3<>/dev/tcp/127.0.0.1/3129; printf 'CONNECT localhost:{port} HTTP/1.1\\r\\nHost: localhost:{port}\\r\\n\\r\\n' >&3; timeout 10 cat <&3"
        ),
    );
    assert!(
        tunnel.contains("403") && tunnel.contains("blocked_by_egress_policy"),
        "denied host: the gateway did not refuse the tunnel by egress policy: {tunnel}"
    );
    let after = seen.lock().unwrap().clone();
    assert_eq!(
        after.len(),
        before,
        "the denied request reached the page server: {after:?}"
    );
    assert!(
        !after.iter().any(|r| r.contains("/denied")),
        "the denied request reached the page server: {after:?}"
    );
    st.finish().await;
}

/// Poll until `check` holds or `secs` pass.
async fn eventually(secs: u64, mut check: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    check()
}

/// RFE-006 / SEC-45: freeze an agent whose turn is running a request the gateway holds for
/// approval. The turn ends aborted/frozen and the held request is dropped (never forwarded), the
/// approval is abandoned, the sandbox is parked (`stop_reason = frozen`, container removed, home
/// volume kept), every door answers 423/410, schedules are skipped without being disabled, and
/// unfreezing restores service without any credential being redistributed.
#[tokio::test]
async fn s10_freeze_aborts_the_turn_drops_the_held_request_parks_the_sandbox_and_resumes() {
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
    // A schedule and a webhook of the agent.
    let (s, sched) = api
        .post(
            &format!("/v1/agents/{}/schedules", api.daneel),
            json!({"name": "tick", "prompt": "p", "cron": "0 9 * * *", "timezone": "UTC"}),
        )
        .await;
    assert_eq!(s, 201, "{sched}");
    let (s, hook) = api
        .post(
            &format!("/v1/agents/{}/webhooks", api.daneel),
            json!({"name": "hook", "instructions": "i"}),
        )
        .await;
    assert_eq!(s, 201, "{hook}");
    let hook_path = hook["url"]
        .as_str()
        .and_then(|u| u.split_once("/v1/"))
        .map(|(_, p)| format!("/v1/{p}"))
        .unwrap();

    // The turn's request is held for approval: a live, in-flight gateway request.
    let v = api
        .say(&conv, &format!("[[http: GET http://{host}/held]]"))
        .await;
    let turn = turn_id(&v);
    let pending = api.wait_pending().await;
    assert_eq!(pending["path"], "/held");
    let approval = pending["id"].as_str().unwrap().to_owned();
    let volumes_before = volumes();
    assert_eq!(volumes_before.len(), 1);

    let (s, b) = api
        .post(
            &format!("/v1/agents/{}/freeze", api.daneel),
            json!({"reason": "e2e drill"}),
        )
        .await;
    assert_eq!(s, 200, "{b}");
    assert_eq!(b["effective_frozen"], true);

    // The turn ends aborted / frozen; the held request is never forwarded; the approval dies.
    let (status, err) = st
        .wait_turn(&turn, &["aborted", "failed", "done"], 60)
        .await;
    assert_eq!(
        (status.as_str(), err.as_deref()),
        ("aborted", Some("frozen")),
        "{}",
        st.log_tails()
    );
    let a_status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1::uuid")
        .bind(&approval)
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(a_status, "abandoned");
    assert_eq!(up.received_requests().await.unwrap().len(), 0);

    // The sandbox is parked with its home kept.
    st.wait_sandbox(&api, "stopped", 120).await;
    let reason: Option<String> = sqlx::query_scalar("SELECT stop_reason FROM sandboxes")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(reason.as_deref(), Some("frozen"));
    assert!(containers().is_empty(), "container removed on park");
    assert_eq!(volumes(), volumes_before, "home volume kept for forensics");

    // Doors: messages 423, webhook 410 + paused delivery (still enabled), schedule skipped.
    let (s, b) = api
        .post(
            &format!("/v1/conversations/{conv}/messages"),
            json!({"text": "hello?"}),
        )
        .await;
    assert_eq!(
        (s, b["error"]["code"].as_str()),
        (423, Some("frozen")),
        "{b}"
    );
    let (s, b) = api.post(&hook_path, json!({})).await;
    assert_eq!(
        (s, b["error"]["code"].as_str()),
        (410, Some("frozen")),
        "{b}"
    );
    let (delivery, enabled): (String, bool) = sqlx::query_as(
        "SELECT d.status || ':' || coalesce(d.reason, ''), w.enabled
         FROM webhook_deliveries d JOIN webhooks w ON w.id = d.webhook_id
         ORDER BY d.created_at DESC LIMIT 1",
    )
    .fetch_one(st.pool())
    .await
    .unwrap();
    assert_eq!((delivery.as_str(), enabled), ("paused:frozen", true));
    sqlx::query("UPDATE schedules SET next_fire_at = now() - interval '1 minute'")
        .execute(st.pool())
        .await
        .unwrap();
    let mut skipped = false;
    for _ in 0..60 {
        let o: Option<String> = sqlx::query_scalar("SELECT last_outcome FROM schedules")
            .fetch_one(st.pool())
            .await
            .unwrap();
        if o.as_deref() == Some("skipped_frozen") {
            skipped = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(skipped, "{}", st.log_tails());
    let still_enabled: bool = sqlx::query_scalar("SELECT enabled FROM schedules")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert!(still_enabled, "a freeze never disables a schedule");
    assert!(
        containers().is_empty(),
        "no sandbox start for a frozen agent"
    );

    // Unfreeze: service resumes at once, with the same credentials and the same home.
    let (s, b) = api
        .post(&format!("/v1/agents/{}/unfreeze", api.daneel), json!({}))
        .await;
    assert_eq!(s, 200, "{b}");
    run_turn(&st, &api, &conv, "[[say: back again]]").await;
    assert_eq!(containers().len(), 1);
    assert_eq!(volumes(), volumes_before);
    st.finish().await;
}

/// RFE-010 / SEC-47: deleting a conversation also deletes the harness session files in the agent's
/// home: live while the sandbox runs, and (when it is parked) before the next turn after it boots.
#[tokio::test]
async fn s9_deleted_conversation_session_files_are_purged_live_and_after_a_park() {
    if !enabled() {
        return;
    }
    // Timing (P2-TEST-1, FU-P2-TEST-hosted): this scenario used a 10 s idle stop. A `podman exec`
    // into the container right after a turn raced that stop (rc 125 once the idle stop had removed
    // the container; the idle clock can already be old after a slow boot). The default idle window
    // (30 min) means the sandbox stays up for every live probe, and the "parked" phase below is
    // reached deterministically through the restart endpoint (which stops the sandbox and leaves
    // it stopped, as s8 relies on) instead of waiting for the idle timer. The purge-at-next-boot
    // behaviour under test is the same for any stopped sandbox; the idle stop itself is covered by
    // s4.
    let (st, api) = fx(Opts::default()).await;
    // The fake harness stores a marker file per session: `/workspace/.fake-sessions/<session ref>`.
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "[[say: remember the codeword]]").await;
    let marker = format!("/workspace/.fake-sessions/fake-{conv}");
    let c = containers();
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(
        exec_in(&c[0], &format!("test -f {marker}")).0,
        0,
        "session stored"
    );

    // Live: the running sandbox receives `session.purge` and removes the files.
    let (s, b) = api
        .call(
            reqwest::Method::DELETE,
            &format!("/v1/conversations/{conv}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{b}");
    let cn = c[0].clone();
    let m2 = marker.clone();
    let gone = eventually(60, move || exec_in(&cn, &format!("test ! -e {m2}")).0 == 0).await;
    assert!(gone, "session files purged live\n{}", st.log_tails());
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_purges WHERE done_at IS NULL")
            .fetch_one(st.pool())
            .await
            .unwrap();
    assert_eq!(pending, 0, "the purge row completes on `session.purged`");

    // Parked: delete while the sandbox is stopped; the purge runs at the next boot, before any turn.
    let conv2 = api.direct().await;
    assert_ne!(conv, conv2, "a new direct conversation is created");
    run_turn(&st, &api, &conv2, "[[say: second secret]]").await;
    let marker2 = format!("/workspace/.fake-sessions/fake-{conv2}");
    let (s, b) = api
        .post(&format!("/v1/agents/{}/restart", api.daneel), json!({}))
        .await;
    assert_eq!(s, 202, "{b}");
    st.wait_sandbox(&api, "stopped", 120).await;
    assert!(containers().is_empty(), "parked");
    let (s, b) = api
        .call(
            reqwest::Method::DELETE,
            &format!("/v1/conversations/{conv2}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{b}");
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_purges WHERE done_at IS NULL")
            .fetch_one(st.pool())
            .await
            .unwrap();
    assert_eq!(queued, 1, "waits for the sandbox to boot");
    let conv3 = api.direct().await;
    run_turn(&st, &api, &conv3, "[[say: wake up]]").await;
    let c = containers();
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(
        exec_in(&c[0], &format!("test ! -e {marker2}")).0,
        0,
        "the parked session's files were purged before the next turn\n{}",
        st.log_tails()
    );
    let cn = c[0].clone();
    assert_eq!(
        exec_in(
            &cn,
            &format!("test -f /workspace/.fake-sessions/fake-{conv3}")
        )
        .0,
        0
    );
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_purges WHERE done_at IS NULL")
            .fetch_one(st.pool())
            .await
            .unwrap();
    assert_eq!(pending, 0);
    st.finish().await;
}

/// The password of the database role the connection stores. It must never reach the sandbox.
const DB_PASSWORD: &str = "db-secret-pw-e2e-4711";

/// Registers a PostgreSQL app, a connection to `target` with sealed stored credentials, and
/// returns the connection id. The snapshot is the shape of `plugins/builtin/postgresql`.
async fn database_connection(
    st: &Stack,
    api: &Api,
    target: (&str, u16),
    label: &str,
) -> uuid::Uuid {
    use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
    use std::sync::Arc;
    let tool = |id: &str| json!({"id": id, "name": id, "host": "*.db.aurora.internal", "path": format!("/{id}"), "methods": ["POST"]});
    let snapshot = json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "postgresql", "version": "1.0.0", "digest": "d", "publisher": "aurora",
                      "hooks_digest": null, "egress": {}, "context": []}],
        "oauth_clients": [],
        "apps": [{
            "app_id": "postgresql", "package_id": "postgresql",
            "about": {"id": "postgresql", "name": "PostgreSQL"},
            "credentials": [], "metadata": [], "connect": [], "token": null,
            "tools": {"read": {"items": [tool("read")]},
                      "write": {"items": [tool("write"), tool("ddl"), tool("admin")]}},
            "approvals": {}, "agent": null, "database": {"engine": "postgres"}
        }],
        "hosts": [], "reserved_hosts": []
    });
    let hash: String = {
        use sha2::{Digest, Sha256};
        Sha256::digest(aurora_registry::snapshot::canonical_bytes(&snapshot))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    sqlx::query(
        "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at) \
         VALUES ($1, 1, $2, $3, 'e2e-db', now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(&hash)
    .bind(&snapshot)
    .execute(st.pool())
    .await
    .unwrap();
    sqlx::query("SELECT pg_notify('aurora_registry', '{}')")
        .execute(st.pool())
        .await
        .unwrap();
    let (org, ws): (uuid::Uuid, uuid::Uuid) = (api.org.parse().unwrap(), api.ws.parse().unwrap());
    let conn = uuid::Uuid::now_v7();
    let keys = {
        use base64::Engine;
        format!(
            "k1:{}",
            base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
        )
    };
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&keys, "k1").unwrap());
    let vault = SecretVault::new(
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap(),
    );
    let sealed = vault
        .seal(
            &SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: conn,
                org_id: org,
                workspace_id: Some(ws),
            },
            aurora_core::Secret::from(
                serde_json::to_vec(&json!({
                    "host": target.0, "port": target.1, "dbname": "postgres", "user": "e2e_login",
                    "password": DB_PASSWORD, "sslmode": "disable"
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'postgresql','postgresql','1.0.0',$4,'active',$5)")
        .bind(conn).bind(org).bind(ws).bind(label).bind(sealed.to_column())
        .execute(st.pool()).await.unwrap();
    conn
}

/// P2-DBL-2: a hosted agent runs `psql service=<label> -c 'select 1'` with no secret on the
/// command line; a write needs approval; the database password never reaches the sandbox.
#[tokio::test]
async fn s11_database_lane_psql_service_read_allowed_write_needs_approval() {
    if !enabled() {
        return;
    }
    let Some((dbhost, dbport)) = db_target() else {
        eprintln!("SKIPPED: AURORA_E2EH_DBTARGET is not set; run scripts/e2e/hosted.sh");
        return;
    };
    // The target database: a table, and the role the stored credentials log in as.
    let tdb = sqlx::PgPool::connect(&format!(
        "postgres://aurora:aurora@{dbhost}:{dbport}/postgres"
    ))
    .await
    .unwrap();
    for q in [
        "DROP TABLE IF EXISTS e2e_t".to_owned(),
        "CREATE TABLE e2e_t (id int PRIMARY KEY, v text)".to_owned(),
        "DROP ROLE IF EXISTS e2e_login".to_owned(),
        format!("CREATE ROLE e2e_login LOGIN PASSWORD '{DB_PASSWORD}'"),
        "GRANT ALL ON e2e_t TO e2e_login".to_owned(),
    ] {
        sqlx::query(&q).execute(&tdb).await.unwrap();
    }
    let (st, api) = fx(Opts::default()).await;
    let conn = database_connection(&st, &api, (&dbhost, dbport), "Orders DB").await;
    // The dashboard's view of the connection: the service name it shows (C-230).
    let mut service = String::new();
    for _ in 0..60 {
        let (s, v) = api
            .get(&format!("/v1/workspaces/{}/connections", api.ws))
            .await;
        assert_eq!(s, 200, "{v}");
        if let Some(sv) = v["items"][0]["database"]["service"].as_str() {
            service = sv.to_owned();
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(
        service, "orders-db",
        "the connection lists its libpq service name"
    );
    // Policy: default block; the grant allows read and asks for everything that changes state.
    api.set_policy("block", json!([])).await;
    let (s, v) = api
        .put(
            &format!("/v1/agents/{}/grants/connections/{conn}", api.daneel),
            json!({"mode": "custom", "allow_tools": ["read"], "ask_tools": ["write"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // The agent runs psql through the process tool (the same environment as any agent command).
    let c = api.direct().await;
    // (The fake harness only simulates tool calls; `process_start` is called over the tools socket,
    // which runs the command with the environment every agent process gets.)
    run_turn(&st, &api, &c, "[[say: warm]]").await;
    let cs = containers();
    assert_eq!(cs.len(), 1);
    let cn = cs[0].as_str();
    let psql = |name: &str, sql: &str| {
        let cmd = format!(
            "psql service={service} -At -c \"{sql}\" > /workspace/{name}.out 2>&1; echo rc=$? >> /workspace/{name}.out"
        );
        let r = tools_sock(
            cn,
            &json!({"id": 1, "session": c, "method": "call", "tool": "process_start",
                    "arguments": {"name": name, "command": cmd}}),
        );
        assert!(r.contains("\"result\""), "process_start: {r}");
    };
    psql("read", "select 40 + 2");
    let read_out = {
        let mut out = String::new();
        for _ in 0..60 {
            out = exec_in(cn, "cat /workspace/read.out 2>/dev/null").1;
            if out.contains("rc=") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        out
    };
    assert!(
        read_out.contains("42") && read_out.contains("rc=0"),
        "psql read: {read_out}\n{}",
        st.log_tails()
    );

    // The libpq service file points at the loopback relay; there is no password file.
    let (_, files) = exec_in(cn, "cd /workspace/.aurora/pg && ls && cat pg_service.conf");
    assert!(
        files.contains(&format!("[{service}]"))
            && files.contains("host=127.0.0.1")
            && files.contains("sslmode=disable"),
        "{files}"
    );
    assert!(files.contains(&format!("user={conn}")), "{files}");
    assert!(
        !files.contains("pgpass") && !files.contains("root.crt"),
        "{files}"
    );
    // Neither the database password nor any gateway credential (the boot token `aub_`, a turn
    // context credential `auc_`) is in a file or in the environment of any process an agent can
    // read (everything below runs as the agent's uid).
    let boot_token = boot_doc(cn)["gateway"]["boot_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(boot_token.starts_with("aub_"), "{boot_token}");
    for needle in [
        DB_PASSWORD,
        boot_token.as_str(),
        "au[bc]_[A-Za-z0-9_-]\\{8,\\}",
    ] {
        let (_, leak) = exec_in(
            cn,
            &format!(
                "grep -rIl -e '{needle}' /workspace /tmp /run /etc /home 2>/dev/null | head; cat /proc/[0-9]*/environ 2>/dev/null | tr '\\0' '\\n' | grep -c -e '{needle}'; env | grep -c -e '{needle}'"
            ),
        );
        assert!(
            leak.lines().all(|l| l.trim() == "0"),
            "a credential reached the sandbox ({needle}): {leak}"
        );
    }

    // A write is held for approval; the row appears only after it.
    psql(
        "write",
        "insert into e2e_t values (1, 'top-secret-literal')",
    );
    let a = api.wait_pending().await;
    assert_eq!(a["path"], "/write", "{a}");
    let summary = a["summary"].to_string();
    assert!(
        summary.contains("insert into e2e_t values (?, ?)"),
        "{summary}"
    );
    assert!(!summary.contains("top-secret-literal"), "{summary}");
    let n: i64 = sqlx::query_scalar("select count(*) from e2e_t")
        .fetch_one(&tdb)
        .await
        .unwrap();
    assert_eq!(n, 0, "held before approval");
    let (s, v) = api
        .post(
            &format!("/v1/approvals/{}/decision", a["id"].as_str().unwrap()),
            json!({"decision": "approve"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let mut write_out = String::new();
    for _ in 0..60 {
        write_out = exec_in(cn, "cat /workspace/write.out 2>/dev/null").1;
        if write_out.contains("rc=") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        write_out.contains("INSERT 0 1") && write_out.contains("rc=0"),
        "psql write: {write_out}"
    );
    let n: i64 = sqlx::query_scalar("select count(*) from e2e_t")
        .fetch_one(&tdb)
        .await
        .unwrap();
    assert_eq!(n, 1);

    // A schema change has no grant: refused with 42501, nothing reaches the database.
    psql("ddl", "drop table e2e_t");
    let mut ddl_out = String::new();
    for _ in 0..60 {
        ddl_out = exec_in(cn, "cat /workspace/ddl.out 2>/dev/null").1;
        if ddl_out.contains("rc=") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ddl_out.contains("rc=1"), "psql ddl: {ddl_out}");
    let n: i64 = sqlx::query_scalar("select count(*) from e2e_t")
        .fetch_one(&tdb)
        .await
        .unwrap();
    assert_eq!(n, 1, "the table survived");
    st.finish().await;
}

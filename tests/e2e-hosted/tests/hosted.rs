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

/// (FIX-151) One tools-socket request made **from the harness's own child**: the fake adapter's
/// `[[shell: …]]` directive runs the python client as a registered child of the harness, in a turn
/// of `conv`; the response line is read back from the container. A plain `podman exec` caller
/// (`tools_sock`) descends from no harness and is refused `session_not_caller`.
async fn tools_sock_as_harness(st: &Stack, api: &Api, c: &str, conv: &str, req: &Value) -> String {
    let body = req.to_string();
    assert!(
        !body.contains("]]"),
        "a directive cannot carry `]]`: {body}"
    );
    let script = "import socket,sys;s=socket.socket(socket.AF_UNIX);s.settimeout(20);s.connect(\"/run/aurora/tools.sock\");s.sendall((sys.argv[1]+\"\\n\").encode());print(s.makefile().readline())";
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let cmd = format!(
        "python3 -c {} {} > /tmp/toolsock.out 2>&1",
        q(script),
        q(&body)
    );
    run_turn(st, api, conv, &format!("[[shell: {cmd}]]")).await;
    exec_in(c, "cat /tmp/toolsock.out").1
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
        // FIX-151 (SEC-121): this `podman exec` process descends from no harness, so the socket
        // refuses it before anything else (`session_not_caller`), whatever conversation it names.
        // (A forged identity from a harness's own child is fenced by the API:
        // `api/it/tools.rs::identity_comes_from_authenticated_facts`.) Never `"ok":true`.
        r.contains("session_not_caller") && !r.contains("\"ok\":true"),
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
async fn process_output(
    st: &Stack,
    api: &Api,
    c: &str,
    conv: &str,
    name: &str,
    command: &str,
    secs: u64,
) -> String {
    let cmd = format!(
        "{{ {command}; }} > /workspace/{name}.out 2>&1; echo done-marker >> /workspace/{name}.out"
    );
    let r = tools_sock_as_harness(
        st,
        api,
        c,
        conv,
        &json!({"id": 1, "session": conv, "method": "call", "tool": "process_start",
                "arguments": {"name": name, "command": cmd}}),
    )
    .await;
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
        &st,
        &api,
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
    let r = tools_sock_as_harness(
        &st,
        &api,
        c,
        &conv,
        &json!({"id": 2, "session": conv, "method": "call", "tool": "process_start",
                "arguments": {"name": "sleeper", "command": "exec sleep 301"}}),
    )
    .await;
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

/// (P4-HAR, D-122, SEC-01, SEC-66) A Codex agent answers through the gateway LLM lane against the
/// mock Responses upstream: a turn, a shell tool call, a resumed second turn; the seeded model key
/// reaches only the upstream (injected by the gateway), and is found nowhere in the sandbox:
/// environment, files, rendered `config.toml`, no `auth.json`, transcripts.
///
/// Needs the full image (it carries the pinned Codex CLI): run
/// `AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh` (otherwise SKIPPED).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s12_codex_agent_answers_through_the_gateway_and_the_sandbox_holds_no_credential() {
    if !enabled() {
        return;
    }
    let Ok(image) = std::env::var("AURORA_E2EH_FULL_IMAGE_TAG") else {
        eprintln!(
            "SKIPPED: AURORA_E2EH_FULL_IMAGE_TAG is not set; run AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh"
        );
        return;
    };
    const SEED: &str = "sk-e2e-codex-SEEDED-7c1d9a40";
    // 127.0.0.2: the sandbox's NO_PROXY covers 127.0.0.1 and localhost, so the model call must
    // go through the proxy to the gateway (which may dial exactly this address).
    let up = aurora_testkit::responses::MockResponses::start_on("127.0.0.2").await;
    let toml = format!(
        r#"
[llm]
enabled_providers = ["litellm"]
provider_preference = ["litellm"]

[[llm.endpoints]]
id = "codex-mock"
provider = "litellm"
display_name = "Mock Responses upstream"
base_url = "{}"
dialects = ["openai_responses"]
default_model = "say"
"#,
        up.base()
    );
    Stack::wipe();
    let st = Stack::start_with(
        Opts {
            image: Some(image),
            ..Opts::default()
        },
        &toml,
    )
    .await;
    let api = st.signup("owner@e2e.test").await;
    sqlx::query(
        "UPDATE agents SET harness = 'codex', model_provider = 'litellm', \
         model_endpoint_id = 'codex-mock', model_id = 'say' WHERE id = $1::uuid",
    )
    .bind(&api.daneel)
    .execute(st.pool())
    .await
    .unwrap();
    let (s, v) = api
        .post(
            &format!("/v1/workspaces/{}/secrets", api.ws),
            json!({"name": "codex-mock-key", "kind": "llm",
                   "llm": {"endpoint_id": "codex-mock", "auth_mode": "api_key"},
                   "value": SEED}),
        )
        .await;
    assert_eq!(s, 201, "llm key: {v}\n{}", st.log_tails());
    api.grant(&api.daneel, v["id"].as_str().unwrap()).await;

    // Turn 1: a plain answer.
    let conv = api.direct().await;
    let v = api.say(&conv, "Reply with exactly the word: pong").await;
    let (status, err) = st
        .wait_turn(&turn_id(&v), &["done", "failed", "aborted"], 120)
        .await;
    if status != "done" {
        let msgs: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT status, error_message FROM turns")
                .fetch_all(st.pool())
                .await
                .unwrap_or_default();
        let logs: String = containers()
            .iter()
            .map(|c| {
                let (_, l) = podman_rc(&["logs", "--tail", "60", c]);
                let (_, cx) = exec_in(c, "ls -la /workspace/.codex 2>&1 | head; cat /workspace/.codex/config.toml 2>&1 | head -40");
                format!("--- {c} ---\n{l}\n{cx}")
            })
            .collect();
        panic!(
            "codex turn {status} {err:?}: {msgs:?}\n{logs}\n{}",
            st.log_tails()
        );
    }
    let t1 = transcript_text(&api.events(&conv).await);
    assert!(
        t1.contains("pong"),
        "answer missing: {t1}\n{}",
        st.log_tails()
    );

    // The agent's harness is what the API stamped; the sandbox booted Codex and nothing else.
    let cs = containers();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let c = cs[0].as_str();
    assert_eq!(boot_doc(c)["harness"]["id"], "codex");
    let (_, home) = exec_in(
        c,
        "ls -a /workspace/.codex 2>&1; echo; cat /workspace/.codex/config.toml 2>&1",
    );
    assert!(home.contains("config.toml"), "{home}");
    assert!(!home.contains("auth.json"), "no auth file: {home}");
    assert!(home.contains("approval_policy = \"never\""), "{home}");
    assert!(
        home.contains("aurora-gateway") || home.contains("127.0.0.1") || home.contains("base_url"),
        "{home}"
    );

    // Turn 2: a shell tool call; the sandbox runs `echo hi`.
    sqlx::query("UPDATE agents SET model_id = 'tool' WHERE id = $1::uuid")
        .bind(&api.daneel)
        .execute(st.pool())
        .await
        .unwrap();
    let v = api
        .say(&conv, "Run `echo hi` with your shell tool, then say done.")
        .await;
    let (status, err) = st
        .wait_turn(&turn_id(&v), &["done", "failed", "aborted"], 120)
        .await;
    if status != "done" {
        let msgs: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT status, error_message FROM turns ORDER BY created_at")
                .fetch_all(st.pool())
                .await
                .unwrap_or_default();
        panic!(
            "codex tool turn {status} {err:?}: {msgs:?}\nupstream saw {} requests\n{}",
            up.requests().len(),
            st.log_tails()
        );
    }
    let t2 = transcript_text(&api.events(&conv).await);
    assert!(
        t2.contains("shell") && t2.contains("done"),
        "tool turn: {t2}\n{}",
        st.log_tails()
    );

    // The second turn resumed the same Codex thread: the upstream saw the first answer again.
    let seen = up.requests();
    assert!(
        seen.iter()
            .skip(1)
            .any(|r| r.body.to_string().contains("pong")),
        "the resumed turn did not carry the earlier conversation: {} requests",
        seen.len()
    );

    // The model calls went THROUGH the gateway proxy (not bypassed by the sandbox's NO_PROXY):
    // the gateway's request log attributes them to this conversation and the injected key.
    let end = std::time::Instant::now() + Duration::from_secs(60);
    let rows = loop {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, Option<String>, Option<String>, i32)> = sqlx::query_as(
            "SELECT path, conversation_id::text, secret_id::text, status FROM request_log \
             WHERE host LIKE '127.0.0.2%' AND path LIKE '/v1/responses%'",
        )
        .fetch_all(st.pool())
        .await
        .unwrap();
        if rows.len() >= 2 || std::time::Instant::now() > end {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    assert!(
        rows.iter()
            .any(|(_, c, s, status)| c.as_deref() == Some(conv.as_str())
                && s.is_some()
                && *status == 200),
        "no attributed 200 /v1/responses row in the gateway request log: {rows:?}\n{}",
        st.log_tails()
    );

    // SEC-01 / SEC-66: only the gateway held the key. The upstream received it; nothing else did.
    assert!(!seen.is_empty());
    for r in &seen {
        assert_eq!(
            r.authorization.as_deref(),
            Some(format!("Bearer {SEED}").as_str()),
            "the gateway did not inject the key: {r:?}"
        );
    }
    let (_, env) = podman_rc(&["inspect", "--format", "{{json .Config.Env}}", c]);
    assert!(
        !env.contains(SEED),
        "key leaked into the container environment"
    );
    let (_, found) = exec_in(
        c,
        &format!(
            "grep -rIl --exclude-dir=proc --exclude-dir=sys -e '{SEED}' /workspace /run /tmp /etc 2>/dev/null; \
                  cat /proc/1/environ 2>/dev/null | tr '\\0' '\\n' | grep -c '{SEED}'; \
                  for p in /proc/[0-9]*; do cat $p/environ 2>/dev/null | tr '\\0' '\\n'; done | grep -c '{SEED}'"
        ),
    );
    assert!(
        found
            .lines()
            .all(|l| l.trim() == "0" || l.trim().is_empty()),
        "the key is present in the sandbox: {found}"
    );
    assert!(!t1.contains(SEED) && !t2.contains(SEED));
    for r in &seen {
        // The model never sees a credential either.
        assert!(!r.body.to_string().contains(SEED));
    }
    st.finish().await;
}

/// (P5-HARN, C-1010..C-1016, SEC-01, SEC-66, SEC-133) A hosted agent on `harness` answers through the gateway
/// LLM lane against the mock upstream of `dialect`: a turn, a shell tool call and a resumed turn; the seeded
/// key reaches only the upstream (injected by the gateway), the sandbox booted that harness and nothing else, and
/// the key is found nowhere in the sandbox (environment, files, rendered config, transcripts).
async fn added_harness_holds_no_credential(
    harness: &str,
    dialect: &str,
    path: &str,
    shell_tool: &str,
    state_dir: &str,
) {
    if !enabled() {
        return;
    }
    let Ok(image) = std::env::var("AURORA_E2EH_FULL_IMAGE_TAG") else {
        eprintln!(
            "SKIPPED: AURORA_E2EH_FULL_IMAGE_TAG is not set; run AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh"
        );
        return;
    };
    let seed = format!("sk-e2e-{harness}-SEEDED-7c1d9a40");
    // 127.0.0.2: the sandbox's NO_PROXY covers 127.0.0.1 and localhost, so the model call must go
    // through the proxy to the gateway (which may dial exactly this address).
    let up = aurora_testkit::harness_upstream::HarnessUpstream::start_on(
        "127.0.0.2",
        "aurora_ping",
        shell_tool,
        "task",
    )
    .await;
    let base = up.base().to_owned();
    let toml = format!(
        r#"
[llm]
enabled_providers = ["litellm"]
provider_preference = ["litellm"]

[[llm.endpoints]]
id = "mock-{harness}"
provider = "litellm"
display_name = "Mock upstream"
base_url = "{base}"
dialects = ["{dialect}"]
default_model = "say"
"#
    );
    Stack::wipe();
    let st = Stack::start_with(
        Opts {
            image: Some(image),
            ..Opts::default()
        },
        &toml,
    )
    .await;
    let api = st.signup("owner@e2e.test").await;
    sqlx::query(
        "UPDATE agents SET harness = $2, model_provider = 'litellm', \
         model_endpoint_id = $3, model_id = 'say' WHERE id = $1::uuid",
    )
    .bind(&api.daneel)
    .bind(harness)
    .bind(format!("mock-{harness}"))
    .execute(st.pool())
    .await
    .unwrap();
    let (s, v) = api
        .post(
            &format!("/v1/workspaces/{}/secrets", api.ws),
            json!({"name": format!("{harness}-mock-key"), "kind": "llm",
                   "llm": {"endpoint_id": format!("mock-{harness}"), "auth_mode": "api_key"},
                   "value": seed}),
        )
        .await;
    assert_eq!(s, 201, "llm key: {v}\n{}", st.log_tails());
    api.grant(&api.daneel, v["id"].as_str().unwrap()).await;

    let conv = api.direct().await;
    let v = api.say(&conv, "Reply with exactly the word: pong").await;
    let (status, err) = st
        .wait_turn(&turn_id(&v), &["done", "failed", "aborted"], 180)
        .await;
    if status != "done" {
        let msgs: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT status, error_message FROM turns")
                .fetch_all(st.pool())
                .await
                .unwrap_or_default();
        let logs: String = containers()
            .iter()
            .map(|c| format!("--- {c} ---\n{}", podman_rc(&["logs", "--tail", "60", c]).1))
            .collect();
        panic!(
            "{harness} turn {status} {err:?}: {msgs:?}\n{logs}\n{}",
            st.log_tails()
        );
    }
    let t1 = transcript_text(&api.events(&conv).await);
    assert!(t1.contains("pong"), "answer missing: {t1}");

    // The sandbox booted the agent's harness and nothing else, with its state in its own directory.
    let cs = containers();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let c = cs[0].as_str();
    assert_eq!(boot_doc(c)["harness"]["id"], harness);
    let (_, home) = exec_in(c, &format!("ls -R {state_dir} 2>&1 | head -40"));
    assert!(!home.contains("No such file"), "{state_dir}: {home}");
    assert!(
        !home.contains("auth.json") && !home.contains("secrets.yaml"),
        "no credential store: {home}"
    );

    // A shell tool call, then a resumed turn that carries the first answer.
    sqlx::query("UPDATE agents SET model_id = 'tool' WHERE id = $1::uuid")
        .bind(&api.daneel)
        .execute(st.pool())
        .await
        .unwrap();
    let v = api
        .say(&conv, "Run `echo hi` with your shell tool, then say done.")
        .await;
    let (status, err) = st
        .wait_turn(&turn_id(&v), &["done", "failed", "aborted"], 180)
        .await;
    assert_eq!(
        status,
        "done",
        "{harness} tool turn {status} {err:?}\n{}",
        st.log_tails()
    );
    let t2 = transcript_text(&api.events(&conv).await);
    assert!(t2.contains("done"), "tool turn: {t2}");
    let seen = up.requests();
    assert!(
        seen.iter()
            .skip(1)
            .any(|r| r.body.to_string().contains("pong")),
        "the resumed turn did not carry the earlier conversation: {} requests",
        seen.len()
    );

    // The model calls went through the gateway: its request log attributes them to this conversation.
    let end = std::time::Instant::now() + Duration::from_secs(60);
    let rows = loop {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, Option<String>, Option<String>, i32)> = sqlx::query_as(
            "SELECT path, conversation_id::text, secret_id::text, status FROM request_log \
             WHERE host LIKE '127.0.0.2%' AND path LIKE $1",
        )
        .bind(format!("{path}%"))
        .fetch_all(st.pool())
        .await
        .unwrap();
        if rows.len() >= 2 || std::time::Instant::now() > end {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    assert!(
        rows.iter()
            .any(|(_, c, s, status)| c.as_deref() == Some(conv.as_str())
                && s.is_some()
                && *status == 200),
        "no attributed 200 {path} row in the gateway request log: {rows:?}\n{}",
        st.log_tails()
    );

    // SEC-01 / SEC-66 / SEC-133: only the gateway held the key. Every request the upstream saw (the
    // inference path and, for goose on LiteLLM, the `GET /model/info` the gateway forwards, D-367)
    // carried it; nothing else of the sandbox did.
    assert!(!seen.is_empty());
    for r in &seen {
        assert_eq!(
            r.credential().as_deref(),
            Some(seed.as_str()),
            "the gateway did not inject the key: {r:?}"
        );
        // (`GET /v1/models` is the API verifying the stored key when it is granted, not the harness.)
        assert!(
            r.path == path || r.path == "/model/info" || r.path == "/v1/models",
            "a request outside the inference path reached the endpoint: {r:?}"
        );
        assert!(!r.body.to_string().contains(&seed));
    }
    assert_sandbox_holds_none(c, &[seed.as_str()]);
    assert!(!t1.contains(&seed) && !t2.contains(&seed));
    st.finish().await;
}

// (Pi has no probe here: it is Node.js with undici's `EnvHttpProxyAgent`, which tunnels with CONNECT
// even to an `http://` origin, and the gateway terminates CONNECT as TLS, so it cannot reach this
// suite's plain-HTTP mock upstream. It works against https endpoints like Claude Code does; a probe
// needs a TLS mock upstream. Reported in the P5-HARN completion report.)

/// (P5-HARN, SEC-133, D-367) goose on LiteLLM's Chat dialect (it asks `GET /model/info`, which the gateway
/// forwards with the key).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s41_goose_agent_answers_through_the_gateway_and_the_sandbox_holds_no_credential() {
    added_harness_holds_no_credential(
        "goose",
        "openai_chat",
        "/v1/chat/completions",
        "shell",
        "/workspace/.aurora/harness/goose",
    )
    .await;
}

/// (P5-HARN, SEC-133) OpenCode on the Responses dialect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s42_opencode_agent_answers_through_the_gateway_and_the_sandbox_holds_no_credential() {
    added_harness_holds_no_credential(
        "opencode",
        "openai_responses",
        "/v1/responses",
        "bash",
        "/workspace/.aurora/harness/opencode",
    )
    .await;
}

/// (P4-LLM-5, SEC-01, SEC-64, SEC-97) The sandbox holds none of `needles`: not in the container's configured
/// environment, not in any file under the places a harness writes, not in any process environment.
fn assert_sandbox_holds_none(c: &str, needles: &[&str]) {
    let (_, env) = podman_rc(&["inspect", "--format", "{{json .Config.Env}}", c]);
    for n in needles {
        assert!(!env.contains(n), "{n} is in the container environment");
        let (_, found) = exec_in(
            c,
            &format!(
                "grep -rIl --exclude-dir=proc --exclude-dir=sys -e '{n}' /workspace /run /tmp /etc /home 2>/dev/null; \
                 for p in /proc/[0-9]*; do cat $p/environ 2>/dev/null | tr '\\0' '\\n'; done | grep -c -e '{n}'"
            ),
        );
        assert!(
            found
                .lines()
                .all(|l| l.trim() == "0" || l.trim().is_empty()),
            "{n} is present in the sandbox: {found}"
        );
    }
}

/// (P4-LLM-5, D-261, C-809, SEC-64) A Codex agent on `azure_openai` with each auth mode: the turn completes, the
/// gateway injects `api-key` (resource key) or a minted `Bearer` token (Entra application) and nothing Azure-specific is
/// rendered for Codex; neither the key, the client secret nor the minted token is found in the sandbox or the
/// transcript. Needs the full image (pinned Codex CLI): `AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s25_codex_on_azure_openai_holds_no_credential_in_the_sandbox_for_either_auth_mode() {
    use aurora_testkit::llm::entra::{MockEntra, TestApp};
    if !enabled() {
        return;
    }
    let Ok(image) = std::env::var("AURORA_E2EH_FULL_IMAGE_TAG") else {
        eprintln!(
            "SKIPPED: AURORA_E2EH_FULL_IMAGE_TAG is not set; run AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh"
        );
        return;
    };
    const KEY: &str = "azure-resource-key-SEEDED-5b2e7f10";
    let app = TestApp {
        tenant_id: "11111111-2222-3333-4444-555555555555".into(),
        client_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
        client_secret: "entra-client-secret-SEEDED-3a9c41d2".into(),
    };
    for entra_mode in [false, true] {
        let entra = MockEntra::start(&app).await;
        // 127.0.0.2: outside the sandbox's NO_PROXY, so the model call goes through the gateway.
        let up = aurora_testkit::responses::MockResponses::start_on("127.0.0.2").await;
        let toml = format!(
            r#"
[llm]
enabled_providers = ["azure_openai"]
provider_preference = ["azure_openai"]

[llm.dev_upstream_overrides]
"https://login.microsoftonline.com" = "{}"

[[llm.endpoints]]
id = "az"
provider = "azure_openai"
base_url = "{}"
models = ["say", "tool"]
default_model = "say"
"#,
            entra.base(),
            up.base()
        );
        Stack::wipe();
        let st = Stack::start_with(
            Opts {
                image: Some(image.clone()),
                ..Opts::default()
            },
            &toml,
        )
        .await;
        let api = st.signup("owner@e2e.test").await;
        sqlx::query(
            "UPDATE agents SET harness = 'codex', model_provider = 'azure_openai', \
             model_endpoint_id = 'az', model_id = 'say' WHERE id = $1::uuid",
        )
        .bind(&api.daneel)
        .execute(st.pool())
        .await
        .unwrap();
        let (mode, value) = if entra_mode {
            ("azure_entra", app.json())
        } else {
            ("api_key", KEY.to_owned())
        };
        let (s, v) = api
            .post(
                &format!("/v1/workspaces/{}/secrets", api.ws),
                json!({"name": "azure-key", "kind": "llm",
                       "llm": {"endpoint_id": "az", "auth_mode": mode}, "value": value}),
            )
            .await;
        assert_eq!(s, 201, "{mode}: {v}\n{}", st.log_tails());
        api.grant(&api.daneel, v["id"].as_str().unwrap()).await;

        let conv = api.direct().await;
        let v = api.say(&conv, "Reply with exactly the word: pong").await;
        let (status, err) = st
            .wait_turn(&turn_id(&v), &["done", "failed", "aborted"], 120)
            .await;
        assert_eq!(status, "done", "{mode}: {err:?}\n{}", st.log_tails());
        let t = transcript_text(&api.events(&conv).await);
        assert!(t.contains("pong"), "{mode}: {t}\n{}", st.log_tails());

        let seen = up.requests();
        assert!(!seen.is_empty(), "{mode}: the upstream saw nothing");
        let tokens = entra.issued();
        for r in &seen {
            if entra_mode {
                let token = tokens.last().expect("a minted token");
                assert_eq!(
                    r.authorization.as_deref(),
                    Some(format!("Bearer {token}").as_str()),
                    "{mode}: {r:?}"
                );
                assert!(r.api_key.is_none(), "{mode}: {r:?}");
            } else {
                assert_eq!(r.api_key.as_deref(), Some(KEY), "{mode}: {r:?}");
                assert!(
                    r.authorization.is_none(),
                    "the placeholder is replaced: {r:?}"
                );
            }
            assert!(!r.body.to_string().contains(KEY));
        }
        let cs = containers();
        assert_eq!(cs.len(), 1, "{cs:?}");
        let c = cs[0].as_str();
        let (_, cfg) = exec_in(c, "cat /workspace/.codex/config.toml 2>&1");
        assert!(
            !cfg.contains("api-version") && !cfg.contains("azure_") && !cfg.contains("auth.json"),
            "{cfg}"
        );
        let mut needles = vec![KEY, app.client_secret.as_str()];
        needles.extend(tokens.iter().map(String::as_str));
        assert_sandbox_holds_none(c, &needles);
        for n in &needles {
            assert!(!t.contains(n), "{mode}: transcript holds {n}");
        }
        st.finish().await;
    }
}

/// (P4-LLM-5, D-261, C-809, SEC-64, SEC-97) A Claude Code agent on `bedrock` with each of `aws_assume_role`,
/// `aws_sigv4` and `api_key`: the turn completes against the verifying Bedrock mock (valid signature from the session
/// key, the static key, or the Bedrock API key as bearer); the sandbox's Claude Code environment (`harness.env`) is
/// byte-identical across the three modes and holds only the "skip auth" switches; none of the source key, secret,
/// session credentials, API key or the signature is in the sandbox or the transcript. Needs the full image.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s26_claude_code_on_bedrock_holds_no_credential_in_the_sandbox_for_every_auth_mode() {
    use aurora_testkit::llm::{bedrock::MockBedrock, sts::MockSts};
    if !enabled() {
        return;
    }
    let Ok(image) = std::env::var("AURORA_E2EH_FULL_IMAGE_TAG") else {
        eprintln!(
            "SKIPPED: AURORA_E2EH_FULL_IMAGE_TAG is not set; run AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh"
        );
        return;
    };
    const SOURCE_KEY: &str = "AKIAE2EHSOURCE000001";
    const SOURCE_SECRET: &str = "e2eh-source-secret-SEEDED-8d41c07a";
    const API_KEY: &str = "bedrock-api-key-SEEDED-61f0a3be";
    const MODEL: &str = "anthropic.claude-3-haiku-20240307-v1:0";
    let mut env_files: Vec<(&str, String)> = Vec::new();
    for mode in ["aws_assume_role", "aws_sigv4", "api_key"] {
        let bedrock = MockBedrock::start_on("127.0.0.2").await;
        let sts = MockSts::start(SOURCE_KEY, SOURCE_SECRET, "123456789012").await;
        {
            let b = bedrock.clone();
            sts.on_session(move |ak, sk, tok| b.accept_key(ak, sk, Some(tok)));
        }
        bedrock.accept_key(SOURCE_KEY, SOURCE_SECRET, None);
        bedrock.accept_api_key(API_KEY);
        let toml = format!(
            r#"
[llm]
enabled_providers = ["bedrock"]
provider_preference = ["bedrock"]

[llm.dev_upstream_overrides]
"https://sts.us-east-1.amazonaws.com" = "{sts}"
"https://bedrock.us-east-1.amazonaws.com" = "{bd}"

[[llm.endpoints]]
id = "bd"
provider = "bedrock"
region = "us-east-1"
base_url = "{bd}"
models = ["{MODEL}"]
default_model = "{MODEL}"
"#,
            sts = sts.base(),
            bd = bedrock.base()
        );
        Stack::wipe();
        let st = Stack::start_with(
            Opts {
                image: Some(image.clone()),
                ..Opts::default()
            },
            &toml,
        )
        .await;
        let api = st.signup("owner@e2e.test").await;
        sqlx::query(
            "UPDATE agents SET harness = 'claude-code', model_provider = 'bedrock', \
             model_endpoint_id = 'bd', model_id = $2 WHERE id = $1::uuid",
        )
        .bind(&api.daneel)
        .bind(MODEL)
        .execute(st.pool())
        .await
        .unwrap();
        let value = match mode {
            "aws_assume_role" => {
                json!({"access_key_id": SOURCE_KEY, "secret_access_key": SOURCE_SECRET,
                "role_arn": "arn:aws:iam::123456789012:role/aurora-e2eh"})
                .to_string()
            }
            "aws_sigv4" => {
                json!({"access_key_id": SOURCE_KEY, "secret_access_key": SOURCE_SECRET}).to_string()
            }
            _ => API_KEY.to_owned(),
        };
        let (s, v) = api
            .post(
                &format!("/v1/workspaces/{}/secrets", api.ws),
                json!({"name": "bedrock-key", "kind": "llm",
                       "llm": {"endpoint_id": "bd", "auth_mode": mode}, "value": value}),
            )
            .await;
        assert_eq!(s, 201, "{mode}: {v}\n{}", st.log_tails());
        api.grant(&api.daneel, v["id"].as_str().unwrap()).await;

        let conv = api.direct().await;
        let v = api.say(&conv, "Reply with exactly the word: pong").await;
        let (status, err) = st
            .wait_turn(&turn_id(&v), &["done", "failed", "aborted"], 180)
            .await;
        if status != "done" {
            let ev = api.events(&conv).await;
            panic!(
                "{mode}: {status} {err:?}\nevents: {ev:?}\n{}",
                st.log_tails()
            );
        }
        let t = transcript_text(&api.events(&conv).await);
        assert!(t.contains("pong"), "{mode}: {t}\n{}", st.log_tails());

        // The runtime saw a verified request of the right credential and no agent-side AWS material.
        let seen: Vec<_> = bedrock
            .requests()
            .into_iter()
            .filter(|r| {
                r.req.path.ends_with("/invoke-with-response-stream")
                    || r.req.path.ends_with("/invoke")
            })
            .collect();
        assert!(
            !seen.is_empty(),
            "{mode}: no inference request reached Bedrock"
        );
        let mut needles: Vec<String> =
            vec![SOURCE_KEY.into(), SOURCE_SECRET.into(), API_KEY.into()];
        for r in &seen {
            match mode {
                "api_key" => assert!(r.bearer_ok, "{mode}: {r:?}"),
                "aws_sigv4" => assert_eq!(r.verified.as_ref().unwrap().access_key, SOURCE_KEY),
                _ => assert!(
                    r.verified
                        .as_ref()
                        .unwrap()
                        .access_key
                        .starts_with("ASIAMOCKSESSION"),
                    "{mode}: {r:?}"
                ),
            }
            if let Some((_, a)) = r.req.headers.iter().find(|(k, _)| k == "authorization")
                && let Some(sig) = a.split("Signature=").nth(1)
            {
                needles.push(sig.to_owned());
            }
        }
        for (ak, sk, tok) in sts.sessions() {
            needles.extend([ak, sk, tok]);
        }
        let cs = containers();
        assert_eq!(cs.len(), 1, "{cs:?}");
        let c = cs[0].as_str();
        let refs: Vec<&str> = needles.iter().map(String::as_str).collect();
        assert_sandbox_holds_none(c, &refs);
        for n in &refs {
            assert!(!t.contains(n), "{mode}: transcript holds {n}");
        }
        let (_, env_file) = exec_in(c, "cat /run/aurora/harness.env 2>&1");
        assert!(
            env_file.contains("CLAUDE_CODE_USE_BEDROCK")
                && env_file.contains("CLAUDE_CODE_SKIP_BEDROCK_AUTH"),
            "{mode}: {env_file}"
        );
        assert!(
            !env_file.contains("AWS_SECRET")
                && !env_file.contains("AWS_ACCESS")
                && !env_file.contains("AWS_BEARER"),
            "{mode}: {env_file}"
        );
        env_files.push((mode, env_file));
        st.finish().await;
    }
    // The environment is the same whatever the credential (the mode never reaches the sandbox).
    let first = &env_files[0].1;
    for (mode, f) in &env_files[1..] {
        // The base URL carries the mock's port, which differs per run: compare with it masked.
        let mask = |s: &str| {
            s.lines()
                .map(|l| {
                    if l.contains("127.0.0.2:") {
                        "<base>"
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            mask(first),
            mask(f),
            "{mode} differs from {}",
            env_files[0].0
        );
    }
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
        sqlx::query(sqlx::AssertSqlSafe(&*q))
            .execute(&tdb)
            .await
            .unwrap();
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
    let psql = |name: &'static str, sql: &'static str| {
        let cmd = format!(
            "psql service={service} -At -c \"{sql}\" > /workspace/{name}.out 2>&1; echo rc=$? >> /workspace/{name}.out"
        );
        let (st, api, conv) = (&st, &api, c.as_str());
        async move {
            let r = tools_sock_as_harness(
                st,
                api,
                cn,
                conv,
                &json!({"id": 1, "session": conv, "method": "call", "tool": "process_start",
                        "arguments": {"name": name, "command": cmd}}),
            )
            .await;
            assert!(r.contains("\"result\""), "process_start: {r}");
        }
    };
    psql("read", "select 40 + 2").await;
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
    )
    .await;
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
    psql("ddl", "drop table e2e_t").await;
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

// ---------------------------------------------------------------------------------------------
// FIX-138: the web terminal's shells and harness wrappers.
//
// A terminal is an interactive login shell on a PTY with the environment of every supervisor
// child (the container's minus `AURORA_*`). The probes below run a real PTY driver through the
// supervisor's `process_start` (the same environment, the same PATH), so no part of the shell
// start-up is imitated.

/// Python PTY driver: starts `<shell> -l` on a pseudo-terminal like the supervisor's PTY engine
/// (`TERM=xterm-256color`), sends each step's bytes, and prints what the terminal showed after
/// each one (ESC rendered as `<ESC>`), reading until the output is idle.
const TERM_DRIVER: &str = r#"
import json, os, pty, select, sys, time
shell, steps_path = sys.argv[1], sys.argv[2]
steps = json.load(open(steps_path))
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.environ["SHELL"] = shell
    os.execv(shell, [shell, "-l"])
def drain(idle=0.8, cap=10.0):
    out = b""
    end = time.time() + cap
    last = time.time()
    while time.time() < end and time.time() - last < idle:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                d = os.read(fd, 65536)
            except OSError:
                break
            if not d:
                break
            out += d
            last = time.time()
    return out.decode("utf8", "replace").replace("\x1b", "<ESC>")
print("=== step 0")
print(drain(1.5, 20.0))
for i, s in enumerate(steps):
    os.write(fd, s.encode())
    print("=== step %d" % (i + 1))
    print(drain())
os.write(fd, b"exit\r")
print("=== end")
"#;

/// Runs `steps` in an interactive login `shell` through the supervisor (as the agent) and returns
/// the transcript, one string per step (index 0 = the start-up output, index n = step n).
async fn terminal_steps(
    st: &Stack,
    api: &Api,
    c: &str,
    conv: &str,
    name: &str,
    shell: &str,
    steps: &[&str],
) -> Vec<String> {
    use base64::Engine as _;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let steps_json = serde_json::to_string(steps).unwrap();
    let (rc, w) = exec_in(
        c,
        &format!(
            "echo {} | base64 -d > /workspace/term-driver.py && echo {} | base64 -d > /workspace/{name}.steps.json",
            b64(TERM_DRIVER),
            b64(&steps_json)
        ),
    );
    assert_eq!(rc, 0, "could not write the PTY driver: {w}");
    let out = process_output(
        st,
        api,
        c,
        conv,
        name,
        &format!("python3 /workspace/term-driver.py {shell} /workspace/{name}.steps.json"),
        240,
    )
    .await;
    // "=== step 0" opens the transcript: the pieces after splitting are [prefix, step 0, step 1, …].
    out.split("=== step ").skip(1).map(str::to_owned).collect()
}

const IMAGE_PATH: &str = "/workspace/.local/bin:/opt/aurora/bin:/usr/local/bin:/usr/bin:/bin";

/// FIX-138 (e2e, hosted, both shells): Tab completes, history (Up) works, `ll`/`ls` are aliased,
/// `ls` emits colour without `--color`, `PATH` is the image's one, `claude`/`codex` resolve to the
/// wrappers (which refuse where the harness binary is not in the image), a non-interactive shell
/// never sees the aliases, and no `AURORA_*` variable reaches the terminal. Runs on whatever image
/// the stack uses (the min image has no harness binaries: the wrappers' refusal is exercised).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s13_terminal_shells_complete_alias_colour_and_resolve_the_wrappers() {
    if !enabled() {
        return;
    }
    let (st, api) = fx(Opts::default()).await;
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "[[say: warm]]").await;
    let cs = containers();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let c = cs[0].as_str();
    let has_claude = exec_in(
        c,
        "test -x /opt/aurora/harness/claude-code/node_modules/.bin/claude",
    )
    .0 == 0;
    let steps: [&str; 12] = [
        "ech\t",                                           // 1: Tab completes `echo`
        "\u{15}alias ll\r",                                // 2: Ctrl-U clears the line first
        "alias ls\r",                                      // 3
        "mkdir -p ~/cdir && ls ~\r",                       // 4: colour without --color
        "echo PATH=$PATH\r",                               // 5
        "which claude codex\r",                            // 6
        "claude --version; echo rc=$?\r",                  // 7
        "bash -c 'type ll'; echo rc=$?\r",                 // 8: a turn's shell never sees the alias
        "echo history-probe\r",                            // 9
        "\u{1b}[A",                                        // 10: Up recalls it
        "echo bootvars=$(env | grep -c '^AURORA_BOOT')\r", // 11
        "\r",                                              // 12
    ];
    for (shell, tag) in [("/bin/bash", "bash"), ("/usr/bin/zsh", "zsh")] {
        let parts =
            terminal_steps(&st, &api, c, &conv, &format!("term-{tag}"), shell, &steps).await;
        assert!(
            parts.len() >= 13,
            "{tag}: the driver did not finish: {parts:?}"
        );
        let step = |n: usize| parts[n].as_str();
        assert!(
            step(1).contains("echo"),
            "{tag}: Tab did not complete: {}",
            step(1)
        );
        assert!(
            step(2).contains("ls -l"),
            "{tag}: no `ll` alias: {}",
            step(2)
        );
        assert!(
            step(3).contains("--color=auto"),
            "{tag}: no `ls` alias: {}",
            step(3)
        );
        assert!(
            step(4).contains("<ESC>[01;34m") && step(4).contains("cdir"),
            "{tag}: ls emitted no colour for a directory: {}",
            step(4)
        );
        assert!(
            step(5).contains(&format!("PATH={IMAGE_PATH}")),
            "{tag}: PATH: {}",
            step(5)
        );
        assert!(
            step(6).contains("/opt/aurora/bin/claude") && step(6).contains("/opt/aurora/bin/codex"),
            "{tag}: the wrappers are not first on PATH: {}",
            step(6)
        );
        if !has_claude {
            assert!(
                step(7).contains("not enabled on this instance") && step(7).contains("rc=127"),
                "{tag}: a harness missing from the image must refuse: {}",
                step(7)
            );
        }
        assert!(
            step(8).contains("not found") && step(8).contains("rc=1"),
            "{tag}: the alias leaked into a non-interactive shell: {}",
            step(8)
        );
        assert!(
            step(10).contains("echo history-probe"),
            "{tag}: Up did not recall: {}",
            step(10)
        );
        assert!(
            step(11).contains("bootvars=0"),
            "{tag}: the boot document in the terminal: {}",
            step(11)
        );
    }
    st.finish().await;
}

/// FIX-138 item 1b (e2e, hosted, **full image**): with only Codex enabled on the instance, the
/// terminal's `codex` wrapper reaches the mock Responses upstream **through the gateway** (the
/// placeholder is replaced there: the upstream sees the seeded key, the terminal never does), the
/// attribution is the agent's with no conversation, its state is separate from the agent
/// harness's, `rg` is installed, and `claude` — in the image but not enabled — refuses. Needs
/// `AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh` (otherwise SKIPPED).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s13b_terminal_codex_wrapper_reaches_the_gateway_and_a_disabled_harness_refuses() {
    if !enabled() {
        return;
    }
    let Ok(image) = std::env::var("AURORA_E2EH_FULL_IMAGE_TAG") else {
        eprintln!(
            "SKIPPED: AURORA_E2EH_FULL_IMAGE_TAG is not set; run AURORA_E2EH_FULL_IMAGE=1 scripts/e2e/hosted.sh"
        );
        return;
    };
    const SEED: &str = "sk-e2e-terminal-SEEDED-3e8b51f2";
    let up = aurora_testkit::responses::MockResponses::start_on("127.0.0.2").await;
    let toml = format!(
        r#"
[llm]
enabled_providers = ["litellm"]
provider_preference = ["litellm"]

[[llm.endpoints]]
id = "codex-mock"
provider = "litellm"
display_name = "Mock Responses upstream"
base_url = "{}"
dialects = ["openai_responses"]
default_model = "say"
"#,
        up.base()
    );
    Stack::wipe();
    let st = Stack::start_with(
        Opts {
            image: Some(image),
            only_harness: Some("codex".into()),
            ..Opts::default()
        },
        &toml,
    )
    .await;
    let api = st.signup("owner@e2e.test").await;
    sqlx::query(
        "UPDATE agents SET harness = 'codex', model_provider = 'litellm', \
         model_endpoint_id = 'codex-mock', model_id = 'say' WHERE id = $1::uuid",
    )
    .bind(&api.daneel)
    .execute(st.pool())
    .await
    .unwrap();
    let (s, v) = api
        .post(
            &format!("/v1/workspaces/{}/secrets", api.ws),
            json!({"name": "codex-mock-key", "kind": "llm",
                   "llm": {"endpoint_id": "codex-mock", "auth_mode": "api_key"},
                   "value": SEED}),
        )
        .await;
    assert_eq!(s, 201, "llm key: {v}\n{}", st.log_tails());
    api.grant(&api.daneel, v["id"].as_str().unwrap()).await;

    // One agent turn: it boots the sandbox and gives the supervisor the agent's model, which it
    // writes to /run/aurora/harness.env for the terminal's wrappers.
    let conv = api.direct().await;
    run_turn(&st, &api, &conv, "Reply with exactly the word: pong").await;
    let cs = containers();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let c = cs[0].as_str();
    let (_, env_file) = exec_in(
        c,
        "stat -c '%a' /run/aurora/harness.env; cat /run/aurora/harness.env",
    );
    assert!(env_file.starts_with("644"), "harness.env mode: {env_file}");
    assert!(env_file.contains("HARNESSES_ENABLED='codex'"), "{env_file}");
    assert!(env_file.contains("AURORA_CODEX_MODEL='say'"), "{env_file}");
    assert!(
        env_file.contains("OPENAI_API_KEY='aurora-placeholder'"),
        "{env_file}"
    );
    assert!(
        !env_file.contains(SEED),
        "a real key in harness.env: {env_file}"
    );
    let seen_before = up.requests().len();
    let (_, agent_cfg_before) = exec_in(c, "sha256sum /workspace/.codex/config.toml 2>&1");

    // The terminal, as a person types it: `rg`, the wrappers, a refused harness, the environment.
    let out = process_output(
        &st,
        &api,
        c,
        &conv,
        "termcli",
        "rg --version | head -1; which codex; claude --version; echo claude-rc=$?; echo '--- env'; env",
        90,
    )
    .await;
    assert!(out.contains("ripgrep"), "rg: {out}");
    assert!(out.contains("/opt/aurora/bin/codex"), "which codex: {out}");
    assert!(
        out.contains("claude: not enabled on this instance") && out.contains("claude-rc=127"),
        "an image harness that is not enabled must refuse: {out}"
    );
    let env_part = out.split("--- env").nth(1).unwrap_or_default();
    assert!(!env_part.contains(SEED), "the key is in the terminal env");
    assert!(
        !env_part.lines().any(|l| l.starts_with("AURORA_BOOT")),
        "the boot document in the terminal env: {env_part}"
    );

    let run = process_output(
        &st,
        &api,
        c,
        &conv,
        "termcodex",
        "codex exec --skip-git-repo-check 'Reply with exactly the word: pong'; echo codex-rc=$?",
        150,
    )
    .await;
    assert!(
        run.contains("pong") && run.contains("codex-rc=0"),
        "the wrapper did not reach the model: {run}\n{}",
        st.log_tails()
    );
    assert!(!run.contains(SEED), "the key reached the terminal: {run}");

    // The upstream got the request through the gateway with the key injected (the terminal held
    // the placeholder only); the gateway attributes it to the agent, with no conversation.
    let seen = up.requests();
    assert!(
        seen.len() > seen_before,
        "no terminal request reached the upstream"
    );
    for r in &seen {
        assert_eq!(
            r.authorization.as_deref(),
            Some(format!("Bearer {SEED}").as_str()),
            "the gateway did not replace the placeholder: {r:?}"
        );
    }
    let end = std::time::Instant::now() + Duration::from_secs(60);
    let rows = loop {
        let rows: Vec<(Option<String>, Option<String>, i32)> = sqlx::query_as(
            "SELECT conversation_id::text, secret_id::text, status FROM request_log \
             WHERE host LIKE '127.0.0.2%' AND path LIKE '/v1/responses%' AND conversation_id IS NULL",
        )
        .fetch_all(st.pool())
        .await
        .unwrap();
        if !rows.is_empty() || std::time::Instant::now() > end {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    assert!(
        rows.iter()
            .any(|(_, s, status)| s.is_some() && *status == 200),
        "no agent-attributed, conversation-less 200 row for the terminal's model call: {rows:?}"
    );

    // Separate state: the agent harness's own Codex home is byte-identical, the terminal's is its own.
    let (_, agent_cfg_after) = exec_in(c, "sha256sum /workspace/.codex/config.toml 2>&1");
    assert_eq!(
        agent_cfg_before, agent_cfg_after,
        "the terminal changed the agent harness's config"
    );
    let (_, term_home) = exec_in(c, "ls -A /workspace/.aurora/terminal/codex | head -5");
    assert!(
        !term_home.trim().is_empty(),
        "the wrapper's own CODEX_HOME is empty: {term_home:?}"
    );
    st.finish().await;
}

/// (P5-GUIDE-2, C-643, D-295, SEC-117) A hosted agent granted `fixture-guide` under a custom grant
/// has `.aurora/apps/guide-demo/` projected into its real sandbox (generated files and the
/// package's own, read-only), the instruction file lists the app as one index line with none of the
/// package's notes, and revoking the grant removes the folder at the next sync.
#[tokio::test]
async fn s14_agent_guide_folder_is_projected_into_the_sandbox_and_removed_on_revoke() {
    if !enabled() {
        return;
    }
    let (st, owner) = fx(Opts::default()).await;
    let email = "owner@e2e.test";
    sqlx::query(
        "UPDATE users SET is_instance_admin = true, email_verified_at = now() WHERE email = $1",
    )
    .bind(email)
    .execute(st.pool())
    .await
    .unwrap();
    let admin = owner.relogin(email).await;

    // The checked-in fixture package, signed with a key the instance is told to trust.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/fixtures/fixture-guide");
    let read = |p: &str| std::fs::read(dir.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let mut b = aurora_registry::builder::PackageBuilder::new(
        &String::from_utf8(read("plugin.yaml")).unwrap(),
    );
    for f in ["book-free-room.md", "api.md", "codes.tsv"] {
        b.set_file(
            &format!("agent/guide-demo/{f}"),
            &read(&format!("agent/guide-demo/{f}")),
        );
    }
    let key = aurora_crypto::generate_signing_key().unwrap();
    let key_id = aurora_registry::builder::PackageBuilder::key_id(&key);
    b.sign(&key, &key_id);
    let (s, v) = admin
        .post(
            "/v1/admin/trusted-keys",
            json!({"publisher": "fixtures", "name": "e2e-guide", "public_key": key_id}),
        )
        .await;
    assert_eq!(s, 201, "trusted key: {v}");
    let (s, pkg) = admin.upload_plugin(&b.build().unwrap()).await;
    assert_eq!(s, 201, "upload: {pkg}");
    assert_eq!(pkg["status"], "pending_approval", "{pkg}");
    let (s, a) = admin
        .post(
            &format!("/v1/admin/plugins/{}/approve", pkg["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "approve: {a}");

    // A connection of the app and a custom grant to Daneel.
    let (s, c) = admin
        .post(
            "/v1/apps/guide-demo/connect",
            json!({"scope": "workspace", "workspace_id": admin.ws, "method_id": "api_key",
                   "fields": {"api_token": "tok-guide-e2e"}}),
        )
        .await;
    assert_eq!(s, 200, "connect: {c}");
    let conn = c["connection"]["id"].as_str().unwrap().to_owned();
    let grant = format!("/v1/agents/{}/grants/connections/{conn}", admin.daneel);
    let (s, g) = admin
        .put(
            &grant,
            json!({"mode": "custom", "allow_tools": ["list_rooms"], "ask_tools": ["book_room"]}),
        )
        .await;
    assert_eq!(s, 200, "grant: {g}");

    // The sandbox boots with the folder (a turn makes sure the sync was applied).
    let conv = admin.direct().await;
    run_turn(&st, &admin, &conv, "[[say: warm]]").await;
    let cs = containers();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let c = cs[0].as_str();
    let root = "/workspace/.aurora/apps/guide-demo";
    let ls = || {
        exec_in(
            c,
            // Exit status 0 only while the folder exists (`sort` would hide stat's status).
            &format!("test -d {root} && stat -c '%a %n' {root} {root}/* | sort -k2"),
        )
    };
    let present = eventually(60, || ls().0 == 0).await;
    let (_, listing) = ls();
    assert!(present, "no folder: {listing}\n{}", st.log_tails());
    // Directories 0555, files 0444 (C-643), the generated files and the package's own.
    for want in [
        "555 /workspace/.aurora/apps/guide-demo",
        "444 /workspace/.aurora/apps/guide-demo/guide.md",
        "444 /workspace/.aurora/apps/guide-demo/tools.md",
        "444 /workspace/.aurora/apps/guide-demo/book-free-room.md",
        "444 /workspace/.aurora/apps/guide-demo/api.md",
        "444 /workspace/.aurora/apps/guide-demo/codes.tsv",
    ] {
        assert!(
            listing.lines().any(|l| l == want),
            "missing `{want}` in:\n{listing}"
        );
    }
    // The custom grant is in the generated files: the allowed tool and the ask tool, by id.
    let (_, tools) = exec_in(c, &format!("cat {root}/tools.md"));
    let access = |id: &str, mark: &str| {
        tools
            .lines()
            .any(|l| l.contains(&format!("`{id}`")) && l.ends_with(mark))
    };
    assert!(access("list_rooms", "[allowed]"), "{tools}");
    assert!(access("book_room", "[approval]"), "{tools}");
    assert!(access("get_room", "[blocked]"), "{tools}");
    // The recipe carries the third-party procedure header; the credential is nowhere.
    let (_, recipe) = exec_in(c, &format!("head -3 {root}/book-free-room.md"));
    assert!(recipe.to_lowercase().contains("procedure"), "{recipe}");
    let (_, leak) = exec_in(
        c,
        "grep -rIl 'tok-guide-e2e' /workspace/.aurora 2>/dev/null | wc -l",
    );
    assert_eq!(leak.trim(), "0", "the credential reached the home");
    // The instruction file: one index line for the app, none of the package's notes.
    let (_, instr) = exec_in(c, "cat /workspace/AGENTS.md");
    assert!(
        instr.contains("guide-demo") && instr.contains(".aurora/apps/"),
        "{instr}"
    );
    assert!(
        !instr.contains("Rooms are addressed by numeric id"),
        "notes inlined: {instr}"
    );
    // Nothing writable: the agent cannot edit a guide file.
    let (rc, _) = exec_in(c, &format!("echo x >> {root}/guide.md"));
    assert_ne!(rc, 0, "a guide file is writable");

    // Revoke: the folder goes at the next sync.
    let (s, _) = admin.delete(&grant).await;
    assert_eq!(s, 204);
    run_turn(&st, &admin, &conv, "[[say: again]]").await;
    let gone = eventually(60, || ls().0 != 0).await;
    let (_, after) = ls();
    assert!(
        gone,
        "the folder survived the revoke: {after}\n{}",
        st.log_tails()
    );
    let (_, instr) = exec_in(c, "cat /workspace/AGENTS.md");
    assert!(
        !instr.contains("guide-demo"),
        "the app is still in the instruction file: {instr}"
    );
    st.finish().await;
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-4 (C-1058..C-1060, D-408, D-412, D-416, SEC-141): the computer in words for everyone
//! who can use the agent, the engine for instance admins only, the start phase that never moves
//! backwards, Sleep now, an open session's keep-awake and the new `boot_id` after a restart.

use crate::common::*;
use crate::hosted::{Live, Rig, rig};
use base64::Engine;
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// A hosted agent whose start was claimed (a message is queued): the start item's sandbox and boot,
/// and the bootstrap token of its boot document.
async fn claimed(r: &Rig, handle: &str) -> (Live, String) {
    let agent = r.agent(handle, true).await;
    let conv = r.direct(agent).await;
    r.say(conv, "hello").await;
    let items = r.work().await;
    let start = items.iter().find(|i| i["kind"] == "sandbox.start").unwrap();
    let raw = start["spec"]["boot_secret"].as_str().unwrap();
    let doc: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .unwrap(),
    )
    .unwrap();
    (
        Live {
            agent,
            conv,
            sb: start["sandbox_id"].as_str().unwrap().to_owned(),
            boot: start["boot_id"].as_str().unwrap().to_owned(),
        },
        doc["bootstrap_token"].as_str().unwrap().to_owned(),
    )
}

/// The supervisor connects (claim + hold): `start_phase` becomes `booting`.
async fn connect(r: &Rig, lv: &Live, token: &str) {
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/channel/claim",
            json!({ "bootstrap_token": token }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/channel/hold",
            json!({"sandbox_id": lv.sb, "boot_id": lv.boot}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

async fn harness_ready(r: &Rig, lv: &Live) {
    assert_eq!(
        r.report(
            lv,
            json!({"kind": "supervisor.ready", "version": "0.1",
                "harness": {"id": "fake", "capabilities": ["steer", "resume"]}}),
        )
        .await,
        "applied"
    );
}

async fn running(r: &Rig, lv: &Live) {
    assert_eq!(
        r.report(
            lv,
            json!({"kind": "sandbox.status", "state": "running", "container_ref": "c"})
        )
        .await,
        "applied"
    );
}

async fn phase(r: &Rig, lv: &Live) -> Option<String> {
    sqlx::query_scalar("SELECT start_phase FROM sandboxes WHERE id = $1::uuid")
        .bind(&lv.sb)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

/// Session headers of a new user bound to the rig's workspace with `ws_role`.
async fn caller(r: &Rig, email: &str, instance_admin: bool, ws_role: &str) -> String {
    let u = r.h.user(email, instance_admin).await;
    r.h.member(r.org, u, "member", "active").await;
    r.h.bind_ws(r.ws, u, ws_role).await;
    let (_, sess) = r.h.good_session(u).await;
    cookie(&sess)
}

async fn sandbox(r: &Rig, c: &str, agent: Uuid) -> Value {
    let (s, v) =
        r.h.get(&format!("/v1/agents/{agent}/sandbox"), &[("cookie", c)])
            .await;
    assert_eq!(s, 200, "{v}");
    v
}

async fn finish_turns(r: &Rig, agent: Uuid) {
    sqlx::query("UPDATE turns SET status = 'done' WHERE agent_id = $1")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
}

async fn quiet_for(r: &Rig, lv: &Live, secs: i32) {
    sqlx::query(
        "UPDATE sandboxes SET last_engaged_at = now() - make_interval(secs => $2),
             last_active_at = now() - make_interval(secs => $2) WHERE id = $1::uuid",
    )
    .bind(&lv.sb)
    .bind(secs)
    .execute(r.h.pool())
    .await
    .unwrap();
}

/// (C-1059) A supervisor that connects — and a harness that reports ready — before the runner's
/// `running` report leave the start phase where it is: it never moves back to `waiting_supervisor`.
#[tokio::test]
async fn the_running_report_never_moves_the_phase_backwards() {
    let r = rig().await;
    // The ordinary order: `running` first, then the supervisor.
    let (lv, token) = claimed(&r, "cv-order").await;
    running(&r, &lv).await;
    assert_eq!(phase(&r, &lv).await.as_deref(), Some("waiting_supervisor"));
    connect(&r, &lv, &token).await;
    assert_eq!(phase(&r, &lv).await.as_deref(), Some("booting"));
    harness_ready(&r, &lv).await;
    assert_eq!(phase(&r, &lv).await.as_deref(), Some("ready"));

    // The supervisor first: `booting` stays.
    let (lv, token) = claimed(&r, "cv-early").await;
    connect(&r, &lv, &token).await;
    running(&r, &lv).await;
    assert_eq!(phase(&r, &lv).await.as_deref(), Some("booting"));

    // Supervisor and harness first: `ready` stays, and the computer reads `on`.
    let (lv, token) = claimed(&r, "cv-ready").await;
    connect(&r, &lv, &token).await;
    harness_ready(&r, &lv).await;
    running(&r, &lv).await;
    assert_eq!(phase(&r, &lv).await.as_deref(), Some("ready"));
    let v = sandbox(&r, &r.cookie, lv.agent).await;
    assert_eq!(v["computer"]["word"], "on", "{v}");
    assert_eq!(v["computer"]["ready"], true);
    assert_eq!(v["computer"]["phase"], "ready");
}

/// SEC-141 (D-412, amends D-178 Q-R5-5): a member, a workspace owner and an instance admin all get
/// the `computer` object, the phases, the times and `boot_id`; the image, the runner, the start
/// attempt's detail and events and the start log's details go to the instance admin only.
#[tokio::test]
async fn the_engine_is_for_instance_admins_only() {
    let r = rig().await;
    let (lv, token) = claimed(&r, "cv-roles").await;
    let secret = "10.1.2.3:4105 runner-a.svc";
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "sandbox.status", "state": "starting", "reason_code": "creating",
                "message": format!("pulling at {secret}"),
                "events": [format!("Pulled registry.example/aurora-sandbox:9.9.9 on {secret}")]}),
        )
        .await,
        "applied"
    );
    connect(&r, &lv, &token).await;
    harness_ready(&r, &lv).await;
    running(&r, &lv).await;

    let member = r.cookie.clone();
    let owner = caller(&r, "cv-owner@x.io", false, "owner").await;
    let admin = caller(&r, "cv-admin@x.io", true, "member").await;
    for (who, c) in [("member", &member), ("owner", &owner), ("admin", &admin)] {
        let v = sandbox(&r, c, lv.agent).await;
        let comp = &v["computer"];
        assert_eq!(
            (comp["word"].as_str(), comp["ready"].as_bool()),
            (Some("on"), Some(true)),
            "{who}: {v}"
        );
        // A message is queued: the agent is working, no idle countdown.
        assert_eq!(comp["working"], true, "{who}");
        assert!(comp["sleeps_in_secs"].is_null(), "{who}");
        assert_eq!(comp["asleep"], false, "{who}");
        assert!(comp["started_at"].is_string(), "{who}: {v}");
        assert_eq!(comp["started_at"], v["started_at"], "{who}");
        // The computer object never names where it runs.
        let c_text = comp.to_string();
        assert!(
            !c_text.contains("10.1.2.3") && !c_text.contains("registry") && !c_text.contains("svc"),
            "{who}: {c_text}"
        );
        assert_eq!(
            v["boot_id"],
            lv.boot.as_str(),
            "{who}: boot_id for everyone"
        );
        assert!(v["start_attempt"]["phase"].is_string(), "{who}: {v}");
        assert!(!v["start_log"].as_array().unwrap().is_empty(), "{who}");
        let engine = who == "admin";
        assert_eq!(v["image"].is_string(), engine, "{who}: image {v}");
        assert_eq!(v["runner_id"].is_string(), engine, "{who}: runner {v}");
        assert_eq!(
            v["start_attempt"]["detail"].is_string(),
            engine,
            "{who}: detail {v}"
        );
        assert_eq!(
            !v["start_attempt"]["events"].as_array().unwrap().is_empty(),
            engine,
            "{who}: events {v}"
        );
        assert_eq!(
            v["start_log"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["detail"].is_string()),
            engine,
            "{who}: log details {v}"
        );
        if !engine {
            let text = v.to_string();
            assert!(
                !text.contains("10.1.2.3") && !text.contains("aurora-sandbox:9.9.9"),
                "{who} sees the engine: {text}"
            );
        }
    }
}

/// (C-1059) `sleeps_in_secs` counts down while the computer is on and idle; `asleep` once stopped.
#[tokio::test]
async fn sleeps_in_counts_down_while_idle() {
    let r = rig_idle(900).await;
    let lv = r.boot("cv-idle").await;
    finish_turns(&r, lv.agent).await;
    quiet_for(&r, &lv, 100).await;
    let v = sandbox(&r, &r.cookie, lv.agent).await;
    let left = v["computer"]["sleeps_in_secs"].as_u64().unwrap();
    assert!((790..=800).contains(&left), "{v}");
    assert_eq!(v["computer"]["working"], false);
    // A keep-awake reason stops the countdown.
    sqlx::query(
        "UPDATE sandboxes SET terminal_active_until = now() + interval '2 minutes' WHERE id = $1::uuid",
    )
    .bind(&lv.sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    let v = sandbox(&r, &r.cookie, lv.agent).await;
    assert!(v["computer"]["sleeps_in_secs"].is_null(), "{v}");
    assert!(
        v["keep_awake_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "terminal")
    );
}

async fn rig_idle(secs: u64) -> Rig {
    crate::hosted::rig_with(move |c| c.hosted.idle_stop_secs = secs).await
}

async fn stop_item(r: &Rig) -> Option<Value> {
    r.work()
        .await
        .into_iter()
        .find(|i| i["kind"] == "sandbox.stop")
}

async fn sleep(r: &Rig, c: &str, agent: Uuid) -> (u16, Value) {
    r.h.call(
        c,
        Method::POST,
        &format!("/v1/agents/{agent}/sandbox/sleep"),
        None,
    )
    .await
}

async fn sleep_audits(r: &Rig, agent: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'agent.sandbox_sleep' AND target_id = $1",
    )
    .bind(agent.to_string())
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

/// (C-1060) Sleep now: refused while work runs (409 `sandbox_busy` with the reasons), 202 on an
/// idle computer (the stop arm then stops it with reason `idle`), 200 and nothing written when
/// repeated or already asleep, one audit row, 404 for a caller without access, 409 for a BYO agent
/// and a failed computer.
#[tokio::test]
async fn sleep_now_outcomes() {
    let r = rig().await;
    let lv = r.boot("cv-sleep").await;
    let me = r.cookie.clone();

    // A running turn.
    let (s, v) = sleep(&r, &me, lv.agent).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("sandbox_busy")),
        "{v}"
    );
    assert_eq!(v["error"]["details"]["reasons"], json!(["turn"]), "{v}");
    finish_turns(&r, lv.agent).await;

    // A busy process, then an armed watch.
    let p = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO processes (id, agent_id, sandbox_id, boot_id, name, command, status, busy, started_at)
         VALUES ($1,$2,$3::uuid,$4::uuid,'dev','sleep 1000','running',true,now())",
    )
    .bind(p)
    .bind(lv.agent)
    .bind(&lv.sb)
    .bind(&lv.boot)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = sleep(&r, &me, lv.agent).await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["details"]["reasons"], json!(["process"]), "{v}");
    sqlx::query("UPDATE processes SET busy = false WHERE id = $1")
        .bind(p)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO watches (id, process_id, agent_id, conversation_id, kind, status, expires_at)
         VALUES ($1,$2,$3,$4,'exit','armed', now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(p)
    .bind(lv.agent)
    .bind(lv.conv)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = sleep(&r, &me, lv.agent).await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["details"]["reasons"], json!(["watch"]), "{v}");
    sqlx::query("DELETE FROM watches WHERE process_id = $1")
        .bind(p)
        .execute(r.h.pool())
        .await
        .unwrap();
    assert_eq!(sleep_audits(&r, lv.agent).await, 0);

    // A caller with no access to the agent: 404.
    let stranger = r.h.user("cv-stranger@x.io", false).await;
    let (_, sess) = r.h.good_session(stranger).await;
    let (s, _) = sleep(&r, &cookie(&sess), lv.agent).await;
    assert_eq!(s, 404);

    // An open terminal does not block it.
    sqlx::query(
        "UPDATE sandboxes SET terminal_active_until = now() + interval '2 minutes' WHERE id = $1::uuid",
    )
    .bind(&lv.sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = sleep(&r, &me, lv.agent).await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["state"], "running");
    // Repeated before the runner acted: 200, nothing written.
    let (s, _) = sleep(&r, &me, lv.agent).await;
    assert_eq!(s, 200);
    let stop = stop_item(&r).await.expect("the stop arm stops it");
    assert_eq!(stop["reason"], "idle", "{stop}");
    assert_eq!(
        r.report(&lv, json!({"kind": "sandbox.status", "state": "stopped"}))
            .await,
        "applied"
    );
    let (s, v) = sleep(&r, &me, lv.agent).await;
    assert_eq!((s, v["state"].as_str()), (200, Some("stopped")), "{v}");
    assert_eq!(v["computer"]["asleep"], true, "{v}");
    assert_eq!(v["computer"]["word"], "off");
    // One audit row (best effort: poll).
    let mut n = 0;
    for _ in 0..100 {
        n = sleep_audits(&r, lv.agent).await;
        if n > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!((n, sleep_audits(&r, lv.agent).await), (1, 1));

    // A failed computer: retry is the way out.
    sqlx::query(
        "UPDATE sandboxes SET state = 'failed', failure_code = 'pod_failed' WHERE id = $1::uuid",
    )
    .bind(&lv.sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = sleep(&r, &me, lv.agent).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("sandbox_failed")),
        "{v}"
    );

    // A BYO agent has no computer.
    let byo = r.h.byo_agent(r.org, r.ws, "cv-byo", "cv-byo").await;
    let (s, v) = sleep(&r, &me, byo).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_hosted")),
        "{v}"
    );
}

/// (C-1058, D-416) The runner's keep-awake stamps hold an idle computer awake for as long as they
/// keep coming (an open terminal's heartbeat), and `keep_awake_reasons` says `terminal`; when they
/// stop (the session closed) the grace lapses and the idle stop takes it. The grace is shortened by
/// expiring each stamp in the database.
#[tokio::test]
async fn heartbeat_stamps_keep_an_idle_computer_awake() {
    let r = rig_idle(60).await;
    let lv = r.boot("cv-awake").await;
    finish_turns(&r, lv.agent).await;
    quiet_for(&r, &lv, 3600).await;
    let stamp = || async {
        let (s, v) = r
            .runner(
                Some(r.rep),
                "/v1/runner/terminal/keep-awake",
                json!({ "sandbox_id": lv.sb }),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    };
    let expire = || async {
        sqlx::query(
            "UPDATE sandboxes SET terminal_active_until = now() - interval '1 second' WHERE id = $1::uuid",
        )
        .bind(&lv.sb)
        .execute(r.h.pool())
        .await
        .unwrap();
    };
    for beat in 0..3 {
        stamp().await;
        assert!(stop_item(&r).await.is_none(), "beat {beat}: kept awake");
        let v = sandbox(&r, &r.cookie, lv.agent).await;
        assert!(
            v["keep_awake_reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x == "terminal"),
            "{v}"
        );
        assert_eq!(v["terminal_in_use"], true);
        expire().await;
    }
    // No more stamps: the idle stop takes it.
    let stop = stop_item(&r)
        .await
        .expect("idle stop after the session closed");
    assert_eq!(stop["reason"], "idle", "{stop}");
}

/// (P5-REFINE-20 batch 5, D-416) A computer woken for a session (the wake demand a terminal or an
/// SSH channel writes on a parked computer, no turn) gets the idle stop's whole window from the
/// moment it is running, so the session can connect and stamp. Before the fix the idle clock read
/// the last engagement first (an hour old here), and the first stop arm after `running` parked it
/// again: SSH to a sleeping computer failed with "did not start in time" and the terminal looped.
#[tokio::test]
async fn a_computer_woken_for_a_session_waits_for_it() {
    let r = rig_idle(60).await;
    let lv = r.boot("cv-woken").await;
    finish_turns(&r, lv.agent).await;
    quiet_for(&r, &lv, 3600).await;
    let stop = stop_item(&r).await.expect("idle stop");
    assert_eq!(stop["reason"], "idle", "{stop}");
    assert_eq!(
        r.report(&lv, json!({"kind": "sandbox.status", "state": "stopped"}))
            .await,
        "applied"
    );
    // The wake demand `terminal::service::wake_demand` writes (terminal open or SSH channel).
    sqlx::query("UPDATE sandboxes SET terminal_wake_at = now() WHERE id = $1::uuid")
        .bind(&lv.sb)
        .execute(r.h.pool())
        .await
        .unwrap();
    let items = r.work().await;
    let start = items
        .iter()
        .find(|i| i["kind"] == "sandbox.start")
        .expect("the wake demand starts the computer");
    let woken = Live {
        agent: lv.agent,
        conv: lv.conv,
        sb: lv.sb.clone(),
        boot: start["boot_id"].as_str().unwrap().to_owned(),
    };
    assert_ne!(woken.boot, lv.boot);
    running(&r, &woken).await;
    assert!(
        stop_item(&r).await.is_none(),
        "a computer woken for a session is not parked before the session connects"
    );
    // No session came: one idle window after `running`, it parks again.
    sqlx::query(
        "UPDATE sandboxes SET last_active_at = now() - interval '61 seconds' WHERE id = $1::uuid",
    )
    .bind(&lv.sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    let stop = stop_item(&r).await.expect("idle stop after the window");
    assert_eq!(stop["reason"], "idle", "{stop}");
}

/// (C-1058) The restart notice's fact: a restarted computer has a new `boot_id`, visible to a
/// member (never reduced).
#[tokio::test]
async fn a_restart_gives_a_new_boot_id() {
    let r = rig().await;
    let lv = r.boot("cv-restart").await;
    let before = sandbox(&r, &r.cookie, lv.agent).await;
    assert_eq!(before["boot_id"], lv.boot.as_str());
    let (s, v) = r
        .user_call(
            Method::POST,
            &format!("/v1/agents/{}/restart", lv.agent),
            None,
        )
        .await;
    assert_eq!(s, 202, "{v}");
    let stop = stop_item(&r).await.expect("restart stop");
    assert_eq!(stop["reason"], "restart");
    assert_eq!(
        r.report(&lv, json!({"kind": "sandbox.status", "state": "stopped"}))
            .await,
        "applied"
    );
    r.say(lv.conv, "again").await;
    let items = r.work().await;
    let start = items
        .iter()
        .find(|i| i["kind"] == "sandbox.start")
        .expect("a new start");
    let new_boot = start["boot_id"].as_str().unwrap();
    assert_ne!(new_boot, lv.boot);
    let after = sandbox(&r, &r.cookie, lv.agent).await;
    assert_eq!(after["boot_id"], new_boot, "{after}");
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (P5-SCHED-ONCE, D-421, D-422, C-1092..C-1095, SEC-144) One-time schedules: `run_at`, Done,
//! late rather than lost while paused, a reminder that runs in its creator's own chat with a Web
//! Push, the tools' time zone, and retention.

use crate::common::*;
use crate::hosted::{Live, Rig, rig, rig_with};
use crate::web_push::{HOST, SUBJECT};
use aurora_api::domains::push::{deliver, vapid};
use aurora_testkit::push::{MockPush, VapidExpect};
use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const ZURICH: Tz = chrono_tz::Europe::Zurich;

/// Tomorrow's date in `tz`, `YYYY-MM-DD`.
fn tomorrow(tz: Tz) -> String {
    (Utc::now() + Duration::days(1))
        .with_timezone(&tz)
        .format("%Y-%m-%d")
        .to_string()
}

/// The instant of a local `YYYY-MM-DDTHH:MM` in `tz` (the earlier one when ambiguous).
fn instant(local: &str, tz: Tz) -> DateTime<Utc> {
    let n = NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M").unwrap();
    tz.from_local_datetime(&n)
        .earliest()
        .unwrap()
        .with_timezone(&Utc)
}

fn at(v: &Value) -> DateTime<Utc> {
    v.as_str().unwrap().parse().unwrap()
}

/// The field a 400 `invalid_request` names, and its message.
fn field(v: &Value) -> (String, String) {
    let f = &v["error"]["details"]["fields"][0];
    (
        f["path"].as_str().unwrap_or_default().to_owned(),
        f["message"].as_str().unwrap_or_default().to_owned(),
    )
}

async fn create(r: &Rig, agent: Uuid, body: Value) -> (u16, Value) {
    r.user_call(
        Method::POST,
        &format!("/v1/agents/{agent}/schedules"),
        Some(body),
    )
    .await
}

async fn patch(r: &Rig, id: &str, body: Value) -> (u16, Value) {
    r.user_call(Method::PATCH, &format!("/v1/schedules/{id}"), Some(body))
        .await
}

async fn due(r: &Rig, id: &str) {
    sqlx::query(
        "UPDATE schedules SET next_fire_at = now() - interval '1 minute' WHERE id = $1::uuid",
    )
    .bind(id)
    .execute(r.h.pool())
    .await
    .unwrap();
}

async fn runs(r: &Rig, id: &str) -> Vec<(String, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT outcome, turn_id FROM schedule_runs WHERE schedule_id = $1::uuid ORDER BY created_at, id",
    )
    .bind(id)
    .fetch_all(r.h.pool())
    .await
    .unwrap()
}

/// `(completed, next_fire_at set, enabled, disabled_reason, last_outcome)`.
async fn state(r: &Rig, id: &str) -> (bool, bool, bool, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT completed_at IS NOT NULL, next_fire_at IS NOT NULL, enabled, disabled_reason,
                last_outcome FROM schedules WHERE id = $1::uuid",
    )
    .bind(id)
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

async fn user_id(r: &Rig) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = 'm@x.io'")
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

/// Apply the home sync the supervisor would (a schedule change bumps the generation).
async fn synced(r: &Rig) {
    sqlx::query(
        "UPDATE sandboxes s SET home_applied_generation =
             a.home_sync_generation + w.home_sync_generation + 1
         FROM agents a JOIN workspaces w ON w.id = a.workspace_id WHERE a.id = s.agent_id",
    )
    .execute(r.h.pool())
    .await
    .unwrap();
}

/// A tool call in `conv` during `turn`.
async fn tool(r: &Rig, lv: &Live, conv: Uuid, turn: &str, name: &str, args: Value) -> Value {
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/tool-calls",
            json!({"sandbox_id": lv.sb, "boot_id": lv.boot, "call_id": Uuid::now_v7().to_string(),
                   "conversation_id": conv, "turn_id": turn, "tool": name, "args": args}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v
}

async fn finish(r: &Rig, lv: &Live, turn: &str) {
    r.report(lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    r.report(
        lv,
        json!({"kind": "turn.result", "turn_id": turn, "status": "done"}),
    )
    .await;
}

/// The run's turn: `(conversation, role, status)`.
async fn run_turn(r: &Rig, id: &str) -> (Uuid, Uuid, String, String) {
    sqlx::query_as(
        "SELECT t.id, t.conversation_id, t.role, t.status FROM schedule_runs r
           JOIN turns t ON t.id = r.turn_id WHERE r.schedule_id = $1::uuid
         ORDER BY r.created_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

/// C-1092 through the API: create, refusals, the earlier instant, the limit, a zone-only patch,
/// Done and re-arming, switching kinds both ways, and the preview.
#[tokio::test]
async fn one_time_create_validate_patch_and_preview() {
    let r = rig().await;
    let agent = r.agent("once-api", true).await;
    let day = tomorrow(ZURICH);
    let local = format!("{day}T09:00");
    let (s, v) = create(
        &r,
        agent,
        json!({"name": "Call Ada", "prompt": "p", "run_at": local, "timezone": "Europe/Zurich"}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert!(v["cron"].is_null());
    assert_eq!(v["run_at"], local);
    assert!(v["completed_at"].is_null());
    assert_eq!(v["next_runs"].as_array().unwrap().len(), 1);
    assert_eq!(at(&v["next_runs"][0]), instant(&local, ZURICH));
    assert_eq!(at(&v["next_fire_at"]), instant(&local, ZURICH));
    let id = v["id"].as_str().unwrap().to_owned();

    // Refusals: the field and the message.
    let past = "2020-01-01T09:00".to_owned();
    let six = format!(
        "{}-01-01T09:00",
        Utc::now().format("%Y").to_string().parse::<i32>().unwrap() + 6
    );
    for (body, path, msg) in [
        (
            json!({"cron": "0 9 * * *", "run_at": local}),
            "/run_at",
            "set either cron or run_at, not both",
        ),
        (json!({}), "/cron", "set either cron or run_at"),
        (
            json!({"run_at": format!("{local}:00+01:00")}),
            "/run_at",
            "without an offset",
        ),
        (
            json!({"run_at": format!("{local}Z")}),
            "/run_at",
            "without an offset",
        ),
        (json!({"run_at": past}), "/run_at", "is in the past"),
        (json!({"run_at": six}), "/run_at", "at most 5 years ahead"),
        // The spring-forward gap in Zurich: 2027-03-28 02:00..03:00 does not exist.
        (
            json!({"run_at": "2027-03-28T02:30", "timezone": "Europe/Zurich"}),
            "/run_at",
            "daylight-saving gap",
        ),
    ] {
        let mut b = body.clone();
        b["name"] = json!("n");
        b["prompt"] = json!("p");
        let (s, v) = create(&r, agent, b).await;
        assert_eq!(s, 400, "{body}: {v}");
        let (p, m) = field(&v);
        assert_eq!(p, path, "{body}: {v}");
        assert!(m.contains(msg), "{body}: {m}");
    }
    // Clocks go back in Zurich on 2027-10-31: 02:30 is the earlier instant (CEST, 00:30Z).
    let (s, v) = create(
        &r,
        agent,
        json!({"name": "fb", "prompt": "p", "run_at": "2027-10-31T02:30", "timezone": "Europe/Zurich"}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(at(&v["next_runs"][0]), at(&json!("2027-10-31T00:30:00Z")));

    // A zone-only patch keeps 09:00 local.
    let (s, v) = patch(&r, &id, json!({"timezone": "America/New_York"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["run_at"], local);
    assert_eq!(
        at(&v["next_runs"][0]),
        instant(&local, chrono_tz::America::New_York)
    );
    // At most one of cron / run_at on a patch.
    let (s, v) = patch(&r, &id, json!({"cron": "0 9 * * *", "run_at": local})).await;
    assert_eq!((s, field(&v).0.as_str()), (400, "/run_at"), "{v}");

    // Switch to recurring and back.
    let (s, v) = patch(&r, &id, json!({"cron": "0 9 * * *"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["cron"].as_str(), v["run_at"].is_null()),
        (Some("0 9 * * *"), true)
    );
    assert_eq!(v["next_runs"].as_array().unwrap().len(), 3);
    let (s, v) = patch(
        &r,
        &id,
        json!({"run_at": local, "timezone": "Europe/Zurich"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["cron"].is_null());
    assert_eq!(v["run_at"], local);

    // Done: `enabled` is refused, a new run_at re-arms it.
    sqlx::query(
        "UPDATE schedules SET completed_at = now(), next_fire_at = NULL WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert!(v["completed_at"].is_string());
    assert_eq!(v["next_runs"], json!([]));
    for enabled in [true, false] {
        let (s, v) = patch(&r, &id, json!({"enabled": enabled})).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (409, Some("schedule_completed")),
            "{v}"
        );
    }
    let again = format!("{}T10:30", tomorrow(ZURICH));
    let (s, v) = patch(&r, &id, json!({"run_at": again})).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["completed_at"].is_null());
    assert_eq!(at(&v["next_runs"][0]), instant(&again, ZURICH));

    // The 50-per-agent limit ignores Done rows.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM schedules WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO schedules (id, agent_id, name, prompt, run_at, timezone, completed_at)
         SELECT gen_random_uuid(), $1, 'done' || g, 'p', now() - interval '1 day', 'UTC', now()
         FROM generate_series(1, 60) g",
    )
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = create(
        &r,
        agent,
        json!({"name": "ok", "prompt": "p", "run_at": local}),
    )
    .await;
    assert_eq!(s, 201, "Done rows don't count: {v}");
    sqlx::query(
        "INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone)
         SELECT gen_random_uuid(), $1, 'r' || g, 'p', '0 9 * * *', 'UTC'
         FROM generate_series(1, $2) g",
    )
    .bind(agent)
    .bind(i32::try_from(50 - (n + 1)).unwrap())
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = create(
        &r,
        agent,
        json!({"name": "over", "prompt": "p", "run_at": local}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("schedule_limit_reached")),
        "{v}"
    );

    // Preview: one instant, no interval; a past time is 422 `invalid_run_at` at `/run_at`.
    let uri = format!("/v1/agents/{agent}/schedules/preview");
    let (s, v) = r
        .user_call(
            Method::POST,
            &uri,
            Some(json!({"run_at": local, "timezone": "Europe/Zurich"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["next"].as_array().unwrap().len(), 1);
    assert_eq!(at(&v["next"][0]), instant(&local, ZURICH));
    assert!(v["interval_secs"].is_null());
    assert_eq!(v["too_frequent"], false);
    let (s, v) = r
        .user_call(
            Method::POST,
            &uri,
            Some(json!({"run_at": "2020-01-01T09:00"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("invalid_run_at")),
        "{v}"
    );
    assert_eq!(field(&v), ("/run_at".into(), "is in the past".into()));
    let (s, v) = r
        .user_call(
            Method::POST,
            &uri,
            Some(json!({"cron": "0 9 * * *", "run_at": local})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("invalid_run_at")),
        "{v}"
    );
}

/// C-1092 firing: due → one run, Done in the same transaction, never again; Run now leaves
/// `run_at` and `completed_at` alone; paused → one `waiting_paused` run, then late after Resume;
/// unauthorized → disabled, not Done, and after a reassignment it fires at once.
#[tokio::test]
async fn a_one_time_schedule_fires_once_waits_while_paused_and_resumes_late() {
    let r = rig().await;
    let agent = r.agent("once-fire", true).await;
    let local = format!("{}T09:00", tomorrow(ZURICH));
    let body = json!({"name": "n", "prompt": "p", "run_at": local, "timezone": "Europe/Zurich"});

    // Run now on a pending one: a manual run in its own thread; nothing moves.
    let (_, v) = create(&r, agent, body.clone()).await;
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, run) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(s, 201, "{run}");
    let (_, v) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(
        (v["run_at"].as_str(), v["completed_at"].is_null()),
        (Some(local.as_str()), true)
    );
    assert_eq!(at(&v["next_runs"][0]), instant(&local, ZURICH));

    // Due (the Run now turn is still active: a one-time fire is never skipped as busy).
    due(&r, &id).await;
    r.work().await;
    let rr = runs(&r, &id).await;
    assert_eq!(rr.iter().filter(|x| x.0 == "fired").count(), 2, "{rr:?}");
    let (done, armed, enabled, _, last) = state(&r, &id).await;
    assert_eq!(
        (done, armed, enabled, last.as_deref()),
        (true, false, true, Some("ok"))
    );
    r.work().await;
    assert_eq!(runs(&r, &id).await.len(), 2, "never fired again");
    // Its own thread (no origin), and the run reads `running`.
    let (_, conv, role, _) = run_turn(&r, &id).await;
    let kind: String = sqlx::query_scalar("SELECT kind FROM conversations WHERE id = $1")
        .bind(conv)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!((kind.as_str(), role.as_str()), ("schedule", "turn"));

    // Paused (frozen) agent: one `waiting_paused` run over several ticks, checked every minute.
    let (_, v) = create(&r, agent, body.clone()).await;
    let id2 = v["id"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'org_admin' WHERE id = $1")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    for _ in 0..3 {
        due(&r, &id2).await;
        r.work().await;
    }
    assert_eq!(
        runs(&r, &id2).await,
        vec![("waiting_paused".to_owned(), None)]
    );
    let (done, armed, enabled, why, last) = state(&r, &id2).await;
    assert_eq!(
        (done, armed, enabled, why, last.as_deref()),
        (false, true, true, None, Some("waiting_paused"))
    );
    let recheck: bool = sqlx::query_scalar(
        "SELECT next_fire_at BETWEEN now() + interval '50 seconds' AND now() + interval '61 seconds'
         FROM schedules WHERE id = $1::uuid",
    )
    .bind(&id2)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(recheck);
    let (_, page) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id2}/runs"), None)
        .await;
    assert_eq!(
        (
            page["items"][0]["result"].as_str(),
            page["items"][0]["reason"].as_str()
        ),
        (Some("skipped"), Some("paused"))
    );
    // Resume: it fires on the next check (the minute is simulated), late.
    sqlx::query("UPDATE agents SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL WHERE id = $1")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE schedules SET run_at = now() - interval '1 hour' WHERE id = $1::uuid")
        .bind(&id2)
        .execute(r.h.pool())
        .await
        .unwrap();
    due(&r, &id2).await;
    r.work().await;
    assert_eq!(runs(&r, &id2).await.last().unwrap().0, "fired");
    assert!(state(&r, &id2).await.0, "Done");
    // The turn context says it is late.
    let (turn, _, _, _) = run_turn(&r, &id2).await;
    let mut conn = r.h.pool().acquire().await.unwrap();
    let t = aurora_db::repo::turns::get(&mut *conn, turn)
        .await
        .unwrap()
        .unwrap();
    let a = aurora_db::repo::agents::get(&mut *conn, agent)
        .await
        .unwrap()
        .unwrap();
    let b = aurora_api::domains::people::turn_context::build(&mut conn, &t, &a, Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert!(b.contains("a one-time schedule due at "), "{b}");
    assert!(b.contains("it will not run again"), "{b}");
    assert!(
        b.contains("running late: once-fire was paused at the time"),
        "{b}"
    );
    drop(conn);

    // Unauthorized: disabled `authorization`, not Done; reassigned and resumed: fires at once.
    let (_, v) = create(&r, agent, body.clone()).await;
    let id3 = v["id"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE org_id = $1")
        .bind(r.org)
        .execute(r.h.pool())
        .await
        .unwrap();
    due(&r, &id3).await;
    r.work().await;
    assert_eq!(runs(&r, &id3).await[0].0, "skipped_unauthorized");
    let (done, _, enabled, why, _) = state(&r, &id3).await;
    assert_eq!(
        (done, enabled, why.as_deref()),
        (false, false, Some("authorization"))
    );
    sqlx::query("UPDATE org_members SET status = 'active' WHERE org_id = $1")
        .bind(r.org)
        .execute(r.h.pool())
        .await
        .unwrap();
    // Its time has passed meanwhile.
    sqlx::query("UPDATE schedules SET run_at = now() - interval '2 hours' WHERE id = $1::uuid")
        .bind(&id3)
        .execute(r.h.pool())
        .await
        .unwrap();
    let owner = r.h.user("owner@x.io", false).await;
    r.h.member(r.org, owner, "member", "active").await;
    r.h.bind_ws(r.ws, owner, "owner").await;
    let (_, otok) = r.h.good_session(owner).await;
    let me = user_id(&r).await;
    let (s, v) =
        r.h.call(
            &cookie(&otok),
            Method::PUT,
            &format!("/v1/schedules/{id3}/creator"),
            Some(json!({"user_id": me, "re_enable": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["enabled"], true);
    assert!(at(&v["next_fire_at"]) <= Utc::now(), "armed for now: {v}");
    r.work().await;
    assert_eq!(runs(&r, &id3).await.last().unwrap().0, "fired");
    assert!(state(&r, &id3).await.0, "Done");
}

/// A quiet gate still completes a one-time schedule; failures never disable it.
#[tokio::test]
async fn a_quiet_gate_completes_and_a_failure_never_disables() {
    let r = rig().await;
    let lv = r.boot("once-gate").await;
    let d = r.deliver().await;
    finish(&r, &lv, d["turn_id"].as_str().unwrap()).await;
    let local = format!("{}T09:00", tomorrow(ZURICH));
    for (outcome, n) in [("quiet", "q"), ("failed", "f")] {
        let (_, v) = create(
            &r,
            lv.agent,
            json!({"name": n, "prompt": "p", "run_at": local, "gate_command": "./check.sh"}),
        )
        .await;
        let id = v["id"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE schedules SET consecutive_failures = 4 WHERE id = $1::uuid")
            .bind(&id)
            .execute(r.h.pool())
            .await
            .unwrap();
        due(&r, &id).await;
        r.work().await;
        synced(&r).await;
        let d = r.deliver().await;
        assert_eq!(d["gate"]["command"], "./check.sh");
        let t = d["turn_id"].as_str().unwrap().to_owned();
        assert_eq!(
            r.report(
                &lv,
                json!({"kind": "gate.result", "turn_id": t, "outcome": outcome, "output": "x"})
            )
            .await,
            "applied"
        );
        let (done, _, enabled, why, _) = state(&r, &id).await;
        assert_eq!((done, enabled, why), (true, true, None), "{outcome}");
    }
}

/// D-422, C-1093, C-1095, SEC-144: a reminder set by the agent in the person's own chat runs in
/// that chat, queued behind the active turn (never steered into it), with no "finished" notice;
/// the turn context says the person reads it; one push per opted-in device, never the answer;
/// a schedule turn cannot schedule again; the tools read times in the person's zone.
#[tokio::test]
async fn a_reminder_runs_in_the_creators_chat_queued_and_pushed() {
    let mock = MockPush::start().await;
    let url = mock.url();
    let r = rig_with(move |c| {
        c.push.enabled = true;
        c.push.vapid.subject = Some(SUBJECT.into());
        c.push.allowed_endpoint_hosts = vec![HOST.into()];
        c.push.dev_endpoint_overrides.insert(HOST.to_owned(), url);
    })
    .await;
    let key = vapid::active(&r.h.state).await.unwrap().1.public_b64u();
    mock.expect_vapid(VapidExpect {
        aud: format!("https://{HOST}"),
        subject: SUBJECT.into(),
        public_key: key.clone(),
    });
    for (path, details) in [("/push/plain", false), ("/push/named", true)] {
        let k = mock.subscribe(path);
        let (s, v) = r
            .user_call(
                Method::POST,
                "/v1/me/push-subscriptions",
                Some(json!({"endpoint": format!("https://{HOST}{path}"),
                    "keys": {"p256dh": k.p256dh(), "auth": k.auth()}, "label": "Laptop",
                    "show_details": details, "vapid_public_key": key})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
    let me = user_id(&r).await;
    sqlx::query("UPDATE users SET timezone = 'Europe/Zurich' WHERE id = $1")
        .bind(me)
        .execute(r.h.pool())
        .await
        .unwrap();
    let lv = r.boot("remind").await;
    let d = r.deliver().await;
    let hello = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": hello}))
        .await;

    // The person asks for a reminder; no timezone: theirs (Zurich).
    let local = format!("{}T09:00", tomorrow(ZURICH));
    let v = tool(
        &r,
        &lv,
        lv.conv,
        &hello,
        "schedule_create",
        json!({"name": "Call Ada", "prompt": "Remind them to call Ada (CANARYPROMPT)", "run_at": local}),
    )
    .await;
    assert_eq!(v["ok"], true, "{v}");
    let s = &v["content"][0]["value"];
    assert_eq!(
        (s["timezone"].as_str(), s["run_at"].as_str()),
        (Some("Europe/Zurich"), Some(local.as_str()))
    );
    assert!(s["cron"].is_null() && s["completed_at"].is_null());
    let id = s["id"].as_str().unwrap().to_owned();
    // A mistake comes back with the field and the API's message.
    let v = tool(
        &r,
        &lv,
        lv.conv,
        &hello,
        "schedule_create",
        json!({"name": "x", "prompt": "p", "run_at": "2020-01-01T09:00"}),
    )
    .await;
    assert_eq!(v["ok"], false);
    assert!(v.to_string().contains("run_at is in the past"), "{v}");

    // Due while the hello turn is running: a follow-up in the person's chat, not steered.
    due(&r, &id).await;
    r.work().await;
    r.work().await;
    let (_, conv, role, status) = run_turn(&r, &id).await;
    assert_eq!(
        (conv, role.as_str(), status.as_str()),
        (lv.conv, "followup", "joining")
    );
    assert!(state(&r, &id).await.0, "Done at fire");
    let threads: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conversations WHERE automation_id = $1::uuid")
            .bind(&id)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(threads, 0, "no run thread");

    // The hello turn ends: the run becomes the next turn and keeps the schedule's display.
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": hello, "status": "done"}),
    )
    .await;
    let (turn, conv, role, status) = run_turn(&r, &id).await;
    assert_eq!(
        (conv, role.as_str(), status.as_str()),
        (lv.conv, "turn", "queued")
    );
    let queued = r
        .events(lv.conv)
        .await
        .into_iter()
        .rfind(|e| e.0 == "turn.queued")
        .unwrap()
        .1;
    assert_eq!(queued["turn_id"], json!(turn));
    assert_eq!(queued["display"]["kind"], "schedule");
    assert_eq!(queued["display"]["name"], "Call Ada");
    assert!(queued["promoted_from"].is_string());
    let (_, page) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}/runs"), None)
        .await;
    assert_eq!(page["items"][0]["conversation_id"], json!(lv.conv));

    // Delivered with the turn context of a run in the person's own chat.
    synced(&r).await;
    let d = r.deliver().await;
    assert_eq!(d["turn_id"], json!(turn));
    let text = d["input"]["text"].as_str().unwrap();
    let name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
        .bind(me)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert!(
        text.contains(&format!(
            "Arrived through: schedule \"Call Ada\", a one-time schedule due at {} (Europe/Zurich); it will not run again. {name} set this one-time schedule in this conversation and reads it; your answer is shown here.",
            local.replace('T', " ")
        )),
        "{text}"
    );
    assert!(!text.contains("Nobody reads"), "{text}");
    // A schedule turn has no speaker: it cannot schedule itself again.
    let t = turn.to_string();
    let v = tool(
        &r,
        &lv,
        lv.conv,
        &t,
        "schedule_create",
        json!({"name": "again", "prompt": "p", "run_at": local}),
    )
    .await;
    assert_eq!(v["ok"], false, "{v}");
    assert!(
        v.to_string()
            .contains("only be created for a person"),
        "{v}"
    );

    // No push before the answer is done.
    assert_eq!(
        deliver::sweep_schedule_answers(&r.h.state).await.unwrap(),
        0
    );
    r.report(&lv, json!({"kind": "turn.started", "turn_id": t}))
        .await;
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": t, "status": "done"}),
    )
    .await;
    assert_eq!(r.turn_status(&t).await.0, "done");
    let ev = r.events(lv.conv).await;
    assert!(
        !ev.iter()
            .any(|e| e.0 == "notice" && e.1["code"] == "schedule_run"),
        "no finished notice: {ev:?}"
    );

    // One push per device: fixed fields, the conversation's path, the name only with details.
    assert_eq!(
        deliver::sweep_schedule_answers(&r.h.state).await.unwrap(),
        2
    );
    let got = mock.received();
    assert_eq!(got.len(), 2);
    let by_path = |p: &str| got.iter().find(|x| x.path == p).unwrap().json().unwrap();
    let plain = by_path("/push/plain");
    let mut keys: Vec<_> = plain.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["type", "url", "v"]);
    assert_eq!(plain["type"], "schedule_answer");
    let (org, ws): (String, String) = sqlx::query_as(
        "SELECT o.slug, w.slug FROM workspaces w JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(r.ws)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        plain["url"],
        format!("/o/{org}/w/{ws}/agents/remind?c={}", lv.conv)
    );
    let named = by_path("/push/named");
    assert_eq!(named["title"], "Call Ada");
    for x in &got {
        let all = x.json().unwrap().to_string();
        assert!(!all.contains("CANARY"), "{all}");
    }
    assert_eq!(
        deliver::sweep_schedule_answers(&r.h.state).await.unwrap(),
        0,
        "never twice"
    );

    // No `users.timezone`: the workspace's zone.
    sqlx::query("UPDATE users SET timezone = NULL WHERE id = $1")
        .bind(me)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE workspaces SET timezone = 'America/New_York' WHERE id = $1")
        .bind(r.ws)
        .execute(r.h.pool())
        .await
        .unwrap();
    r.say(lv.conv, "and a weekly one").await;
    synced(&r).await;
    let d = r.deliver().await;
    let t2 = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": t2}))
        .await;
    let v = tool(
        &r,
        &lv,
        lv.conv,
        &t2,
        "schedule_create",
        json!({"name": "weekly", "prompt": "p", "cron": "0 9 * * 1"}),
    )
    .await;
    assert_eq!(
        v["content"][0]["value"]["timezone"], "America/New_York",
        "{v}"
    );
}

/// SEC-144: after a reassignment to another member, or with the origin archived, the run gets
/// its own thread and nothing is written into the old owner's chat.
#[tokio::test]
async fn after_reassignment_or_archive_the_run_gets_its_own_thread() {
    let r = rig().await;
    let lv = r.boot("once-sec").await;
    let d = r.deliver().await;
    let hello = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": hello}))
        .await;
    let local = format!("{}T09:00", tomorrow(ZURICH));
    let mut ids = Vec::new();
    for n in ["moved", "archived"] {
        let v = tool(
            &r,
            &lv,
            lv.conv,
            &hello,
            "schedule_create",
            json!({"name": n, "prompt": "p", "run_at": local}),
        )
        .await;
        ids.push(v["content"][0]["value"]["id"].as_str().unwrap().to_owned());
    }
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": hello, "status": "done"}),
    )
    .await;
    let before = r.events(lv.conv).await.len();

    // Reassigned to another member (who may use the workspace).
    let other = r.h.user("other@x.io", false).await;
    r.h.member(r.org, other, "member", "active").await;
    r.h.bind_ws(r.ws, other, "member").await;
    sqlx::query("UPDATE schedules SET creator_user_id = $2 WHERE id = $1::uuid")
        .bind(&ids[0])
        .bind(other)
        .execute(r.h.pool())
        .await
        .unwrap();
    due(&r, &ids[0]).await;
    r.work().await;
    let (turn, conv, _, _) = run_turn(&r, &ids[0]).await;
    assert_ne!(conv, lv.conv);
    let (kind, origin): (String, Option<Uuid>) =
        sqlx::query_as("SELECT kind, origin_conversation_id FROM conversations WHERE id = $1")
            .bind(conv)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!((kind.as_str(), origin), ("schedule", Some(lv.conv)));
    // Its run ends: the "finished" notice does not go to the old owner's chat either.
    let mut c = r.h.pool().acquire().await.unwrap();
    aurora_api::domains::hosted::lifecycle::finish(&mut c, turn, "done", None, None, None)
        .await
        .unwrap();
    drop(c);
    assert_eq!(
        r.events(lv.conv).await.len(),
        before,
        "nothing in the old owner's chat"
    );

    // The origin archived: a run thread too.
    sqlx::query("UPDATE conversations SET archived_at = now() WHERE id = $1")
        .bind(lv.conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    due(&r, &ids[1]).await;
    r.work().await;
    let (_, conv, _, _) = run_turn(&r, &ids[1]).await;
    assert_ne!(conv, lv.conv);
    assert!(state(&r, &ids[1]).await.0);
}

/// C-1092 retention: a Done schedule older than 30 days is deleted by the daily pass with its
/// threads marked; younger and pending ones stay.
#[tokio::test]
async fn retention_removes_done_schedules_after_thirty_days() {
    let r = rig().await;
    let agent = r.agent("once-prune", true).await;
    let local = format!("{}T09:00", tomorrow(ZURICH));
    let mut ids = Vec::new();
    for n in ["old", "young", "pending"] {
        let (_, v) = create(
            &r,
            agent,
            json!({"name": n, "prompt": "p", "run_at": local}),
        )
        .await;
        ids.push(v["id"].as_str().unwrap().to_owned());
    }
    let (s, run) = r
        .user_call(
            Method::POST,
            &format!("/v1/schedules/{}/run-now", ids[0]),
            None,
        )
        .await;
    assert_eq!(s, 201, "{run}");
    let thread: Uuid = run["conversation_id"].as_str().unwrap().parse().unwrap();
    for (id, days) in [(&ids[0], 31), (&ids[1], 1)] {
        sqlx::query(
            "UPDATE schedules SET completed_at = now() - make_interval(days => $2), next_fire_at = NULL
             WHERE id = $1::uuid",
        )
        .bind(id)
        .bind(days)
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    let report = aurora_api::jobs::run_retention_prune(r.h.pool())
        .await
        .unwrap();
    assert_eq!(report.schedules_done, 1);
    let left: Vec<String> =
        sqlx::query_scalar("SELECT name FROM schedules WHERE agent_id = $1 ORDER BY name")
            .bind(agent)
            .fetch_all(r.h.pool())
            .await
            .unwrap();
    assert_eq!(left, ["pending", "young"]);
    let marked: bool = sqlx::query_scalar(
        "SELECT automation_deleted_at IS NOT NULL FROM conversations WHERE id = $1",
    )
    .bind(thread)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(marked, "its thread reads as from a deleted automation");
    assert_eq!(
        aurora_api::jobs::run_retention_prune(r.h.pool())
            .await
            .unwrap()
            .schedules_done,
        0
    );
}

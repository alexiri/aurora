#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-062: schedules - API, cron, arm 9, gates, authority, tools.

use crate::common::*;
use crate::hosted::{Live, Rig, rig};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

impl Rig {
    async fn sched(&self, agent: Uuid, body: Value) -> Value {
        let (s, v) = self
            .user_call(
                Method::POST,
                &format!("/v1/agents/{agent}/schedules"),
                Some(body),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v
    }

    async fn due(&self, id: &str) {
        sqlx::query(
            "UPDATE schedules SET next_fire_at = now() - interval '2 days' WHERE id = $1::uuid",
        )
        .bind(id)
        .execute(self.h.pool())
        .await
        .unwrap();
    }

    async fn runs(&self, id: &str) -> Vec<(String, Option<Uuid>)> {
        sqlx::query_as(
            "SELECT outcome, turn_id FROM schedule_runs WHERE schedule_id = $1::uuid ORDER BY created_at, id",
        )
        .bind(id)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    /// Deliver and finish the initial "hello" turn so the sandbox is idle.
    async fn finish_hello(&self, lv: &Live) {
        let d = self.deliver().await;
        let t = d["turn_id"].as_str().unwrap().to_owned();
        self.report(lv, json!({"kind": "turn.started", "turn_id": t}))
            .await;
        self.report(
            lv,
            json!({"kind": "turn.result", "turn_id": t, "status": "done"}),
        )
        .await;
    }

    /// Fire `id` and return the delivery of its run turn (needs a booted sandbox).
    async fn fire_and_deliver(&self, id: &str) -> Value {
        self.due(id).await;
        self.work().await; // arm 9 creates the turn
        // Creating/changing a schedule bumps the home-sync generation (the schedules summary is
        // projected, FIX-005); dispatch waits for the sync, so apply it as the supervisor would.
        sqlx::query(
            "UPDATE sandboxes s SET home_applied_generation =
                 a.home_sync_generation + w.home_sync_generation + 1
             FROM agents a JOIN workspaces w ON w.id = a.workspace_id WHERE a.id = s.agent_id",
        )
        .execute(self.h.pool())
        .await
        .unwrap();
        self.deliver().await
    }

    async fn gate(&self, lv: &Live, turn: &str, outcome: &str, output: &str) -> String {
        self.report(
            lv,
            json!({"kind": "gate.result", "turn_id": turn, "outcome": outcome, "output": output}),
        )
        .await
    }

    async fn sched_row(&self, id: &str) -> (bool, Option<String>, i32) {
        sqlx::query_as(
            "SELECT enabled, disabled_reason, consecutive_failures FROM schedules WHERE id = $1::uuid",
        )
        .bind(id)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn api_validation_crud_and_run_now() {
    let r = rig().await;
    let agent = r.agent("sched1", true).await;
    for (cron, tz) in [
        ("0 0 9 * * *", "UTC"),
        ("* * * *", "UTC"),
        ("*/2 * * * *", "UTC"),
        ("0 9 * * *", "Not/AZone"),
    ] {
        let (s, v) = r
            .user_call(
                Method::POST,
                &format!("/v1/agents/{agent}/schedules"),
                Some(json!({"name": "n", "prompt": "p", "cron": cron, "timezone": tz})),
            )
            .await;
        assert_eq!(s, 400, "{cron} {tz}: {v}");
    }
    let v = r
        .sched(
            agent,
            json!({"name": "daily", "prompt": "do it", "cron": "0 9 * * *"}),
        )
        .await;
    assert_eq!(v["timezone"], "UTC");
    assert_eq!(v["enabled"], true);
    assert!(v["next_fire_at"].is_string());
    assert!(v["created_by"]["display_name"].is_string());
    let id = v["id"].as_str().unwrap().to_owned();

    let (s, v) = r
        .user_call(Method::GET, &format!("/v1/agents/{agent}/schedules"), None)
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
    let (s, v) = r
        .user_call(
            Method::PATCH,
            &format!("/v1/schedules/{id}"),
            Some(json!({"enabled": false, "gate_command": "echo hi"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["disabled_reason"], "user");
    assert!(v["next_fire_at"].is_null());
    assert_eq!(v["gate_command"], "echo hi");
    let (_, v) = r
        .user_call(
            Method::PATCH,
            &format!("/v1/schedules/{id}"),
            Some(json!({"enabled": true, "gate_command": null})),
        )
        .await;
    assert!(v["gate_command"].is_null() && v["next_fire_at"].is_string());

    // run-now: creates a turn, a second call while it is active is busy.
    let (s, run) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(s, 201, "{run}");
    assert_eq!(run["outcome"], "fired");
    let (s, v) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("schedule_busy"))
    );
    let (s, v) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}/runs"), None)
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
    let text: String = sqlx::query_scalar("SELECT text FROM turns WHERE id = $1")
        .bind(Uuid::parse_str(run["turn_id"].as_str().unwrap()).unwrap())
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert!(text.starts_with("Scheduled task: daily"));

    let (s, _) = r
        .user_call(Method::DELETE, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(s, 204);
    let (s, _) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn missed_fires_coalesce_and_pollers_fire_once() {
    let r = rig().await;
    let agent = r.agent("sched2", true).await;
    let v = r
        .sched(
            agent,
            json!({"name": "d", "prompt": "p", "cron": "0 * * * *"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    r.due(&id).await;
    let (a, b) = tokio::join!(r.work(), r.work());
    let _ = (a, b);
    let runs = r.runs(&id).await;
    let outcomes: Vec<&str> = runs.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(
        outcomes,
        ["coalesced", "fired"],
        "one fire, one coalesce marker"
    );
    let (_, v) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert!(v["last_fired_at"].is_string());
    let future: bool = sqlx::query_scalar(
        "SELECT next_fire_at > now() AND fire_lease_until IS NULL FROM schedules WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(future);
    // Not due again.
    r.work().await;
    assert_eq!(r.runs(&id).await.len(), 2);
}

/// (FIX-110) A PATCH that read the schedule before a fire claimed it must not restore the old
/// `next_fire_at` or clear the fire lease (which would fire it twice): the edit starts from the
/// locked current row and writes only the edited columns. A timing change still reschedules.
#[tokio::test]
async fn patch_racing_a_fire_keeps_next_fire_at_and_the_lease() {
    use aurora_api::domains::schedules::service::{Patch, patch_with};
    let r = rig().await;
    let agent = r.agent("racer", true).await;
    let v = r
        .sched(
            agent,
            json!({"name": "d", "prompt": "p", "cron": "0 * * * *"}),
        )
        .await;
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let seen = aurora_db::repo::schedules::get(r.h.pool(), id)
        .await
        .unwrap()
        .unwrap();
    let pool = r.h.pool().clone();
    // Between the caller's read and the update a fire claims the schedule and moves it on.
    let fire = async {
        sqlx::query(
            "UPDATE schedules SET next_fire_at = now() + interval '3 hours',
                 fire_lease_until = now() + interval '5 minutes' WHERE id = $1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    };
    let row = patch_with(
        r.h.pool(),
        seen.clone(),
        Patch {
            name: Some("renamed".into()),
            ..Patch::default()
        },
        fire,
    )
    .await
    .unwrap();
    assert_eq!(row.name, "renamed");
    let (moved, leased): (bool, bool) = sqlx::query_as(
        "SELECT next_fire_at > now() + interval '2 hours', fire_lease_until > now()
         FROM schedules WHERE id = $1",
    )
    .bind(id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(moved, "the fire's next_fire_at survived the edit");
    assert!(leased, "the live fire lease survived the edit");
    // A timing change reschedules and clears the lease.
    let row = patch_with(
        r.h.pool(),
        seen,
        Patch {
            cron: Some("30 * * * *".into()),
            ..Patch::default()
        },
        async {},
    )
    .await
    .unwrap();
    assert_eq!(row.cron.as_deref(), Some("30 * * * *"));
    let (soon, leased): (bool, bool) = sqlx::query_as(
        "SELECT next_fire_at < now() + interval '70 minutes',
                coalesce(fire_lease_until > now(), false) FROM schedules WHERE id = $1",
    )
    .bind(id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(soon && !leased);
}

/// (FIX-110) The cron minimum interval holds over a full cycle through the API too.
#[tokio::test]
async fn cron_with_a_close_pair_later_in_the_cycle_is_refused() {
    let r = rig().await;
    let agent = r.agent("closepair", true).await;
    let (s, v) = r
        .user_call(
            Method::POST,
            &format!("/v1/agents/{agent}/schedules"),
            Some(json!({"name": "d", "prompt": "p", "cron": "0,5,10,15,20,21 * * * *"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn creator_losing_access_disables_with_authorization_and_notice() {
    let r = rig().await;
    let agent = r.agent("sched3", true).await;
    let origin = r.direct(agent).await;
    let v = r
        .sched(
            agent,
            json!({"name": "n", "prompt": "p", "cron": "0 * * * *"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE schedules SET origin_conversation_id = $2 WHERE id = $1::uuid")
        .bind(&id)
        .bind(origin)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE org_id = $1")
        .bind(r.org)
        .execute(r.h.pool())
        .await
        .unwrap();
    r.due(&id).await;
    r.work().await;
    assert_eq!(r.runs(&id).await[0].0, "skipped_unauthorized");
    let (en, why, _) = r.sched_row(&id).await;
    assert_eq!((en, why.as_deref()), (false, Some("authorization")));
    let ev = r.events(origin).await;
    assert!(
        ev.iter()
            .any(|e| e.0 == "notice" && e.1["code"] == "schedule_disabled"),
        "{ev:?}"
    );
}

#[tokio::test]
async fn gates_quiet_wake_fail_and_failure_disable() {
    let r = rig().await;
    let lv = r.boot("sched4").await;
    r.finish_hello(&lv).await;
    let origin = lv.conv;
    let v = r
        .sched(
            lv.agent,
            json!({"name": "g", "prompt": "check", "cron": "0 * * * *", "gate_command": "./check.sh"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE schedules SET origin_conversation_id = $2 WHERE id = $1::uuid")
        .bind(&id)
        .bind(origin)
        .execute(r.h.pool())
        .await
        .unwrap();

    // Quiet: run gated_quiet, no turn attached, turn closed.
    let d = r.fire_and_deliver(&id).await;
    assert_eq!(d["gate"]["command"], "./check.sh");
    assert_eq!(d["gate"]["timeout_secs"], 30);
    let t1 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t1, "quiet", "").await, "applied");
    assert_eq!(r.gate(&lv, &t1, "quiet", "").await, "duplicate");
    let runs = r.runs(&id).await;
    assert_eq!(runs.last().unwrap(), &("gated_quiet".to_owned(), None));
    assert_eq!(r.turn_status(&t1).await.0, "done");

    // Wake: output appended as a notice in the run conversation, turn continues.
    let d = r.fire_and_deliver(&id).await;
    let t2 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t2, "wake", "3 new items").await, "applied");
    assert_eq!(r.gate(&lv, &t2, "wake", "3 new items").await, "duplicate");
    let rc: Uuid = sqlx::query_scalar("SELECT conversation_id FROM turns WHERE id = $1::uuid")
        .bind(&t2)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let notes: Vec<_> = r
        .events(rc)
        .await
        .into_iter()
        .filter(|e| e.0 == "notice" && e.1["code"] == "gate_output")
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].1["text"], "3 new items");
    assert_eq!(r.runs(&id).await.last().unwrap().0, "fired");
    r.report(&lv, json!({"kind": "turn.started", "turn_id": t2}))
        .await;
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": t2, "status": "done"}),
    )
    .await;
    let origin_ev = r.events(origin).await;
    assert!(origin_ev.iter().any(|e| e.1["code"] == "schedule_run"));

    // Failed: counts toward the failure disable; replay on a second AppState is a no-op.
    let d = r.fire_and_deliver(&id).await;
    let t3 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t3, "failed", "exit 2").await, "applied");
    assert_eq!(r.runs(&id).await.last().unwrap().0, "gate_failed");
    assert_eq!(
        r.turn_status(&t3).await,
        ("failed".into(), Some("gate_failed".into()))
    );
    assert_eq!(r.sched_row(&id).await, (true, None, 1));
    let state2 = aurora_api::app::AppState::new(r.h.pool().clone(), config(|_| {}));
    let app2 = test_router(state2);
    let st = r
        .report_on(
            &app2,
            &lv,
            json!({"kind": "gate.result", "turn_id": t3, "outcome": "failed", "output": "exit 2"}),
        )
        .await;
    assert_ne!(st, "applied");
    assert_eq!(r.sched_row(&id).await.2, 1);

    // The fifth consecutive failure disables.
    sqlx::query("UPDATE schedules SET consecutive_failures = 4 WHERE id = $1::uuid")
        .bind(&id)
        .execute(r.h.pool())
        .await
        .unwrap();
    let d = r.fire_and_deliver(&id).await;
    let t4 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t4, "failed", "boom").await, "applied");
    let (en, why, n) = r.sched_row(&id).await;
    assert_eq!((en, why.as_deref(), n), (false, Some("failures"), 5));
    assert!(
        r.events(origin)
            .await
            .iter()
            .any(|e| e.1["code"] == "schedule_disabled")
    );
}

#[tokio::test]
async fn schedule_tools_via_tool_calls() {
    let r = rig().await;
    let lv = r.boot("sched5").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    let call = |tool: &'static str, args: Value| {
        let (r, lv, turn) = (&r, &lv, turn.clone());
        async move {
            let (s, v) = r
                .runner(
                    Some(r.rep),
                    "/v1/runner/tool-calls",
                    json!({"sandbox_id": lv.sb, "boot_id": lv.boot, "call_id": "c1",
                           "conversation_id": lv.conv, "turn_id": turn, "tool": tool, "args": args}),
                )
                .await;
            assert_eq!(s, 200, "{v}");
            v
        }
    };
    let v = call(
        "schedule_create",
        json!({"name": "t", "prompt": "p", "cron": "30 8 * * 1-5", "timezone": "Europe/Madrid"}),
    )
    .await;
    assert_eq!(v["ok"], true, "{v}");
    let id = v["content"][0]["value"]["id"].as_str().unwrap().to_owned();
    let v = call(
        "schedule_create",
        json!({"name": "t", "prompt": "p", "cron": "* * * * *"}),
    )
    .await;
    assert_eq!(v["ok"], false);
    // RFE-021: a tool cannot anchor an automation to the workspace (unknown argument).
    let v = call(
        "schedule_create",
        json!({"name": "t2", "prompt": "p", "cron": "30 8 * * 1-5", "anchor": "workspace"}),
    )
    .await;
    assert_eq!(v["ok"], false, "{v}");
    let anchors: Vec<String> = sqlx::query_scalar("SELECT anchor FROM schedules")
        .fetch_all(r.h.pool())
        .await
        .unwrap();
    assert_eq!(anchors, ["creator"]);
    let (origin, creator): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT origin_conversation_id, creator_user_id FROM schedules WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(origin, Some(lv.conv));
    assert!(creator.is_some());
    let v = call("schedule_list", json!({})).await;
    assert_eq!(
        v["content"][0]["value"]["schedules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let v = call("schedule_update", json!({"id": id, "enabled": false})).await;
    assert_eq!(v["content"][0]["value"]["disabled_reason"], "user");
    // Another agent's schedule is invisible to the tool.
    let other = r.agent("sched5b", false).await;
    let o = r
        .sched(
            other,
            json!({"name": "o", "prompt": "p", "cron": "0 * * * *"}),
        )
        .await;
    let v = call("schedule_delete", json!({"id": o["id"]})).await;
    assert_eq!(v["ok"], false);
    let v = call("schedule_delete", json!({"id": id})).await;
    assert_eq!(v["ok"], true, "{v}");
}

/// (FIX-111 5) The run history pages by keyset cursor: following `next_cursor` through more
/// than 200 runs (some sharing one `created_at`) returns every run exactly once, newest first.
#[tokio::test]
async fn run_history_pages_by_cursor() {
    let r = rig().await;
    let agent = r.agent("pager", true).await;
    let v = r
        .sched(
            agent,
            json!({"name": "daily", "prompt": "p", "cron": "0 9 * * *"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    sqlx::query(
        "INSERT INTO schedule_runs (id, schedule_id, outcome, fired_at, created_at)
         SELECT gen_random_uuid(), $1::uuid, 'gated_quiet', now() - make_interval(secs => g / 3),
                now() - make_interval(secs => g / 3)
         FROM generate_series(1, 450) g",
    )
    .bind(&id)
    .execute(r.h.pool())
    .await
    .unwrap();
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let uri = match &cursor {
            None => format!("/v1/schedules/{id}/runs?limit=200"),
            Some(c) => format!("/v1/schedules/{id}/runs?limit=200&cursor={c}"),
        };
        let (s, v) = r.user_call(Method::GET, &uri, None).await;
        assert_eq!(s, 200, "{v}");
        seen.extend(
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["id"].as_str().unwrap().to_owned()),
        );
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 450);
    let unique: std::collections::HashSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 450, "each run exactly once");
}

// ------------------------------------------------------------------ P5-REFINE-7

/// Mark a turn `done` (as its result would) without a runner.
async fn finish_turn(r: &Rig, turn: &str) {
    let mut conn = r.h.pool().acquire().await.unwrap();
    aurora_api::domains::hosted::lifecycle::finish(
        &mut conn,
        turn.parse().unwrap(),
        "done",
        None,
        None,
        None,
    )
    .await
    .unwrap();
}

/// (P5-REFINE-7, C-1074, C-1077) Every run gets its own thread with the schedule's origin; the
/// busy rule is the latest run's turn; Run now answers `running` with its thread; a rename
/// follows and a delete marks the threads.
#[tokio::test]
async fn each_run_gets_its_own_thread_and_the_busy_rule_is_the_latest_run() {
    let r = rig().await;
    let agent = r.agent("threads", true).await;
    let origin = r.direct(agent).await;
    let v = r
        .sched(
            agent,
            json!({"name": "Morning digest", "prompt": "p", "cron": "0 9 * * *"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE schedules SET origin_conversation_id = $2 WHERE id = $1::uuid")
        .bind(&id)
        .bind(origin)
        .execute(r.h.pool())
        .await
        .unwrap();
    // C-1077: three next instants while enabled, no last result yet.
    assert_eq!(v["next_runs"].as_array().unwrap().len(), 3);
    assert!(v["last_result"].is_null());

    // Run now: 201 `running` with its own thread.
    let (s, run1) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(s, 201, "{run1}");
    assert_eq!(run1["result"], "running");
    assert!(run1["reason"].is_null());
    let c1: Uuid = run1["conversation_id"].as_str().unwrap().parse().unwrap();
    let (ext, kind, origin_of, title, source): (
        String,
        String,
        Option<Uuid>,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT external_ref, kind, origin_conversation_id, title, title_source
             FROM conversations WHERE id = $1",
    )
    .bind(c1)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        ext,
        format!("schedule:{id}:{}", run1["id"].as_str().unwrap())
    );
    assert_eq!(kind, "schedule");
    assert_eq!(
        origin_of,
        Some(origin),
        "FIX-192 finds the origin through the thread"
    );
    assert_eq!(title.as_deref(), Some("Morning digest"));
    assert_eq!(source, "automation");
    // The people's view of the run turn (C-1079); the agent's text is unchanged.
    let queued = r
        .events(c1)
        .await
        .into_iter()
        .find(|e| e.0 == "turn.queued")
        .unwrap()
        .1;
    assert_eq!(
        queued["display"],
        json!({"kind": "schedule", "name": "Morning digest", "schedule_id": id,
               "run_id": run1["id"]})
    );
    assert!(
        queued["text"]
            .as_str()
            .unwrap()
            .starts_with("Scheduled task: Morning digest")
    );

    // Busy while the latest run's turn is active: Run now 409, a due fire `skipped_busy`.
    let (s, v) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("schedule_busy"))
    );
    r.due(&id).await;
    r.work().await;
    let last: Option<String> =
        sqlx::query_scalar("SELECT last_outcome FROM schedules WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(last.as_deref(), Some("skipped_busy"));
    assert_eq!(
        r.runs(&id).await.iter().filter(|x| x.0 == "fired").count(),
        1
    );

    // Once it is done, Run now starts a second thread.
    finish_turn(&r, run1["turn_id"].as_str().unwrap()).await;
    let (s, run2) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(s, 201, "{run2}");
    let c2: Uuid = run2["conversation_id"].as_str().unwrap().parse().unwrap();
    assert_ne!(c1, c2);
    let origins: Vec<Option<Uuid>> = sqlx::query_scalar(
        "SELECT origin_conversation_id FROM conversations
         WHERE automation_kind = 'schedule' AND automation_id = $1::uuid",
    )
    .bind(&id)
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    assert_eq!(origins, vec![Some(origin), Some(origin)]);
    let run_conv: Option<Uuid> =
        sqlx::query_scalar("SELECT run_conversation_id FROM schedules WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(run_conv, Some(c2), "points at the latest thread");

    // Runs in words, with their threads.
    let (_, runs) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}/runs"), None)
        .await;
    let fired: Vec<&Value> = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["outcome"] == "fired")
        .collect();
    assert_eq!(fired.len(), 2);
    assert_eq!(
        (
            fired[0]["result"].as_str(),
            fired[0]["conversation_id"].as_str()
        ),
        (Some("running"), Some(c2.to_string().as_str()))
    );
    assert_eq!(
        (
            fired[1]["result"].as_str(),
            fired[1]["conversation_id"].as_str()
        ),
        (Some("answered"), Some(c1.to_string().as_str()))
    );
    let (_, sv) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(sv["last_result"], "running");

    // Paused: no next runs.
    let (_, paused) = r
        .user_call(
            Method::PATCH,
            &format!("/v1/schedules/{id}"),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(paused["next_runs"], json!([]));

    // A rename follows into the threads; a delete marks them and keeps the transcripts.
    let (s, _) = r
        .user_call(
            Method::PATCH,
            &format!("/v1/schedules/{id}"),
            Some(json!({"name": "Evening digest"})),
        )
        .await;
    assert_eq!(s, 200);
    let (_, list) = r
        .user_call(
            Method::GET,
            &format!("/v1/agents/{agent}/conversations?automation=only&automation_id={id}"),
            None,
        )
        .await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for c in items {
        assert_eq!(c["title"], "Evening digest");
        assert_eq!(c["automation"]["name"], "Evening digest");
        assert_eq!(c["automation"]["deleted"], false);
    }
    let (_, main) = r
        .user_call(
            Method::GET,
            &format!("/v1/agents/{agent}/conversations?automation=none"),
            None,
        )
        .await;
    assert!(
        main["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["automation"].is_null())
    );
    let (_, folders) = r
        .user_call(
            Method::GET,
            &format!("/v1/agents/{agent}/conversation-folders"),
            None,
        )
        .await;
    assert_eq!(folders["items"][0]["name"], "Evening digest");
    assert_eq!(folders["items"][0]["thread_count"], 2);
    assert_eq!(folders["items"][0]["last_result"], "running");
    finish_turn(&r, run2["turn_id"].as_str().unwrap()).await;
    let (s, _) = r
        .user_call(Method::DELETE, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(s, 204);
    let (_, c) = r
        .user_call(Method::GET, &format!("/v1/conversations/{c1}"), None)
        .await;
    assert_eq!(c["automation"]["deleted"], true);
    assert_eq!(c["automation"]["name"], "Evening digest");
    let marked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversations WHERE automation_id = $1::uuid
           AND automation_deleted_at IS NOT NULL",
    )
    .bind(&id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(marked, 2);
    assert!(!r.events(c1).await.is_empty(), "the transcript stays");
}

/// (C-1077) Runs in words for a gate-quiet run, a failed run and a frozen skip.
#[tokio::test]
async fn runs_say_what_happened_quiet_failed_and_frozen() {
    let r = rig().await;
    let lv = r.boot("words").await;
    r.finish_hello(&lv).await;
    let v = r
        .sched(
            lv.agent,
            json!({"name": "g", "prompt": "check", "cron": "0 * * * *", "gate_command": "./c.sh"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    let d = r.fire_and_deliver(&id).await;
    let t1 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t1, "quiet", "").await, "applied");
    let d = r.fire_and_deliver(&id).await;
    let t2 = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": t2}))
        .await;
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": t2, "status": "failed",
               "error_code": "provider_overloaded", "error_message": "overloaded"}),
    )
    .await;
    sqlx::query("UPDATE agents SET frozen_at = now() WHERE id = $1")
        .bind(lv.agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    r.due(&id).await;
    r.work().await;
    let (_, runs) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}/runs"), None)
        .await;
    let words: Vec<(String, Option<String>, String)> = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["outcome"].as_str().unwrap().to_owned(),
                x["reason"].as_str().map(str::to_owned),
                x["result"].as_str().unwrap().to_owned(),
            )
        })
        .filter(|x| x.0 != "coalesced")
        .collect();
    assert_eq!(
        words,
        vec![
            (
                "skipped_frozen".into(),
                Some("frozen".into()),
                "skipped".into()
            ),
            (
                "fired".into(),
                Some("provider_overloaded".into()),
                "failed".into()
            ),
            ("gated_quiet".into(), None, "skipped_by_gate".into()),
        ]
    );
    for x in runs["items"].as_array().unwrap() {
        if x["outcome"] == "coalesced" {
            assert_eq!(
                (x["result"].as_str(), x["reason"].as_str()),
                (Some("skipped"), Some("missed"))
            );
        }
    }
    let (_, sv) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(sv["last_result"], "skipped");
}

/// (C-1077) The preview: three instants, the 5-minute minimum as a flag, field errors.
#[tokio::test]
async fn preview_says_the_next_runs_and_the_five_minute_minimum() {
    let r = rig().await;
    let agent = r.agent("preview", true).await;
    let uri = format!("/v1/agents/{agent}/schedules/preview");
    let (s, v) = r
        .user_call(
            Method::POST,
            &uri,
            Some(json!({"cron": "*/5 * * * *", "timezone": "Europe/Zurich"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["next"].as_array().unwrap().len(), 3);
    assert_eq!(v["interval_secs"], 300);
    assert_eq!(v["too_frequent"], false);
    assert_eq!(v["min_interval_secs"], 300);
    for cron in ["* * * * *", "*/4 * * * *"] {
        let (s, v) = r
            .user_call(Method::POST, &uri, Some(json!({"cron": cron})))
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["too_frequent"], true, "{cron}");
    }
    let (s, v) = r
        .user_call(Method::POST, &uri, Some(json!({"cron": "0 9 * *"})))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("invalid_cron"))
    );
    let (s, v) = r
        .user_call(
            Method::POST,
            &uri,
            Some(json!({"cron": "0 9 * * *", "timezone": "Mars/Base"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("invalid_timezone"))
    );
    // Nothing was saved.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM schedules WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

/// (C-1074) The volume guard keeps an automation's newest threads (lowered here) and deletes
/// the rest through the ordinary delete path; their runs then answer `conversation_id: null`.
#[tokio::test]
async fn the_volume_guard_keeps_the_newest_threads() {
    let r = rig().await;
    let agent = r.agent("volume", true).await;
    let v = r
        .sched(
            agent,
            json!({"name": "v", "prompt": "p", "cron": "0 9 * * *"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    let mut threads = Vec::new();
    for _ in 0..4 {
        let (s, run) = r
            .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
            .await;
        assert_eq!(s, 201, "{run}");
        finish_turn(&r, run["turn_id"].as_str().unwrap()).await;
        threads.push(run["conversation_id"].as_str().unwrap().to_owned());
    }
    let n = aurora_api::domains::conversations::lifecycle::sweep_automation_overflow(r.h.pool(), 2)
        .await
        .unwrap();
    assert_eq!(n, 2);
    let left: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE automation_id = $1::uuid ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    let left: Vec<String> = left.iter().map(Uuid::to_string).collect();
    assert_eq!(left, threads[2..].to_vec(), "the newest two stay");
    let (_, runs) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}/runs"), None)
        .await;
    let convs: Vec<Option<&str>> = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["conversation_id"].as_str())
        .collect();
    assert_eq!(
        convs,
        vec![
            Some(threads[3].as_str()),
            Some(threads[2].as_str()),
            None,
            None
        ]
    );
    // An audit row names the reason (written in the delete's transaction).
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.delete'
           AND metadata->>'reason' = 'automation_threads' AND (metadata->>'kept')::int = 2
           AND metadata->'conversation_ids' @> to_jsonb(ARRAY[$1::text, $2::text])",
    )
    .bind(&threads[0])
    .bind(&threads[1])
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(audited, 1);
    // Again: nothing more to do.
    assert_eq!(
        aurora_api::domains::conversations::lifecycle::sweep_automation_overflow(r.h.pool(), 2)
            .await
            .unwrap(),
        0
    );
}

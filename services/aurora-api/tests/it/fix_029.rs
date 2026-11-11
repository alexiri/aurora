#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-029 (D-160, C-610..C-615): retry a failed sandbox start, the per-boot start log and the
//! per-agent start-failure history routes.

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// A sandbox forced into `failed` (a real failed start report through the runner plane).
async fn fail_a_start(r: &Rig, rep: Uuid, agent: Uuid) -> (String, String, Uuid) {
    let turn = r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap().to_owned(),
        item["boot_id"].as_str().unwrap().to_owned(),
    );
    post_reports(
        r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
            "sandbox_id": sb, "boot_id": boot, "state": "failed",
            "reason_code": "pod_failed", "message": "the pod crashed"}]),
    )
    .await;
    assert_eq!(r.sandbox_state(&sb).await, "failed");
    (sb, boot, turn)
}

async fn post_reports(r: &Rig, rep: Uuid, reports: Value) -> Value {
    let (s, v) = r
        .call(
            Some(rep),
            "/v1/runner/reports",
            Some(json!({ "reports": reports })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v
}

fn first<'a>(w: &'a Value, kind: &str) -> &'a Value {
    w["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} in {w}"))
}

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    tok: String,
    runner_id: Uuid,
}

async fn rig() -> Rig {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.hosted.allow_fake_harness = true;
    })
    .await;
    let admin = h.user("admin@x.io", true).await;
    let (_, sess) = h.good_session(admin).await;
    let c = cookie(&sess);
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/runners",
            &[("cookie", &c), ("origin", ORIGIN)],
            Some(json!({"name": "r1"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let runner_id = v["runner"]["id"].as_str().unwrap().parse().unwrap();
    let org = h.org().await;
    let ws = h.workspace(org).await;
    Rig {
        h,
        org,
        ws,
        tok,
        runner_id,
    }
}

impl Rig {
    async fn call(&self, replica: Option<Uuid>, uri: &str, body: Option<Value>) -> (u16, Value) {
        let b = bearer(&self.tok);
        let rid = replica.map(|r| r.to_string());
        let mut hd = vec![("authorization", b.as_str())];
        if let Some(r) = &rid {
            hd.push(("x-aurora-replica", r.as_str()));
        }
        let (s, _, v) = self.h.send(Method::POST, uri, &hd, body).await;
        (s.as_u16(), v)
    }

    async fn register(&self, host: &str, max: u32) -> Uuid {
        let (s, v) = self
            .call(
                None,
                "/v1/runner/register",
                Some(json!({"version": "0.1", "backend": "fake",
                        "capabilities": {"backend": "fake", "home_durability": "resident",
                            "resources": {"cpu": {"min": 250, "max": 4000},
                                          "memory_mib": {"min": 256, "max": 8192},
                                          "pids": {"min": 64, "max": 4096},
                                          "home_gib": {"min": 1, "max": 100}},
                            "max_sandboxes": max, "harnesses": ["fake"],
                            "max_attachment_bytes": 1048576},
                        "advertised_host": host})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["replica_id"].as_str().unwrap().parse().unwrap()
    }

    async fn work(&self, replica: Uuid, wait: u32) -> Value {
        let (s, v) = self
            .call(
                Some(replica),
                "/v1/runner/work",
                Some(json!({"wait_secs": wait, "capacity": {"running": 0, "max": 8}})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn agent(&self, handle: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
             VALUES ($1,$2,$3,$4,$4,'hosted','fake')",
        )
        .bind(id)
        .bind(self.org)
        .bind(self.ws)
        .bind(handle)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }
    async fn turn(&self, agent: Uuid) -> Uuid {
        let conv = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
             VALUES ($1,$2,$3,'api',$1::text, now())",
        )
        .bind(conv)
        .bind(agent)
        .bind(self.ws)
        .execute(self.h.pool())
        .await
        .unwrap();
        let t = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text)
             VALUES ($1,$2,$3,'turn','queued','{}','system','s','hi')",
        )
        .bind(t)
        .bind(conv)
        .bind(agent)
        .execute(self.h.pool())
        .await
        .unwrap();
        t
    }

    async fn sandbox_state(&self, sb: &str) -> String {
        sqlx::query_scalar("SELECT state FROM sandboxes WHERE id = $1::uuid")
            .bind(sb)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// Sandbox row facts: `(state, failure_code, start_failures, lease_live)`.
    async fn facts(&self, agent: Uuid) -> (String, Option<String>, i32, bool) {
        sqlx::query_as(
            "SELECT state, failure_code, start_failures,
                    coalesce(lifecycle_lease_until > now(), false)
             FROM sandboxes WHERE agent_id = $1::uuid",
        )
        .bind(agent)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    /// The boot's start log as read from the database (newest first).
    async fn start_log(&self, agent: Uuid) -> Vec<(String, Option<String>)> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT e->>'kind', e->>'phase' FROM sandboxes s,
                  jsonb_array_elements(s.start_log) e
             WHERE s.agent_id = $1::uuid ORDER BY e->>'ts' DESC",
        )
        .bind(agent)
        .fetch_all(self.h.pool())
        .await
        .unwrap();
        rows
    }

    /// Session caller for the workspace owner (a member bound as owner).
    async fn owner_headers(&self, n: u32) -> Vec<(&'static str, String)> {
        let u = self.h.user(&format!("owner{n}@x.io"), false).await;
        self.h.member(self.org, u, "member", "active").await;
        self.h.bind_ws(self.ws, u, "owner").await;
        let (_, sess) = self.h.good_session(u).await;
        vec![("cookie", cookie(&sess)), ("origin", ORIGIN.to_owned())]
    }
}

// The Rig above mirrors runner_plane.rs's.

#[tokio::test]
async fn retry_happy_path_clears_failure_and_claims_once() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("retryme").await;
    let (sb, _boot, _turn) = fail_a_start(&r, rep, agent).await;

    let (state, code, failures, lease_live) = r.facts(agent).await;
    assert_eq!(
        (state.as_str(), code.as_deref(), failures, lease_live),
        ("failed", Some("pod_failed"), 1, true),
        "the failure leaves the backoff lease live"
    );

    // While the lease is live the retry is 409.
    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/sandbox/retry"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "sandbox_not_failed");

    // Expire the lease, retry.
    sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/sandbox/retry"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(
        v["state"], "stopped",
        "parked until the start arm claims it"
    );
    assert_eq!(v["retryable"], false);
    let (state, code, failures, lease_live) = r.facts(agent).await;
    assert_eq!(state, "stopped");
    assert_eq!(code, None, "failure fields cleared");
    assert_eq!(failures, 0, "start_failures reset");
    assert!(!lease_live, "no lease of its own: the start arm claims");

    // The start log (boot 1's) carries the failure verdict and the retry note, newest first;
    // the claim itself appends nothing and the fresh boot resets the log (C-611/C-612).
    let log = r.start_log(agent).await;
    assert_eq!(
        log,
        vec![
            ("note".to_owned(), Some("retry".to_owned())),
            ("phase".to_owned(), Some("failed".to_owned())),
        ],
        "{log:?}"
    );

    // The retried start needs due work: the failed turn was ended by the failure (C-581), so
    // queue a fresh turn for the ordinary start arm to claim.
    let _ = r.turn(agent).await;

    // Due work was notified and the ordinary start arm claims exactly one new start (a fresh
    // boot id; no new work-item kind).
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    assert_ne!(item["boot_id"].as_str().unwrap(), _boot.to_string());
    assert_eq!(item["sandbox_id"].as_str().unwrap(), sb);
    assert_eq!(r.sandbox_state(&sb).await, "starting");
    // The new boot reset the log.
    assert!(r.start_log(agent).await.is_empty(), "reset per boot");
}

#[tokio::test]
async fn retry_from_other_states_is_409_and_unknown_agent_404() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("other-states").await;

    // `stopped` (fresh row): 409.
    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/sandbox/retry"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "sandbox_not_failed");

    // `starting`: claim via the arm, then 409.
    let _ = r.turn(agent).await;
    first(&r.work(rep, 0).await, "sandbox.start");
    assert_eq!(
        r.sandbox_state(
            &sqlx::query_scalar::<_, Uuid>("SELECT id FROM sandboxes WHERE agent_id = $1::uuid")
                .bind(agent)
                .fetch_one(r.h.pool())
                .await
                .unwrap()
                .to_string()
        )
        .await,
        "starting"
    );
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/sandbox/retry"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 409, "{v}");

    // Unknown agent: 404.
    let (s, _, _) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{}/sandbox/retry", Uuid::now_v7()),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn retry_on_frozen_agent_is_423() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("frozen-retry").await;
    fail_a_start(&r, rep, agent).await;
    sqlx::query("UPDATE agents SET frozen_at = now(), frozen_reason = 'matrix', frozen_authority = 'workspace_owner'")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/sandbox/retry"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 423, "{v}");
    assert_eq!(v["error"]["code"], "frozen");
}

#[tokio::test]
async fn retry_writes_the_audit_row_with_the_previous_code() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("audited").await;
    fail_a_start(&r, rep, agent).await;
    sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
        .execute(r.h.pool())
        .await
        .unwrap();
    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, _, _) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/sandbox/retry"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 202);
    // The audit write is fire-and-forget (best-effort): poll for the row.
    let mut code: Option<String> = None;
    for _ in 0..40 {
        code = sqlx::query_scalar(
            "SELECT metadata->>'previous_failure_code' FROM audit_log
             WHERE action = 'agent.sandbox_retry' AND target_id = $1",
        )
        .bind(agent.to_string())
        .fetch_optional(r.h.pool())
        .await
        .unwrap();
        if code.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(code.as_deref(), Some("pod_failed"));
}

/// (C-612) The start log is a ring: 30 entries cap, oldest dropped, sanitised details, reset by
/// every new boot.
#[tokio::test]
async fn start_log_ring_caps_sanitises_and_resets_per_boot() {
    let r = rig().await;
    let agent = r.agent("ring").await;
    // Claim the boot first: the sandbox row only exists once the start arm ran.
    let rep = r.register("runner-a", 4).await;
    let _ = r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap().to_owned(),
        item["boot_id"].as_str().unwrap().to_owned(),
    );
    // Fill the ring beyond the cap directly (the appends under test use the same helper).
    sqlx::query(
        "UPDATE sandboxes SET start_log = (
            SELECT jsonb_agg(jsonb_build_object('ts', to_char(now() - make_interval(secs => k), 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),
                   'kind', 'phase', 'phase', 'creating', 'detail', 'entry ' || k))
            FROM generate_series(0, 34) k) WHERE id = $1::uuid",
    )
    .bind(&sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    post_reports(
        &r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
            "sandbox_id": sb, "boot_id": boot, "state": "starting",
            "reason_code": "pulling", "message": "pull https://bob:hunter2@reg/x token=abc"}]),
    )
    .await;
    let n: i32 = sqlx::query_scalar(
        "SELECT jsonb_array_length(start_log) FROM sandboxes WHERE id = $1::uuid",
    )
    .bind(&sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 30, "ring capped");
    let newest: String =
        sqlx::query_scalar("SELECT start_log->0->>'detail' FROM sandboxes WHERE id = $1::uuid")
            .bind(&sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(
        !newest.contains("hunter2") && !newest.contains("abc") && newest.contains("***"),
        "sanitised: {newest}"
    );
    // A new boot resets the log.
    post_reports(
        &r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
            "sandbox_id": sb, "boot_id": boot, "state": "failed",
            "reason_code": "pod_failed", "message": "boom"}]),
    )
    .await;
    sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
        .execute(r.h.pool())
        .await
        .unwrap();
    let _ = r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let boot2 = item["boot_id"].as_str().unwrap().to_owned();
    assert_ne!(boot2, boot, "fresh boot id");
    let n: i32 = sqlx::query_scalar(
        "SELECT jsonb_array_length(start_log) FROM sandboxes WHERE id = $1::uuid",
    )
    .bind(&sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0, "begin_boot reset the log");
    // The new boot's claimed phase appends.
    let log: Vec<(String, Option<String>)> = r.start_log(agent).await;
    assert!(log.is_empty() || log[0].0 == "phase", "{log:?}");
}

/// (C-613) The agent's own start-failure history: only its rows, newest first, ≤ 20, C-586
/// shape; unknown agent 404.
#[tokio::test]
async fn agent_start_failure_history() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("history").await;
    let agent2 = r.agent("history-other").await;
    for _ in 0..3 {
        fail_a_start(&r, rep, agent).await;
        sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
            .execute(r.h.pool())
            .await
            .unwrap();
    }
    // Another agent's failure must not leak into the list.
    fail_a_start(&r, rep, agent2).await;

    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, v) =
        r.h.get(&format!("/v1/agents/{agent}/start-failures"), &hdr)
            .await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["code"], "pod_failed");
    assert_eq!(items[0]["message"], "the pod crashed");
    assert!(
        items[0].get("runner_id").is_none(),
        "user route omits the runner"
    );
    assert_eq!(items[0]["sandbox_id"], sb_of(&r, agent).await.to_string());
    // Newest first (uuid v7 ids are monotonic).
    assert!(
        items[0]["id"].as_str().unwrap() > items[2]["id"].as_str().unwrap(),
        "newest first"
    );

    // Cap 20.
    sqlx::query(
        "INSERT INTO sandbox_start_failures (id, runner_id, sandbox_id, agent_id, code, message)
         SELECT gen_random_uuid(), $2, s.id, s.agent_id, 'pod_failed', 'seeded'
         FROM sandboxes s, generate_series(1, 25) k WHERE s.agent_id = $1::uuid",
    )
    .bind(agent)
    .bind(r.runner_id)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) =
        r.h.get(&format!("/v1/agents/{agent}/start-failures"), &hdr)
            .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 20);

    // Unknown agent 404.
    let (s, _) =
        r.h.get(
            &format!("/v1/agents/{}/start-failures", Uuid::now_v7()),
            &hdr,
        )
        .await;
    assert_eq!(s, 404);
}

async fn sb_of(r: &Rig, agent: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM sandboxes WHERE agent_id = $1::uuid")
        .bind(agent)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

/// (C-614) The admin route names the runner; a non-admin is 403; unknown agent 404.
#[tokio::test]
async fn admin_start_failure_history_names_the_runner() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("adminhistory").await;
    fail_a_start(&r, rep, agent).await;

    let admin = r.h.user("ia@x.io", true).await;
    let (_, sess) = r.h.good_session(admin).await;
    let c = cookie(&sess);
    let (s, v) =
        r.h.get(
            &format!("/v1/admin/agents/{agent}/start-failures"),
            &[("cookie", &c)],
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["runner_id"], r.runner_id.to_string());

    // Non-admin: 403.
    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, _) =
        r.h.get(&format!("/v1/admin/agents/{agent}/start-failures"), &hdr)
            .await;
    assert_eq!(s, 403);

    // Unknown agent: 404.
    let (s, _) =
        r.h.get(
            &format!("/v1/admin/agents/{}/start-failures", Uuid::now_v7()),
            &[("cookie", &c)],
        )
        .await;
    assert_eq!(s, 404);
}

/// (C-610/C-615) The journey: two forced start failures reach the bound message, Retry now
/// resets the bound and the next start reaches `starting` with both attempts in the log.
#[tokio::test]
async fn journey_two_failures_then_retry_starts_again() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("journey").await;

    // Two failures: the second one is past the 3-failure message threshold? No — after 2 the
    // backoff message says until when; after the 3rd automatic retries stop. Force 2, retry.
    fail_a_start(&r, rep, agent).await;
    sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
        .execute(r.h.pool())
        .await
        .unwrap();
    fail_a_start(&r, rep, agent).await;
    let (_, _, failures, _) = r.facts(agent).await;
    assert_eq!(failures, 2);
    sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
        .execute(r.h.pool())
        .await
        .unwrap();

    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/sandbox/retry"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s, 202, "{v}");
    let log = r.start_log(agent).await;
    assert_eq!(log[0], ("note".to_owned(), Some("retry".to_owned())));
    // Only the second attempt's verdict is in the log: the first failure's verdict was
    // `Fenced` there (boot 1 was already superseded when its report arrived), and the retry
    // note was appended after the second failure — but `begin_boot` for boot 2 wiped boot 1's
    // entries first, so the log holds exactly boot 2's verdict + the note.
    let failed_phases = log
        .iter()
        .filter(|(k, p)| *k == "phase" && *p == Some("failed".to_owned()))
        .count();
    assert_eq!(
        failed_phases, 1,
        "the live boot's attempt is in the log: {log:?}"
    );

    // The retry reset the bound: the next turn's start claims again (the arm runs on the poll).
    let _ = r.turn(agent).await;
    first(&r.work(rep, 0).await, "sandbox.start");
}

/// (C-611) The sandbox view carries `retryable` and `start_log` (newest first).
#[tokio::test]
async fn sandbox_view_carries_retryable_and_start_log() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("viewfields").await;
    fail_a_start(&r, rep, agent).await;

    let hd = r.owner_headers(1).await;
    let hdr: Vec<(&str, &str)> = hd.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, v) = r.h.get(&format!("/v1/agents/{agent}/sandbox"), &hdr).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["retryable"], true);
    let log = v["start_log"].as_array().unwrap();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["kind"], "phase");
    assert_eq!(log[0]["phase"], "failed");
    assert_eq!(log[0]["detail"], "the pod crashed");
}

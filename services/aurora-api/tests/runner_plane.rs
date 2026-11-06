#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use aurora_core::token;
use base64::Engine;
use common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    tok: String,
    runner_id: Uuid,
}

fn caps(max: u32) -> Value {
    json!({
        "backend": "fake", "home_durability": "resident",
        "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                      "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
        "max_sandboxes": max, "harnesses": ["fake"], "max_attachment_bytes": 1048576
    })
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
    assert!(tok.starts_with("aur_"));
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
        let m = if body.is_some() || !uri.ends_with("/sandboxes") {
            Method::POST
        } else {
            Method::GET
        };
        let (s, _, v) = self.h.send(m, uri, &hd, body).await;
        (s.as_u16(), v)
    }

    async fn register(&self, host: &str, max: u32) -> Uuid {
        let (s, v) = self
            .call(
                None,
                "/v1/runner/register",
                Some(
                    json!({"version": "0.1", "backend": "fake", "capabilities": caps(max),
                            "advertised_host": host}),
                ),
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

    /// Hosted agent with a queued turn; returns (agent, conversation).
    async fn agent_with_turn(&self, handle: &str) -> Uuid {
        let agent = self.agent(handle).await;
        self.turn(agent).await;
        agent
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

    async fn report(&self, replica: Uuid, rid: Uuid, sb: &str, boot: &str, state: &str) -> Value {
        let (s, v) = self
            .call(
                Some(replica),
                "/v1/runner/reports",
                Some(
                    json!({"reports": [{"report_id": rid, "kind": "sandbox.status",
                    "sandbox_id": sb, "boot_id": boot, "state": state, "container_ref": "c1"}]}),
                ),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn sandbox_state(&self, sb: &str) -> String {
        sqlx::query_scalar("SELECT state FROM sandboxes WHERE id = $1::uuid")
            .bind(sb)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn first<'a>(w: &'a Value, kind: &str) -> &'a Value {
    w["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} in {w}"))
}

fn boot_doc(item: &Value) -> Value {
    let raw = item["spec"]["boot_secret"].as_str().unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn admin_provision_register_and_list() {
    let r = rig().await;
    // Unauthenticated / wrong token / missing replica.
    let (s, _, _) =
        r.h.send(Method::POST, "/v1/runner/work", &[], Some(json!({})))
            .await;
    assert_eq!(s, 401);
    let rep = r.register("runner-a", 4).await;
    let caps_db: Value = sqlx::query_scalar("SELECT capabilities FROM runners WHERE id = $1")
        .bind(r.runner_id)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(caps_db["max_sandboxes"], 4);
    let (s, _) = r
        .call(
            None,
            "/v1/runner/work",
            Some(json!({"wait_secs":0,"capacity":{"running":0,"max":1}})),
        )
        .await;
    assert_eq!(s, 403, "work without replica header");
    let _ = rep;

    let admin = r.h.user("a2@x.io", true).await;
    let (_, sess) = r.h.good_session(admin).await;
    let c = cookie(&sess);
    let (s, _, v) =
        r.h.send(Method::GET, "/v1/admin/runners", &[("cookie", &c)], None)
            .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["live_replicas"], 1);
    assert_eq!(v["items"][0]["replicas"][0]["advertised_host"], "runner-a");
    assert!(v["items"][0].get("token").is_none());
    // Rotate: old token dies, new works once.
    let id = r.runner_id;
    let (s, _, v2) =
        r.h.send(
            Method::POST,
            &format!("/v1/admin/runners/{id}/token"),
            &[("cookie", &c), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 200, "{v2}");
    let (s, _) = r.call(Some(rep), "/v1/runner/retire", None).await;
    assert_eq!(s, 401);
    // Duplicate name conflicts.
    let (s, _, _) =
        r.h.send(
            Method::POST,
            "/v1/admin/runners",
            &[("cookie", &c), ("origin", ORIGIN)],
            Some(json!({"name":"r1"})),
        )
        .await;
    assert_eq!(s, 409);
}

#[tokio::test]
async fn start_item_has_valid_boot_document_and_tokens_match_db() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent_with_turn("daneel").await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let sb = item["sandbox_id"].as_str().unwrap();
    let boot = item["boot_id"].as_str().unwrap();
    let doc = boot_doc(item);
    assert_eq!(doc["v"], 1);
    assert_eq!(doc["boot_id"], boot);
    assert_eq!(doc["agent"]["handle"], "daneel");
    assert_eq!(doc["runner_url"], "ws://runner-a:4105/v1/supervisor");
    assert!(
        doc["ca_pem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE")
    );
    assert_eq!(doc["harness"]["id"], "fake");
    let bootstrap = doc["bootstrap_token"].as_str().unwrap();
    let boot_tok = doc["gateway"]["boot_token"].as_str().unwrap();
    assert!(bootstrap.starts_with("aup_") && boot_tok.starts_with("aub_"));
    assert!(!doc["gateway"]["proxy"].as_str().unwrap().contains("://"));

    let (h, owner, state): (Vec<u8>, Uuid, String) = sqlx::query_as(
        "SELECT bootstrap_token_hash, owner_replica_id, state FROM sandboxes WHERE id = $1::uuid",
    )
    .bind(sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(h, token::hash(bootstrap).to_vec());
    assert_eq!(owner, rep);
    assert_eq!(state, "starting");
    let (kind, aid, cboot, revoked): (String, Uuid, Option<Uuid>, bool) = sqlx::query_as(
        "SELECT kind, agent_id, boot_id, revoked_at IS NOT NULL FROM gateway_credentials WHERE token_hash = $1",
    )
    .bind(&token::hash(boot_tok)[..])
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!((kind.as_str(), aid, revoked), ("boot", agent, false));
    assert_eq!(cboot.unwrap().to_string(), boot);
    // Nothing more is due.
    assert!(r.work(rep, 0).await["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_pollers_never_receive_the_same_start() {
    let r = rig().await;
    let a = r.register("runner-a", 8).await;
    let b = r.register("runner-b", 8).await;
    for i in 0..3 {
        r.agent_with_turn(&format!("bot{i}")).await;
    }
    let (w1, w2, w3, w4) = tokio::join!(r.work(a, 0), r.work(b, 0), r.work(a, 0), r.work(b, 0));
    let mut ids = vec![];
    for w in [w1, w2, w3, w4] {
        for i in w["items"].as_array().unwrap() {
            assert_eq!(i["kind"], "sandbox.start");
            ids.push(i["sandbox_id"].as_str().unwrap().to_owned());
        }
    }
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(n, 3, "all three started");
    assert_eq!(ids.len(), 3, "no duplicates");
}

#[tokio::test]
async fn long_poll_wakes_on_work_notify() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let started = std::time::Instant::now();
    let poll = r.work(rep, 20);
    let feed = async {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        r.agent_with_turn("late").await;
        sqlx::query("SELECT pg_notify('aurora_work', '{}')")
            .execute(r.h.pool())
            .await
            .unwrap();
    };
    let (w, ()) = tokio::join!(poll, feed);
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    first(&w, "sandbox.start");
}

#[tokio::test]
async fn idle_stop_then_stopped_report_revokes_boot_token() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent_with_turn("idler").await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap(),
        item["boot_id"].as_str().unwrap(),
    );
    let doc = boot_doc(item);
    let boot_hash = token::hash(doc["gateway"]["boot_token"].as_str().unwrap());
    let v = r.report(rep, Uuid::now_v7(), sb, boot, "running").await;
    assert_eq!(v["results"][0]["status"], "applied");
    assert_eq!(r.sandbox_state(sb).await, "running");
    let open: i64 =
        sqlx::query_scalar("SELECT count(*) FROM runtime_intervals WHERE ended_at IS NULL")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(open, 1);

    // Active turn keeps it awake even when old.
    sqlx::query("UPDATE sandboxes SET last_engaged_at = now() - interval '3 hours', last_active_at = now() - interval '3 hours'")
        .execute(r.h.pool()).await.unwrap();
    assert!(r.work(rep, 0).await["items"].as_array().unwrap().is_empty());
    sqlx::query("UPDATE turns SET status = 'done' WHERE agent_id = $1")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    let w = r.work(rep, 0).await;
    let stop = first(&w, "sandbox.stop");
    assert_eq!(stop["reason"], "idle");
    assert_eq!(stop["boot_id"], boot);
    assert_eq!(r.sandbox_state(sb).await, "stopping");

    let v = r.report(rep, Uuid::now_v7(), sb, boot, "stopped").await;
    assert_eq!(v["results"][0]["status"], "applied");
    assert_eq!(r.sandbox_state(sb).await, "stopped");
    let revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM gateway_credentials WHERE token_hash = $1",
    )
    .bind(&boot_hash[..])
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(revoked);
    let open: i64 =
        sqlx::query_scalar("SELECT count(*) FROM runtime_intervals WHERE ended_at IS NULL")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(open, 0);
    // A new turn wakes it with a fresh boot.
    r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let again = first(&w, "sandbox.start");
    assert_ne!(again["boot_id"], boot);
}

#[tokio::test]
async fn restart_request_stops_running_sandbox() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    r.agent_with_turn("rs").await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap(),
        item["boot_id"].as_str().unwrap(),
    );
    r.report(rep, Uuid::now_v7(), sb, boot, "running").await;
    sqlx::query("UPDATE sandboxes SET restart_requested_at = now()")
        .execute(r.h.pool())
        .await
        .unwrap();
    let w = r.work(rep, 0).await;
    assert_eq!(first(&w, "sandbox.stop")["reason"], "restart");
}

#[tokio::test]
async fn ownership_fencing_takeover_and_duplicates() {
    let r = rig().await;
    let a = r.register("runner-a", 4).await;
    let b = r.register("runner-b", 4).await;
    r.agent_with_turn("own").await;
    let w = r.work(a, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap(),
        item["boot_id"].as_str().unwrap(),
    );

    // Non-owner report is fenced.
    let v = r.report(b, Uuid::now_v7(), sb, boot, "running").await;
    assert_eq!(v["results"][0]["status"], "fenced");
    assert_eq!(r.sandbox_state(sb).await, "starting");

    // Owner report applies; replay of the same id is a duplicate.
    let rid = Uuid::now_v7();
    assert_eq!(
        r.report(a, rid, sb, boot, "running").await["results"][0]["status"],
        "applied"
    );
    assert_eq!(
        r.report(a, rid, sb, boot, "running").await["results"][0]["status"],
        "duplicate"
    );
    // Same transition under a new id: state-based duplicate.
    assert_eq!(
        r.report(a, Uuid::now_v7(), sb, boot, "running").await["results"][0]["status"],
        "duplicate"
    );

    // Reconciler: oldest live replica only.
    assert_eq!(r.work(a, 0).await["reconciler"], true);
    assert_eq!(r.work(b, 0).await["reconciler"], false);

    // Owner dies: B takes over and may then report.
    sqlx::query(
        "UPDATE runner_replicas SET last_seen_at = now() - interval '10 minutes' WHERE id = $1",
    )
    .bind(a)
    .execute(r.h.pool())
    .await
    .unwrap();
    assert_eq!(r.work(b, 0).await["reconciler"], true);
    let owner: Uuid =
        sqlx::query_scalar("SELECT owner_replica_id FROM sandboxes WHERE id = $1::uuid")
            .bind(sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(owner, b);
    assert_eq!(
        r.report(b, Uuid::now_v7(), sb, boot, "stopped").await["results"][0]["status"],
        "applied"
    );
    // Stale boot id from the old owner is fenced too.
    assert_eq!(
        r.report(
            b,
            Uuid::now_v7(),
            sb,
            &Uuid::now_v7().to_string(),
            "running"
        )
        .await["results"][0]["status"],
        "fenced"
    );
}

#[tokio::test]
async fn channel_claim_is_single_use_and_hold_is_fenced() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    r.agent_with_turn("chan").await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap(),
        item["boot_id"].as_str().unwrap(),
    );
    let tok = boot_doc(item)["bootstrap_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (s, v) = r
        .call(
            Some(rep),
            "/v1/runner/channel/claim",
            Some(json!({"bootstrap_token": tok})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["sandbox_id"].as_str().unwrap(),
            v["boot_id"].as_str().unwrap()
        ),
        (sb, boot)
    );
    let (s, v) = r
        .call(
            Some(rep),
            "/v1/runner/channel/claim",
            Some(json!({"bootstrap_token": tok})),
        )
        .await;
    assert_eq!(s, 409);
    assert_eq!(v["error"]["code"], "bootstrap_spent");
    let (s, _) = r
        .call(
            Some(rep),
            "/v1/runner/channel/claim",
            Some(json!({"bootstrap_token": "aup_nope"})),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = r
        .call(
            Some(rep),
            "/v1/runner/channel/hold",
            Some(json!({"sandbox_id": sb, "boot_id": boot})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = r
        .call(
            Some(rep),
            "/v1/runner/channel/hold",
            Some(json!({"sandbox_id": sb, "boot_id": Uuid::now_v7()})),
        )
        .await;
    assert_eq!(s, 409);
    assert_eq!(v["error"]["code"], "fenced");
    let (s, _) = r
        .call(
            Some(rep),
            "/v1/runner/channel/release",
            Some(json!({"sandbox_id": sb, "boot_id": boot})),
        )
        .await;
    assert_eq!(s, 200);
    // Reconcile calls and placeholders.
    let (s, v) = r.call(Some(rep), "/v1/runner/sandboxes", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["sandboxes"][0]["desired"], "running");
    let (s, v) = r
        .call(
            Some(rep),
            "/v1/runner/sandboxes/check",
            Some(json!({"sandbox_ids": [sb, Uuid::now_v7()]})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["known"].as_array().unwrap().len(), 1);
    let (s, v) = r
        .call(Some(rep), "/v1/runner/tool-calls", Some(json!({})))
        .await;
    // Implemented by BE-053 (see tests/tools.rs): an empty body is now a 4xx.
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn failed_start_backs_off() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent_with_turn("flaky").await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap(),
        item["boot_id"].as_str().unwrap(),
    );
    assert_eq!(
        r.report(rep, Uuid::now_v7(), sb, boot, "failed").await["results"][0]["status"],
        "applied"
    );
    assert_eq!(r.sandbox_state(sb).await, "failed");
    // The start failure fails the queued turn (10 §Failure semantics); a new one wants a retry.
    let failed: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE status = 'failed'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(failed, 1);
    r.turn(agent).await;
    assert!(
        r.work(rep, 0).await["items"].as_array().unwrap().is_empty(),
        "backoff holds"
    );
    sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
        .execute(r.h.pool())
        .await
        .unwrap();
    first(&r.work(rep, 0).await, "sandbox.start");
}

#[tokio::test]
async fn replayed_reports_through_a_second_app_state_have_no_double_effects() {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let r = rig().await;
    // A second API replica: fresh in-memory state, same database.
    let state2 = aurora_api::app::AppState::new(r.h.pool().clone(), config(|_| {}));
    let app2 = test_router(state2);
    let post2 = |replica: Uuid, body: Value| {
        let app2 = app2.clone();
        let tok = r.tok.clone();
        async move {
            let req = http::Request::builder()
                .method(Method::POST)
                .uri("/v1/runner/reports")
                .header("authorization", bearer(&tok))
                .header("x-aurora-replica", replica.to_string())
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let res = app2.oneshot(req).await.unwrap();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }
    };
    let rep = r.register("runner-a", 4).await;
    r.agent_with_turn("replay").await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap(),
        item["boot_id"].as_str().unwrap(),
    );
    let body = |rid: Uuid, state: &str| {
        json!({"reports": [{"report_id": rid, "kind": "sandbox.status", "sandbox_id": sb,
            "boot_id": boot, "state": state, "container_ref": "c1"}]})
    };
    let (run_id, stop_id) = (Uuid::now_v7(), Uuid::now_v7());
    assert_eq!(
        r.report(rep, run_id, sb, boot, "running").await["results"][0]["status"],
        "applied"
    );
    let v = post2(rep, body(run_id, "running")).await;
    assert_eq!(v["results"][0]["status"], "duplicate");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_intervals")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(r.sandbox_state(sb).await, "running");

    sqlx::query("UPDATE sandboxes SET stop_reason = 'idle', state = 'stopping'")
        .execute(r.h.pool())
        .await
        .unwrap();
    assert_eq!(
        r.report(rep, stop_id, sb, boot, "stopped").await["results"][0]["status"],
        "applied"
    );
    let revoked_at = || async {
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT revoked_at FROM gateway_credentials WHERE kind = 'boot'",
        )
        .fetch_one(r.h.pool())
        .await
        .unwrap()
    };
    let (t1, ended1): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) = (
        revoked_at().await,
        sqlx::query_scalar("SELECT ended_at FROM runtime_intervals")
            .fetch_one(r.h.pool())
            .await
            .unwrap(),
    );
    let v = post2(rep, body(stop_id, "stopped")).await;
    assert_eq!(v["results"][0]["status"], "duplicate");
    // Replaying the older `running` report after the stop must not resurrect anything.
    let v = post2(rep, body(run_id, "running")).await;
    assert_eq!(v["results"][0]["status"], "fenced");
    assert_eq!(r.sandbox_state(sb).await, "stopped");
    assert_eq!(revoked_at().await, t1);
    let ended2: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT ended_at FROM runtime_intervals")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(ended1, ended2);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_intervals")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

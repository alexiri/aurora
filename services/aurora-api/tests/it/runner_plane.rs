#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_core::token;
use base64::Engine;
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
    caps_with(max, None)
}

fn caps_with(max: u32, ceiling: Option<u32>) -> Value {
    let mut v = caps_base(max);
    if let Some(c) = ceiling {
        v["held_awake_ceiling"] = json!(c);
    }
    v
}

fn caps_base(max: u32) -> Value {
    json!({
        "backend": "fake", "home_durability": "resident",
        "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                      "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
        "max_sandboxes": max, "harnesses": ["fake"], "max_attachment_bytes": 1048576
    })
}

async fn rig() -> Rig {
    rig_cfg(|_| {}).await
}

async fn rig_cfg(extra: impl FnOnce(&mut aurora_config::ApiConfig) + 'static) -> Rig {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.hosted.allow_fake_harness = true;
        extra(c);
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
        self.register_with(host, caps(max)).await
    }

    async fn register_with(&self, host: &str, capabilities: Value) -> Uuid {
        let (s, v) = self
            .call(
                None,
                "/v1/runner/register",
                Some(
                    json!({"version": "0.1", "backend": "fake", "capabilities": capabilities,
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
    // A runner that advertises no image gets the versioned default in the start item.
    assert_eq!(
        item["spec"]["image"],
        format!("aurora-sandbox:{}", env!("CARGO_PKG_VERSION"))
    );
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
    // (phase 1, RFE-002) the supervisor dials the gateway over TLS, verifying the proxy host
    assert_eq!(doc["gateway"]["tls"], true);
    let proxy = doc["gateway"]["proxy"].as_str().unwrap();
    assert_eq!(
        doc["gateway"]["server_name"].as_str().unwrap(),
        proxy.rsplit_once(':').map_or(proxy, |(h, _)| h)
    );

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

/// (FIX-025, D-158) Regression for the owner's "Sandbox starting" forever: a start that takes
/// longer than the old 300 s lease (capacity wait + pod start) was re-claimed with a new boot, and
/// the runner's failure for the old boot was fenced and dropped. Now the lease outlives the
/// runner's own bound, `starting`/`held` progress renews it, the start is never re-claimed while
/// the runner reports progress, and the failure of the current attempt fails the turn with its
/// reason.
#[tokio::test]
async fn slow_start_is_never_reclaimed_and_its_failure_reaches_the_turn() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent_with_turn("slow").await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap().to_owned(),
        item["boot_id"].as_str().unwrap().to_owned(),
    );
    // The runner's bound is strictly inside the lease.
    assert_eq!(item["start_timeout_secs"], 270);
    let lease_left: f64 = sqlx::query_scalar(
        "SELECT extract(epoch from lifecycle_lease_until - now())::float8 FROM sandboxes WHERE id = $1::uuid",
    )
    .bind(&sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(lease_left > 270.0 + 50.0, "lease {lease_left} s");

    let progress = |state: &str, phase: Option<&str>, msg: &str| {
        let mut rep_body = json!({"report_id": Uuid::now_v7(), "kind": "sandbox.status",
            "sandbox_id": sb, "boot_id": boot, "state": state, "message": msg});
        if let Some(p) = phase {
            rep_body["reason_code"] = json!(p);
        }
        json!({"reports": [rep_body]})
    };
    // Time passes beyond the old 300 s lease while the runner waits for capacity and the pod:
    // each time the lease is about to lapse, a progress report renews it.
    for (state, phase, msg) in [
        ("held", None, ""),
        (
            "starting",
            Some("scheduling"),
            "pod unschedulable: 0/3 nodes are available",
        ),
        ("starting", Some("creating"), "ContainerCreating"),
    ] {
        sqlx::query(
            "UPDATE sandboxes SET lifecycle_lease_until = now() + interval '1 second',
                 start_attempt_at = start_attempt_at - interval '200 seconds'
             WHERE id = $1::uuid",
        )
        .bind(&sb)
        .execute(r.h.pool())
        .await
        .unwrap();
        let (s, v) = r
            .call(
                Some(rep),
                "/v1/runner/reports",
                Some(progress(state, phase, msg)),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["results"][0]["status"], "applied", "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let w = r.work(rep, 0).await;
        assert!(
            !w["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["kind"] == "sandbox.start"),
            "a start the runner still works on is never re-claimed: {w}"
        );
        let (cur_boot, st, phase_now): (Option<Uuid>, String, Option<String>) =
            sqlx::query_as("SELECT boot_id, state, start_phase FROM sandboxes WHERE id = $1::uuid")
                .bind(&sb)
                .fetch_one(r.h.pool())
                .await
                .unwrap();
        assert_eq!(cur_boot.map(|b| b.to_string()), Some(boot.clone()));
        assert_eq!(st, "starting");
        assert_eq!(
            phase_now.as_deref(),
            Some(phase.unwrap_or("waiting_capacity"))
        );
    }
    // The current attempt's failure is applied (not fenced) and reaches the queued turn.
    let fail = json!({"reports": [{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
        "sandbox_id": sb, "boot_id": boot, "state": "failed",
        "reason_code": "home_volume_unbound",
        "message": "the sandbox home volume `home-x` cannot be bound: FailedBinding: no storage class is set",
        "events": ["FailedBinding: no persistent volumes available for this claim and no storage class is set"]}]});
    let (s, v) = r.call(Some(rep), "/v1/runner/reports", Some(fail)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["results"][0]["status"], "applied", "{v}");
    let (code, msg): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT error_code, error_message FROM turns WHERE agent_id = $1 AND role = 'turn'",
    )
    .bind(agent)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(code.as_deref(), Some("home_volume_unbound"));
    assert!(msg.unwrap().contains("no storage class is set"));
    let (fcode, fphase, events): (String, Option<String>, Value) = sqlx::query_as(
        "SELECT code, phase, events FROM sandbox_start_failures WHERE sandbox_id = $1::uuid",
    )
    .bind(&sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(fcode, "home_volume_unbound");
    assert_eq!(fphase.as_deref(), Some("creating"));
    assert_eq!(events.as_array().unwrap().len(), 1);
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

async fn turn_error(r: &Rig, turn: Uuid) -> (String, Option<String>, Option<String>) {
    sqlx::query_as("SELECT status, error_code, error_message FROM turns WHERE id = $1")
        .bind(turn)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

async fn sweep(r: &Rig) -> aurora_api::domains::hosted::readiness::Swept {
    aurora_api::domains::hosted::readiness::sweep_once(&r.h.state)
        .await
        .unwrap()
}

/// (FIX-025, C-583) The pod runs but the supervisor never connects (egress to runner:4105 blocked,
/// e.g. Cilium): after `RUNNER_SUPERVISOR_HELLO` the turn fails with `supervisor_unreachable`,
/// the runner address and the NetworkPolicy hint; the sandbox is stopped and ends `failed` with the
/// backoff; the failure is recorded for admins.
#[tokio::test]
async fn supervisor_that_never_connects_fails_the_turn_as_supervisor_unreachable() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("mute").await;
    let turn = r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap().to_owned(),
        item["boot_id"].as_str().unwrap().to_owned(),
    );
    post_reports(&r, rep, json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
        "sandbox_id": sb, "boot_id": boot, "state": "running", "container_ref": "p1",
        "message": "waiting for the supervisor to connect to the runner at aurora-runner.aurora (10.0.0.9):4105"}]))
    .await;
    // Not yet overdue: nothing happens.
    assert_eq!(sweep(&r).await.unreachable, 0);
    sqlx::query(
        "UPDATE sandboxes SET running_at = now() - interval '61 seconds' WHERE id = $1::uuid",
    )
    .bind(&sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    assert_eq!(sweep(&r).await.unreachable, 1);
    let (status, code, msg) = turn_error(&r, turn).await;
    assert_eq!(status, "failed");
    assert_eq!(code.as_deref(), Some("supervisor_unreachable"));
    let msg = msg.unwrap();
    assert!(
        msg.contains("aurora-runner.aurora (10.0.0.9):4105"),
        "{msg}"
    );
    assert!(msg.contains("networkPolicy.cilium.enabled"), "{msg}");
    // The chat shows it as an error event.
    let ev: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript_events WHERE turn_id = $1 AND type = 'error'
           AND payload->>'code' = 'supervisor_unreachable'",
    )
    .bind(turn)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(ev, 1);
    assert_eq!(r.sandbox_state(&sb).await, "stopping");
    // The runner is told to stop it; the stop lands in `failed` with the backoff.
    let w = r.work(rep, 0).await;
    let stop = first(&w, "sandbox.stop");
    assert_eq!(stop["reason"], "failure");
    post_reports(
        &r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
        "sandbox_id": sb, "boot_id": boot, "state": "stopped"}]),
    )
    .await;
    let (state, code, backoff): (String, Option<String>, bool) = sqlx::query_as(
        "SELECT state, failure_code, lifecycle_lease_until > now() FROM sandboxes WHERE id = $1::uuid",
    )
    .bind(&sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (state.as_str(), code.as_deref(), backoff),
        ("failed", Some("supervisor_unreachable"), true)
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sandbox_start_failures WHERE sandbox_id = $1::uuid AND code = 'supervisor_unreachable'",
    )
    .bind(&sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
}

/// (FIX-025, C-582) No runner connected: the readiness deadline fails the turn with
/// `runner_unavailable` (the arms never run without a poll); a turn not yet due is untouched, and
/// the failed turn's joining follow-up is promoted (kept), not lost.
#[tokio::test]
async fn readiness_deadline_without_a_runner_is_runner_unavailable_and_keeps_followups() {
    let r = rig().await;
    let agent = r.agent("lonely").await;
    let turn = r.turn(agent).await;
    let conv: Uuid = sqlx::query_scalar("SELECT conversation_id FROM turns WHERE id = $1")
        .bind(turn)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let fu = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, parent_turn_id, door, speaker_kind, speaker_label, text)
         VALUES ($1,$2,$3,'followup','joining',$4,'{}','system','s','and also')",
    )
    .bind(fu)
    .bind(conv)
    .bind(agent)
    .bind(turn)
    .execute(r.h.pool())
    .await
    .unwrap();
    assert_eq!(sweep(&r).await.turns_failed, 0, "not yet due");
    sqlx::query("UPDATE turns SET created_at = now() - interval '301 seconds' WHERE id = $1")
        .bind(turn)
        .execute(r.h.pool())
        .await
        .unwrap();
    assert_eq!(sweep(&r).await.turns_failed, 1);
    let (status, code, msg) = turn_error(&r, turn).await;
    assert_eq!(
        (status.as_str(), code.as_deref()),
        ("failed", Some("runner_unavailable"))
    );
    assert!(msg.unwrap().contains("No runner is connected"));
    let promoted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM turns WHERE conversation_id = $1 AND role = 'turn' AND status = 'queued' AND text = 'and also'",
    )
    .bind(conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(promoted, 1, "the follow-up stays queued as the next turn");
    // Idempotent: a second sweep does nothing more.
    assert_eq!(sweep(&r).await, Default::default());
}

/// (FIX-025, C-582) A start still in flight at the deadline is given up with its last phase,
/// stopped, and its other queued turns fail with the same reason.
#[tokio::test]
async fn start_in_flight_at_the_deadline_is_given_up_with_its_last_phase() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("pulling").await;
    let t1 = r.turn(agent).await;
    let t2 = r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap().to_owned(),
        item["boot_id"].as_str().unwrap().to_owned(),
    );
    post_reports(
        &r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
        "sandbox_id": sb, "boot_id": boot, "state": "starting", "reason_code": "creating",
        "message": "ContainerCreating"}]),
    )
    .await;
    sqlx::query("UPDATE turns SET created_at = now() - interval '301 seconds' WHERE id = $1")
        .bind(t1)
        .execute(r.h.pool())
        .await
        .unwrap();
    sweep(&r).await;
    for t in [t1, t2] {
        let (status, code, msg) = turn_error(&r, t).await;
        assert_eq!(
            (status.as_str(), code.as_deref()),
            ("failed", Some("sandbox_start_failed"))
        );
        assert!(
            msg.unwrap()
                .contains("last phase: creating: ContainerCreating")
        );
    }
    assert_eq!(r.sandbox_state(&sb).await, "stopping");
    // A late `running` from the runner for that boot is fenced; the stop goes out.
    let v = post_reports(
        &r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
        "sandbox_id": sb, "boot_id": boot, "state": "running", "container_ref": "p1"}]),
    )
    .await;
    assert_eq!(v["results"][0]["status"], "fenced");
    first(&r.work(rep, 0).await, "sandbox.stop");
}

/// (FIX-025, coordinator) An upgraded deployment with a sandbox already stuck after many failed
/// starts (start_failures above the threshold, `last_engaged_at` never written): automatic retries
/// stay stopped, an overdue turn says so and until when, and the next message starts it again
/// without any manual database edit.
#[tokio::test]
async fn sandbox_stuck_after_many_failures_recovers_on_the_next_message() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("daneel").await;
    r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap().to_owned(),
        item["boot_id"].as_str().unwrap().to_owned(),
    );
    post_reports(
        &r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
        "sandbox_id": sb, "boot_id": boot, "state": "failed", "reason_code": "at_capacity",
        "message": "wake capacity wait expired"}]),
    )
    .await;
    // The state the owner's Daneel is in: many failures, backoff long over, never engaged.
    sqlx::query(
        "UPDATE sandboxes SET start_failures = 6, last_engaged_at = NULL,
             lifecycle_lease_until = now() - interval '1 hour', updated_at = now() - interval '2 hours'
         WHERE id = $1::uuid",
    )
    .bind(&sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    // A turn queued before the last failure (as on the cluster) does not restart it automatically…
    let old = r.turn(agent).await;
    sqlx::query("UPDATE turns SET created_at = now() - interval '3 hours' WHERE id = $1")
        .bind(old)
        .execute(r.h.pool())
        .await
        .unwrap();
    assert!(
        !r.work(rep, 0).await["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "sandbox.start"),
        "automatic retries stay stopped"
    );
    // …and the deadline explains it instead of waiting forever.
    sweep(&r).await;
    let (status, code, msg) = turn_error(&r, old).await;
    assert_eq!(
        (status.as_str(), code.as_deref()),
        ("failed", Some("at_capacity"))
    );
    assert!(
        msg.unwrap()
            .contains("Automatic retries stopped after 6 failed starts")
    );
    // The next message starts it again.
    r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let again = first(&w, "sandbox.start");
    assert_eq!(again["sandbox_id"].as_str().unwrap(), sb);
    assert_ne!(again["boot_id"].as_str().unwrap(), boot);
}

/// (FIX-025, C-582) An overdue turn during the failure backoff names the backoff and its end.
#[tokio::test]
async fn overdue_turn_in_backoff_says_until_when() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("backoff").await;
    r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let (sb, boot) = (
        item["sandbox_id"].as_str().unwrap().to_owned(),
        item["boot_id"].as_str().unwrap().to_owned(),
    );
    post_reports(
        &r,
        rep,
        json!([{"report_id": Uuid::now_v7(), "kind": "sandbox.status",
        "sandbox_id": sb, "boot_id": boot, "state": "failed", "reason_code": "image_unavailable",
        "message": "image unavailable: `reg/x:1`: ErrImagePull"}]),
    )
    .await;
    let t = r.turn(agent).await;
    sqlx::query("UPDATE turns SET created_at = now() - interval '301 seconds' WHERE id = $1")
        .bind(t)
        .execute(r.h.pool())
        .await
        .unwrap();
    sweep(&r).await;
    let (status, code, msg) = turn_error(&r, t).await;
    assert_eq!(
        (status.as_str(), code.as_deref()),
        ("failed", Some("image_unavailable"))
    );
    let msg = msg.unwrap();
    assert!(
        msg.contains("ErrImagePull") && msg.contains("next start attempt is allowed after"),
        "{msg}"
    );
}

/// (FIX-025) Without progress (the runner is gone), an expired lease is still re-driven.
#[tokio::test]
async fn silent_start_lease_still_expires_and_is_redriven() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    r.agent_with_turn("gone").await;
    let w = r.work(rep, 0).await;
    let boot = first(&w, "sandbox.start")["boot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query("UPDATE sandboxes SET lifecycle_lease_until = now() - interval '1 second'")
        .execute(r.h.pool())
        .await
        .unwrap();
    let w = r.work(rep, 0).await;
    assert_ne!(
        first(&w, "sandbox.start")["boot_id"].as_str().unwrap(),
        boot
    );
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

/// Two idle running sandboxes, then a third agent's start: eviction happens iff the idle count
/// exceeds the runner's registered ceiling (default `max(1, max_sandboxes - 1)`).
async fn evicts(ceiling: Option<u32>) -> bool {
    let r = rig().await;
    let rep = r.register_with("runner-a", caps_with(4, ceiling)).await;
    for h in ["idle-a", "idle-b"] {
        let agent = r.agent_with_turn(h).await;
        let w = r.work(rep, 0).await;
        let item = first(&w, "sandbox.start");
        let (sb, boot) = (
            item["sandbox_id"].as_str().unwrap(),
            item["boot_id"].as_str().unwrap(),
        );
        r.report(rep, Uuid::now_v7(), sb, boot, "running").await;
        sqlx::query("UPDATE turns SET status = 'done' WHERE agent_id = $1")
            .bind(agent)
            .execute(r.h.pool())
            .await
            .unwrap();
    }
    r.agent_with_turn("waiting").await;
    let w = r.work(rep, 0).await;
    w["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["kind"] == "sandbox.stop" && i["reason"] == "evict")
}

#[tokio::test]
async fn eviction_uses_the_runners_registered_ceiling() {
    assert!(evicts(Some(1)).await, "2 idle > ceiling 1 must evict");
    assert!(!evicts(None).await, "default ceiling is not exceeded");
}

impl Rig {
    /// Start `agent`'s sandbox and report it running; returns (sandbox, boot).
    async fn boot_running(&self, rep: Uuid) -> (String, String) {
        let w = self.work(rep, 0).await;
        let item = first(&w, "sandbox.start");
        let (sb, boot) = (
            item["sandbox_id"].as_str().unwrap().to_owned(),
            item["boot_id"].as_str().unwrap().to_owned(),
        );
        self.report(rep, Uuid::now_v7(), &sb, &boot, "running")
            .await;
        (sb, boot)
    }

    async fn finish_turns(&self, agent: Uuid) {
        sqlx::query("UPDATE turns SET status = 'done' WHERE agent_id = $1")
            .bind(agent)
            .execute(self.h.pool())
            .await
            .unwrap();
    }

    async fn process(&self, agent: Uuid, sb: &str, boot: &str, busy: bool) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO processes (id, agent_id, sandbox_id, boot_id, name, command, status, busy, started_at)
             VALUES ($1,$2,$3::uuid,$4::uuid,'dev','sleep 1000','running',$5,now())",
        )
        .bind(id)
        .bind(agent)
        .bind(sb)
        .bind(boot)
        .bind(busy)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    async fn age(&self, sb: &str, hours: i32) {
        sqlx::query(
            "UPDATE sandboxes SET last_engaged_at = now() - make_interval(hours => $2),
                 last_active_at = now() - make_interval(hours => $2) WHERE id = $1::uuid",
        )
        .bind(sb)
        .bind(hours)
        .execute(self.h.pool())
        .await
        .unwrap();
    }
}

/// TEST-014 (arm 2): a busy process holds the box awake past `IDLE_STOP`, only until
/// `BACKGROUND_CAP`, then the stop reason is `cap`.
#[tokio::test]
async fn background_process_holds_awake_until_the_cap() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent_with_turn("bg").await;
    let (sb, boot) = r.boot_running(rep).await;
    r.finish_turns(agent).await;
    r.process(agent, &sb, &boot, true).await;
    // Idle (3 h > 30 min) but background-busy: stays awake.
    r.age(&sb, 3).await;
    assert!(r.work(rep, 0).await["items"].as_array().unwrap().is_empty());
    assert_eq!(r.sandbox_state(&sb).await, "running");
    // Beyond the 12 h cap: stopped with reason `cap`.
    r.age(&sb, 13).await;
    let w = r.work(rep, 0).await;
    let stop = first(&w, "sandbox.stop");
    assert_eq!(
        (stop["reason"].as_str(), stop["boot_id"].as_str()),
        (Some("cap"), Some(boot.as_str()))
    );
}

/// TEST-014 (arm 2): with a start waiting and held-awake boxes over the ceiling, the
/// longest-idle one is evicted.
#[tokio::test]
async fn eviction_stops_the_longest_idle_when_a_start_is_waiting() {
    let r = rig().await;
    let rep = r.register("runner-a", 2).await;
    let work2 = || async {
        let (s, v) = r
            .call(
                Some(rep),
                "/v1/runner/work",
                Some(json!({"wait_secs": 0, "capacity": {"running": 2, "max": 2}})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    };
    let a1 = r.agent_with_turn("e1").await;
    let a2 = r.agent_with_turn("e2").await;
    let w = r.work(rep, 0).await;
    let starts: Vec<&Value> = w["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "sandbox.start")
        .collect();
    assert_eq!(starts.len(), 2, "{w}");
    let mut ids = Vec::new();
    for it in starts {
        let (sb, boot) = (
            it["sandbox_id"].as_str().unwrap(),
            it["boot_id"].as_str().unwrap(),
        );
        r.report(rep, Uuid::now_v7(), sb, boot, "running").await;
        ids.push(sb.to_owned());
    }
    let (s1, s2) = (ids[0].clone(), ids[1].clone());
    r.finish_turns(a1).await;
    r.finish_turns(a2).await;
    // Both are idle but younger than IDLE_STOP; s1 has been idle longer. Nothing is waiting yet.
    sqlx::query(
        "UPDATE sandboxes SET last_engaged_at = now() - interval '10 minutes' WHERE id = $1::uuid",
    )
    .bind(&s1)
    .execute(r.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE sandboxes SET last_engaged_at = now() - interval '1 minute' WHERE id = $1::uuid",
    )
    .bind(&s2)
    .execute(r.h.pool())
    .await
    .unwrap();
    assert!(work2().await["items"].as_array().unwrap().is_empty());
    // A third agent needs a box: the longest-idle one is evicted.
    r.agent_with_turn("e3").await;
    let w = work2().await;
    let stop = first(&w, "sandbox.stop");
    assert_eq!(stop["reason"], "evict");
    assert_eq!(stop["sandbox_id"], s1.as_str());
    assert_eq!(r.sandbox_state(&s2).await, "running");
}

/// TEST-014 (arm 10/11): on boot end, running processes become `parked` (platform stop) or
/// `lost` (container died) and their armed watches convert likewise with a notice.
#[tokio::test]
async fn boot_end_parks_or_loses_processes_and_converts_watches() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    for (kind, state, expect) in [("park", "stopped", "parked"), ("die", "failed", "lost")] {
        let agent = r.agent_with_turn(kind).await;
        let (sb, boot) = r.boot_running(rep).await;
        let conv: Uuid =
            sqlx::query_scalar("SELECT conversation_id FROM turns WHERE agent_id = $1")
                .bind(agent)
                .fetch_one(r.h.pool())
                .await
                .unwrap();
        let p = r.process(agent, &sb, &boot, false).await;
        let w = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO watches (id, process_id, agent_id, conversation_id, kind, status, expires_at)
             VALUES ($1,$2,$3,$4,'exit','armed', now() + interval '1 hour')",
        )
        .bind(w)
        .bind(p)
        .bind(agent)
        .bind(conv)
        .execute(r.h.pool())
        .await
        .unwrap();
        if state == "stopped" {
            sqlx::query(
                "UPDATE sandboxes SET state = 'stopping', stop_reason = 'idle' WHERE id = $1::uuid",
            )
            .bind(&sb)
            .execute(r.h.pool())
            .await
            .unwrap();
        }
        let v = r.report(rep, Uuid::now_v7(), &sb, &boot, state).await;
        assert_eq!(v["results"][0]["status"], "applied", "{v}");
        let ps: String = sqlx::query_scalar("SELECT status FROM processes WHERE id = $1")
            .bind(p)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
        let ws: String = sqlx::query_scalar("SELECT status FROM watches WHERE id = $1")
            .bind(w)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
        assert_eq!((ps.as_str(), ws.as_str()), (expect, expect));
        let notice: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM transcript_events WHERE conversation_id = $1 AND type = 'notice'",
        )
        .bind(conv)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
        assert_eq!(notice["code"], format!("watch_{expect}"));
        // A duplicate stop report does not notify twice.
        r.report(rep, Uuid::now_v7(), &sb, &boot, state).await;
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM transcript_events WHERE conversation_id = $1 AND type = 'notice'",
        )
        .bind(conv)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
        assert_eq!(n, 1);
    }
}

// ---- P1-003 (RFE-003, SEC-41): the browser capability of a boot ----

fn caps_with_image(declared: &[&str]) -> Value {
    let mut v = caps_base(4);
    v["image_capabilities"] = json!(declared);
    v
}

/// Boot document capabilities and the capabilities of the minted `aub_` credential.
async fn start_caps(
    r: &Rig,
    rep_host: &str,
    declared: Option<&[&str]>,
    handle: &str,
) -> (Value, Vec<String>) {
    let rep = match declared {
        Some(d) => r.register_with(rep_host, caps_with_image(d)).await,
        None => r.register(rep_host, 4).await,
    };
    r.agent_with_turn(handle).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    let doc = boot_doc(item);
    let tok = doc["gateway"]["boot_token"].as_str().unwrap();
    let db: Vec<String> = sqlx::query_scalar(
        "SELECT unnest(capabilities) FROM gateway_credentials WHERE token_hash = $1",
    )
    .bind(&token::hash(tok)[..])
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    (doc["capabilities"].clone(), db)
}

/// FIX-092 (STRM-12, C-623): `terminal` rides the boot document only when the operator enabled
/// it **and** the runner advertised it — a runner that did not could not serve the `pty.*`
/// frames the boot would promise.
#[tokio::test]
async fn boot_terminal_capability_needs_the_runner_to_advertise_it() {
    let r = rig_cfg(|c| c.terminal.enabled = true).await;
    // runner-a registers without `terminal`.
    let (doc, db) = start_caps(&r, "runner-a", Some(&[]), "daneel").await;
    assert_eq!(
        doc,
        json!([]),
        "no terminal without the runner's advertisement"
    );
    assert!(db.is_empty(), "{db:?}");
}

/// The other half: the same instance with a runner that advertised `terminal` gets it.
#[tokio::test]
async fn boot_terminal_capability_is_present_when_both_sides_agree() {
    let r = rig_cfg(|c| c.terminal.enabled = true).await;
    let mut caps = caps_with_image(&[]);
    caps["terminal"] = json!(true);
    let rep = r.register_with("runner-a", caps).await;
    r.agent_with_turn("daneel").await;
    let w = r.work(rep, 0).await;
    let doc = boot_doc(first(&w, "sandbox.start"));
    assert_eq!(doc["capabilities"], json!(["terminal"]));
}

#[tokio::test]
async fn image_label_sets_boot_capability_and_credential_capability() {
    let r = rig().await;
    let (doc, db) = start_caps(&r, "runner-a", Some(&["browser"]), "daneel").await;
    assert_eq!(doc, json!(["browser"]));
    assert_eq!(db, ["browser"]);
}

#[tokio::test]
async fn image_without_the_label_gets_no_capability_fail_closed() {
    let r = rig().await;
    let (doc, db) = start_caps(&r, "runner-a", None, "daneel").await;
    assert_eq!(doc, json!([]));
    assert!(db.is_empty());
    let r = rig().await;
    let (doc, db) = start_caps(&r, "runner-a", Some(&["unknown"]), "daneel").await;
    assert_eq!(doc, json!([]), "unknown capabilities are ignored");
    assert!(db.is_empty());
}

#[tokio::test]
async fn disabled_posture_removes_the_capability() {
    // workspace posture
    let r = rig().await;
    sqlx::query(
        "INSERT INTO policy_generations (id, org_id, workspace_id, number, status, open_lane_posture)
         VALUES ($1,$2,$3,1,'published','disabled')",
    )
    .bind(Uuid::now_v7())
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (doc, db) = start_caps(&r, "runner-a", Some(&["browser"]), "daneel").await;
    assert_eq!((doc, db.len()), (json!([]), 0));
    // org posture
    let r = rig().await;
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(r.org)
        .bind(json!({"egress": {"open_lane": "disabled"}}))
        .execute(r.h.pool())
        .await
        .unwrap();
    let (doc, db) = start_caps(&r, "runner-a", Some(&["browser"]), "daneel").await;
    assert_eq!((doc, db.len()), (json!([]), 0));
    // allowlist_only keeps the capability (the gateway enforces the list)
    let r = rig().await;
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(r.org)
        .bind(json!({"egress": {"open_lane": "allowlist_only"}}))
        .execute(r.h.pool())
        .await
        .unwrap();
    let (doc, _) = start_caps(&r, "runner-a", Some(&["browser"]), "daneel").await;
    assert_eq!(doc, json!(["browser"]));
}

/// (FIX-058) Claims a start for `agent` and returns `(sandbox, boot, turn)`.
async fn claimed_start(r: &Rig, rep: Uuid, agent: Uuid) -> (Uuid, Uuid, Uuid) {
    let turn = r.turn(agent).await;
    let w = r.work(rep, 0).await;
    let item = first(&w, "sandbox.start");
    (
        item["sandbox_id"].as_str().unwrap().parse().unwrap(),
        item["boot_id"].as_str().unwrap().parse().unwrap(),
        turn,
    )
}

/// (FIX-058) The runner holding the attempt is gone: the give-up keeps `runner_unavailable`
/// (not rewritten to `agent_start_failed`) and the sandbox ends `failed` with the backoff, in
/// the same sweep — never `stopping` (nothing would send the stop), whether the attempt was
/// `starting` or already `stopping`.
#[tokio::test]
async fn give_up_on_a_dead_runner_keeps_runner_unavailable_and_ends_failed() {
    for state in ["starting", "stopping"] {
        let r = rig().await;
        let rep = r.register("runner-a", 4).await;
        let agent = r.agent("orphan").await;
        let (sb, _boot, turn) = claimed_start(&r, rep, agent).await;
        // Another replica keeps the platform "up" is not needed: every replica is silent.
        sqlx::query("UPDATE runner_replicas SET last_seen_at = now() - interval '1 hour'")
            .execute(r.h.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE sandboxes SET state = $2 WHERE id = $1")
            .bind(sb)
            .bind(state)
            .execute(r.h.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE turns SET created_at = now() - interval '301 seconds' WHERE id = $1")
            .bind(turn)
            .execute(r.h.pool())
            .await
            .unwrap();
        assert_eq!(sweep(&r).await.turns_failed, 1, "{state}");
        let (status, code, msg) = turn_error(&r, turn).await;
        assert_eq!(
            (status.as_str(), code.as_deref()),
            ("failed", Some("runner_unavailable")),
            "{state}"
        );
        assert!(msg.unwrap().contains("No runner is connected"), "{state}");
        let (st, failures, lease_live, code): (String, i32, bool, Option<String>) = sqlx::query_as(
            "SELECT state, start_failures, coalesce(lifecycle_lease_until > now(), false),
                        failure_code FROM sandboxes WHERE id = $1",
        )
        .bind(sb)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
        assert_eq!(st, "failed", "{state}: not wedged in stopping");
        assert_eq!(
            (failures, lease_live),
            (1, true),
            "{state}: backoff applies"
        );
        assert_eq!(code.as_deref(), Some("runner_unavailable"), "{state}");
    }
}

/// (FIX-071) A sandbox selected as hello-overdue whose supervisor connects before the update —
/// but which is not yet fully ready (no channel/harness/home) — is not given up as
/// `supervisor_unreachable`; while still unconnected it is.
#[tokio::test]
async fn hello_overdue_give_up_is_fenced_on_still_unconnected() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("half-ready").await;
    let (sb, boot, _turn) = claimed_start(&r, rep, agent).await;
    sqlx::query(
        "UPDATE sandboxes SET state = 'running', running_at = now() - interval '1 hour',
             bootstrap_used_at = NULL WHERE id = $1",
    )
    .bind(sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    let mut conn = r.h.pool().acquire().await.unwrap();
    let overdue = aurora_db::repo::sandboxes::hello_overdue(&mut conn, 60.0, 10)
        .await
        .unwrap();
    assert!(
        overdue.iter().any(|(s, b, _)| (*s, *b) == (sb, boot)),
        "selected"
    );
    // The supervisor connects (hello) but the sandbox is not ready: no channel, harness, home.
    sqlx::query("UPDATE sandboxes SET bootstrap_used_at = now() WHERE id = $1")
        .bind(sb)
        .execute(r.h.pool())
        .await
        .unwrap();
    let ran = aurora_db::repo::sandboxes::fail_attempt_unconnected(
        &mut conn,
        sb,
        boot,
        "supervisor_unreachable",
        "x",
    )
    .await
    .unwrap();
    assert!(ran.is_none(), "a connected supervisor is not given up on");
    let (st, failures): (String, i32) =
        sqlx::query_as("SELECT state, start_failures FROM sandboxes WHERE id = $1")
            .bind(sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!((st.as_str(), failures), ("running", 0));
    // Still unconnected: given up.
    sqlx::query("UPDATE sandboxes SET bootstrap_used_at = NULL WHERE id = $1")
        .bind(sb)
        .execute(r.h.pool())
        .await
        .unwrap();
    let ran = aurora_db::repo::sandboxes::fail_attempt_unconnected(
        &mut conn,
        sb,
        boot,
        "supervisor_unreachable",
        "x",
    )
    .await
    .unwrap();
    assert!(ran.is_some());
}

/// (FIX-071) The hello-overdue give-up goes through `fail_attempt`: a sandbox selected as
/// hello-overdue whose supervisor then connected and became ready before the update is not
/// failed, and is no longer selected on the next pass.
#[tokio::test]
async fn hello_overdue_selection_that_became_ready_is_not_failed() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("late-hello").await;
    let (sb, boot, _turn) = claimed_start(&r, rep, agent).await;
    sqlx::query(
        "UPDATE sandboxes SET state = 'running', running_at = now() - interval '1 hour',
             bootstrap_used_at = NULL WHERE id = $1",
    )
    .bind(sb)
    .execute(r.h.pool())
    .await
    .unwrap();
    let mut conn = r.h.pool().acquire().await.unwrap();
    let overdue = aurora_db::repo::sandboxes::hello_overdue(&mut conn, 60.0, 10)
        .await
        .unwrap();
    assert!(
        overdue.iter().any(|(s, b, _)| (*s, *b) == (sb, boot)),
        "selected"
    );
    // Between the selection and the update the supervisor connects and the sandbox is ready.
    sqlx::query(
        "UPDATE sandboxes SET bootstrap_used_at = now(), channel_replica_id = $2,
             resource_profile = '{\"harness\": {}}', home_applied_generation = 1000000
         WHERE id = $1",
    )
    .bind(sb)
    .bind(rep)
    .execute(r.h.pool())
    .await
    .unwrap();
    let ran = aurora_db::repo::sandboxes::fail_attempt(
        &mut conn,
        sb,
        boot,
        "supervisor_unreachable",
        "x",
        false,
    )
    .await
    .unwrap();
    assert!(ran.is_none(), "a sandbox that became ready is not failed");
    let (st, failures): (String, i32) =
        sqlx::query_as("SELECT state, start_failures FROM sandboxes WHERE id = $1")
            .bind(sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!((st.as_str(), failures), ("running", 0));
    let overdue = aurora_db::repo::sandboxes::hello_overdue(&mut conn, 60.0, 10)
        .await
        .unwrap();
    assert!(
        overdue.iter().all(|(s, _, _)| *s != sb),
        "no longer selected"
    );
}

/// (FIX-058) A sandbox the sweep selected as overdue that became ready before the update is left
/// alone: `fail_attempt` returns `None` and counts no failure; a not-yet-ready one is failed.
#[tokio::test]
async fn fail_attempt_leaves_a_sandbox_that_became_ready() {
    let r = rig().await;
    let rep = r.register("runner-a", 4).await;
    let agent = r.agent("lucky").await;
    let (sb, boot, _turn) = claimed_start(&r, rep, agent).await;
    // Ready: running, supervisor bootstrapped, channel held, harness ready, home synced.
    sqlx::query(
        "UPDATE sandboxes SET state = 'running', running_at = now(), bootstrap_used_at = now(),
             channel_replica_id = $2, resource_profile = '{\"harness\": {}}',
             home_applied_generation = 1000000 WHERE id = $1",
    )
    .bind(sb)
    .bind(rep)
    .execute(r.h.pool())
    .await
    .unwrap();
    let mut conn = r.h.pool().acquire().await.unwrap();
    let ran =
        aurora_db::repo::sandboxes::fail_attempt(&mut conn, sb, boot, "pod_failed", "x", false)
            .await
            .unwrap();
    assert!(ran.is_none(), "a ready sandbox is not failed");
    let (st, failures): (String, i32) =
        sqlx::query_as("SELECT state, start_failures FROM sandboxes WHERE id = $1")
            .bind(sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!((st.as_str(), failures), ("running", 0));
    // The same row, no longer ready (channel dropped), is given up.
    sqlx::query("UPDATE sandboxes SET channel_replica_id = NULL WHERE id = $1")
        .bind(sb)
        .execute(r.h.pool())
        .await
        .unwrap();
    let ran =
        aurora_db::repo::sandboxes::fail_attempt(&mut conn, sb, boot, "pod_failed", "x", false)
            .await
            .unwrap();
    assert!(ran.is_some());
    let st: String = sqlx::query_scalar("SELECT state FROM sandboxes WHERE id = $1")
        .bind(sb)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(st, "stopping");
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-011B-1 (SEC-74, D-264(b), C-827): the single-use grant of an approved parked request. The
//! test plays the API's part (`approve_ready` writes `resume_state = 'ready'`); the gateway lets
//! the **identical** request through exactly once and never matches anything else.

use crate::approval_park::{
    Env, Opts, approve_ready, derived, log_rows, pending, rows, send, send_ex, start,
};
use aurora_db::repo::gateway::PolicyRuleRow;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use uuid::Uuid;

const BODY: &str = "{\"a\":1}";

/// Parks `POST /items?q=1` with `BODY`, approves it (window 30 min) and returns its id.
async fn parked_and_approved(env: &Env) -> Uuid {
    let r = send(env, "POST", "/items?q=1", BODY).await;
    assert_eq!(r.code(), "approval_parked", "{}", r.body);
    let id: Uuid = r.details()["approval_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    approve_ready(&env.pool, id, 1800).await;
    id
}

async fn resume_state(env: &Env, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT resume_state FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_identical_retry_is_forwarded_once_and_logged_resumed() {
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let id = parked_and_approved(&env).await;
    let r = send(&env, "POST", "/items?q=1", BODY).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("body={\"a\":1}"), "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 1);
    assert_eq!(resume_state(&env, id).await.as_deref(), Some("consumed"));
    let consumed: bool =
        sqlx::query_scalar("SELECT consumed_at IS NOT NULL FROM approvals WHERE id=$1")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(consumed);
    // Single use: the second identical request parks a new approval and is not forwarded.
    let again = send(&env, "POST", "/items?q=1", BODY).await;
    assert_eq!(again.code(), "approval_parked");
    assert_ne!(again.details()["approval_id"], json!(id));
    assert_eq!(env.hits.load(Ordering::SeqCst), 1);
    // C-827: `approved`, `resumed`, with the approval id.
    let log = log_rows(&env).await;
    let row = log.iter().find(|r| r.resumed).expect("a resumed row");
    assert_eq!(
        (row.decision, row.approval_id, row.status),
        ("approved", Some(id), 200)
    );
}

#[tokio::test]
async fn thirty_two_concurrent_retries_forward_exactly_one() {
    let db = aurora_testkit::TestDb::new().await;
    let env = Arc::new(start(db.pool().clone(), Opts::default()).await);
    parked_and_approved(&env).await;
    let mut ts = Vec::new();
    for _ in 0..32 {
        let e = env.clone();
        ts.push(tokio::spawn(async move {
            send(&e, "POST", "/items?q=1", BODY).await
        }));
    }
    let mut ok = 0;
    let mut parked = 0;
    for t in ts {
        let r = t.await.unwrap();
        match (r.status, r.code().as_str()) {
            (200, _) => ok += 1,
            (403, "approval_parked") => parked += 1,
            other => panic!("unexpected {other:?}: {}", r.body),
        }
    }
    assert_eq!((ok, parked), (1, 31));
    assert_eq!(env.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nothing_but_the_identical_request_matches() {
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let id = parked_and_approved(&env).await;
    // Each component differs: body, query, query order, path, method.
    for (m, q, b) in [
        ("POST", "/items?q=1", "{\"a\":2}"),
        ("POST", "/items?q=2", BODY),
        ("POST", "/items?q=1&", BODY),
        ("POST", "/items", BODY),
        ("PUT", "/items?q=1", BODY),
    ] {
        let r = send(&env, m, q, b).await;
        assert_eq!(r.code(), "approval_parked", "{m} {q}: {}", r.body);
    }
    // Another conversation of the same agent.
    env.set_who(|w| w.conversation = Some(Uuid::now_v7()));
    let r = send(&env, "POST", "/items?q=1", BODY).await;
    assert_eq!(r.code(), "approval_parked", "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
    assert_eq!(resume_state(&env, id).await.as_deref(), Some("ready"));
    // The identical request in its own conversation still passes.
    env.set_who(|w| w.conversation = Some(env.conv));
    assert_eq!(send(&env, "POST", "/items?q=1", BODY).await.status, 200);
}

#[tokio::test]
async fn a_policy_change_never_lets_a_grant_through() {
    // To block: refused as on arrival, the grant is not spent.
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let id = parked_and_approved(&env).await;
    env.edit(|ws, _| {
        ws.rules.push(PolicyRuleRow {
            logical_id: Uuid::now_v7(),
            name: "stop-items".into(),
            enabled: true,
            identities: json!([]),
            targets: json!([{"type":"network","host":"127.0.0.1","path":"/items"}]),
            action: "block".into(),
            require_approval: false,
            approval_timeout_secs: None,
            rate_limit: None,
            conditions: json!([]),
        });
    });
    let r = send(&env, "POST", "/items?q=1", BODY).await;
    assert_eq!(r.status, 403);
    assert!(r.body.contains("blocked_by_policy"), "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
    assert_eq!(resume_state(&env, id).await.as_deref(), Some("ready"));

    // To another rule (same effect, new `logical_id`): the fingerprint differs, so it parks anew.
    let db2 = aurora_testkit::TestDb::new().await;
    let env2 = start(db2.pool().clone(), Opts::default()).await;
    let id2 = parked_and_approved(&env2).await;
    env2.edit(|ws, conns| {
        let conn = conns[0].id;
        ws.derived.clear();
        ws.derived
            .push(derived("d-other", conn, "write_item", true, json!([])));
    });
    let r = send(&env2, "POST", "/items?q=1", BODY).await;
    assert_eq!(r.code(), "approval_parked", "{}", r.body);
    assert_eq!(env2.hits.load(Ordering::SeqCst), 0);
    assert_eq!(resume_state(&env2, id2).await.as_deref(), Some("ready"));

    // To allow: it simply allows, and the grant is not consumed.
    let db3 = aurora_testkit::TestDb::new().await;
    let env3 = start(db3.pool().clone(), Opts::default()).await;
    let id3 = parked_and_approved(&env3).await;
    env3.edit(|ws, conns| {
        let conn = conns[0].id;
        ws.derived.clear();
        ws.derived
            .push(derived("d-allow", conn, "write_item", false, json!([])));
    });
    assert_eq!(send(&env3, "POST", "/items?q=1", BODY).await.status, 200);
    assert_eq!(resume_state(&env3, id3).await.as_deref(), Some("ready"));
}

#[tokio::test]
async fn a_window_that_ended_does_not_match() {
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let id = parked_and_approved(&env).await;
    sqlx::query("UPDATE approvals SET resume_expires_at = now() - interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let r = send(&env, "POST", "/items?q=1", BODY).await;
    assert_eq!(r.code(), "approval_parked", "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
    assert_eq!(resume_state(&env, id).await.as_deref(), Some("ready"));
}

#[tokio::test]
async fn turning_parking_off_still_consumes_a_ready_grant_once_and_holds_new_requests() {
    let db = aurora_testkit::TestDb::new().await;
    let env = Arc::new(start(db.pool().clone(), Opts::default()).await);
    let id = parked_and_approved(&env).await;
    let mut cfg = (*env.holds.park_config()).clone();
    cfg.enabled = false;
    env.holds.set_park(cfg);
    assert_eq!(send(&env, "POST", "/items?q=1", BODY).await.status, 200);
    assert_eq!(resume_state(&env, id).await.as_deref(), Some("consumed"));
    assert_eq!(env.hits.load(Ordering::SeqCst), 1);
    // A new long request is held now (a pending, unparked row), not parked.
    let e2 = env.clone();
    let t = tokio::spawn(async move { send_ex(&e2, "POST", "/items?q=1", BODY, false).await });
    let held = pending(&env.pool, env.w.ws).await;
    let parked: bool = sqlx::query_scalar("SELECT parked FROM approvals WHERE id=$1")
        .bind(held)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!parked);
    sqlx::query("UPDATE approvals SET status='denied', decided_at=now() WHERE id=$1")
        .bind(held)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("SELECT pg_notify('aurora_approval', $1)")
        .bind(json!({"id": held, "workspace_id": env.w.ws, "event": "decided"}).to_string())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(t.await.unwrap().code(), "manual_approval_denied");
}

#[tokio::test]
async fn consumption_precedes_forwarding_a_failed_upstream_leaves_the_grant_consumed() {
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let id = parked_and_approved(&env).await;
    env.upstream_down.store(true, Ordering::SeqCst);
    let r = send(&env, "POST", "/items?q=1", BODY).await;
    assert_ne!(r.status, 200, "{}", r.body);
    assert_eq!(resume_state(&env, id).await.as_deref(), Some("consumed"));
    // Fail closed: the agent needs a new approval, the old grant is gone.
    env.upstream_down.store(false, Ordering::SeqCst);
    assert_eq!(
        send(&env, "POST", "/items?q=1", BODY).await.code(),
        "approval_parked"
    );
}

#[tokio::test]
async fn a_database_outage_parks_and_consumes_nothing() {
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let id = parked_and_approved(&env).await;
    let health = aurora_gateway::health::Health::new(|| true);
    let holds = Arc::new(
        aurora_gateway::hold::Holds::new(
            db.pool().clone(),
            aurora_gateway::invalidation::Hub::new(),
            Uuid::now_v7(),
            std::time::Duration::from_secs(7200),
            None,
        )
        .with_park((*env.holds.park_config()).clone())
        .with_health(health.clone()),
    );
    health.report_db_error();
    let inputs = aurora_policy::park::Inputs {
        agent_id: env.w.agent,
        conversation_id: env.conv,
        method: "POST",
        host: "127.0.0.1",
        port: 80,
        path: "/items",
        query: Some("q=1"),
        body_sha256: aurora_policy::park::body_digest(BODY.as_bytes()),
        rule_logical_id: Uuid::nil(),
        credential: aurora_policy::park::CredentialRef::None,
        tool_id: None,
    };
    // Nothing is looked up, so nothing can be consumed; a park is refused as unavailable.
    assert!(
        holds
            .open_grants(env.w.agent, env.conv)
            .await
            .is_some_and(|g| g.is_empty())
    );
    let answer = holds
        .park(aurora_gateway::park::ParkRequest {
            hold: aurora_gateway::hold::HoldRequest {
                identity: &aurora_gateway::auth::RequestIdentity {
                    credential_id: Uuid::nil(),
                    kind: aurora_gateway::auth::CredentialKind::Context,
                    agent_id: env.w.agent,
                    workspace_id: env.w.ws,
                    org_id: env.w.org,
                    conversation_id: Some(env.conv),
                    turn_id: Some(Uuid::now_v7()),
                    trace_context: None,
                    context_label: None,
                    speaker_user_id: None,
                    speaker_group_ids: vec![],
                    capabilities: vec![],
                    byo: None,
                },
                method: "POST",
                host: "127.0.0.1",
                path: "/items",
                rule: None,
                connection_id: None,
                batch: &Default::default(),
                summary: &aurora_summary::generic(
                    "POST",
                    "127.0.0.1",
                    "/items",
                    None,
                    None,
                    b"",
                    false,
                ),
                rule_timeout_secs: None,
            },
            inputs: &inputs,
        })
        .await;
    assert_eq!(answer, aurora_gateway::park::ParkAnswer::StoreUnavailable);
    assert_eq!(resume_state(&env, id).await.as_deref(), Some("ready"));
    assert_eq!(rows(db.pool(), env.w.ws).await.len(), 1);
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-011B-3 (D-134, D-266, D-267(b); C-830..C-834, C-838, C-839; SEC-75, SEC-76): Web Push on the
//! server side, against the testkit's mock push service reached through
//! `push.dev_endpoint_overrides`. The mock verifies the VAPID JWT and decrypts the payload with an
//! independent RFC 8291 implementation.

use crate::approvals::{Fx, fixture};
use crate::common::*;
use aurora_api::domains::push::{cli, deliver, vapid};
use aurora_testkit::push::{Answer, MockPush, Received, VapidExpect};
use http::{HeaderMap, Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

pub(crate) const HOST: &str = "fcm.googleapis.com";
pub(crate) const SUBJECT: &str = "mailto:ops@example.org";

pub(crate) struct W {
    pub h: Harness,
    pub mock: MockPush,
    pub f: Fx,
    pub key: String,
}

pub(crate) struct P {
    pub id: Uuid,
    pub cookie: String,
    pub session: Uuid,
}

pub(crate) async fn world_with(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> W {
    let mock = MockPush::start().await;
    let url = mock.url();
    let h = Harness::with_config(|c| {
        c.push.enabled = true;
        c.push.vapid.subject = Some(SUBJECT.into());
        c.push.allowed_endpoint_hosts = vec![HOST.into(), "*.push.services.mozilla.com".into()];
        c.push
            .dev_endpoint_overrides
            .insert(HOST.to_owned(), url.clone());
        mutate(c);
    })
    .await;
    let f = fixture(&h).await;
    let key = vapid::active(&h.state).await.unwrap().1.public_b64u();
    mock.expect_vapid(VapidExpect {
        aud: format!("https://{HOST}"),
        subject: SUBJECT.into(),
        public_key: key.clone(),
    });
    W { h, mock, f, key }
}

pub(crate) async fn world() -> W {
    world_with(|_| {}).await
}

pub(crate) async fn person(w: &W, email: &str, ws_role: &str) -> P {
    let id = w.h.user(email, false).await;
    w.h.member(w.f.org, id, "member", "active").await;
    w.h.bind_ws(w.f.ws, id, ws_role).await;
    let (session, tok) = w.h.good_session(id).await;
    P {
        id,
        cookie: cookie(&tok),
        session,
    }
}

pub(crate) async fn call(
    w: &W,
    p: &P,
    m: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    w.h.send(m, uri, &[("cookie", &p.cookie), ("origin", ORIGIN)], body)
        .await
}

pub(crate) fn reg_body(
    w: &W,
    path: &str,
    k: &aurora_testkit::push::SubscriberKeys,
    details: bool,
) -> Value {
    json!({
        "endpoint": format!("https://{HOST}{path}"),
        "keys": { "p256dh": k.p256dh(), "auth": k.auth() },
        "label": "Laptop",
        "show_details": details,
        "vapid_public_key": w.key,
    })
}

/// Register a fresh test browser at `path`; returns the subscription id.
pub(crate) async fn register(w: &W, p: &P, path: &str, details: bool) -> Uuid {
    let k = w.mock.subscribe(path);
    let (s, _, v) = call(
        w,
        p,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(reg_body(w, path, &k, details)),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v["id"].as_str().unwrap().parse().unwrap()
}

/// A pending approval whose every free-text field carries a canary; returns its id.
pub(crate) async fn approval(w: &W, batch: Option<&str>, parked: bool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, method, host, path,
            summary, batch_id, rule_name, status, parked, fp_salt, fingerprint, deadline)
         VALUES ($1,$2,$3,$4,$5,'POST','CANARYHOST.example.com','/CANARYPATH?q=CANARYQUERY',
            '{\"title\":\"CANARYTITLE\",\"fields\":[{\"label\":\"k\",\"value\":\"CANARYFIELD\"}],\"text\":\"CANARYBODY\",\"link\":null,\"group\":null,\"source\":\"generic\"}',
            $6,'CANARYRULE','pending',$7,
            CASE WHEN $7 THEN '\\x0102'::bytea END, CASE WHEN $7 THEN '\\x0304'::bytea END,
            now() + interval '10 minutes')",
    )
    .bind(id)
    .bind(w.f.org)
    .bind(w.f.ws)
    .bind(w.f.agent)
    .bind(w.f.gw)
    .bind(batch)
    .bind(parked)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

pub(crate) async fn age(w: &W) {
    sqlx::query("UPDATE approvals SET created_at = now() - interval '30 seconds'")
        .execute(w.h.pool())
        .await
        .unwrap();
}

pub(crate) async fn sweep(w: &W) -> usize {
    deliver::sweep_once(&w.h.state).await.unwrap()
}

async fn count(w: &W, sql: &'static str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

async fn sub_exists(w: &W, id: Uuid) -> bool {
    count(
        w,
        "SELECT count(*) FROM push_subscriptions WHERE id = $1",
        id,
    )
    .await
        == 1
}

fn types(r: &[Received]) -> Vec<String> {
    r.iter()
        .map(|x| x.json().unwrap()["type"].as_str().unwrap().to_owned())
        .collect()
}

async fn wait_audit(w: &W, action: &str) -> Value {
    for _ in 0..200 {
        let v: Option<Value> = sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(action)
        .fetch_optional(w.h.pool())
        .await
        .unwrap();
        if let Some(v) = v {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no audit row {action}");
}

#[tokio::test]
async fn off_means_404_no_key_and_no_outbound_call() {
    let mock = MockPush::start().await;
    let url = mock.url();
    let h = Harness::with_config(|c| {
        c.push.enabled = false;
        c.push.dev_endpoint_overrides.insert(HOST.to_owned(), url);
    })
    .await;
    let f = fixture(&h).await;
    let u = h.user("a@x.io", false).await;
    h.member(f.org, u, "member", "active").await;
    let (_, tok) = h.good_session(u).await;
    for (m, uri) in [
        (Method::GET, "/v1/push/config"),
        (Method::GET, "/v1/me/push-subscriptions"),
        (Method::POST, "/v1/me/push-subscriptions"),
    ] {
        let (s, _, v) = h
            .send(
                m,
                uri,
                &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
                Some(json!({})),
            )
            .await;
        assert_eq!(s, 404, "{uri}");
        assert_eq!(v["error"]["code"], "push_unavailable");
    }
    let (_, inst) = h.get("/v1/instance", &[]).await;
    assert_eq!(inst["features"]["push"], false);
    assert_eq!(
        deliver::sweep_once(&h.state).await.unwrap(),
        0,
        "no sweep work with push off"
    );
    let keys: i64 = sqlx::query_scalar("SELECT count(*) FROM webpush_vapid_keys")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(keys, 0, "no key is generated while off");
    assert!(mock.received().is_empty());
}

#[tokio::test]
async fn routes_validate_limit_and_stay_session_only() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let b = person(&w, "b@x.io", "approver").await;
    let (_, inst) = w.h.get("/v1/instance", &[]).await;
    assert_eq!(inst["features"]["push"], true);

    let (s, _, cfg) = call(&w, &a, Method::GET, "/v1/push/config", None).await;
    assert_eq!(s, 200);
    assert_eq!(cfg["available"], true);
    assert_eq!(cfg["vapid_public_key"], w.key.as_str());
    assert_eq!(cfg["vapid_public_key"].as_str().unwrap().len(), 87);
    assert!(cfg["allowed_hosts"].as_array().unwrap().len() >= 2);
    assert_eq!(w.h.get("/v1/push/config", &[]).await.0, 401);

    // Session only: an API key is refused.
    let (_, key) = w.h.api_key(a.id, w.f.org, None, false).await;
    let (s, _, _) =
        w.h.send(
            Method::GET,
            "/v1/me/push-subscriptions",
            &[("authorization", &bearer(&key))],
            None,
        )
        .await;
    assert_eq!(s, 403);

    // Registration; the answer never carries the endpoint, host or keys.
    let k = w.mock.subscribe("/push/one");
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(reg_body(&w, "/push/one", &k, false)),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let text = v.to_string();
    for leak in [
        "fcm.googleapis",
        "/push/one",
        &k.p256dh(),
        &k.auth(),
        "endpoint",
    ] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    assert_eq!(v["state"], "active");
    assert_eq!(v["this_session"], true);
    assert_eq!(v["show_details"], false);
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (_, _, list) = call(&w, &a, Method::GET, "/v1/me/push-subscriptions", None).await;
    assert_eq!(list["subscriptions"].as_array().unwrap().len(), 1);

    // Refusals.
    let other_key = w.mock.subscribe("/push/x").p256dh();
    let mut stale = reg_body(&w, "/push/two", &k, false);
    stale["vapid_public_key"] = json!(other_key);
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(stale),
    )
    .await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (409, Some("push_key_stale"))
    );
    let mut bad = reg_body(&w, "/push/two", &k, false);
    bad["keys"]["p256dh"] = json!("AAAA");
    let (s, _, v) = call(&w, &a, Method::POST, "/v1/me/push-subscriptions", Some(bad)).await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (422, Some("push_keys_invalid"))
    );
    let mut bad = reg_body(&w, "/push/two", &k, false);
    bad["keys"]["auth"] = json!("AAAA");
    assert_eq!(
        call(&w, &a, Method::POST, "/v1/me/push-subscriptions", Some(bad))
            .await
            .0,
        422
    );

    // Ownership: another person's id is 404 for patch and test; delete is idempotent and leaves it.
    let uri = format!("/v1/me/push-subscriptions/{id}");
    assert_eq!(
        call(&w, &b, Method::PATCH, &uri, Some(json!({"label": "x"})))
            .await
            .0,
        404
    );
    assert_eq!(
        call(&w, &b, Method::POST, &format!("{uri}/test"), None)
            .await
            .0,
        404
    );
    assert_eq!(call(&w, &b, Method::DELETE, &uri, None).await.0, 204);
    assert!(
        sub_exists(&w, id).await,
        "another person's delete removes nothing"
    );

    // Patch: label, details, pause.
    let (s, _, v) = call(
        &w,
        &a,
        Method::PATCH,
        &uri,
        Some(json!({"label": "Phone", "show_details": true, "paused": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["label"].as_str(),
            v["show_details"].clone(),
            v["state"].as_str()
        ),
        (Some("Phone"), json!(true), Some("paused"))
    );

    // Audit rows carry the host, never the endpoint.
    let meta = wait_audit(&w, "push_subscription.create").await;
    assert_eq!(meta["endpoint_host"], HOST);
    assert!(!meta.to_string().contains("/push/one"));

    // The limit: PUSH_SUBSCRIPTIONS_PER_USER (10) in all.
    for i in 0..9 {
        register(&w, &a, &format!("/push/n{i}"), false).await;
    }
    let k = w.mock.subscribe("/push/over");
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(reg_body(&w, "/push/over", &k, false)),
    )
    .await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (409, Some("push_subscription_limit_reached"))
    );

    // Delete is idempotent.
    assert_eq!(call(&w, &a, Method::DELETE, &uri, None).await.0, 204);
    assert_eq!(call(&w, &a, Method::DELETE, &uri, None).await.0, 204);
    assert!(!sub_exists(&w, id).await);
    wait_audit(&w, "push_subscription.delete").await;
}

#[tokio::test]
async fn an_endpoint_moves_between_people_and_a_rebind_needs_the_owner() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let b = person(&w, "b@x.io", "approver").await;
    let id = register(&w, &a, "/push/shared", true).await;
    let owner_of = |id: Uuid| {
        let pool = w.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, Uuid>("SELECT user_id FROM push_subscriptions WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    // The deliberate gesture of another person moves the device to them; their choices apply.
    let k = w.mock.subscribe("/push/shared");
    let mut body = reg_body(&w, "/push/shared", &k, false);
    body["label"] = json!("Bob's browser");
    let (s, _, v) = call(
        &w,
        &b,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(body),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(owner_of(id).await, b.id);
    assert_eq!(v["show_details"], false);
    assert_eq!(v["id"].as_str().unwrap(), id.to_string());
    wait_audit(&w, "push_subscription.move").await;
    let (_, _, list) = call(&w, &a, Method::GET, "/v1/me/push-subscriptions", None).await;
    assert!(list["subscriptions"].as_array().unwrap().is_empty());

    // A silent re-bind by the previous person is 404 and nothing moves back.
    let mut rb = reg_body(&w, "/push/shared", &k, false);
    rb["rebind"] = json!(true);
    // A silent re-bind carries no label or flags of its own: the stored ones are kept.
    rb.as_object_mut().unwrap().remove("label");
    rb.as_object_mut().unwrap().remove("show_details");
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(rb.clone()),
    )
    .await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (404, Some("push_subscription_not_found"))
    );
    assert_eq!(owner_of(id).await, b.id);
    // An endpoint nobody registered: 404 and nothing is created.
    let mut unknown = reg_body(&w, "/push/never", &k, false);
    unknown["rebind"] = json!(true);
    assert_eq!(
        call(
            &w,
            &a,
            Method::POST,
            "/v1/me/push-subscriptions",
            Some(unknown)
        )
        .await
        .0,
        404
    );

    // The owner re-binds to a new session: 200, same row, label kept.
    let (new_session, tok) = w.h.good_session(b.id).await;
    let b2 = P {
        id: b.id,
        cookie: cookie(&tok),
        session: new_session,
    };
    let (s, _, v) = call(&w, &b2, Method::POST, "/v1/me/push-subscriptions", Some(rb)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["label"], "Bob's browser");
    assert_eq!(v["this_session"], true);
    let bound: Uuid = sqlx::query_scalar("SELECT session_id FROM push_subscriptions WHERE id = $1")
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(bound, new_session);
    let _ = b.session;
}

#[tokio::test]
async fn the_payload_is_encrypted_fixed_and_free_of_approval_text() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let plain = register(&w, &a, "/push/plain", false).await;
    let detailed = register(&w, &a, "/push/detail", true).await;
    let id = approval(&w, None, false).await;

    assert_eq!(sweep(&w).await, 0, "nothing before the delay");
    age(&w).await;
    assert_eq!(sweep(&w).await, 2, "one push per device");
    let got = w.mock.received();
    assert_eq!(got.len(), 2);
    for r in &got {
        assert_eq!(r.vapid, Ok(()), "JWT verified by the mock");
        assert_eq!(r.content_encoding.as_deref(), Some("aes128gcm"));
        assert_eq!(r.urgency.as_deref(), Some("high"));
        let ttl: u64 = r.ttl.as_deref().unwrap().parse().unwrap();
        assert!((60..=600).contains(&ttl), "ttl {ttl}");
        let raw = String::from_utf8(r.plaintext.clone().expect("decrypts")).unwrap();
        for canary in ["CANARY", "canary", "api.example", "example.com", "POST"] {
            assert!(!raw.contains(canary), "{canary} leaked: {raw}");
        }
        assert!(
            !r.topic
                .as_deref()
                .unwrap()
                .contains(&id.simple().to_string())
        );
    }
    let by_path = |p: &str| got.iter().find(|r| r.path == p).unwrap().json().unwrap();
    let mut keys: Vec<String> = by_path("/push/plain")
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "approval_id",
            "expires_at",
            "parked",
            "type",
            "url",
            "v",
            "workspace_id"
        ]
    );
    let d = by_path("/push/detail");
    assert_eq!(d["agent"], "Bot");
    assert_eq!(d["workspace"], "w");
    let p = by_path("/push/plain");
    assert_eq!(p["approval_id"], id.to_string());
    assert_eq!(p["type"], "approval.pending");
    assert_eq!(p["parked"], false);
    assert_eq!(
        p["url"],
        format!(
            "/o/{}/w/{}/approvals?focus={id}",
            org_slug(&w).await,
            w.f.ws.simple()
        )
    );
    // One approval, two devices: the Topic differs (unlinkable across devices).
    let topics: Vec<_> = got.iter().map(|r| r.topic.clone().unwrap()).collect();
    assert_ne!(topics[0], topics[1]);
    // Success is recorded on both rows; the approval itself is untouched.
    for s in [plain, detailed] {
        let ok: bool = sqlx::query_scalar(
            "SELECT last_success_at IS NOT NULL AND failure_count = 0 FROM push_subscriptions WHERE id = $1",
        )
        .bind(s)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        assert!(ok);
    }
    let status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(sweep(&w).await, 0, "idempotent");
    // Secret scan: nothing of the endpoint or the keys reached the audit log.
    let audit: String =
        sqlx::query_scalar("SELECT COALESCE(string_agg(metadata::text, ' '), '') FROM audit_log")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    for leak in ["/push/plain", "/push/detail", "p256dh"] {
        assert!(!audit.contains(leak), "{leak} in audit");
    }
}

async fn org_slug(w: &W) -> String {
    sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
        .bind(w.f.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn delivery_follows_the_decider_the_watcher_and_the_session_at_send_time() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let id = register(&w, &a, "/push/a", false).await;
    let _ = id;
    let ap = approval(&w, None, false).await;
    age(&w).await;

    // A live watcher in the workspace (an open Aurora tab) suppresses; a stale one does not.
    let rep = Uuid::now_v7();
    sqlx::query("INSERT INTO approval_watchers (replica_id, user_id, workspace_id, last_seen_at) VALUES ($1,$2,$3,now())")
        .bind(rep).bind(a.id).bind(w.f.ws).execute(w.h.pool()).await.unwrap();
    assert_eq!(sweep(&w).await, 0, "live watcher");
    sqlx::query("UPDATE approval_watchers SET last_seen_at = now() - interval '60 seconds' WHERE replica_id = $1")
        .bind(rep).execute(w.h.pool()).await.unwrap();

    // Lost can_approve: no longer a decider → nothing.
    sqlx::query("UPDATE workspace_access SET role = 'member' WHERE user_id = $1")
        .bind(a.id)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(sweep(&w).await, 0, "lost can_approve");
    sqlx::query("UPDATE workspace_access SET role = 'approver' WHERE user_id = $1")
        .bind(a.id)
        .execute(w.h.pool())
        .await
        .unwrap();

    // The registering session: revoked, past absolute expiry, idle — each stops delivery.
    for (label, sql) in [
        (
            "revoked",
            "UPDATE sessions SET revoked_at = now() WHERE id = $1",
        ),
        (
            "expired",
            "UPDATE sessions SET expires_at = now() - interval '1 second' WHERE id = $1",
        ),
        (
            "idle",
            "UPDATE sessions SET last_seen_at = now() - interval '400 days' WHERE id = $1",
        ),
    ] {
        sqlx::query(sql)
            .bind(a.session)
            .execute(w.h.pool())
            .await
            .unwrap();
        assert_eq!(sweep(&w).await, 0, "{label} session");
        sqlx::query("UPDATE sessions SET revoked_at = NULL, expires_at = now() + interval '1 hour', last_seen_at = now() WHERE id = $1")
            .bind(a.session).execute(w.h.pool()).await.unwrap();
    }
    assert!(w.mock.received().is_empty());

    // Decided, expired or frozen before the push: nothing (re-checked right before claiming).
    sqlx::query("UPDATE approvals SET status = 'approved' WHERE id = $1")
        .bind(ap)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(sweep(&w).await, 0, "decided");
    sqlx::query("UPDATE approvals SET status = 'pending', deadline = now() - interval '1 second' WHERE id = $1")
        .bind(ap)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(sweep(&w).await, 0, "expired");
    sqlx::query("UPDATE approvals SET deadline = now() + interval '10 minutes' WHERE id = $1")
        .bind(ap)
        .execute(w.h.pool())
        .await
        .unwrap();

    // Paused and disabled devices are skipped; then everything is in order and one push goes out.
    sqlx::query("UPDATE push_subscriptions SET paused_at = now()")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(sweep(&w).await, 0, "paused");
    sqlx::query("UPDATE push_subscriptions SET paused_at = NULL, disabled_at = now()")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(sweep(&w).await, 0, "disabled");
    sqlx::query("UPDATE push_subscriptions SET disabled_at = NULL")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(sweep(&w).await, 1);
    assert_eq!(w.mock.received().len(), 1);
}

#[tokio::test]
async fn claims_make_two_replicas_batches_the_cap_and_the_reminder_exact() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    register(&w, &a, "/push/a", false).await;

    // Two replicas sweeping at once claim once.
    approval(&w, None, false).await;
    age(&w).await;
    let (x, y) = tokio::join!(
        deliver::sweep_once(&w.h.state),
        deliver::sweep_once(&w.h.state)
    );
    assert_eq!(x.unwrap() + y.unwrap(), 1);
    assert_eq!(w.mock.received().len(), 1);

    // A batch of three is one push.
    w.mock.clear();
    for _ in 0..3 {
        approval(&w, Some("batch-1"), false).await;
    }
    age(&w).await;
    assert_eq!(sweep(&w).await, 1, "one push per batch");
    let folded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM approval_push_claims WHERE delivery = 'batch'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(folded, 2);

    // The hourly cap, then one digest, then silence until the hour ends.
    w.mock.clear();
    sqlx::query(
        "INSERT INTO approval_push_claims (approval_id, user_id, stage, kind, delivery)
         SELECT gen_random_uuid(), $1, 'initial', 'gateway', 'push' FROM generate_series(1, 30)",
    )
    .bind(a.id)
    .execute(w.h.pool())
    .await
    .unwrap();
    approval(&w, None, false).await;
    age(&w).await;
    assert_eq!(sweep(&w).await, 1);
    let got = w.mock.received();
    assert_eq!(types(&got), ["approval.digest"]);
    let digest = got[0].json().unwrap();
    assert!(digest["count"].as_u64().unwrap() >= 1);
    assert!(digest.get("approval_id").is_none());
    assert_eq!(got[0].urgency.as_deref(), Some("normal"));
    approval(&w, None, false).await;
    age(&w).await;
    assert_eq!(sweep(&w).await, 0, "folded silently");
    assert_eq!(w.mock.received().len(), 1);

    // The e-mail path is independent: it still mails this decider (own claims table).
    let mailed = aurora_api::domains::approvals::notify::notify_once(&w.h.state)
        .await
        .unwrap();
    assert!(mailed >= 1, "e-mail unaffected by push claims");
}

#[tokio::test]
async fn a_parked_approval_is_reminded_once_at_half_time() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    register(&w, &a, "/push/a", false).await;
    let id = approval(&w, None, true).await;
    // Created 20 minutes ago with 10 left: outside the initial catch-up, past half its time.
    sqlx::query("UPDATE approvals SET created_at = now() - interval '20 minutes', deadline = now() + interval '10 minutes' WHERE id = $1")
        .bind(id).execute(w.h.pool()).await.unwrap();
    assert_eq!(sweep(&w).await, 1);
    let got = w.mock.received();
    assert_eq!(types(&got), ["approval.reminder"]);
    let j = got[0].json().unwrap();
    assert_eq!(j["parked"], true);
    assert_eq!(got[0].urgency.as_deref(), Some("high"));
    assert_eq!(sweep(&w).await, 0, "once");
    // Before half time: nothing.
    let early = approval(&w, None, true).await;
    sqlx::query("UPDATE approvals SET created_at = now() - interval '2 minutes' WHERE id = $1")
        .bind(early)
        .execute(w.h.pool())
        .await
        .unwrap();
    let before = w.mock.received().len();
    sweep(&w).await;
    assert!(
        !types(&w.mock.received()[before..])
            .iter()
            .any(|t| t == "approval.reminder"),
        "no reminder before half time"
    );
}

#[tokio::test]
async fn revocation_paths_delete_the_sessions_subscriptions() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    // A second session of the same person with its own device.
    let (s2, tok2) = w.h.good_session(a.id).await;
    let a2 = P {
        id: a.id,
        cookie: cookie(&tok2),
        session: s2,
    };
    let d1 = register(&w, &a, "/push/s1", false).await;
    let d2 = register(&w, &a2, "/push/s2", false).await;

    // "Sign out other sessions" removes the other session's device only.
    let (s, _, _) = call(
        &w,
        &a,
        Method::POST,
        "/v1/auth/sessions/revoke-others",
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert!(sub_exists(&w, d1).await && !sub_exists(&w, d2).await);
    let meta = wait_audit(&w, "session.revoke_others").await;
    assert_eq!(meta["push_subscriptions_removed"], 1, "{meta}");

    // Sign-out removes this session's device.
    let (s, _, _) = call(&w, &a, Method::POST, "/v1/auth/logout", None).await;
    assert_eq!(s, 204);
    assert!(!sub_exists(&w, d1).await);
    let meta = wait_audit(&w, "user.logout").await;
    assert_eq!(meta["push_subscriptions_removed"], 1, "{meta}");

    // An administrator's deactivation removes every device of the person.
    let b = person(&w, "b@x.io", "approver").await;
    let d3 = register(&w, &b, "/push/b", false).await;
    let admin = w.h.user("admin@x.io", true).await;
    let (_, tok) = w.h.good_session(admin).await;
    let (s, _, v) =
        w.h.send(
            Method::PATCH,
            &format!("/v1/admin/users/{}", b.id),
            &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
            Some(json!({"disabled": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(!sub_exists(&w, d3).await);
}

#[tokio::test]
async fn push_service_answers_decide_what_happens_to_the_device() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let id = register(&w, &a, "/push/a", false).await;

    // 429 is retried up to PUSH_RETRY_MAX tries honouring Retry-After, then counts as a failure.
    w.mock.respond(Answer {
        retry_after: Some(1),
        ..Answer::status(429)
    });
    approval(&w, None, false).await;
    age(&w).await;
    sweep(&w).await;
    assert_eq!(w.mock.received().len(), 3, "PUSH_RETRY_MAX tries");
    let fails: i32 =
        sqlx::query_scalar("SELECT failure_count FROM push_subscriptions WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(fails, 1);

    // 400 five times in a row (one approval each) disables the device.
    w.mock.respond(Answer::status(400));
    for _ in 0..4 {
        w.mock.clear();
        approval(&w, None, false).await;
        age(&w).await;
        sweep(&w).await;
        assert_eq!(w.mock.received().len(), 1, "a rejection is not retried");
    }
    let (fails, disabled): (i32, bool) = sqlx::query_as(
        "SELECT failure_count, disabled_at IS NOT NULL FROM push_subscriptions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!((fails, disabled), (5, true));
    let (_, _, list) = call(&w, &a, Method::GET, "/v1/me/push-subscriptions", None).await;
    assert_eq!(list["subscriptions"][0]["state"], "disabled");
    w.mock.clear();
    approval(&w, None, false).await;
    age(&w).await;
    assert_eq!(sweep(&w).await, 0, "a disabled device is skipped");

    // 410 deletes the subscription.
    let gone = register(&w, &a, "/push/gone", false).await;
    w.mock.respond(Answer::status(410));
    sweep(&w).await;
    assert!(!sub_exists(&w, gone).await);
}

#[tokio::test]
async fn a_host_that_left_the_allow_list_is_rejected_and_the_device_disabled() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let id = register(&w, &a, "/push/a", false).await;
    approval(&w, None, false).await;
    age(&w).await;
    // The operator narrowed the allow-list after the registration.
    let mut st = w.h.state.clone();
    let mut cfg = (*st.config).clone();
    cfg.push.allowed_endpoint_hosts = vec!["updates.push.services.mozilla.com".into()];
    st.config = std::sync::Arc::new(cfg);
    deliver::sweep_once(&st).await.unwrap();
    assert!(
        w.mock.received().is_empty(),
        "no call to a host off the list"
    );
    let disabled: bool =
        sqlx::query_scalar("SELECT disabled_at IS NOT NULL FROM push_subscriptions WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(disabled);
}

#[tokio::test]
async fn the_test_route_sends_the_test_payload_and_is_rate_limited() {
    let w = world().await;
    let a = person(&w, "a@x.io", "approver").await;
    let id = register(&w, &a, "/push/a", true).await;
    let uri = format!("/v1/me/push-subscriptions/{id}/test");
    let (s, _, _) = call(&w, &a, Method::POST, &uri, None).await;
    assert_eq!(s, 202);
    let got = w.mock.wait_for(1, Duration::from_secs(10)).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].json().unwrap(), json!({"v": 1, "type": "test"}));
    assert_eq!(got[0].vapid, Ok(()));
    for _ in 0..2 {
        assert_eq!(call(&w, &a, Method::POST, &uri, None).await.0, 202);
    }
    let (s, h, v) = call(&w, &a, Method::POST, &uri, None).await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (429, Some("push_test_rate_limited"))
    );
    assert!(h.get("retry-after").is_some());
}

#[tokio::test]
async fn keys_are_generated_once_sealed_inline_rotated_and_destroyed() {
    // A Vault-style default store must not matter: the key and the subscriptions stay inline.
    let w = world().await;
    let w = W {
        h: w.h.with_helm("[secrets]\ndefault_store = \"helmvault\"\n[[secrets.stores]]\nid = \"helmvault\"\nkind = \"memory\"\n"),
        ..w
    };
    // Concurrent first start (on a fresh state) yields exactly one active key.
    let fresh = Harness::with_config(|c| {
        c.push.enabled = true;
        c.push.vapid.subject = Some(SUBJECT.into());
    })
    .await;
    let results = futures_util_join(&fresh).await;
    assert!(
        results.iter().all(|k| k == &results[0]),
        "one key for every caller"
    );
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webpush_vapid_keys WHERE status = 'active'")
            .fetch_one(fresh.pool())
            .await
            .unwrap();
    assert_eq!(active, 1);

    let a = person(&w, "a@x.io", "approver").await;
    let id = register(&w, &a, "/push/a", false).await;
    let refs: Vec<String> = sqlx::query_scalar(
        "SELECT private_ref FROM webpush_vapid_keys UNION ALL SELECT secret_ref FROM push_subscriptions",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(refs.len(), 2);
    assert!(refs.iter().all(|r| r.starts_with("inline:v1:")), "{refs:?}");

    // Rotation: the old key's subscriber is still delivered (JWT under the old key), is shown as
    // `stale_key`, and a registration under the old key is refused.
    let old = w.key.clone();
    let rotated = vapid::rotate(&w.h.state, false).await.unwrap();
    assert!(matches!(rotated, vapid::Rotation::Rotated { .. }));
    wait_audit(&w, "push.vapid_rotate").await;
    let new_key = vapid::active(&w.h.state).await.unwrap().1.public_b64u();
    assert_ne!(new_key, old);
    let (_, _, list) = call(&w, &a, Method::GET, "/v1/me/push-subscriptions", None).await;
    assert_eq!(list["subscriptions"][0]["state"], "stale_key");
    let k = w.mock.subscribe("/push/b");
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(reg_body(&w, "/push/b", &k, false)),
    )
    .await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (409, Some("push_key_stale"))
    );
    approval(&w, None, false).await;
    age(&w).await;
    assert_eq!(sweep(&w).await, 1);
    assert_eq!(w.mock.received()[0].vapid, Ok(()), "old key still served");
    // A silent re-subscribe on the new key makes it active again.
    let mut again = reg_body(&w, "/push/a", &w.mock.subscribe("/push/a"), false);
    again["vapid_public_key"] = json!(new_key);
    again["rebind"] = json!(true);
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(again),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["state"], "active");

    // The retired key is destroyed once unreferenced.
    assert_eq!(vapid::destroy_expired(&w.h.state).await.unwrap(), 1);
    assert!(sub_exists(&w, id).await, "the migrated subscription stays");
    let keys: i64 = sqlx::query_scalar("SELECT count(*) FROM webpush_vapid_keys")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(keys, 1);

    // A retired key past PUSH_VAPID_RETIRE is destroyed with the subscriptions still on it.
    let kc = w.mock.subscribe("/push/c");
    let mut body = reg_body(&w, "/push/c", &kc, false);
    body["vapid_public_key"] = json!(vapid::active(&w.h.state).await.unwrap().1.public_b64u());
    let (s, _, v) = call(
        &w,
        &a,
        Method::POST,
        "/v1/me/push-subscriptions",
        Some(body),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let stale: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    vapid::rotate(&w.h.state, false).await.unwrap();
    sqlx::query("UPDATE webpush_vapid_keys SET retired_at = now() - interval '31 days' WHERE status = 'retired'")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(vapid::destroy_expired(&w.h.state).await.unwrap(), 1);
    assert!(!sub_exists(&w, stale).await);

    // `rotate-vapid` through the CLI path exits 0; status never prints key material.
    assert_eq!(cli::rotate(&w.h.state, true).await.unwrap(), 0);
    let st = vapid::status(&w.h.state).await.unwrap();
    assert_eq!(st.mode, "generated");
    assert!(!serde_json::to_string(&st).unwrap().contains("BEGIN"));
}

async fn futures_util_join(h: &Harness) -> Vec<String> {
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let st = h.state.clone();
        set.spawn(async move { vapid::active(&st).await.unwrap().1.public_b64u() });
    }
    let mut out = Vec::new();
    while let Some(r) = set.join_next().await {
        out.push(r.unwrap());
    }
    out
}

#[tokio::test]
async fn an_unreadable_operator_key_refuses_start_naming_the_variable() {
    let cfg = config(|c| {
        c.push.enabled = true;
        c.push.vapid.private_key_env = Some("PATH".into());
    });
    let err = aurora_api::domains::push::PushRuntime::from_config(&cfg)
        .err()
        .expect("PATH is not a key");
    assert!(err.contains("PATH"), "{err}");
    let cfg = config(|c| {
        c.push.enabled = true;
        c.push.vapid.private_key_env = Some("AURORA_NO_SUCH_VARIABLE".into());
    });
    let err = aurora_api::domains::push::PushRuntime::from_config(&cfg)
        .err()
        .expect("unset");
    assert!(err.contains("AURORA_NO_SUCH_VARIABLE"), "{err}");
}

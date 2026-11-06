#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-071: connector inbox ingest, operation engine, correlated ops, file transfer, link-start
//! and the inbox processor (docs/13 §4, §4.2, §6 invariants 3 and 5).
mod common;

use async_trait::async_trait;
use aurora_api::domains::connectors::inbox::{self, InboxError, InboxHandler};
use aurora_api::domains::connectors::ops::{self, CorrelatedError, NewOp};
use aurora_core::Secret;
use aurora_crypto::generate_signing_key;
use aurora_db::bus::{BusEvent, Channel, Listener};
use aurora_db::repo::channel_inbox::InboxRow;
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use common::*;
use http::{HeaderMap, Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    tok: String,
    manifest: Value,
    sig: String,
}

fn hd(c: &str) -> [(&str, &str); 2] {
    [("cookie", c), ("origin", ORIGIN)]
}

async fn world() -> W {
    let h = Harness::new().await;
    let root = h.user("root@x.io", true).await;
    let (_, t) = h.good_session(root).await;
    let admin = cookie(&t);
    let key = generate_signing_key().unwrap();
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/trusted-keys",
            &hd(&admin),
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/connectors",
            &hd(&admin),
            Some(json!({"id": "fake", "display_name": "fake"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let org = h.org().await;
    let m = json!({
        "package": {"id": "fake", "version": "1.0.0", "contract": 1, "publisher": "acme"},
        "about": {"name": "Fake"},
        "capabilities": {"threads": true},
        "integration_setup": {"fields": []},
        "presence_setup": {},
        "limits": {},
        "public_callbacks": []
    });
    let typed: aurora_protocol::connector::ConnectorManifest =
        serde_json::from_value(m.clone()).unwrap();
    let canon = serde_json::to_value(&typed).unwrap();
    let d: [u8; 32] = Sha256::digest(aurora_registry::snapshot::canonical_bytes(&canon)).into();
    let sig = PackageBuilder::sign_digest(&key, &PackageBuilder::key_id(&key), &d);
    let w = W {
        h,
        org,
        tok,
        manifest: m.clone(),
        sig: sig.clone(),
    };
    let (s, _, v) = w
        .plane(
            None,
            Method::POST,
            "/register",
            Some(json!({"manifest": m, "signature": sig, "version": "1.0.0"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // A first manifest widens the empty baseline: an admin must approve it.
    assert_eq!(v["status"], "pending_approval", "{v}");
    let digest: String =
        sqlx::query_scalar("SELECT manifest_digest FROM connectors WHERE id='fake'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let (s, _, v) =
        w.h.send(
            Method::POST,
            "/v1/admin/connectors/fake/approve",
            &hd(&admin),
            Some(json!({ "manifest_digest": digest })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "active");
    w
}

impl W {
    async fn plane(
        &self,
        replica: Option<Uuid>,
        m: Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, HeaderMap, Value) {
        let b = bearer(&self.tok);
        let r = replica.map(|r| r.to_string());
        let mut h = vec![("authorization", b.as_str())];
        if let Some(r) = &r {
            h.push(("x-aurora-replica", r.as_str()));
        }
        let (s, hm, v) = self
            .h
            .send(m, &format!("/v1/connector{path}"), &h, body)
            .await;
        (s.as_u16(), hm, v)
    }

    async fn post(&self, r: Uuid, path: &str, body: Value) -> (u16, Value) {
        let (s, _, v) = self.plane(Some(r), Method::POST, path, Some(body)).await;
        (s, v)
    }

    /// Another live replica.
    async fn replica(&self) -> Uuid {
        let (s, _, v) = self
            .plane(
                None,
                Method::POST,
                "/register",
                Some(json!({"manifest": self.manifest, "signature": self.sig, "version": "1.0.0"})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["replica_id"].as_str().unwrap().parse().unwrap()
    }

    /// An `active` integration owned by `owner`.
    async fn integration(&self, tenant: &str, owner: Uuid) -> Uuid {
        let id = Uuid::now_v7();
        let r = self
            .h
            .state
            .vault
            .seal(
                &aurora_api::domains::connectors::integration_seal_ctx(id, self.org),
                Secret::from(b"{}".to_vec()),
            )
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label,
                config, credentials_ref, status, owner_replica_id, owner_lease_until)
             VALUES ($1,$2,'fake',$3,$3,'{}',$4,'active',$5, now() + interval '1 hour')",
        )
        .bind(id)
        .bind(self.org)
        .bind(tenant)
        .bind(r.to_column())
        .bind(owner)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    async fn enqueue(&self, integration: Option<Uuid>, kind: &str, payload: Value) -> Uuid {
        let mut tx = self.h.pool().begin().await.unwrap();
        let id = ops::enqueue(&mut tx, new_op(integration, kind, payload, None))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn claim(&self, r: Uuid, max: u32, wait: u32) -> Vec<Value> {
        let (s, v) = self
            .post(r, "/ops/claim", json!({"max": max, "wait_secs": wait}))
            .await;
        assert_eq!(s, 200, "{v}");
        v["ops"].as_array().unwrap().clone()
    }

    async fn result(&self, r: Uuid, res: Value) -> (u16, Value) {
        self.post(r, "/ops/results", json!({"results": [res]}))
            .await
    }

    async fn op(&self, id: Uuid) -> (String, i32, Option<String>, Option<Value>) {
        sqlx::query_as("SELECT status, attempts, error_code, result FROM channel_ops WHERE id=$1")
            .bind(id)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn conversation(&self) -> Uuid {
        let ws = self.h.workspace(self.org).await;
        let agent = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,'a1','a1','hosted','fake')")
            .bind(agent).bind(self.org).bind(ws).execute(self.h.pool()).await.unwrap();
        let conv = Uuid::now_v7();
        sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api','x',now())")
            .bind(conv).bind(agent).bind(ws).execute(self.h.pool()).await.unwrap();
        conv
    }

    async fn raw(
        &self,
        replica: Uuid,
        method: Method,
        path: &str,
        extra: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        let mut b = Request::builder()
            .method(method)
            .uri(format!("/v1/connector{path}"))
            .header("authorization", bearer(&self.tok))
            .header("x-aurora-replica", replica.to_string());
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let res = self
            .h
            .app
            .clone()
            .oneshot(b.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let (p, body) = res.into_parts();
        (
            p.status.as_u16(),
            body.collect().await.unwrap().to_bytes().to_vec(),
        )
    }
}

fn new_op(integration: Option<Uuid>, kind: &str, payload: Value, dedupe: Option<&str>) -> NewOp {
    NewOp {
        kind: kind.to_owned(),
        connector_id: Some("fake".to_owned()),
        integration_id: integration,
        presence_id: None,
        payload,
        dedupe_key: dedupe.map(ToOwned::to_owned),
    }
}

fn event(id: &str, integration: Uuid) -> Value {
    json!({"event_id": id, "integration_id": integration, "kind": "message.posted",
           "occurred_at": "2026-11-06T10:00:00Z", "payload": {"text": "hi"}})
}

#[tokio::test]
async fn events_dedupe_reject_and_notify() {
    let w = world().await;
    let r1 = w.replica().await;
    let r2 = w.replica().await;
    let mine = w.integration("T1", r1).await;
    let theirs = w.integration("T2", r2).await;
    let mut l = Listener::connect(w.h.pool(), &[Channel::Inbox])
        .await
        .unwrap();
    assert_eq!(l.next().await, BusEvent::Resync);

    let (s, v) = w
        .post(
            r1,
            "/events",
            json!({"events": [
                event("e1", mine), event("e2", mine), event("e1", mine),
                event("e3", theirs), event("e4", Uuid::now_v7()),
                {"event_id": "", "integration_id": mine, "kind": "message.posted",
                 "occurred_at": "x", "payload": {}}
            ]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["accepted"], json!(["e1", "e2"]));
    assert_eq!(v["duplicates"], json!(["e1"]));
    let rej: Vec<(&str, &str)> = v["rejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["event_id"].as_str().unwrap(), r["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        rej,
        vec![("e3", "forbidden"), ("e4", "not_found"), ("", "invalid")]
    );
    // Delivered on commit, before the response is read.
    let ev = tokio::time::timeout(Duration::from_secs(5), l.next())
        .await
        .unwrap();
    assert!(matches!(
        ev,
        BusEvent::Notification {
            channel: Channel::Inbox,
            ..
        }
    ));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbox")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 2);
    // Redelivery of the whole batch is all duplicates.
    let (_, v) = w
        .post(r1, "/events", json!({"events": [event("e1", mine)]}))
        .await;
    assert_eq!(v["duplicates"], json!(["e1"]));
    // More than 100 events is a request error.
    let many: Vec<Value> = (0..101).map(|i| event(&format!("m{i}"), mine)).collect();
    let (s, _) = w.post(r1, "/events", json!({ "events": many })).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn claim_respects_ownership_and_skip_locked_across_replicas() {
    let w = Arc::new(world().await);
    let r1 = w.replica().await;
    let r2 = w.replica().await;
    let i1 = w.integration("T1", r1).await;
    let i2 = w.integration("T2", r2).await;
    let mut want1 = vec![];
    let mut want2 = vec![];
    for k in 0..15 {
        want1.push(w.enqueue(Some(i1), "message.post", json!({"k": k})).await);
        want2.push(w.enqueue(Some(i2), "message.post", json!({"k": k})).await);
    }
    let shared = w.enqueue(None, "credentials.validate", json!({})).await;

    let (a, b) = tokio::join!(
        {
            let w = w.clone();
            async move { w.claim(r1, 100, 0).await }
        },
        {
            let w = w.clone();
            async move { w.claim(r2, 100, 0).await }
        }
    );
    let ids = |v: &[Value]| -> Vec<Uuid> {
        v.iter()
            .map(|o| o["op_id"].as_str().unwrap().parse().unwrap())
            .collect()
    };
    let (a, b) = (ids(&a), ids(&b));
    for id in &want1 {
        assert!(a.contains(id) && !b.contains(id));
    }
    for id in &want2 {
        assert!(b.contains(id) && !a.contains(id));
    }
    // The integration-less op went to exactly one replica.
    assert_eq!(
        usize::from(a.contains(&shared)) + usize::from(b.contains(&shared)),
        1
    );
    assert_eq!(a.len() + b.len(), 31);
    let (st, att, _, _) = w.op(want1[0]).await;
    assert_eq!((st.as_str(), att), ("claimed", 1));
    // Nothing left; a claim never hands out leased ops.
    assert!(w.claim(r1, 100, 0).await.is_empty());
    // Dedupe: same key twice yields one op.
    let mut tx = w.h.pool().begin().await.unwrap();
    let x = ops::enqueue(
        &mut tx,
        new_op(Some(i1), "message.post", json!({}), Some("k1")),
    )
    .await
    .unwrap();
    let y = ops::enqueue(
        &mut tx,
        new_op(Some(i1), "message.post", json!({}), Some("k1")),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(x, y);
    // A foreign replica cannot report on r1's op.
    let (s, v) = w
        .result(r2, json!({"op_id": want1[1], "outcome": "ok", "data": {}}))
        .await;
    assert_eq!(s, 403, "{v}");
    let (s, _) = w
        .result(
            r1,
            json!({"op_id": Uuid::now_v7(), "outcome": "ok", "data": {}}),
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn long_poll_wakes_on_new_op() {
    let w = Arc::new(world().await);
    let r1 = w.replica().await;
    let i1 = w.integration("T1", r1).await;
    let t0 = std::time::Instant::now();
    let poll = {
        let w = w.clone();
        tokio::spawn(async move { w.claim(r1, 10, 20).await })
    };
    tokio::time::sleep(Duration::from_millis(600)).await;
    let id = w.enqueue(Some(i1), "message.post", json!({})).await;
    let got = tokio::time::timeout(Duration::from_secs(10), poll)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["op_id"], id.to_string());
    assert_eq!(got[0]["maybe_sent"], false);
    assert_eq!(got[0]["attempt"], 1);
    assert!(t0.elapsed() < Duration::from_secs(8), "woke by notify");
    // An empty poll returns after its wait with no ops.
    let t1 = std::time::Instant::now();
    assert!(w.claim(r1, 10, 1).await.is_empty());
    assert!(t1.elapsed() >= Duration::from_millis(900));
}

#[tokio::test]
async fn error_codes_drive_core_behaviour() {
    let w = world().await;
    let r1 = w.replica().await;
    let i1 = w.integration("T1", r1).await;
    let pool = w.h.pool();

    // ok -> done + result
    let id = w.enqueue(Some(i1), "message.post", json!({})).await;
    w.claim(r1, 1, 0).await;
    let (s, v) = w
        .result(
            r1,
            json!({"op_id": id, "outcome": "ok", "data": {"message_ref": "m1"}}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (st, _, _, res) = w.op(id).await;
    assert_eq!(st, "done");
    assert_eq!(res.unwrap(), json!({"message_ref": "m1"}));
    // A repeated result is an idempotent no-op.
    let (s, _) = w
        .result(r1, json!({"op_id": id, "outcome": "ok", "data": {"x": 1}}))
        .await;
    assert_eq!(s, 200);
    assert_eq!(w.op(id).await.3.unwrap(), json!({"message_ref": "m1"}));

    let err = |id: Uuid, code: &str, retry: Option<u32>| {
        let mut e = json!({"code": code, "message": "m"});
        if let Some(r) = retry {
            e["retry_after_secs"] = json!(r);
        }
        json!({"op_id": id, "outcome": "error", "error": e})
    };

    // rate_limited: back to pending, delayed, attempt refunded.
    let id = w.enqueue(Some(i1), "message.post", json!({})).await;
    assert_eq!(w.claim(r1, 1, 0).await.len(), 1);
    let (s, _) = w.result(r1, err(id, "rate_limited", Some(120))).await;
    assert_eq!(s, 200);
    let (st, att, _, _) = w.op(id).await;
    assert_eq!((st.as_str(), att), ("pending", 0));
    let secs: f64 = sqlx::query_scalar(
        "SELECT extract(epoch from not_before - now())::float8 FROM channel_ops WHERE id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!((100.0..=121.0).contains(&secs), "{secs}");
    assert!(w.claim(r1, 1, 0).await.is_empty(), "not due yet");
    // Default retry_after is 30 s.
    sqlx::query("UPDATE channel_ops SET not_before=now() WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    w.claim(r1, 1, 0).await;
    w.result(r1, err(id, "rate_limited", None)).await;
    let secs: f64 = sqlx::query_scalar(
        "SELECT extract(epoch from not_before - now())::float8 FROM channel_ops WHERE id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!((25.0..=31.0).contains(&secs), "{secs}");

    // unavailable: exponential backoff, then failed at OP_MAX_ATTEMPTS.
    let id = w.enqueue(Some(i1), "message.post", json!({})).await;
    let mut last = 0.0;
    for attempt in 1..aurora_core::limits::OP_MAX_ATTEMPTS as i32 {
        sqlx::query("UPDATE channel_ops SET not_before=now() WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        let got = w.claim(r1, 1, 0).await;
        assert_eq!(got[0]["attempt"], attempt);
        w.result(r1, err(id, "unavailable", None)).await;
        let (st, att, _, _) = w.op(id).await;
        assert_eq!((st.as_str(), att), ("pending", attempt));
        let secs: f64 = sqlx::query_scalar(
            "SELECT extract(epoch from not_before - now())::float8 FROM channel_ops WHERE id=$1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert!(secs > last * 0.9 || secs >= 590.0, "{secs} after {last}");
        last = secs;
    }
    sqlx::query("UPDATE channel_ops SET not_before=now() WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    w.claim(r1, 1, 0).await;
    w.result(r1, err(id, "unavailable", None)).await;
    let (st, _, code, _) = w.op(id).await;
    assert_eq!(
        (st.as_str(), code.as_deref()),
        ("failed", Some("unavailable"))
    );

    // credential_dead: integration disabled with a reason, op failed.
    let id = w.enqueue(Some(i1), "message.post", json!({})).await;
    w.claim(r1, 1, 0).await;
    w.result(r1, err(id, "credential_dead", None)).await;
    let (st, _, code, _) = w.op(id).await;
    assert_eq!(
        (st.as_str(), code.as_deref()),
        ("failed", Some("credential_dead"))
    );
    let (ist, reason): (String, Option<String>) =
        sqlx::query_as("SELECT status, status_reason FROM channel_integrations WHERE id=$1")
            .bind(i1)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        (ist.as_str(), reason.as_deref()),
        ("disabled", Some("credential_dead"))
    );

    // The remaining codes on a fresh integration.
    let i2 = w.integration("T2", r1).await;
    for (code, want) in [
        ("forbidden", "failed"),
        ("invalid", "failed"),
        ("too_large", "failed"),
        ("unsupported", "failed"),
        ("not_found", "dropped"),
        ("stale_version", "failed"),
        ("something_new", "failed"),
    ] {
        let id = w.enqueue(Some(i2), "message.post", json!({})).await;
        w.claim(r1, 1, 0).await;
        let (s, v) = w.result(r1, err(id, code, None)).await;
        assert_eq!(s, 200, "{code}: {v}");
        let (st, _, ec, res) = w.op(id).await;
        assert_eq!((st.as_str(), ec.as_deref()), (want, Some(code)), "{code}");
        assert_eq!(res.unwrap()["error"]["code"], code);
    }
    // Bad shapes are rejected.
    let (s, _) = w
        .result(r1, json!({"op_id": Uuid::now_v7(), "outcome": "error"}))
        .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn lapsed_lease_reclaim_sets_maybe_sent() {
    let w = world().await;
    let r1 = w.replica().await;
    let r2 = w.replica().await;
    let i1 = w.integration("T1", r1).await;
    let pool = w.h.pool();

    // Direct re-claim of a `claimed` op whose lease lapsed.
    let a = w.enqueue(Some(i1), "message.post", json!({})).await;
    let got = w.claim(r1, 1, 0).await;
    assert_eq!(
        (got[0]["maybe_sent"].as_bool(), got[0]["attempt"].as_u64()),
        (Some(false), Some(1))
    );
    assert!(w.claim(r1, 1, 0).await.is_empty(), "lease still live");
    sqlx::query("UPDATE channel_ops SET lease_until = now() - interval '1 second' WHERE id=$1")
        .bind(a)
        .execute(pool)
        .await
        .unwrap();
    let got = w.claim(r1, 1, 0).await;
    assert_eq!(got.len(), 1);
    assert_eq!(
        (got[0]["maybe_sent"].as_bool(), got[0]["attempt"].as_u64()),
        (Some(true), Some(2))
    );

    // Through the sweeper: lapsed -> pending, next claim (even after takeover) is maybe_sent.
    sqlx::query("UPDATE channel_ops SET lease_until = now() - interval '1 second' WHERE id=$1")
        .bind(a)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(ops::sweep_lapsed(pool).await.unwrap(), 1);
    assert_eq!(w.op(a).await.0, "pending");
    sqlx::query("UPDATE channel_integrations SET owner_replica_id=$2 WHERE id=$1")
        .bind(i1)
        .bind(r2)
        .execute(pool)
        .await
        .unwrap();
    let got = w.claim(r2, 1, 0).await;
    assert_eq!(
        (got[0]["maybe_sent"].as_bool(), got[0]["attempt"].as_u64()),
        (Some(true), Some(3))
    );
    // The previous claimant may no longer report.
    let (s, _) = w
        .result(r1, json!({"op_id": a, "outcome": "ok", "data": {}}))
        .await;
    assert_eq!(s, 403);
    let (s, _) = w
        .result(r2, json!({"op_id": a, "outcome": "ok", "data": {}}))
        .await;
    assert_eq!(s, 200);

    // Exhausted attempts fail at the sweep.
    let b = w.enqueue(Some(i1), "message.post", json!({})).await;
    w.claim(r2, 1, 0).await;
    sqlx::query(
        "UPDATE channel_ops SET attempts=10, lease_until=now() - interval '1 second' WHERE id=$1",
    )
    .bind(b)
    .execute(pool)
    .await
    .unwrap();
    ops::sweep_lapsed(pool).await.unwrap();
    let (st, _, code, _) = w.op(b).await;
    assert_eq!(
        (st.as_str(), code.as_deref()),
        ("failed", Some("unavailable"))
    );
}

#[tokio::test]
async fn correlated_calls_succeed_fail_and_time_out() {
    let w = Arc::new(world().await);
    let r1 = w.replica().await;
    let r2 = w.replica().await;

    // Success; the op has no integration, so the *second* replica can serve it.
    let call = {
        let w = w.clone();
        tokio::spawn(async move {
            ops::call_correlated(
                &w.h.state,
                new_op(None, "credentials.validate", json!({"config": {}}), None),
                Duration::from_secs(10),
            )
            .await
        })
    };
    let got = loop {
        let got = w.claim(r2, 5, 5).await;
        if !got.is_empty() {
            break got;
        }
    };
    let id: Uuid = got[0]["op_id"].as_str().unwrap().parse().unwrap();
    assert!(got[0].get("integration_id").is_none());
    let corr: Option<Uuid> =
        sqlx::query_scalar("SELECT correlation_id FROM channel_ops WHERE id=$1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(corr.is_some());
    let (s, _) = w
        .result(
            r2,
            json!({"op_id": id, "outcome": "ok", "data": {"tenant_ref": "T9"}}),
        )
        .await;
    assert_eq!(s, 200);
    let v = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(v, json!({"tenant_ref": "T9"}));

    // Definitive error is mapped, not timed out.
    let call = {
        let w = w.clone();
        tokio::spawn(async move {
            ops::call_correlated(
                &w.h.state,
                new_op(None, "history.read", json!({}), None),
                Duration::from_secs(10),
            )
            .await
        })
    };
    let got = loop {
        let got = w.claim(r1, 5, 5).await;
        if !got.is_empty() {
            break got;
        }
    };
    let id: Uuid = got[0]["op_id"].as_str().unwrap().parse().unwrap();
    w.result(
        r1,
        json!({"op_id": id, "outcome": "error", "error": {"code": "forbidden", "message": "no"}}),
    )
    .await;
    let e = call.await.unwrap().unwrap_err();
    assert_eq!(
        e,
        CorrelatedError::Failed {
            code: "forbidden".into(),
            message: "no".into()
        }
    );

    // Nobody claims it: connector_unavailable, and the op is dropped.
    let t = std::time::Instant::now();
    let e = ops::call_correlated(
        &w.h.state,
        new_op(None, "user.lookup", json!({}), None),
        Duration::from_millis(1500),
    )
    .await
    .unwrap_err();
    assert_eq!(e, CorrelatedError::Unavailable);
    assert!(t.elapsed() >= Duration::from_millis(1400));
    let api: aurora_core::ApiError = e.into();
    assert_eq!(api.code(), "connector_unavailable");
    let st: String = sqlx::query_scalar("SELECT status FROM channel_ops WHERE kind='user.lookup'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(st, "dropped");
    assert!(w.claim(r1, 5, 0).await.is_empty());
}

#[tokio::test]
async fn file_put_and_get_authorization_and_size_cap() {
    let w = world().await;
    let r1 = w.replica().await;
    let r2 = w.replica().await;
    let i1 = w.integration("T1", r1).await;
    let conv = w.conversation().await;

    let fetch = w
        .enqueue(
            Some(i1),
            "file.fetch",
            json!({"file_ref": "F1", "max_bytes": 16, "conversation_id": conv}),
        )
        .await;
    // Not claimed yet -> 404.
    let (s, _) = w
        .raw(
            r1,
            Method::PUT,
            &format!("/files/{fetch}"),
            &[],
            b"abc".to_vec(),
        )
        .await;
    assert_eq!(s, 404);
    w.claim(r1, 1, 0).await;
    // Another replica -> 404.
    let (s, _) = w
        .raw(
            r2,
            Method::PUT,
            &format!("/files/{fetch}"),
            &[],
            b"abc".to_vec(),
        )
        .await;
    assert_eq!(s, 404);
    // Over the op's max_bytes (declared and actual) -> 413, nothing stored.
    let (s, _) = w
        .raw(
            r1,
            Method::PUT,
            &format!("/files/{fetch}"),
            &[],
            vec![0; 17],
        )
        .await;
    assert_eq!(s, 413);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    // Stored: sanitized name, inbound, pending.
    let (s, body) = w
        .raw(
            r1,
            Method::PUT,
            &format!("/files/{fetch}"),
            &[
                ("x-aurora-filename", "../../etc/pa ss.txt"),
                ("content-type", "text/plain; charset=utf-8"),
            ],
            b"hello".to_vec(),
        )
        .await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&body));
    let att: Uuid = serde_json::from_slice::<Value>(&body).unwrap()["attachment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (name, dir, st, size, ct, sha): (String, String, String, i64, String, String) = sqlx::query_as(
        "SELECT filename, direction, status, size_bytes, content_type, sha256 FROM attachments WHERE id=$1",
    )
    .bind(att)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (name.as_str(), dir.as_str(), st.as_str(), size, ct.as_str()),
        ("pa_ss.txt", "in", "pending", 5, "text/plain")
    );
    assert_eq!(sha, format!("{:x}", Sha256::digest(b"hello")));

    // A non-file.fetch op and an op without destination are refused.
    let post = w.enqueue(Some(i1), "message.post", json!({})).await;
    let nodest = w
        .enqueue(
            Some(i1),
            "file.fetch",
            json!({"file_ref": "F2", "max_bytes": 16}),
        )
        .await;
    w.claim(r1, 5, 0).await;
    let (s, _) = w
        .raw(
            r1,
            Method::PUT,
            &format!("/files/{post}"),
            &[],
            b"x".to_vec(),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .raw(
            r1,
            Method::PUT,
            &format!("/files/{nodest}"),
            &[],
            b"x".to_vec(),
        )
        .await;
    assert_eq!(s, 422);

    // GET: only for an attachment referenced by a message.post claimed by this replica.
    let (s, _) = w
        .raw(r1, Method::GET, &format!("/files/{att}"), &[], vec![])
        .await;
    assert_eq!(s, 404, "unreferenced");
    let p = w
        .enqueue(
            Some(i1),
            "message.post",
            json!({"files": [{"attachment_id": att, "name": "a", "download_url": "u"}]}),
        )
        .await;
    let (s, _) = w
        .raw(r1, Method::GET, &format!("/files/{att}"), &[], vec![])
        .await;
    assert_eq!(s, 404, "referenced but not claimed");
    let got = w.claim(r1, 5, 0).await;
    assert!(got.iter().any(|o| o["op_id"] == p.to_string()));
    let (s, _) = w
        .raw(r2, Method::GET, &format!("/files/{att}"), &[], vec![])
        .await;
    assert_eq!(s, 404, "claimed by someone else");
    let (s, body) = w
        .raw(r1, Method::GET, &format!("/files/{att}"), &[], vec![])
        .await;
    assert_eq!((s, body.as_slice()), (200, b"hello".as_slice()));
    // Once the op is done the file is no longer readable.
    w.result(r1, json!({"op_id": p, "outcome": "ok", "data": {}}))
        .await;
    let (s, _) = w
        .raw(r1, Method::GET, &format!("/files/{att}"), &[], vec![])
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn link_start_creates_a_hashed_30_minute_request() {
    let w = world().await;
    let r1 = w.replica().await;
    let r2 = w.replica().await;
    let i1 = w.integration("T1", r1).await;

    let (s, v) = w
        .post(
            r1,
            "/link-start",
            json!({"integration_id": i1, "platform_user_ref": "U1"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let url = v["url"].as_str().unwrap();
    let base =
        w.h.state
            .config
            .external_url
            .trim_end_matches('/')
            .to_owned();
    let tok = url
        .strip_prefix(&format!("{base}/link/channel?token="))
        .unwrap();
    assert!(tok.starts_with("aui_"));
    let (hash, ttl): (Vec<u8>, f64) = sqlx::query_as(
        "SELECT token_hash, extract(epoch from expires_at - now())::float8 FROM link_requests
          WHERE platform_user_ref='U1' AND integration_id=$1",
    )
    .bind(i1)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(hash, aurora_core::token::hash(tok).to_vec());
    assert!((1700.0..=1801.0).contains(&ttl), "{ttl}");
    assert!(chrono::DateTime::parse_from_rfc3339(v["expires_at"].as_str().unwrap()).is_ok());

    // Not owned by this replica, unknown, or empty user ref.
    let (s, _) = w
        .post(
            r2,
            "/link-start",
            json!({"integration_id": i1, "platform_user_ref": "U1"}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .post(
            r1,
            "/link-start",
            json!({"integration_id": Uuid::now_v7(), "platform_user_ref": "U1"}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .post(
            r1,
            "/link-start",
            json!({"integration_id": i1, "platform_user_ref": ""}),
        )
        .await;
    assert_eq!(s, 400);
}

struct Counting {
    seen: AtomicUsize,
    kind: &'static str,
}

#[async_trait]
impl InboxHandler for Counting {
    fn handles(&self, kind: &str) -> bool {
        kind == self.kind
    }
    async fn handle(
        &self,
        _: &aurora_api::app::AppState,
        row: &InboxRow,
    ) -> Result<Value, InboxError> {
        self.seen.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        if row.event_id == "bad" {
            return Err(InboxError::Fatal("boom".into()));
        }
        if row.event_id == "flaky" && row.attempts == 1 {
            return Err(InboxError::Retry("try later".into()));
        }
        Ok(json!({"handled": row.event_id}))
    }
}

#[tokio::test]
async fn inbox_processor_handles_each_row_exactly_once() {
    let w = Arc::new(world().await);
    let r1 = w.replica().await;
    let i1 = w.integration("T1", r1).await;
    let mut events: Vec<Value> = (0..40).map(|i| event(&format!("e{i}"), i1)).collect();
    events.push(event("bad", i1));
    events.push(event("flaky", i1));
    let mut other = event("o1", i1);
    other["kind"] = json!("bot.added");
    events.push(other);
    let (s, v) = w.post(r1, "/events", json!({ "events": events })).await;
    assert_eq!(s, 200, "{v}");

    let h = Arc::new(Counting {
        seen: AtomicUsize::new(0),
        kind: "message.posted",
    });
    let handlers: Vec<Arc<dyn InboxHandler>> = vec![h.clone()];
    let run = |w: Arc<W>, handlers: Vec<Arc<dyn InboxHandler>>| async move {
        let mut n = 0;
        loop {
            let k = inbox::process_batch(&w.h.state, &handlers).await.unwrap();
            if k == 0 {
                break;
            }
            n += k;
        }
        n
    };
    let (a, b) = tokio::join!(
        run(w.clone(), handlers.clone()),
        run(w.clone(), handlers.clone())
    );
    // 43 rows claimed once each; the flaky one is re-queued with backoff, not re-claimed yet.
    assert_eq!(a + b, 43);
    // Every message.posted row but the retried one was handled exactly once.
    assert_eq!(h.seen.load(Ordering::SeqCst), 42);
    let rows: Vec<(String, String, i32, Option<Value>)> = sqlx::query_as(
        "SELECT event_id, status, attempts, outcome FROM channel_inbox ORDER BY event_id",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    for (id, st, att, out) in &rows {
        match id.as_str() {
            "bad" => assert_eq!((st.as_str(), *att), ("failed", 1)),
            "flaky" => assert_eq!((st.as_str(), *att), ("pending", 1)),
            "o1" => {
                assert_eq!(st, "done");
                assert_eq!(out.as_ref().unwrap(), &json!({"result": "unhandled"}));
            }
            _ => {
                assert_eq!((st.as_str(), *att), ("done", 1), "{id}");
                assert_eq!(out.as_ref().unwrap(), &json!({"handled": id}));
            }
        }
    }
    // The retry becomes due and is then handled once more.
    sqlx::query("UPDATE channel_inbox SET next_attempt_at=now() WHERE event_id='flaky'")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(
        inbox::process_batch(&w.h.state, &handlers).await.unwrap(),
        1
    );
    let st: String = sqlx::query_scalar("SELECT status FROM channel_inbox WHERE event_id='flaky'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(st, "done");
    // A lapsed processing lease is redelivered.
    sqlx::query("UPDATE channel_inbox SET status='processing', lease_until=now() - interval '1 second' WHERE event_id='e0'")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(
        inbox::process_batch(&w.h.state, &handlers).await.unwrap(),
        1
    );
}

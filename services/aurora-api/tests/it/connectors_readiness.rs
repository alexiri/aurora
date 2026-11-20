#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (P5-REFINE-6, C-1068, D-407) One readiness for a chat platform: `GET /v1/connector-availability`
//! (the organization's Chat platforms) and `GET /v1/admin/connectors` (Administration) agree on
//! every state, a managed connector whose type a live host announces is `registering` (never
//! "waiting for the connector host") and becomes `active` with its first registration, and a
//! development host's unsigned connector registers under `plugins.allow_unsigned` (the cause of
//! UX review 5 org-1/admin-5).

use crate::common::*;
use crate::managed_support::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

/// `register` on the connector plane as the host, for connector `id`, with `signature`.
async fn register_as_host(m: &M, id: &str, manifest: &Value, signature: &str) -> (u16, Value) {
    let b = bearer(&m.host_token);
    let (s, _, v) = m
        .h
        .send(
            Method::POST,
            "/v1/connector/register",
            &[("authorization", b.as_str()), ("aurora-connector", id)],
            Some(json!({"manifest": manifest, "signature": signature, "version": "1.0.0"})),
        )
        .await;
    (s.as_u16(), v)
}

async fn availability(m: &M, who: &str) -> Vec<Value> {
    let (s, _, v) =
        m.h.send(Method::GET, "/v1/connector-availability", &hd(who), None)
            .await;
    assert_eq!(s, 200, "{v}");
    v["items"].as_array().unwrap().clone()
}

async fn admin_list(m: &M) -> Vec<Value> {
    let (s, v) = m.admin(Method::GET, "/v1/admin/connectors", None).await;
    assert_eq!(s, 200, "{v}");
    v["items"].as_array().unwrap().clone()
}

fn by_type<'a>(items: &'a [Value], type_id: &str) -> &'a Value {
    items
        .iter()
        .find(|i| i["type_id"] == type_id)
        .unwrap_or_else(|| panic!("no {type_id}: {items:?}"))
}

fn by_id<'a>(items: &'a [Value], id: &str) -> &'a Value {
    items
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("no {id}: {items:?}"))
}

/// An organization admin's session (the `connect` next step).
async fn org_admin(m: &M) -> String {
    let org = m.h.org().await;
    let u = m.h.user("orgadmin@x.io", false).await;
    m.h.member(org, u, "admin", "active").await;
    cookie(&m.h.good_session(u).await.1)
}

#[tokio::test]
async fn a_running_hosts_unsigned_connector_registers_and_becomes_connectable() {
    // A development install: the host build embeds no signature (connectors/builtin/build.rs).
    let m = M::with_allow_unsigned().await;
    let alpha = manifest("alpha");
    let host = Uuid::now_v7();
    let (s, v) = m
        .host_register_raw(
            host,
            "1.0.0",
            json!([{"type_id": "alpha", "manifest": alpha, "manifest_signature": ""}]),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["rejected"], json!([]), "{v}");
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(m.create_body("alpha")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // Added, its host runs: registering on both pages, never "waiting for the connector host".
    let a = availability(&m, &m.admin).await;
    let item = by_type(&a, "alpha");
    assert_eq!(item["setup_state"], "registering", "{item}");
    assert_eq!(item["host_running"], true);
    assert_eq!(item["can_connect"], false);
    let row = by_id(&admin_list(&m).await, "alpha").clone();
    assert_eq!(row["setup_state"], "registering", "{row}");
    assert_eq!(row["host_running"], true);
    // The host hands it out (one poll) and the connector registers with the empty signature the
    // host embedded: accepted under `allow_unsigned`, as the host's own `register` was.
    let (_, _, asg) = m.assignments(Some(host), None, 0).await;
    assert_eq!(asg["connectors"][0]["id"], "alpha", "{asg}");
    let (s, v) = register_as_host(&m, "alpha", &alpha, "").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "active", "{v}");
    let a = availability(&m, &m.admin).await;
    let item = by_type(&a, "alpha");
    assert_eq!(item["setup_state"], "active", "{item}");
    assert_eq!(item["can_connect"], true);
    // The instance admin belongs to no organization: an organization owner connects it.
    assert_eq!(item["next_step"], "owner_connects");
    let row = by_id(&admin_list(&m).await, "alpha").clone();
    assert_eq!(
        (row["status"].as_str(), row["setup_state"].as_str()),
        (Some("active"), Some("active"))
    );
    assert_eq!(row["next_step"], "owner_connects");
    // An organization admin is told to connect it.
    let oa = org_admin(&m).await;
    assert_eq!(
        by_type(&availability(&m, &oa).await, "alpha")["next_step"],
        "connect"
    );
}

#[tokio::test]
async fn an_unsigned_registration_is_refused_without_allow_unsigned_and_a_bad_one_always() {
    let m = M::new().await;
    let alpha = manifest("alpha");
    m.add_managed("alpha").await;
    let (s, v) = register_as_host(&m, "alpha", &alpha, "").await;
    assert_eq!(s, 422, "{v}");
    let row = by_id(&admin_list(&m).await, "alpha").clone();
    assert_eq!(row["status"], "awaiting_registration");
    // With the flag, a present but wrong signature is still verified and refused.
    let m = M::with_allow_unsigned().await;
    m.add_managed("alpha").await;
    let (s, v) = register_as_host(&m, "alpha", &alpha, "garbage").await;
    assert_eq!(s, 422, "{v}");
}

#[tokio::test]
async fn both_pages_report_the_same_state_for_every_state() {
    let m = M::new().await;
    // Announced by a live host: alpha (registering), bravo (active), charlie (disabled),
    // delta (pending approval), echo (not added).
    let host = Uuid::now_v7();
    let ms: Vec<Value> = ["alpha", "bravo", "charlie", "delta", "echo"]
        .iter()
        .map(|t| manifest(t))
        .collect();
    let refs: Vec<&Value> = ms.iter().collect();
    let (s, v) = m.host_register(host, "1.0.0", &refs).await;
    assert_eq!(s, 200, "{v}");
    for t in ["alpha", "bravo", "charlie", "delta"] {
        let (s, v) = m
            .admin(Method::POST, "/v1/admin/connectors", Some(m.create_body(t)))
            .await;
        assert_eq!(s, 201, "{v}");
    }
    let (s, v) = register_as_host(&m, "bravo", &ms[1], &m.sign(&ms[1])).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/charlie",
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    sqlx::query("UPDATE connectors SET status = 'pending_approval' WHERE id = 'delta'")
        .execute(m.h.pool())
        .await
        .unwrap();
    // No live host: a self-hosted connector that never registered.
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(json!({"id": "selfhosted", "display_name": "Self"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");

    let avail = availability(&m, &m.admin).await;
    let admin = admin_list(&m).await;
    let want = [
        ("alpha", "registering", true),
        ("bravo", "active", true),
        ("charlie", "disabled", true),
        ("delta", "needs_approval", true),
        ("selfhosted", "waiting_for_host", false),
    ];
    for (id, state, running) in want {
        let a = avail
            .iter()
            .find(|i| i["connector_id"] == id)
            .unwrap_or_else(|| panic!("no {id}: {avail:?}"));
        let r = by_id(&admin, id);
        assert_eq!(a["setup_state"], state, "{id}: {a}");
        assert_eq!(a["setup_state"], r["setup_state"], "{id}: the two pages agree");
        assert_eq!(a["host_running"], json!(running), "{id}: {a}");
        assert_eq!(a["host_running"], r["host_running"], "{id}: the two pages agree");
        assert_eq!(a["can_connect"], json!(state == "active"), "{id}");
    }
    let next = |id: &str| {
        avail
            .iter()
            .find(|i| i["connector_id"] == id)
            .unwrap()["next_step"]
            .clone()
    };
    assert_eq!(next("charlie"), "enable");
    assert_eq!(next("delta"), "approve");
    assert_eq!(next("selfhosted"), "start_host");
    assert_eq!(by_id(&admin, "selfhosted")["next_step"], "start_host");
    let echo = by_type(&avail, "echo");
    assert_eq!(
        (
            echo["setup_state"].as_str(),
            echo["host_running"].as_bool(),
            echo["next_step"].as_str()
        ),
        (Some("not_added"), Some(true), Some("add"))
    );
}

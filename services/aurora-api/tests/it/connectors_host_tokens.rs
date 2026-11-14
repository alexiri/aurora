#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-120 (D-178 Q-R5-8, D-189; C-690..C-693; SEC-69/SEC-70): one connector-host token per host,
//! bound to the connector types that host announced.

use crate::common::*;
use crate::managed_support::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

/// Issue a bound token as the instance admin; its `auh_…` value and id.
async fn issue(m: &M, name: &str, types: &[&str]) -> (String, String) {
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connector-host-tokens",
            Some(json!({"name": name, "type_ids": types})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    (
        v["token"].as_str().unwrap().to_owned(),
        v["id"].as_str().unwrap().to_owned(),
    )
}

async fn register(m: &M, tok: &str, instance: Uuid, types: &[&str]) -> (u16, Value) {
    let ts: Vec<Value> = types
        .iter()
        .map(|t| {
            let mf = manifest(t);
            json!({"type_id": t, "manifest": mf, "manifest_signature": m.sign(&mf)})
        })
        .collect();
    let (s, _, v) = m
        .host_req(
            Some(tok),
            Method::POST,
            "/register",
            Some(json!({"instance_id": instance, "version": "1.0.0", "types": ts})),
        )
        .await;
    (s, v)
}

async fn poll(m: &M, tok: &str, instance: Option<Uuid>) -> (u16, Vec<String>) {
    let q = instance.map_or(String::new(), |i| format!("&instance_id={i}"));
    let (s, _, v) = m
        .host_req(
            Some(tok),
            Method::GET,
            &format!("/assignments?wait=0{q}"),
            None,
        )
        .await;
    let ids = v["connectors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| c["id"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default();
    (s, ids)
}

async fn status(m: &M, tok: &str, instance: Uuid, connector: &str) -> u16 {
    m.host_req(
        Some(tok),
        Method::POST,
        "/status",
        Some(json!({"instance_id": instance,
            "connectors": [{"id": connector, "state": "running"}]})),
    )
    .await
    .0
}

/// Two managed connectors (`alpha`, `bravo`) created while only the shared bootstrap token exists.
async fn two_types(m: &M) {
    m.add_managed("alpha").await;
    m.add_managed("bravo").await;
}

#[tokio::test]
async fn a_token_bound_to_one_type_cannot_register_another() {
    let m = M::new().await;
    let (tok, _) = issue(&m, "mm-a", &["alpha"]).await;
    let host = Uuid::now_v7();
    // The bound type registers; any other type in the request refuses the whole call.
    let (s, v) = register(&m, &tok, host, &["alpha"]).await;
    assert_eq!(s, 200, "{v}");
    let other = Uuid::now_v7();
    let (s, v) = register(&m, &tok, other, &["alpha", "bravo"]).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "type_not_bound", "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("bravo"),
        "{v}"
    );
    // Nothing of the refused request was stored.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connector_hosts WHERE instance_id = $1")
        .bind(other)
        .fetch_one(m.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn assignments_and_credentials_never_cross_types_or_hosts() {
    let m = M::new().await;
    two_types(&m).await;
    let (ta, _) = issue(&m, "mm-a", &["alpha"]).await;
    let (tb, _) = issue(&m, "mm-b", &["bravo"]).await;
    let (xa, yb) = (Uuid::now_v7(), Uuid::now_v7());
    assert_eq!(register(&m, &ta, xa, &["alpha"]).await.0, 200);
    assert_eq!(register(&m, &tb, yb, &["bravo"]).await.0, 200);

    // Each token receives only its own type's connector (and so its decrypted settings).
    assert_eq!(
        poll(&m, &ta, Some(xa)).await,
        (200, vec!["alpha".to_owned()])
    );
    assert_eq!(poll(&m, &ta, None).await, (200, vec!["alpha".to_owned()]));
    assert_eq!(
        poll(&m, &tb, Some(yb)).await,
        (200, vec!["bravo".to_owned()])
    );
    assert_eq!(poll(&m, &tb, None).await, (200, vec!["bravo".to_owned()]));

    // Host X's token with host Y's instance id: refused on every call.
    assert_eq!(poll(&m, &ta, Some(yb)).await.0, 403);
    assert_eq!(poll(&m, &tb, Some(xa)).await.0, 403);
    assert_eq!(status(&m, &ta, yb, "alpha").await, 403);
    let (s, v) = register(&m, &ta, yb, &["alpha"]).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("host_instance_mismatch"))
    );

    // Status reports about another type's connector are ignored, not stored.
    assert_eq!(status(&m, &ta, xa, "bravo").await, 204);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connector_host_status WHERE connector_id = 'bravo'",
    )
    .fetch_one(m.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(status(&m, &ta, xa, "alpha").await, 204);
}

#[tokio::test]
async fn delegated_auth_is_limited_to_the_bound_types() {
    let m = M::new().await;
    two_types(&m).await;
    let (ta, _) = issue(&m, "mm-a", &["alpha"]).await;
    let call = |id: &'static str| {
        let (m, ta) = (&m, ta.clone());
        async move {
            let b = bearer(&ta);
            let mf = manifest(id);
            let (s, _, v) =
                m.h.send(
                    Method::POST,
                    "/v1/connector/register",
                    &[("authorization", &b), ("aurora-connector", id)],
                    Some(json!({"manifest": mf, "signature": m.sign(&mf), "version": "1.0.0"})),
                )
                .await;
            (s.as_u16(), v)
        }
    };
    let (s, v) = call("alpha").await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call("bravo").await;
    assert_eq!(s, 401, "{v}");
}

#[tokio::test]
async fn the_shared_bootstrap_token_works_until_a_bound_token_exists_and_a_revoked_one_stops() {
    let m = M::new().await;
    let legacy = m.host_token.clone();
    // Before any bound token: the shared token acts for any type (existing installs).
    let host = Uuid::now_v7();
    assert_eq!(
        register(&m, &legacy, host, &["alpha", "bravo"]).await.0,
        200
    );
    assert_eq!(poll(&m, &legacy, None).await.0, 200);
    let (_, list) = m
        .admin(Method::GET, "/v1/admin/connector-host-tokens", None)
        .await;
    assert_eq!(list["items"][0]["state"], "active", "{list}");
    assert!(list["items"][0]["type_ids"].is_null());

    let (tok, id) = issue(&m, "mm-a", &["alpha"]).await;
    // From the first bound token on, it is refused.
    let (s, _, v) = m
        .host_req(Some(&legacy), Method::GET, "/assignments?wait=0", None)
        .await;
    assert_eq!(s, 401, "{v}");
    assert_eq!(register(&m, &legacy, host, &["alpha"]).await.0, 401);
    assert_eq!(poll(&m, &tok, None).await.0, 200);
    let (_, list) = m
        .admin(Method::GET, "/v1/admin/connector-host-tokens", None)
        .await;
    let states: Vec<(String, String)> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["name"].as_str().unwrap_or("-").to_owned(),
                i["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        states.contains(&("-".to_owned(), "refused".to_owned())),
        "{states:?}"
    );
    assert!(
        states.contains(&("mm-a".to_owned(), "active".to_owned())),
        "{states:?}"
    );
    // The listing never carries the token itself.
    assert!(!list.to_string().contains(&tok));

    // Revoked: the next call is 401. With no live bound token left the shared token is accepted
    // again (the operator's rollback).
    let (s, _) = m
        .admin(
            Method::DELETE,
            &format!("/v1/admin/connector-host-tokens/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(poll(&m, &tok, None).await.0, 401);
    assert_eq!(poll(&m, &legacy, None).await.0, 200);
    let (s, _) = m
        .admin(
            Method::DELETE,
            &format!("/v1/admin/connector-host-tokens/{}", Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(s, 404);
}

/// The chart's path: `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKENS` is applied at start by name (hash
/// only, idempotent); a rotated token of the same name revokes the old one; the bound token is
/// limited to its types and ends the shared token's validity.
#[tokio::test]
async fn bootstrap_applies_bound_tokens_by_name_and_a_rotation_revokes_the_old_one() {
    use aurora_core::token::{self, TokenPrefix};
    let shared = token::generate(TokenPrefix::ConnectorHost).expose().clone();
    let t1 = token::generate(TokenPrefix::ConnectorHost).expose().clone();
    let t2 = token::generate(TokenPrefix::ConnectorHost).expose().clone();
    let apply = |tok: String| {
        let shared = shared.clone();
        async move {
            let h = Harness::with_config(move |c| {
                c.bootstrap_connector_host_token = Some(aurora_config::Redacted::new(shared));
                c.bootstrap_connector_host_tokens = vec![(
                    "connector-host".to_owned(),
                    aurora_config::Redacted::new(tok),
                    vec!["alpha".to_owned()],
                )];
            })
            .await;
            h
        }
    };
    let h = apply(t1.clone()).await;
    for _ in 0..2 {
        aurora_api::domains::connectors::bootstrap_host_token(&h.state)
            .await
            .unwrap();
    }
    let live = |h: &Harness| {
        let pool = h.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM connector_host_tokens WHERE type_ids IS NOT NULL AND revoked_at IS NULL",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(live(&h).await, 1);
    let get = |tok: &str| {
        let b = bearer(tok);
        let h = &h;
        async move {
            h.send(
                Method::GET,
                "/v1/connector-host/assignments?wait=0",
                &[("authorization", &b)],
                None,
            )
            .await
            .0
            .as_u16()
        }
    };
    assert_eq!(get(&t1).await, 200);
    assert_eq!(
        get(&shared).await,
        401,
        "the shared token ends with the first bound one"
    );
    // Rotation: same name, new token.
    sqlx::query("UPDATE connector_host_tokens SET name = name")
        .execute(h.pool())
        .await
        .unwrap();
    let h2 = apply(t2.clone()).await;
    // A second harness has its own database: replay the rotation on the first one instead.
    drop(h2);
    let mut conn = h.pool().acquire().await.unwrap();
    aurora_db::repo::connector_hosts::upsert_bound_token(
        &mut conn,
        &token::hash(&t2),
        "auh_",
        "connector-host",
        &["alpha".to_owned()],
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(live(&h).await, 1);
    assert_eq!(get(&t1).await, 401);
    assert_eq!(get(&t2).await, 200);
}

#[tokio::test]
async fn issuing_validates_audits_and_needs_an_instance_admin() {
    let m = M::new().await;
    for body in [
        json!({"name": "", "type_ids": ["alpha"]}),
        json!({"name": "x", "type_ids": []}),
        json!({"name": "x", "type_ids": ["Not A Type"]}),
    ] {
        let (s, v) = m
            .admin(Method::POST, "/v1/admin/connector-host-tokens", Some(body))
            .await;
        assert_eq!(s, 400, "{v}");
    }
    issue(&m, "dup", &["alpha"]).await;
    let (s, _) = m
        .admin(
            Method::POST,
            "/v1/admin/connector-host-tokens",
            Some(json!({"name": "dup", "type_ids": ["bravo"]})),
        )
        .await;
    assert_eq!(s, 409);
    // A person who is not an instance admin gets 403.
    let u = m.h.user("member@x.io", false).await;
    let c = cookie(&m.h.good_session(u).await.1);
    let (s, _, _) =
        m.h.send(
            Method::GET,
            "/v1/admin/connector-host-tokens",
            &hd(&c),
            None,
        )
        .await;
    assert_eq!(s, 403);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'admin.connector_host_token_issue'",
    )
    .fetch_one(m.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
}

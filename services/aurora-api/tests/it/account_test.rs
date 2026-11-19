//! (phase 5, P5-UX-14, D-335, C-943, C-944, SEC-129) Test of an account: the connect method's probe with the
//! account's own credentials (after the gateway refreshed or minted the token), the answer a code with at most a
//! host and a status, never an upstream body; the result stored and audited; the health in the accounts list.
//!
//! A child module of `connect` (it reuses that file's world: the fixture package, the mock provider, the owner
//! and a member).
use super::*;
use aurora_api::domains::accounts::gateway::{AccountGateway, Answer, Check};
use aurora_core::Secret;
use aurora_secrets::{Purpose, SealContext};
use std::sync::{Arc, Mutex};

/// The gateway's side of Test, scripted: records what it was asked and answers as told.
struct FakeGateway {
    calls: Mutex<Vec<Check>>,
    answer: Mutex<Answer>,
}

impl FakeGateway {
    fn new(result: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            answer: Mutex::new(Answer {
                result: result.into(),
                host: None,
                code: None,
            }),
        })
    }

    fn calls(&self) -> Vec<Check> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AccountGateway for FakeGateway {
    async fn check(&self, _org: Uuid, _conn: Uuid, check: Check) -> Answer {
        self.calls.lock().unwrap().push(check);
        self.answer.lock().unwrap().clone()
    }
}

/// The world with the fake gateway installed, and a connected `acme_app` account ("fine" token).
async fn connected(gw: &Arc<FakeGateway>) -> (W, Uuid) {
    let mut w = world(None).await;
    w.h.state = w.h.state.clone().with_account_gateway(gw.clone());
    w.h.app = test_router(w.h.state.clone());
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "fine"});
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 200, "{v}");
    let id = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    (w, id)
}

/// Replace the stored token (what the app now refuses) the way a reconnect would seal it.
async fn set_token(w: &W, id: Uuid, token: &str) {
    let ctx = SealContext {
        purpose: Purpose::ConnectionCredentials,
        owner_id: id,
        org_id: w.org,
        workspace_id: Some(w.ws),
    };
    let r =
        w.h.state
            .vault
            .seal(
                &ctx,
                Secret::from(json!({"api_token": token}).to_string().into_bytes()),
            )
            .await
            .unwrap();
    sqlx::query("UPDATE app_connections SET credentials_ref = $2 WHERE id = $1")
        .bind(id)
        .bind(r.to_column())
        .execute(w.h.pool())
        .await
        .unwrap();
}

async fn test_it(w: &W, who: &str, id: Uuid) -> (u16, Value) {
    let (s, _, v) = w
        .call(
            who,
            Method::POST,
            &format!("/v1/accounts/connection/{id}/test"),
            None,
        )
        .await;
    (s, v)
}

async fn stored(w: &W, id: Uuid) -> (Option<String>, bool) {
    sqlx::query_as(
        "SELECT last_test_result, last_test_at IS NOT NULL FROM app_connections WHERE id = $1 \
         AND last_check_at IS NULL AND last_ok_at IS NULL AND last_error_at IS NULL",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap()
}

async fn wait_audit(w: &W, action: &str) -> Value {
    for _ in 0..100 {
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(action)
        .fetch_optional(w.h.pool())
        .await
        .unwrap();
        if let Some(r) = row {
            return r;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no audit row for {action}");
}

async fn health(w: &W, id: Uuid) -> Value {
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/workspaces/{}/accounts", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["ref"]["id"] == id.to_string())
        .unwrap()["health"]
        .clone()
}

#[tokio::test]
async fn a_member_cannot_test_an_account() {
    let gw = FakeGateway::new("ok");
    let (w, id) = connected(&gw).await;
    let (s, v) = test_it(&w, &w.member, id).await;
    assert_eq!(s, 403, "{v}");
    assert!(
        gw.calls().is_empty(),
        "nothing was contacted for a refused caller"
    );
    assert_eq!(stored(&w, id).await, (None, false));
}

#[tokio::test]
async fn a_working_account_is_ok_stored_audited_and_shown_in_the_list() {
    let gw = FakeGateway::new("ok");
    let (w, id) = connected(&gw).await;
    assert_eq!(health(&w, id).await["state"], "not_used");
    let (s, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["result"], "ok", "{v}");
    assert!(v["checked_at"].is_string());
    // The app's token is hook-minted: the gateway refreshed it first, then the probe ran from the API.
    assert_eq!(gw.calls(), vec![Check::Fresh]);
    assert_eq!(stored(&w, id).await, (Some("ok".to_owned()), true));
    let a = wait_audit(&w, "account.test").await;
    assert_eq!(a["result"], "ok");
    // (D-383, C-1033) A passed Test is shown next to the state, never as the account working: only an
    // agent's real call does that.
    let h = health(&w, id).await;
    assert_eq!(h["state"], "not_used", "{h}");
    assert_eq!(h["test"]["result"], "ok", "{h}");
    assert!(h["test"]["at"].is_string());
    assert!(
        h["last_ok_at"].is_null() && h["last_error_at"].is_null(),
        "{h}"
    );
}

/// Real use decides the state: a failed agent call shows `failing` until a later success, and a passed Test
/// changes neither (C-1033, D-383).
#[tokio::test]
async fn a_failed_call_stays_failing_through_a_passed_test_until_a_real_success() {
    let gw = FakeGateway::new("ok");
    let (w, id) = connected(&gw).await;
    sqlx::query(
        "UPDATE app_connections SET last_error_at = now(), last_error = 'rejected' WHERE id = $1",
    )
    .bind(id)
    .execute(w.h.pool())
    .await
    .unwrap();
    let h = health(&w, id).await;
    assert_eq!(
        (h["state"].as_str(), h["detail"].as_str()),
        (Some("failing"), Some("token")),
        "{h}"
    );
    assert_eq!(h["last_error"], "rejected");

    // The Test passes, and the account still fails: nothing an agent did has shown otherwise.
    let (_, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(v["result"], "ok", "{v}");
    let h = health(&w, id).await;
    assert_eq!(h["state"], "failing", "{h}");
    assert_eq!(h["test"]["result"], "ok", "{h}");

    // A later real success (the gateway's write) turns it to working.
    sqlx::query("UPDATE app_connections SET last_ok_at = now() WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(health(&w, id).await["state"], "working");
}

#[tokio::test]
async fn a_token_the_app_refuses_is_rejected_with_the_status_and_no_body() {
    let gw = FakeGateway::new("ok");
    let (w, id) = connected(&gw).await;
    set_token(&w, id, "nope").await;
    let (s, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["result"], "rejected", "{v}");
    assert_eq!(v["detail"]["status"], 401, "{v}");
    assert_eq!(v["detail"]["host"], "api.acme.example", "{v}");
    assert_eq!(stored(&w, id).await.0.as_deref(), Some("rejected"));
    // The Test result is the Test's, apart from use: no agent has called through the account.
    let h = health(&w, id).await;
    assert_eq!(h["state"], "not_used");
    assert_eq!(h["test"]["result"], "rejected", "{h}");
    assert!(h["detail"].is_null(), "{h}");
    // The answer says nothing the provider sent, nor the credential.
    let text = v.to_string();
    assert!(!text.contains("nope") && !text.contains("fine"), "{text}");
}

#[tokio::test]
async fn a_provider_failure_is_unexpected_and_an_unroutable_host_is_unreachable() {
    let gw = FakeGateway::new("ok");
    let (mut w, id) = connected(&gw).await;
    set_token(&w, id, "boom").await;
    let (_, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(
        (v["result"].as_str(), v["detail"]["status"].as_u64()),
        (Some("unexpected"), Some(500)),
        "{v}"
    );
    // Nothing listens on the provider's address.
    set_token(&w, id, "fine").await;
    w.h.state =
        w.h.state
            .clone()
            .with_connect_override("http://127.0.0.1:1");
    w.h.app = test_router(w.h.state.clone());
    let (_, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(v["result"], "unreachable", "{v}");
    assert_eq!(v["detail"]["host"], "api.acme.example", "{v}");
    let h = health(&w, id).await;
    assert_eq!(h["state"], "not_used");
    assert_eq!(h["test"]["result"], "unreachable", "{h}");
}

#[tokio::test]
async fn a_one_mebibyte_answer_is_cut_and_none_of_it_reaches_the_caller() {
    let gw = FakeGateway::new("ok");
    let (mut w, id) = connected(&gw).await;
    // A provider that answers 200 with more than the probe reads, carrying a marker.
    let big = MockServer::start().await;
    let mut body = "SECRET-UPSTREAM-MARKER ".repeat(60_000);
    body.push_str("{}");
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&big)
        .await;
    w.h.state = w.h.state.clone().with_connect_override(&big.uri());
    w.h.app = test_router(w.h.state.clone());
    let (s, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["result"], "unexpected", "{v}");
    assert!(!v.to_string().contains("SECRET-UPSTREAM-MARKER"), "{v}");
}

#[tokio::test]
async fn a_token_that_cannot_be_refreshed_stops_the_test_before_the_probe() {
    let gw = FakeGateway::new("unexpected");
    let (w, id) = connected(&gw).await;
    let (s, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["result"], "unexpected", "{v}");
    assert_eq!(gw.calls(), vec![Check::Fresh]);
    assert_eq!(stored(&w, id).await.0.as_deref(), Some("unexpected"));
}

#[tokio::test]
async fn a_disabled_account_and_an_unknown_type_are_not_tested() {
    let gw = FakeGateway::new("ok");
    let (w, id) = connected(&gw).await;
    sqlx::query("UPDATE app_connections SET status = 'disabled' WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (_, v) = test_it(&w, &w.owner, id).await;
    assert_eq!(v["result"], "not_testable", "{v}");
    assert!(gw.calls().is_empty());
    // `not_testable` is answered, never stored.
    assert_eq!(stored(&w, id).await, (None, false));
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/accounts/widget/{id}/test"),
            None,
        )
        .await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn health_in_the_list_never_shows_the_raw_reason_and_the_status_wins() {
    let gw = FakeGateway::new("ok");
    let (w, id) = connected(&gw).await;
    sqlx::query("UPDATE app_connections SET status = 'reauth_required', status_reason = 'invalid_grant: raw upstream text', last_ok_at = now() WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    let h = health(&w, id).await;
    assert_eq!(h["state"], "sign_in_expired");
    assert!(h["detail"].is_null(), "{h}");
    assert!(!h.to_string().contains("raw upstream text"));
    sqlx::query("UPDATE app_connections SET status = 'disabled' WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(health(&w, id).await["state"], "disabled");
}

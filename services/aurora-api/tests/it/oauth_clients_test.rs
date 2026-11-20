//! (P5-REFINE-6, C-1070, D-407) Test a sign-in app: one token request with a made-up code to the
//! provider (a wiremock token endpoint here, through the connect environment's guarded client and
//! its test override). `invalid_grant` → `ok`, `invalid_client` → `rejected`, a dead endpoint →
//! `unreachable`, anything else → `unexpected`; nothing of the body comes back; the client's
//! `state` follows; the caller rule is `PATCH`'s; the Test is audited.
//!
//! A child module of `connect` (it reuses that file's world and helpers).
use super::*;

const BODY_CANARY: &str = "TOKENBODYCANARY0x5150";

impl W {
    /// Point the connect environment's guarded requests at `origin`.
    fn token_server(&mut self, origin: &str) {
        self.h.state = self.h.state.clone().with_connect_override(origin);
        self.h.app = test_router(self.h.state.clone());
    }

    async fn new_client(&self) -> String {
        let (s, _, v) = self
            .call(&self.owner, Method::POST, &format!("/v1/workspaces/{}/oauth-clients", self.ws),
                Some(json!({"package_id": "acme", "client_key": "goog", "fields": {"client_id": "cid", "client_secret": "csec"}})))
            .await;
        assert_eq!(s, 201, "{v}");
        assert_eq!(v["state"], "ready_not_tested", "{v}");
        assert!(v["test"].is_null(), "{v}");
        v["id"].as_str().unwrap().to_owned()
    }

    async fn test_client(&self, who: &str, id: &str) -> (u16, Value) {
        let (s, _, v) = self
            .call(
                who,
                Method::POST,
                &format!("/v1/oauth-clients/{id}/test"),
                None,
            )
            .await;
        (s, v)
    }

    async fn client(&self, id: &str) -> Value {
        let (s, _, v) = self
            .call(
                &self.owner,
                Method::GET,
                &format!("/v1/workspaces/{}/oauth-clients", self.ws),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .cloned()
            .unwrap()
    }
}

async fn answer(server: &MockServer, status: u16, body: Value) {
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn the_four_answers_map_to_results_and_the_state_follows() {
    let mut w = world(None).await;
    let server = MockServer::start().await;
    w.token_server(&server.uri());
    let id = w.new_client().await;

    // The provider knows the client and refuses the made-up code.
    answer(
        &server,
        400,
        json!({"error": "invalid_grant", "error_description": BODY_CANARY}),
    )
    .await;
    let (s, v) = w.test_client(&w.owner, &id).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["result"], "ok", "{v}");
    assert_eq!(
        v["detail"],
        json!({"host": "accounts.example.com", "status": 400})
    );
    assert!(v["checked_at"].is_string());
    assert!(
        !v.to_string().contains(BODY_CANARY),
        "no upstream body: {v}"
    );
    let c = w.client(&id).await;
    assert_eq!(
        (c["state"].as_str(), c["test"]["result"].as_str()),
        (Some("ready"), Some("ok"))
    );
    // The request was the code exchange's: the client's id and secret, the redirect URI, PKCE.
    let reqs = server.received_requests().await.unwrap();
    let form: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(&reqs[0].body)
            .into_owned()
            .collect();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["client_id"], "cid");
    assert_eq!(form["client_secret"], "csec");
    assert!(form["code"].starts_with("aurora-test-"));
    assert!(form["redirect_uri"].ends_with("/v1/oauth/callback"));
    assert!(form.contains_key("code_verifier"), "the method uses PKCE");

    // The provider refuses the client.
    answer(
        &server,
        401,
        json!({"error": "invalid_client", "error_description": BODY_CANARY}),
    )
    .await;
    let (_, v) = w.test_client(&w.owner, &id).await;
    assert_eq!(v["result"], "rejected", "{v}");
    assert!(!v.to_string().contains(BODY_CANARY));
    assert_eq!(w.client(&id).await["state"], "test_failed");

    // Anything else.
    answer(&server, 500, json!({"message": BODY_CANARY})).await;
    let (_, v) = w.test_client(&w.owner, &id).await;
    assert_eq!(v["result"], "unexpected", "{v}");
    assert_eq!(v["detail"]["status"], 500);
    assert!(!v.to_string().contains(BODY_CANARY));

    // Nothing listens: unreachable.
    w.token_server("http://127.0.0.1:1");
    let (_, v) = w.test_client(&w.owner, &id).await;
    assert_eq!(v["result"], "unreachable", "{v}");
    assert!(v["detail"].get("status").is_none(), "{v}");
    assert_eq!(w.client(&id).await["state"], "test_failed");

    // Audited each time, with the result and never the body.
    let audit: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'oauth_client.test' ORDER BY id",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    let results: Vec<&str> = audit
        .iter()
        .map(|m| m["result"].as_str().unwrap())
        .collect();
    assert_eq!(results, ["ok", "rejected", "unexpected", "unreachable"]);
    assert_eq!(audit[0]["package_id"], "acme");
    assert_eq!(audit[0]["client_key"], "goog");
    assert!(!format!("{audit:?}").contains(BODY_CANARY));
    // No response text is stored.
    let stored: String = sqlx::query_scalar(
        "SELECT row_to_json(c)::text FROM app_oauth_clients c WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(!stored.contains(BODY_CANARY));
}

#[tokio::test]
async fn the_caller_rule_is_patchs_and_a_disabled_client_is_not_testable() {
    let mut w = world(None).await;
    let server = MockServer::start().await;
    w.token_server(&server.uri());
    let id = w.new_client().await;
    // A member of the workspace who is not an owner may not PATCH, so may not Test.
    let (s, v) = w.test_client(&w.member, &id).await;
    assert_eq!(s, 403, "{v}");
    let (s, _, _) = w
        .call(
            &w.member,
            Method::PATCH,
            &format!("/v1/oauth-clients/{id}"),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 403);
    // Off: not testable, nothing sent, nothing stored.
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &format!("/v1/oauth-clients/{id}"),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["state"], "disabled");
    let (s, v) = w.test_client(&w.owner, &id).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["result"], "not_testable");
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(w.client(&id).await["test"].is_null());
}

#[tokio::test]
async fn a_client_that_was_used_is_ready_without_a_test() {
    let w = world(None).await;
    w.oauth_client().await;
    let (url, ck) = w.start_oauth().await;
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("aurora:connected"), "{pg}");
    let (_, _, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/workspaces/{}/oauth-clients", w.ws),
            None,
        )
        .await;
    let c = &v["items"][0];
    assert_eq!(c["connection_count"], 1, "{c}");
    assert_eq!(c["state"], "ready", "verified by use: {c}");
    assert!(c["test"].is_null());
}

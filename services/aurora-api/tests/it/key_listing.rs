#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-170 (D-372, C-1026): an agent on a LiteLLM endpoint works with no model configured: the first
//! allowed chat model its own key serves. Two keys of one endpoint serve different names (two LiteLLM
//! namespaces); the key the gateway injects (the workspace key over the organization key) is the one read.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_config::LlmEndpoint;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// LiteLLM with two namespaces: key `sk-a` serves `m-a`; key `sk-b` serves an embedding model first, then `m-b`.
async fn litellm() -> MockServer {
    let s = MockServer::start().await;
    for (key, models, info) in [
        (
            "sk-a",
            json!({"data": [{"id": "m-a"}]}),
            json!({"data": [{"model_name": "m-a", "litellm_params": {"model": "hosted_vllm/a"}, "model_info": {"mode": "chat"}}]}),
        ),
        (
            "sk-b",
            json!({"data": [{"id": "emb-b"}, {"id": "m-b"}]}),
            json!({"data": [
                {"model_name": "emb-b", "litellm_params": {"model": "hosted_vllm/emb"}, "model_info": {"mode": "embedding"}},
                {"model_name": "m-b", "litellm_params": {"model": "hosted_vllm/b"}, "model_info": {"mode": "chat"}},
            ]}),
        ),
    ] {
        let bearer = format!("Bearer {key}");
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", bearer.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(models))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/model/info"))
            .and(header("authorization", bearer.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(info))
            .mount(&s)
            .await;
    }
    s
}

async fn rig(base: String, default_model: Option<&str>) -> Rig {
    let default_model = default_model.map(str::to_owned);
    rig_with(move |c| {
        c.llm.endpoints.push(LlmEndpoint {
            id: "aigw".into(),
            provider: "litellm".into(),
            base_url: Some(base),
            default_model,
            ..Default::default()
        });
    })
    .await
}

async fn cookie_of(r: &Rig, email: &str, org_role: &str, ws_role: &str) -> String {
    let u = r.h.user(email, false).await;
    r.h.member(r.org, u, org_role, "active").await;
    r.h.bind_ws(r.ws, u, ws_role).await;
    let (_, s) = r.h.good_session(u).await;
    cookie(&s)
}

async fn post_key(r: &Rig, ck: &str, uri: &str, name: &str, value: &str) -> Uuid {
    let (s, _, v) =
        r.h.send(
            Method::POST,
            uri,
            &[("cookie", ck), ("origin", ORIGIN)],
            Some(json!({"name": name, "kind": "llm", "llm": {"endpoint_id": "aigw"}, "value": value})),
        )
        .await;
    assert_eq!(s.as_u16(), 201, "{v}");
    v["id"].as_str().unwrap().parse().unwrap()
}

async fn grant(r: &Rig, agent: Uuid, secret: Uuid) {
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7())
        .bind(r.ws)
        .bind(agent)
        .bind(secret)
        .execute(r.h.pool())
        .await
        .unwrap();
}

async fn readiness(r: &Rig, ck: &str, agent: Uuid) -> Value {
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/agents/{agent}/model-readiness"),
            &[("cookie", ck), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    v
}

async fn claude_agent(r: &Rig, handle: &str) -> Uuid {
    let a = r.agent(handle, false).await;
    sqlx::query("UPDATE agents SET harness = 'claude-code' WHERE id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    a
}

/// Two keys on one endpoint, from two namespaces: the workspace key (the gateway's pick) is read, though the
/// organization key is older, and its first chat model (not the embedding model listed before it) is used.
#[tokio::test]
async fn the_gateways_key_decides_the_model_and_the_start_config_carries_it() {
    let lite = litellm().await;
    let r = rig(lite.uri(), None).await;
    let ws_owner = cookie_of(&r, "owner@x.io", "member", "owner").await;
    let org_admin = cookie_of(&r, "orgadmin@x.io", "admin", "member").await;
    // The organization key (namespace A) is the older one.
    let org_key = post_key(
        &r,
        &org_admin,
        &format!("/v1/orgs/{}/secrets", r.org),
        "org-a",
        "sk-a",
    )
    .await;
    let ws_key = post_key(
        &r,
        &ws_owner,
        &format!("/v1/workspaces/{}/secrets", r.ws),
        "ws-b",
        "sk-b",
    )
    .await;
    let a = claude_agent(&r, "twokeys").await;
    grant(&r, a, org_key).await;
    grant(&r, a, ws_key).await;

    let v = readiness(&r, &ws_owner, a).await;
    assert_eq!(
        v["keys"][0]["scope"], "workspace",
        "the gateway's key comes first: {v}"
    );
    assert_eq!(v["default"]["model_id"], "m-b", "{v}");
    assert_eq!(v["default"]["source"], "key_listing", "{v}");
    assert_eq!(v["warnings"], json!([]), "{v}");

    // The dispatched turn's start config carries the same model (one function for readiness and the dispatcher).
    let _live = r.boot_agent(a).await;
    let d = r.deliver().await;
    assert_eq!(d["model"]["model"], "m-b", "{d}");
}

/// An endpoint default the key does not serve is not sent: warned, and the key's first chat model is used.
#[tokio::test]
async fn an_endpoint_default_the_key_does_not_serve_is_skipped_with_a_warning() {
    let lite = litellm().await;
    let r = rig(lite.uri(), Some("hosted_vllm/glm53-flash-int4")).await;
    let ck = cookie_of(&r, "owner@x.io", "member", "owner").await;
    let key = post_key(
        &r,
        &ck,
        &format!("/v1/workspaces/{}/secrets", r.ws),
        "ws-a",
        "sk-a",
    )
    .await;
    let a = claude_agent(&r, "notserved").await;
    grant(&r, a, key).await;
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["default"]["model_id"], "m-a", "{v}");
    assert_eq!(v["default"]["source"], "key_listing", "{v}");
    assert_eq!(v["warnings"], json!(["default_model_not_served"]), "{v}");

    // An override the key does not serve is kept, with a warning (never re-pointed).
    sqlx::query("UPDATE agents SET model_provider='litellm', model_endpoint_id='aigw', model_id='m-elsewhere' WHERE id=$1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["default"]["model_id"], "m-elsewhere", "{v}");
    assert_eq!(v["default"]["source"], "override", "{v}");
    assert_eq!(v["warnings"], json!(["override_not_served"]), "{v}");
}

/// The owner's production shape: an endpoint with only `base_url` and an agent with no model. The queued turn
/// is dispatched, not failed with `no_default_model`.
#[tokio::test]
async fn a_queued_turn_on_a_base_url_only_endpoint_is_dispatched() {
    let lite = litellm().await;
    let r = rig(lite.uri(), None).await;
    let ck = cookie_of(&r, "owner@x.io", "member", "owner").await;
    let key = post_key(
        &r,
        &ck,
        &format!("/v1/workspaces/{}/secrets", r.ws),
        "ws-b",
        "sk-b",
    )
    .await;
    let a = claude_agent(&r, "firstmessage").await;
    grant(&r, a, key).await;
    let conv = r.direct(a).await;
    let v = r.say(conv, "hello").await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let items = r.work().await;
    assert!(
        items.iter().any(|i| i["kind"] == "sandbox.start"),
        "{items:?}"
    );
    let (status, code) = r.turn_status(&turn).await;
    assert_ne!(code.as_deref(), Some("no_default_model"), "{status}");
}

/// A key whose listing cannot be read resolves nothing, and the turn's message says why.
#[tokio::test]
async fn an_unreadable_listing_fails_the_turn_with_a_message_that_says_so() {
    let lite = litellm().await;
    let r = rig(lite.uri(), None).await;
    let ck = cookie_of(&r, "owner@x.io", "member", "owner").await;
    let key = post_key(
        &r,
        &ck,
        &format!("/v1/workspaces/{}/secrets", r.ws),
        "ws-b",
        "sk-b",
    )
    .await;
    let a = claude_agent(&r, "unreadable").await;
    grant(&r, a, key).await;
    // The endpoint stops answering after the key was admitted.
    lite.reset().await;
    let conv = r.direct(a).await;
    let v = r.say(conv, "hello").await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    r.work().await;
    let (_, code) = r.turn_status(&turn).await;
    assert_eq!(code.as_deref(), Some("no_default_model"));
    let said: Vec<(String,)> =
        sqlx::query_as("SELECT payload::text FROM transcript_events WHERE conversation_id = $1")
            .bind(conv)
            .fetch_all(r.h.pool())
            .await
            .unwrap();
    let said = said.into_iter().map(|(s,)| s).collect::<String>();
    assert!(said.contains("model list could not be read"), "{said}");
}

async fn accounts(r: &Rig, ck: &str, uri: &str) -> (u16, Value) {
    let (s, _, v) =
        r.h.send(
            Method::GET,
            uri,
            &[("cookie", ck), ("origin", ORIGIN)],
            None,
        )
        .await;
    (s.as_u16(), v)
}

fn item(v: &Value, id: Uuid) -> &Value {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["ref"]["id"] == id.to_string())
        .unwrap_or_else(|| panic!("no account {id} in {v}"))
}

/// P5-APIGAP-3 (C-1048): a member reads each AI key's default model from the workspace list; a configured
/// default the key does not serve falls back as D-372 says; a key of another workspace is not listed to them.
#[tokio::test]
async fn a_member_sees_the_default_model_of_a_key_they_can_use() {
    let lite = litellm().await;
    let r = rig(lite.uri(), Some("hosted_vllm/glm53-flash-int4")).await;
    let org_admin = cookie_of(&r, "orgadmin@x.io", "admin", "member").await;
    let member = cookie_of(&r, "member@x.io", "member", "member").await;
    let org_key = post_key(
        &r,
        &org_admin,
        &format!("/v1/orgs/{}/secrets", r.org),
        "org-b",
        "sk-b",
    )
    .await;
    let uri = format!("/v1/workspaces/{}/accounts?kind=ai_key", r.ws);
    let (s, v) = accounts(&r, &member, &uri).await;
    assert_eq!(s, 200, "{v}");
    let k = item(&v, org_key);
    // The configured default is not served: the first chat model of this key's listing (not the embedding model).
    assert_eq!(k["default_model"], "m-b", "{v}");
    assert!(
        k.get("starter").is_none(),
        "a member's list never says starter: {v}"
    );
    // No endpoint URL and no value leak with it.
    assert!(!v.to_string().contains(&lite.uri()), "{v}");

    // A key of a workspace the member is not in is not in their list at all.
    let other = r.h.workspace(r.org).await;
    let u2 = r.h.user("owner3@x.io", false).await;
    r.h.member(r.org, u2, "member", "active").await;
    r.h.bind_ws(other, u2, "owner").await;
    let (_, s2) = r.h.good_session(u2).await;
    let other_key = post_key(
        &r,
        &cookie(&s2),
        &format!("/v1/workspaces/{other}/secrets"),
        "other-a",
        "sk-a",
    )
    .await;
    let (s, v) = accounts(&r, &member, &uri).await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["ref"]["id"] != other_key.to_string()),
        "{v}"
    );
}

/// A served default is the default; without a listing to read (no key value works) nothing is invented.
#[tokio::test]
async fn a_served_configured_default_is_the_keys_default_model() {
    let lite = litellm().await;
    let r = rig(lite.uri(), Some("m-a")).await;
    let ck = cookie_of(&r, "owner@x.io", "member", "owner").await;
    let key = post_key(
        &r,
        &ck,
        &format!("/v1/workspaces/{}/secrets", r.ws),
        "ws-a",
        "sk-a",
    )
    .await;
    let (s, v) = accounts(&r, &ck, &format!("/v1/workspaces/{}/accounts", r.ws)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(item(&v, key)["default_model"], "m-a", "{v}");
}

/// P5-APIGAP-3 (C-1049): an organization admin sees `starter: true` exactly on the starter key, and only on the
/// organization list.
#[tokio::test]
async fn the_organization_list_marks_the_starter_key() {
    let lite = litellm().await;
    let r = rig(lite.uri(), None).await;
    let admin_id = r.h.user("orgadmin@x.io", false).await;
    r.h.member(r.org, admin_id, "admin", "active").await;
    let (_, s) = r.h.good_session(admin_id).await;
    let admin = cookie(&s);
    let starter = post_key(
        &r,
        &admin,
        &format!("/v1/orgs/{}/secrets", r.org),
        "starter",
        "sk-a",
    )
    .await;
    let other = post_key(
        &r,
        &admin,
        &format!("/v1/orgs/{}/secrets", r.org),
        "other",
        "sk-b",
    )
    .await;
    let org_uri = format!("/v1/orgs/{}/accounts?kind=ai_key", r.org);

    // No placement saved: nothing is starter.
    let (st, v) = accounts(&r, &admin, &org_uri).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(item(&v, starter)["starter"], false, "{v}");
    assert_eq!(item(&v, other)["starter"], false, "{v}");

    sqlx::query(
        "INSERT INTO instance_placement (singleton, mode, default_org_id, default_org_role, shared_workspaces,
            personal_enabled, personal_role, name_template, starter_grants, updated_by, updated_at)
         VALUES (true, 'shared_org', $1, 'member', '[]'::jsonb, true, 'owner', '{name}', $2, $3, now())
         ON CONFLICT (singleton) DO UPDATE SET mode = 'shared_org', default_org_id = $1, personal_enabled = true,
            starter_grants = $2",
    )
    .bind(r.org)
    .bind(json!([{"kind": "secret", "id": starter}]))
    .bind(admin_id)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (st, v) = accounts(&r, &admin, &org_uri).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(item(&v, starter)["starter"], true, "{v}");
    assert_eq!(item(&v, other)["starter"], false, "{v}");

    // The workspace list (what a member reads) never carries it.
    let member = cookie_of(&r, "member@x.io", "member", "member").await;
    let (st, v) = accounts(
        &r,
        &member,
        &format!("/v1/workspaces/{}/accounts?kind=ai_key", r.ws),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a.get("starter").is_none()),
        "{v}"
    );
    // A member may not read the organization list at all.
    let (st, _) = accounts(&r, &member, &org_uri).await;
    assert_eq!(st, 403);
}

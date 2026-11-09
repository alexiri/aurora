#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-019 (D-136, C-500, C-501): model readiness, `no_default_model`, and the dispatcher's
//! two pre-start failures.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_config::{KeyOrigin, LlmEndpoint};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn lite(id: &str) -> LlmEndpoint {
    LlmEndpoint {
        id: id.into(),
        provider: "litellm".into(),
        base_url: Some("http://127.0.0.1:1".into()),
        ..Default::default()
    }
}

/// The owner's production shape (`aigw`: only `base_url`) plus one endpoint per resolution step.
async fn rig() -> Rig {
    rig_with(|c| {
        // Workspace-scoped keys are not permitted (reason `scope_not_permitted`).
        c.governance.allowed_key_scopes = vec!["organization".into()];
        c.llm.endpoints.push(lite("aigw"));
        c.llm.endpoints.push(LlmEndpoint {
            allowed_models: vec!["m-allowed".into()],
            ..lite("aigw-allow")
        });
        c.llm.endpoints.push(LlmEndpoint {
            models: vec!["m-static".into()],
            ..lite("aigw-static")
        });
        c.llm.endpoints.push(LlmEndpoint {
            dialects: vec!["openai_chat".into()],
            default_model: Some("m-oai".into()),
            ..lite("aigw-oai")
        });
        c.llm.endpoints.push(LlmEndpoint {
            default_model: Some("m-team".into()),
            key_origin: Some(KeyOrigin {
                method: "litellm_team".into(),
                allowed_team_ids: vec!["t1".into()],
                ..Default::default()
            }),
            ..lite("aigw-team")
        });
    })
    .await
}

fn verified(team: &str) -> Value {
    json!({"status": "verified", "method": "litellm_team", "team_id": team,
           "value_version": 1, "checked_at": "2026-11-09T00:00:00Z"})
}

struct K<'a> {
    name: &'a str,
    endpoint: &'a str,
    provider: &'a str,
    status: &'a str,
    workspace: bool,
    origin: Option<Value>,
}

impl<'a> K<'a> {
    fn on(name: &'a str, endpoint: &'a str) -> Self {
        let provider = if endpoint == "anthropic" {
            "anthropic"
        } else {
            "litellm"
        };
        Self {
            name,
            endpoint,
            provider,
            status: "active",
            workspace: false,
            origin: None,
        }
    }
}

async fn grant(r: &Rig, agent: Uuid, k: K<'_>) -> Uuid {
    let sec = Uuid::now_v7();
    let ws = k.workspace.then_some(r.ws);
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode,
            llm_endpoint_id, host_pattern, injection, value_source, value_ref, status, origin)
         VALUES ($1,$2,$3,$4,'llm',$5,'api_key',$6,'127.0.0.1','{}','managed','x',$7,$8)",
    )
    .bind(sec)
    .bind(r.org)
    .bind(ws)
    .bind(k.name)
    .bind(k.provider)
    .bind(k.endpoint)
    .bind(k.status)
    .bind(k.origin)
    .execute(r.h.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7())
        .bind(r.ws)
        .bind(agent)
        .bind(sec)
        .execute(r.h.pool())
        .await
        .unwrap();
    sec
}

/// A workspace owner's cookie.
async fn owner(r: &Rig) -> String {
    let u = r.h.user("owner@x.io", false).await;
    r.h.member(r.org, u, "member", "active").await;
    r.h.bind_ws(r.ws, u, "owner").await;
    let (_, s) = r.h.good_session(u).await;
    cookie(&s)
}

async fn get(r: &Rig, ck: &str, uri: &str) -> (u16, Value) {
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

async fn readiness(r: &Rig, ck: &str, agent: Uuid) -> Value {
    let (s, v) = get(r, ck, &format!("/v1/agents/{agent}/model-readiness")).await;
    assert_eq!(s, 200, "{v}");
    v
}

fn key<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == name)
        .unwrap_or_else(|| panic!("no key {name} in {v}"))
}

#[tokio::test]
async fn every_reason_and_usable() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = r.agent("reasons", false).await;
    let mut proposed = K::on("k-not-active", "anthropic");
    proposed.status = "proposed";
    let mut gone = K::on("k-endpoint-unknown", "no-such-endpoint");
    gone.provider = "anthropic";
    let mut provider = K::on("k-provider-disabled", "anthropic");
    provider.provider = "openai";
    let mut scope = K::on("k-scope", "anthropic");
    scope.workspace = true;
    let mut team_ok = K::on("k-usable-team", "aigw-team");
    team_ok.origin = Some(verified("t1"));
    let mut team_wrong = K::on("k-not-compliant", "aigw-team");
    team_wrong.origin = Some(verified("t9"));
    let mut team_stale = K::on("k-stale-version", "aigw-team");
    team_stale.origin = Some(json!({"status": "verified", "method": "litellm_team",
        "team_id": "t1", "value_version": 7}));
    for k in [
        proposed,
        gone,
        provider,
        scope,
        K::on("k-unverified", "aigw-team"),
        team_ok,
        team_wrong,
        team_stale,
        K::on("k-dialect", "aigw-oai"),
        K::on("k-usable", "anthropic"),
    ] {
        grant(&r, a, k).await;
    }
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["harness"], "fake");
    assert_eq!(v["keys"].as_array().unwrap().len(), 10);
    let reason = |n: &str| key(&v, n)["reason"].as_str().map(str::to_owned);
    assert_eq!(reason("k-not-active").as_deref(), Some("not_active"));
    assert_eq!(
        reason("k-endpoint-unknown").as_deref(),
        Some("endpoint_unknown")
    );
    assert_eq!(
        reason("k-provider-disabled").as_deref(),
        Some("provider_disabled")
    );
    assert_eq!(reason("k-scope").as_deref(), Some("scope_not_permitted"));
    assert_eq!(reason("k-unverified").as_deref(), Some("origin_unverified"));
    assert_eq!(
        reason("k-stale-version").as_deref(),
        Some("origin_unverified")
    );
    assert_eq!(
        reason("k-not-compliant").as_deref(),
        Some("origin_not_compliant")
    );
    assert_eq!(reason("k-dialect").as_deref(), Some("no_shared_dialect"));
    for ok in ["k-usable", "k-usable-team"] {
        assert_eq!(key(&v, ok)["usable"], true, "{ok}");
        assert_eq!(key(&v, ok)["reason"], Value::Null);
    }
    assert_eq!(key(&v, "k-scope")["scope"], "workspace");
    assert_eq!(key(&v, "k-usable")["scope"], "organization");
    assert_eq!(key(&v, "k-usable")["endpoint_id"], "anthropic");
    // Preference order: litellm keys (oldest first) before anthropic ones.
    let names: Vec<&str> = v["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["name"].as_str().unwrap())
        .collect();
    let first_anthropic = names.iter().position(|n| *n == "k-usable").unwrap();
    let team = names.iter().position(|n| *n == "k-usable-team").unwrap();
    assert!(team < first_anthropic, "{names:?}");
    // The first usable key in order serves the default: the team key's endpoint default.
    assert_eq!(v["default"]["endpoint_id"], "aigw-team");
    assert_eq!(v["default"]["model_id"], "m-team");
    assert_eq!(v["default"]["source"], "endpoint_default");
    assert_eq!(v["override_dropped"], false);
}

#[tokio::test]
async fn default_source_for_each_step_and_null() {
    let r = rig().await;
    let ck = owner(&r).await;
    for (handle, endpoint, model, source) in [
        (
            "d-endpoint",
            "anthropic",
            Some("claude-test"),
            "endpoint_default",
        ),
        ("d-allow", "aigw-allow", Some("m-allowed"), "allow_list"),
        ("d-static", "aigw-static", Some("m-static"), "static_list"),
        // The owner's production endpoint: only `base_url`.
        ("d-null", "aigw", None, ""),
    ] {
        let a = r.agent(handle, false).await;
        grant(&r, a, K::on(&format!("k-{handle}"), endpoint)).await;
        let v = readiness(&r, &ck, a).await;
        assert_eq!(key(&v, &format!("k-{handle}"))["usable"], true, "{v}");
        match model {
            Some(m) => {
                assert_eq!(v["default"]["model_id"], m, "{handle}: {v}");
                assert_eq!(v["default"]["source"], source, "{handle}: {v}");
                assert_eq!(v["default"]["endpoint_id"], endpoint);
            }
            None => assert_eq!(v["default"], Value::Null, "{v}"),
        }
    }

    // Override: stamped with provider + endpoint, wins over the endpoint default.
    let a = r.agent("d-override", false).await;
    grant(&r, a, K::on("k-d-override", "aigw")).await;
    sqlx::query(
        "UPDATE agents SET model_provider='litellm', model_endpoint_id='aigw', model_id='my-model' WHERE id=$1",
    )
    .bind(a)
    .execute(r.h.pool())
    .await
    .unwrap();
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["default"]["model_id"], "my-model");
    assert_eq!(v["default"]["source"], "override");
    assert_eq!(v["override_dropped"], false);

    // An override on an endpoint with no usable key is dropped, never re-pointed.
    let a = r.agent("d-dropped", false).await;
    grant(&r, a, K::on("k-d-dropped", "anthropic")).await;
    sqlx::query(
        "UPDATE agents SET model_provider='litellm', model_endpoint_id='aigw', model_id='my-model' WHERE id=$1",
    )
    .bind(a)
    .execute(r.h.pool())
    .await
    .unwrap();
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["override_dropped"], true);
    assert_eq!(v["default"]["endpoint_id"], "anthropic");
    assert_eq!(v["default"]["source"], "endpoint_default");

    // No usable key: `default = null`.
    let a = r.agent("d-nokey", false).await;
    let mut p = K::on("k-d-nokey", "anthropic");
    p.status = "proposed";
    grant(&r, a, p).await;
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["default"], Value::Null);
}

#[tokio::test]
async fn route_is_can_own_and_hosted_only() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = r.agent("authz", true).await;
    // A workspace member who is not an owner: 403.
    let (s, v) = get(&r, &r.cookie, &format!("/v1/agents/{a}/model-readiness")).await;
    assert_eq!(s, 403, "{v}");
    // Unknown agent: 404.
    let (s, _) = get(
        &r,
        &ck,
        &format!("/v1/agents/{}/model-readiness", Uuid::now_v7()),
    )
    .await;
    assert_eq!(s, 404);
    // A BYO agent has no hosted model: 404.
    let byo = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'byo-one','byo-one','byo')")
        .bind(byo)
        .bind(r.org)
        .bind(r.ws)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, _) = get(&r, &ck, &format!("/v1/agents/{byo}/model-readiness")).await;
    assert_eq!(s, 404);
}

/// Choosing the model through the agent API stamps the provider, so it becomes an override
/// (08: an explicit model is stamped with provider + endpoint). Found by the FIX-019 e2e.
#[tokio::test]
async fn model_chosen_through_the_api_becomes_the_override() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = r.agent("api-model", false).await;
    grant(&r, a, K::on("k-api-model", "aigw")).await;
    assert_eq!(readiness(&r, &ck, a).await["default"], Value::Null);
    let (s, _, v) =
        r.h.send(
            Method::PATCH,
            &format!("/v1/agents/{a}"),
            &[("cookie", &ck), ("origin", ORIGIN)],
            Some(json!({"model_endpoint_id": "aigw", "model_id": "my-model"})),
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["default"]["model_id"], "my-model", "{v}");
    assert_eq!(v["default"]["source"], "override");
    let p: Option<String> = sqlx::query_scalar("SELECT model_provider FROM agents WHERE id = $1")
        .bind(a)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(p.as_deref(), Some("litellm"));
}

/// Every transcript event and turn row of the conversation, as text.
async fn everything_said(r: &Rig, conv: Uuid) -> String {
    let events: Vec<(String,)> =
        sqlx::query_as("SELECT payload::text FROM transcript_events WHERE conversation_id = $1")
            .bind(conv)
            .fetch_all(r.h.pool())
            .await
            .unwrap();
    let turns: Vec<(String,)> =
        sqlx::query_as("SELECT t::text FROM turns t WHERE conversation_id = $1")
            .bind(conv)
            .fetch_all(r.h.pool())
            .await
            .unwrap();
    events
        .into_iter()
        .chain(turns)
        .map(|(s,)| s)
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn no_default_model_fails_before_any_sandbox_start_then_override_dispatches() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = r.agent("nodefault", false).await;
    grant(&r, a, K::on("k-secret-name-aigw", "aigw")).await;
    let conv = r.direct(a).await;
    let v = r.say(conv, "hi").await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let items = r.work().await;
    assert!(
        items.iter().all(|i| i["kind"] != "sandbox.start"),
        "{items:?}"
    );
    assert_eq!(
        r.turn_status(&turn).await,
        ("failed".into(), Some("no_default_model".into()))
    );
    let said = everything_said(&r, conv).await;
    assert!(said.contains("no model is chosen"), "{said}");
    assert!(!said.contains("k-secret-name-aigw"), "{said}");

    // The Instructions picker can fix it: the key is usable, so its models are listed.
    let b = r.agent("picker", false).await;
    grant(&r, b, K::on("k-picker", "aigw-static")).await;
    let (s, v) = get(&r, &ck, &format!("/v1/agents/{b}/models")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["endpoint_id"], "aigw-static", "{v}");
    assert_eq!(v["items"][0]["model_id"], "m-static");

    // With the agent model set (stamped with provider + endpoint) the next turn is dispatched.
    sqlx::query(
        "UPDATE agents SET model_provider='litellm', model_endpoint_id='aigw', model_id='my-model' WHERE id=$1",
    )
    .bind(a)
    .execute(r.h.pool())
    .await
    .unwrap();
    let v = readiness(&r, &ck, a).await;
    assert_eq!(v["default"]["source"], "override");
    r.say(conv, "again").await;
    let items = r.work().await;
    assert!(
        items.iter().any(|i| i["kind"] == "sandbox.start"),
        "{items:?}"
    );
}

#[tokio::test]
async fn no_model_key_message_has_reason_counts_and_no_key_names() {
    let r = rig().await;
    // Granted, but not usable: the message counts reasons, never names keys.
    let a = r.agent("unusable", false).await;
    grant(&r, a, K::on("k-secret-name-1", "aigw-team")).await;
    grant(&r, a, K::on("k-secret-name-2", "aigw-team")).await;
    let mut p = K::on("k-secret-name-3", "anthropic");
    p.status = "proposed";
    grant(&r, a, p).await;
    let conv = r.direct(a).await;
    let v = r.say(conv, "hi").await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let items = r.work().await;
    assert!(items.iter().all(|i| i["kind"] != "sandbox.start"));
    assert_eq!(
        r.turn_status(&turn).await,
        ("failed".into(), Some("no_model_key".into()))
    );
    let said = everything_said(&r, conv).await;
    assert!(
        said.contains(
            "No usable LLM key: 3 granted keys not usable (not_active, origin_unverified ×2)."
        ),
        "{said}"
    );
    assert!(!said.contains("k-secret-name"), "{said}");

    // Nothing granted at all.
    let b = r.agent("ungranted", false).await;
    let conv = r.direct(b).await;
    r.say(conv, "hi").await;
    r.work().await;
    let said = everything_said(&r, conv).await;
    assert!(
        said.contains("No LLM key is granted to this agent."),
        "{said}"
    );
}

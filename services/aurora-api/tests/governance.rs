#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use aurora_config::{KeyOrigin, LlmEndpoint};
use common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    owner: String,
    member: String,
}

async fn world() -> W {
    let h = Harness::with_config(|c| {
        c.llm.endpoints = vec![
            LlmEndpoint {
                id: "anthropic".into(),
                provider: "anthropic".into(),
                allowed_models: vec!["claude-a".into(), "claude-b".into()],
                key_origin: Some(KeyOrigin {
                    method: "provider_account".into(),
                    allowed_organization_ids: vec!["org-a".into(), "org-b".into()],
                    ..KeyOrigin::default()
                }),
                ..LlmEndpoint::default()
            },
            LlmEndpoint {
                id: "other".into(),
                provider: "litellm".into(),
                base_url: Some("http://127.0.0.1:1".into()),
                ..LlmEndpoint::default()
            },
        ];
    })
    .await;
    let org = h.org().await;
    let o = h.user("o@x.io", false).await;
    let m = h.user("m@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.member(org, m, "member", "active").await;
    let (_, owner) = h.good_session(o).await;
    let (_, member) = h.good_session(m).await;
    W {
        h,
        org,
        owner,
        member,
    }
}

impl W {
    async fn call(&self, m: Method, uri: &str, tok: &str, body: Option<Value>) -> (u16, Value) {
        let c = cookie(tok);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", c.as_str()), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }
    fn url(&self, tail: &str) -> String {
        format!("/v1/orgs/{}/governance{tail}", self.org)
    }
    async fn put(&self, doc: Value, rev: i64) -> (u16, Value) {
        self.call(
            Method::PUT,
            &self.url(""),
            &self.owner,
            Some(json!({"document": doc, "revision": rev})),
        )
        .await
    }
}

fn code(v: &Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

#[tokio::test]
async fn validation_stale_and_access() {
    let w = world().await;
    let (s, v) = w.call(Method::GET, &w.url(""), &w.owner, None).await;
    assert_eq!((s, v["revision"].as_i64()), (200, Some(0)));
    assert_eq!(
        v["document"]["llm"]["endpoints"]["anthropic"]["allowed"],
        true
    );

    // subset violations with paths
    let (s, v) = w
        .put(
            json!({"llm": {"endpoints": {"anthropic": {
                "key_scopes": ["bogus"], "allowed_models": ["nope"]}}}}),
            0,
        )
        .await;
    assert_eq!((s, code(&v)), (422, Some("governance_invalid")), "{v}");
    let paths: Vec<&str> = v["error"]["details"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"llm.endpoints.anthropic.key_scopes[0]"),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"llm.endpoints.anthropic.allowed_models[0]"),
        "{paths:?}"
    );

    // stale revision
    let (s, v) = w.put(json!({}), 5).await;
    assert_eq!((s, code(&v)), (409, Some("stale_revision")));

    // valid
    let (s, v) = w
        .put(json!({"llm": {"posture": "sanctioned_only"}}), 0)
        .await;
    assert_eq!((s, v["revision"].as_i64()), (200, Some(1)), "{v}");

    // member: effective yes, document/PUT no
    let (s, v) = w
        .call(Method::GET, &w.url("/effective"), &w.member, None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["llm"]["posture"]["value"], "sanctioned_only");
    assert_eq!(v["llm"]["posture"]["source"], "org");
    let (s, _) = w.call(Method::GET, &w.url(""), &w.member, None).await;
    assert_eq!(s, 403);
    let (s, _) = w
        .call(
            Method::PUT,
            &w.url(""),
            &w.member,
            Some(json!({"document": {}, "revision": 1})),
        )
        .await;
    assert_eq!(s, 403);
    let (s, v) = w
        .call(Method::GET, "/v1/governance/instance", &w.member, None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["llm"]["endpoints"][0]["id"], "anthropic");
}

#[tokio::test]
async fn below_instance_minimum_and_instance_source() {
    let h = Harness::with_config(|c| {
        c.governance.llm_minimum_posture = aurora_config::LlmPosture::SanctionedOnly;
    })
    .await;
    let org = h.org().await;
    let o = h.user("o@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    let (_, t) = h.good_session(o).await;
    let c = cookie(&t);
    let hdr = [("cookie", c.as_str()), ("origin", ORIGIN)];
    let (s, _, v) = h
        .send(
            Method::PUT,
            &format!("/v1/orgs/{org}/governance"),
            &hdr,
            Some(json!({"document": {"llm": {"posture": "open"}}, "revision": 0})),
        )
        .await;
    assert_eq!(s.as_u16(), 422);
    assert_eq!(code(&v), Some("governance_below_instance_minimum"), "{v}");
    assert_eq!(v["error"]["details"]["fields"][0]["path"], "llm.posture");
    let (s, _, v) = h
        .send(
            Method::GET,
            &format!("/v1/orgs/{org}/governance/effective"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(v["llm"]["posture"]["value"], "sanctioned_only");
    assert_eq!(v["llm"]["posture"]["source"], "instance");
}

#[tokio::test]
async fn change_marks_origin_mismatch_and_clears_agent_override() {
    let w = world().await;
    let sid = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref, origin) VALUES ($1,$2,'ant','llm','anthropic','api_key','anthropic','api.anthropic.com','{\"kind\":\"bearer\"}','managed','x:',$3)")
        .bind(sid).bind(w.org)
        .bind(json!({"status": "verified", "method": "provider_account", "organization_id": "org-a", "value_version": 1}))
        .execute(w.h.pool()).await.unwrap();
    let ws = w.h.workspace(w.org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, brief, model_provider, model_endpoint_id, model_id) VALUES ($1,$2,$3,'bot','Bot','byo','','anthropic','anthropic','claude-a')")
        .bind(agent).bind(w.org).bind(ws).execute(w.h.pool()).await.unwrap();

    let (_, v) = w
        .call(Method::GET, &w.url("/effective"), &w.owner, None)
        .await;
    assert_eq!(
        v["non_compliant_secrets"].as_array().unwrap().len(),
        0,
        "{v}"
    );

    // Exclude org-a and restrict models to claude-b.
    let (s, v) = w
        .put(
            json!({"llm": {"endpoints": {
                "anthropic": {"allowed_models": ["claude-b"],
                    "origin": {"allowed_organization_ids": ["org-b"]}},
                "other": {}}}}),
            0,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let st: Value = sqlx::query_scalar("SELECT origin FROM secrets WHERE id = $1")
        .bind(sid)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(st["status"], "mismatch");
    let m: Option<String> =
        sqlx::query_scalar("SELECT model_endpoint_id FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(m.is_none());

    let (_, v) = w
        .call(Method::GET, &w.url("/effective"), &w.owner, None)
        .await;
    let nc = &v["non_compliant_secrets"][0];
    assert_eq!(
        (nc["id"].as_str().unwrap(), nc["reason"].as_str().unwrap()),
        (sid.to_string().as_str(), "mismatch"),
        "{v}"
    );
}

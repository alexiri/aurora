#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-MODELS-1 (D-302, D-303, C-875..C-878, C-882, SEC-122): model roles, the subagent default and the
//! readiness report, which is the dispatcher's own function.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_config::LlmEndpoint;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn roles(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn lite(id: &str) -> LlmEndpoint {
    LlmEndpoint {
        id: id.into(),
        provider: "litellm".into(),
        base_url: Some("http://127.0.0.1:1".into()),
        default_model: Some("m-def".into()),
        ..Default::default()
    }
}

async fn rig() -> Rig {
    rig_with(|c| {
        c.governance.allowed_key_scopes = vec!["organization".into()];
        c.hosted.enabled_harnesses = vec!["claude-code".into(), "codex".into()];
        c.hosted.allow_fake_harness = true;
        // no roles at all
        c.llm.endpoints.push(lite("plain"));
        // operator roles: large and small, medium unset
        c.llm.endpoints.push(LlmEndpoint {
            model_roles: roles(&[("large", "m-big"), ("small", "m-small")]),
            ..lite("roled")
        });
        // roles on a static list and an allow-list
        c.llm.endpoints.push(LlmEndpoint {
            models: vec!["m-def".into(), "m-big".into(), "m-other".into()],
            allowed_models: vec!["m-def".into(), "m-big".into(), "m-other".into()],
            model_roles: roles(&[("large", "m-big")]),
            ..lite("listed")
        });
    })
    .await
}

async fn grant(r: &Rig, agent: Uuid, endpoint: &str, name: &str) {
    let sec = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id,
            host_pattern, injection, value_source, value_ref, status)
         VALUES ($1,$2,$3,'llm','litellm','api_key',$4,'127.0.0.1','{}','managed','x','active')",
    )
    .bind(sec)
    .bind(r.org)
    .bind(name)
    .bind(endpoint)
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
}

/// A hosted `claude-code` agent with one key on `endpoint`.
async fn agent_on(r: &Rig, handle: &str, endpoint: &str) -> Uuid {
    let a = r.agent(handle, false).await;
    sqlx::query("UPDATE agents SET harness = 'claude-code' WHERE id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    grant(r, a, endpoint, &format!("k-{handle}")).await;
    a
}

async fn owner(r: &Rig) -> String {
    let u = r.h.user("owner@x.io", false).await;
    r.h.member(r.org, u, "member", "active").await;
    r.h.bind_ws(r.ws, u, "owner").await;
    let (_, s) = r.h.good_session(u).await;
    cookie(&s)
}

async fn call(r: &Rig, ck: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let (s, _, v) =
        r.h.send(m, uri, &[("cookie", ck), ("origin", ORIGIN)], body)
            .await;
    (s.as_u16(), v)
}

async fn readiness(r: &Rig, ck: &str, a: Uuid) -> Value {
    let (s, v) = call(
        r,
        ck,
        Method::GET,
        &format!("/v1/agents/{a}/model-readiness"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v
}

async fn patch(r: &Rig, ck: &str, a: Uuid, body: Value) -> (u16, Value) {
    call(r, ck, Method::PATCH, &format!("/v1/agents/{a}"), Some(body)).await
}

fn code(v: &Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

#[tokio::test]
async fn endpoint_roles_resolve_and_unset_roles_warn_on_non_anthropic_endpoints() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = agent_on(&r, "roled", "roled").await;
    let v = readiness(&r, &ck, a).await;
    let d = &v["default"];
    assert_eq!(d["model_id"], "m-def");
    assert_eq!(
        d["roles"]["large"],
        json!({"model_id": "m-big", "source": "endpoint", "warning": null})
    );
    assert_eq!(
        d["roles"]["medium"],
        json!({"model_id": null, "source": "none", "warning": "role_unset"})
    );
    assert_eq!(d["roles"]["small"]["model_id"], "m-small");
    assert_eq!(d["roles_dropped"], json!([]));
    assert_eq!(d["subagent_model"], "inherit");
    assert_eq!(d["subagent_warning"], Value::Null);
    // no roles on the endpoint: every role is unset, with the warning
    let b = agent_on(&r, "plain", "plain").await;
    let v = readiness(&r, &ck, b).await;
    for role in ["large", "medium", "small"] {
        assert_eq!(
            v["default"]["roles"][role]["warning"], "role_unset",
            "{role}"
        );
    }
    // a harness that maps no roles has no warnings
    sqlx::query("UPDATE agents SET harness = 'codex' WHERE id = $1")
        .bind(b)
        .execute(r.h.pool())
        .await
        .unwrap();
    let v = readiness(&r, &ck, b).await;
    assert_eq!(v["default"]["roles"]["large"]["warning"], Value::Null);
}

#[tokio::test]
async fn an_agent_role_beats_the_endpoint_role_and_the_patch_checks_hold() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = agent_on(&r, "agentroles", "listed").await;
    // the default selection's endpoint is `listed`
    let (s, v) = patch(
        &r,
        &ck,
        a,
        json!({"model_roles": {"endpoint_id": "listed", "large": "m-other"}, "subagent_model": "large"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["model_roles"]["large"], "m-other");
    assert_eq!(v["model_roles"]["medium"], Value::Null);
    assert_eq!(v["subagent_model"], "large");
    let d = readiness(&r, &ck, a).await["default"].clone();
    assert_eq!(
        d["roles"]["large"],
        json!({"model_id": "m-other", "source": "agent", "warning": null})
    );
    assert_eq!(d["subagent_warning"], Value::Null);
    // 422 model_id_invalid { field }
    for bad in ["a b", "$(id)", "a'b", "sk-ant-api03-x", &"a".repeat(201)] {
        let (s, v) = patch(
            &r,
            &ck,
            a,
            json!({"model_roles": {"endpoint_id": "listed", "small": bad}}),
        )
        .await;
        assert_eq!((s, code(&v)), (422, Some("model_id_invalid")), "{bad}: {v}");
        assert_eq!(v["error"]["details"]["field"], "model_roles.small");
    }
    let (s, v) = patch(&r, &ck, a, json!({"model_id": "a b"})).await;
    assert_eq!((s, code(&v)), (422, Some("model_id_invalid")), "{v}");
    // 422 llm_model_not_allowed { field, model, allowed_models }
    let (s, v) = patch(
        &r,
        &ck,
        a,
        json!({"model_roles": {"endpoint_id": "listed", "small": "m-nope"}}),
    )
    .await;
    assert_eq!((s, code(&v)), (422, Some("llm_model_not_allowed")), "{v}");
    assert_eq!(v["error"]["details"]["model"], "m-nope");
    assert_eq!(v["error"]["details"]["field"], "model_roles.small");
    // 422 model_role_endpoint_mismatch
    let (s, v) = patch(
        &r,
        &ck,
        a,
        json!({"model_roles": {"endpoint_id": "roled", "small": "m-small"}}),
    )
    .await;
    assert_eq!(
        (s, code(&v)),
        (422, Some("model_role_endpoint_mismatch")),
        "{v}"
    );
    assert_eq!(v["error"]["details"]["default_endpoint_id"], "listed");
    // a role replaces the object; null clears it
    let (s, v) = patch(&r, &ck, a, json!({"model_roles": null})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["model_roles"], Value::Null);
    // the subagent default is a closed vocabulary
    let (s, _) = patch(&r, &ck, a, json!({"subagent_model": "huge"})).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn an_agent_role_on_another_endpoint_or_no_longer_allowed_is_dropped_never_repointed() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = agent_on(&r, "dropped", "listed").await;
    // stamped for another endpoint: every agent role is dropped, the endpoint's role applies
    sqlx::query("UPDATE agents SET model_roles = $2 WHERE id = $1")
        .bind(a)
        .bind(json!({"endpoint_id": "roled", "large": "m-other"}))
        .execute(r.h.pool())
        .await
        .unwrap();
    let d = readiness(&r, &ck, a).await["default"].clone();
    assert_eq!(d["roles_dropped"], json!(["large"]));
    assert_eq!(d["roles"]["large"]["model_id"], "m-big");
    assert_eq!(d["roles"]["large"]["source"], "endpoint");
    // stamped right, but one role is off the static list: only that one is dropped
    sqlx::query("UPDATE agents SET model_roles = $2 WHERE id = $1")
        .bind(a)
        .bind(json!({"endpoint_id": "listed", "large": "m-other", "small": "m-gone"}))
        .execute(r.h.pool())
        .await
        .unwrap();
    let d = readiness(&r, &ck, a).await["default"].clone();
    assert_eq!(d["roles_dropped"], json!(["small"]));
    assert_eq!(d["roles"]["large"]["source"], "agent");
    assert_eq!(d["roles"]["small"]["model_id"], Value::Null);
}

#[tokio::test]
async fn the_subagent_role_is_omitted_with_a_warning_when_it_resolves_to_nothing() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = agent_on(&r, "sub", "roled").await;
    // `medium` is unset on this endpoint
    sqlx::query("UPDATE agents SET subagent_model = 'medium' WHERE id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    let d = readiness(&r, &ck, a).await["default"].clone();
    assert_eq!(d["subagent_model"], "medium");
    assert_eq!(d["subagent_warning"], "subagent_role_unset");
    // `small` resolves
    sqlx::query("UPDATE agents SET subagent_model = 'small' WHERE id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    let d = readiness(&r, &ck, a).await["default"].clone();
    assert_eq!(d["subagent_warning"], Value::Null);
}

#[tokio::test]
async fn harnesses_report_the_roles_they_map() {
    let r = rig().await;
    let ck = owner(&r).await;
    let (s, v) = call(&r, &ck, Method::GET, "/v1/harnesses", None).await;
    assert_eq!(s, 200, "{v}");
    let by = |id: &str| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == id)
            .unwrap()["model_roles"]
            .clone()
    };
    assert_eq!(by("claude-code"), json!(["large", "medium", "small"]));
    assert_eq!(by("codex"), json!([]));
}

/// A role change is a render-input change: the agent's home generation moves (so the session is released
/// while idle), the same as a brief change.
#[tokio::test]
async fn a_role_change_bumps_the_home_sync_generation() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = agent_on(&r, "bump", "listed").await;
    let gen_of = || async {
        sqlx::query_scalar::<_, i64>("SELECT home_sync_generation FROM agents WHERE id = $1")
            .bind(a)
            .fetch_one(r.h.pool())
            .await
            .unwrap()
    };
    let before = gen_of().await;
    let (s, v) = patch(
        &r,
        &ck,
        a,
        json!({"model_roles": {"endpoint_id": "listed", "large": "m-other"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let mid = gen_of().await;
    assert!(mid > before);
    let (s, _) = patch(&r, &ck, a, json!({"subagent_model": "large"})).await;
    assert_eq!(s, 200);
    assert!(gen_of().await > mid);
}

/// (D-303) An endpoint role the org's allow-list excludes counts as unset for that org's agents, with the warning,
/// and is not sent; the operator's own list is wider.
#[tokio::test]
async fn an_endpoint_role_outside_the_orgs_narrower_allow_list_is_not_sent() {
    let r = rig().await;
    let ck = owner(&r).await;
    let a = agent_on(&r, "narrow", "listed").await;
    let d = readiness(&r, &ck, a).await["default"].clone();
    assert_eq!(d["roles"]["large"]["model_id"], "m-big", "before narrowing");
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(r.org)
        .bind(json!({"llm": {"endpoints": {"listed": {"allowed_models": ["m-def"]}}}}))
        .execute(r.h.pool())
        .await
        .unwrap();
    let d = readiness(&r, &ck, a).await["default"].clone();
    assert_eq!(d["model_id"], "m-def");
    assert_eq!(
        d["roles"]["large"],
        json!({"model_id": null, "source": "none", "warning": "role_not_allowed"})
    );
}

/// (D-303 drop rule) The dispatcher clears an agent's unusable roles in the database once, delivers only the
/// usable ones, and never re-points a dropped role.
#[tokio::test]
async fn the_dispatcher_clears_dropped_agent_roles_in_the_database() {
    let r = rig().await;
    let a = agent_on(&r, "clearing", "listed").await;
    sqlx::query("UPDATE agents SET model_roles = $2 WHERE id = $1")
        .bind(a)
        .bind(json!({"endpoint_id": "listed", "large": "m-other", "small": "m-gone"}))
        .execute(r.h.pool())
        .await
        .unwrap();
    let _lv = r.boot_agent(a).await;
    let d = r.deliver().await;
    let stored: Value = sqlx::query_scalar("SELECT model_roles FROM agents WHERE id = $1")
        .bind(a)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(stored, json!({"endpoint_id": "listed", "large": "m-other"}));
    assert_eq!(d["model"]["roles"]["large"], "m-other", "{d}");
    // `small` falls back to the endpoint, which names none: it is absent, not re-pointed
    assert!(
        d["model"]["roles"].get("small").is_none_or(Value::is_null),
        "{d}"
    );
}

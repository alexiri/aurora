#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-HAR (D-122, C-420..C-429, SEC-66): harness selection, `GET /v1/harnesses`, placement
//! (`harness_unavailable`), the switch (epoch, purges, 409) and dialect-aware model readiness.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_config::LlmEndpoint;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

async fn rig(enabled: &[&str]) -> Rig {
    let enabled: Vec<String> = enabled.iter().map(|s| (*s).to_owned()).collect();
    rig_with(move |c| {
        c.governance.allowed_key_scopes = vec!["organization".into()];
        c.hosted.enabled_harnesses = enabled;
        c.hosted.allow_fake_harness = true;
        // LiteLLM-style endpoints with explicit dialect sets.
        for (id, dialects) in [
            ("oai-chat", vec!["openai_chat"]),
            ("oai-both", vec!["openai_chat", "openai_responses"]),
        ] {
            c.llm.endpoints.push(LlmEndpoint {
                id: id.into(),
                provider: "litellm".into(),
                base_url: Some("http://127.0.0.1:1".into()),
                dialects: dialects.into_iter().map(str::to_owned).collect(),
                default_model: Some("m-oai".into()),
                ..Default::default()
            });
        }
    })
    .await
}

/// A workspace owner's cookie.
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

fn caps(harnesses: &[&str]) -> Value {
    json!({
        "backend": "fake", "home_durability": "resident",
        "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                      "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
        "max_sandboxes": 4, "harnesses": harnesses, "max_attachment_bytes": 1048576,
        "image": "registry.example/aurora-sandbox:9.9.9"
    })
}

/// The runner re-registers with the harnesses its image now contains.
async fn register(r: &mut Rig, harnesses: &[&str]) {
    let (s, v) = r
        .runner(
            None,
            "/v1/runner/register",
            json!({"version": "0.1", "backend": "fake", "capabilities": caps(harnesses),
                   "advertised_host": "runner-a"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    r.rep = v["replica_id"].as_str().unwrap().parse().unwrap();
}

async fn create(r: &Rig, ck: &str, name: &str, harness: Option<&str>) -> (u16, Value) {
    let mut body = json!({"name": name, "kind": "hosted"});
    if let Some(h) = harness {
        body["harness"] = json!(h);
    }
    call(
        r,
        ck,
        Method::POST,
        &format!("/v1/workspaces/{}/agents", r.ws),
        Some(body),
    )
    .await
}

async fn hosted_agent(r: &Rig, handle: &str, harness: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,$4,$4,'hosted',$5)",
    )
    .bind(id)
    .bind(r.org)
    .bind(r.ws)
    .bind(handle)
    .bind(harness)
    .execute(r.h.pool())
    .await
    .unwrap();
    id
}

async fn grant_llm(r: &Rig, agent: Uuid, endpoint: &str, name: &str) {
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

fn code(v: &Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

#[tokio::test]
async fn harnesses_route_lists_the_enabled_ones_with_availability() {
    let mut r = rig(&["claude-code", "codex"]).await;
    register(&mut r, &["claude-code", "fake"]).await;
    let ck = owner(&r).await;
    let (s, v) = call(&r, &ck, Method::GET, "/v1/harnesses", None).await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["claude-code", "codex"], "fake is not listed: {v}");
    assert_eq!(items[0]["available"], true);
    assert_eq!(items[0]["dialects"], json!(["anthropic_messages"]));
    assert_eq!(items[1]["available"], false, "no replica offers codex");
    assert_eq!(items[1]["dialects"], json!(["openai_responses"]));
    assert_eq!(items[1]["display_name"], "OpenAI Codex");
    assert_eq!(v["default"], "claude-code");
    // The image gains Codex: the runner registers again.
    register(&mut r, &["claude-code", "codex", "fake"]).await;
    let (_, v) = call(&r, &ck, Method::GET, "/v1/harnesses", None).await;
    assert_eq!(v["items"][1]["available"], true, "{v}");
    // Unauthenticated: refused.
    let (s, _, _) =
        r.h.send(Method::GET, "/v1/harnesses", &[("origin", ORIGIN)], None)
            .await;
    assert_eq!(s.as_u16(), 401);
}

#[tokio::test]
async fn default_deployment_lists_only_claude_code_and_refuses_codex() {
    let r = rig(&["claude-code"]).await;
    let ck = owner(&r).await;
    let (_, v) = call(&r, &ck, Method::GET, "/v1/harnesses", None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    let (s, v) = create(&r, &ck, "c1", Some("codex")).await;
    assert_eq!((s, code(&v)), (422, Some("harness_not_enabled")), "{v}");
    let (s, v) = create(&r, &ck, "c2", Some("gemini")).await;
    assert_eq!((s, code(&v)), (422, Some("harness_not_enabled")), "{v}");
    let (s, v) = create(&r, &ck, "c3", None).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["agent"]["harness"], "claude-code");
}

#[tokio::test]
async fn create_and_patch_validate_against_enabled_harnesses() {
    let r = rig(&["claude-code", "codex"]).await;
    let ck = owner(&r).await;
    let (s, v) = create(&r, &ck, "cx", Some("codex")).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["agent"]["harness"], "codex");
    let id = v["agent"]["id"].as_str().unwrap().to_owned();
    // `fake` is accepted only because the rig allows it (`hosted.allow_fake_harness`).
    let (s, v) = create(&r, &ck, "fk", Some("fake")).await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = call(
        &r,
        &ck,
        Method::PATCH,
        &format!("/v1/agents/{id}"),
        Some(json!({"harness": "gemini"})),
    )
    .await;
    assert_eq!((s, code(&v)), (422, Some("harness_not_enabled")), "{v}");
    let (s, v) = call(
        &r,
        &ck,
        Method::PATCH,
        &format!("/v1/agents/{id}"),
        Some(json!({"harness": "claude-code"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["harness"], "claude-code");
}

#[tokio::test]
async fn placement_refuses_a_runner_without_the_harness_and_starts_no_sandbox() {
    let mut r = rig(&["claude-code", "codex"]).await;
    register(&mut r, &["claude-code", "fake"]).await;
    let agent = hosted_agent(&r, "cx-agent", "codex").await;
    grant_llm(&r, agent, "oai-both", "k-cx").await;
    let conv = r.direct(agent).await;
    r.say(conv, "hello").await;
    let items = r.work().await;
    assert!(
        items.iter().all(|i| i["kind"] != "sandbox.start"),
        "no sandbox start for a harness no runner offers: {items:?}"
    );
    let (status, code): (String, Option<String>) = sqlx::query_as(
        "SELECT status, error_code FROM turns WHERE conversation_id = $1 AND role = 'turn'",
    )
    .bind(conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(code.as_deref(), Some("harness_unavailable"));
    // The same agent works as soon as a replica offers the harness.
    register(&mut r, &["claude-code", "codex", "fake"]).await;
    r.say(conv, "again").await;
    let items = r.work().await;
    assert!(
        items.iter().any(|i| i["kind"] == "sandbox.start"),
        "{items:?}"
    );
}

#[tokio::test]
async fn boot_document_carries_the_agents_harness_id() {
    let mut r = rig(&["claude-code", "codex"]).await;
    register(&mut r, &["claude-code", "codex", "fake"]).await;
    let agent = hosted_agent(&r, "cx-boot", "codex").await;
    grant_llm(&r, agent, "oai-both", "k-boot").await;
    let conv = r.direct(agent).await;
    r.say(conv, "hello").await;
    let items = r.work().await;
    let start = items.iter().find(|i| i["kind"] == "sandbox.start").unwrap();
    use base64::Engine;
    let raw = start["spec"]["boot_secret"].as_str().unwrap();
    let doc: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(doc["harness"]["id"], "codex");
    // SEC-66: no key material in the harness section.
    assert!(!doc["harness"].to_string().contains("sk-"));
}

/// The supervisor refuses a boot whose runtime is missing (`sandbox.status failed`,
/// `harness_unsupported`, sent instead of `ready`): the agent's queued turn fails with that reason
/// and nothing falls back to another harness.
#[tokio::test]
async fn boot_refusal_fails_the_queued_turn_with_harness_unsupported() {
    use crate::hosted::Live;
    let mut r = rig(&["claude-code", "codex"]).await;
    register(&mut r, &["claude-code", "codex", "fake"]).await;
    let agent = hosted_agent(&r, "cx-refused", "codex").await;
    grant_llm(&r, agent, "oai-both", "k-refused").await;
    let conv = r.direct(agent).await;
    r.say(conv, "hello").await;
    let items = r.work().await;
    let start = items.iter().find(|i| i["kind"] == "sandbox.start").unwrap();
    let lv = Live {
        agent,
        conv,
        sb: start["sandbox_id"].as_str().unwrap().to_owned(),
        boot: start["boot_id"].as_str().unwrap().to_owned(),
    };
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "sandbox.status", "state": "running", "container_ref": "c"})
        )
        .await,
        "applied"
    );
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "sandbox.status", "state": "failed",
                   "reason_code": "harness_unsupported",
                   "message": "the codex runtime is not in this image"})
        )
        .await,
        "applied"
    );
    let (status, code): (String, Option<String>) = sqlx::query_as(
        "SELECT status, error_code FROM turns WHERE conversation_id = $1 AND role = 'turn'",
    )
    .bind(conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(code.as_deref(), Some("harness_unsupported"));
    let (fc,): (Option<String>,) =
        sqlx::query_as("SELECT failure_code FROM sandboxes WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(fc.as_deref(), Some("harness_unsupported"));
}

#[tokio::test]
async fn switch_bumps_every_epoch_clears_refs_queues_purges_and_keeps_the_transcript() {
    let r = rig(&["claude-code", "codex"]).await;
    let ck = owner(&r).await;
    let agent = hosted_agent(&r, "sw", "claude-code").await;
    let c1 = r.direct(agent).await;
    // A second conversation of the same agent.
    let c2 = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, workspace_id, agent_id, kind, external_ref, title, last_activity_at)
         SELECT $1, workspace_id, agent_id, 'api', 'second', 'second', now() FROM conversations WHERE id = $2",
    )
    .bind(c2)
    .bind(c1)
    .execute(r.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE conversations SET harness_session_ref = 'sess-1', harness_session_epoch = 3 WHERE id = $1",
    )
    .bind(c1)
    .execute(r.h.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE conversations SET harness_session_ref = 'sess-2' WHERE id = $1")
        .bind(c2)
        .execute(r.h.pool())
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript_events WHERE conversation_id = ANY($1)",
    )
    .bind(vec![c1, c2])
    .fetch_one(r.h.pool())
    .await
    .unwrap();

    // Same harness: nothing happens.
    let (s, v) = call(
        &r,
        &ck,
        Method::PATCH,
        &format!("/v1/agents/{agent}"),
        Some(json!({"harness": "claude-code", "name": "Renamed"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM session_purges WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);

    let (s, v) = call(
        &r,
        &ck,
        Method::PATCH,
        &format!("/v1/agents/{agent}"),
        Some(json!({"harness": "codex"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["harness"], "codex");
    let rows: Vec<(Uuid, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, harness_session_epoch, harness_session_ref FROM conversations
         WHERE agent_id = $1 ORDER BY id",
    )
    .bind(agent)
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for (id, epoch, sref) in &rows {
        assert!(sref.is_none(), "{id}: reference cleared");
        assert_eq!(*epoch, if *id == c1 { 4 } else { 1 }, "{id}");
    }
    let purges: Vec<(Option<Uuid>, String, Option<String>)> = sqlx::query_as(
        "SELECT conversation_id, session_ref, harness FROM session_purges
         WHERE agent_id = $1 AND done_at IS NULL ORDER BY session_ref",
    )
    .bind(agent)
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        purges,
        vec![
            (Some(c1), "sess-1".into(), Some("claude-code".into())),
            (Some(c2), "sess-2".into(), Some("claude-code".into()))
        ]
    );
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript_events WHERE conversation_id = ANY($1)",
    )
    .bind(vec![c1, c2])
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(before, after, "the transcript is untouched");

    // A purge of the previous harness is not held back by a new turn (it cannot be in use);
    // a purge of the sandbox's own harness still is.
    r.say(c1, "after the switch").await;
    let mut conn = r.h.pool().acquire().await.unwrap();
    let due = aurora_db::repo::lifecycle::pending_for_agent(&mut conn, agent)
        .await
        .unwrap();
    assert_eq!(due.len(), 2, "{due:?}");
    assert!(
        due.iter()
            .all(|p| p.harness.as_deref() == Some("claude-code"))
    );
    aurora_db::repo::lifecycle::enqueue_purge(&mut conn, agent, Some(c1), "own-1")
        .await
        .unwrap();
    let due = aurora_db::repo::lifecycle::pending_for_agent(&mut conn, agent)
        .await
        .unwrap();
    assert!(!due.iter().any(|p| p.session_ref == "own-1"), "{due:?}");
}

#[tokio::test]
async fn switch_is_refused_during_a_turn_with_409_and_changes_nothing() {
    let r = rig(&["claude-code", "codex"]).await;
    let ck = owner(&r).await;
    let agent = hosted_agent(&r, "sw-busy", "claude-code").await;
    let conv = r.direct(agent).await;
    sqlx::query("UPDATE conversations SET harness_session_ref = 'sess-x' WHERE id = $1")
        .bind(conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    r.say(conv, "working").await; // a queued turn is an active turn
    let (s, v) = call(
        &r,
        &ck,
        Method::PATCH,
        &format!("/v1/agents/{agent}"),
        Some(json!({"harness": "codex", "name": "nope"})),
    )
    .await;
    assert_eq!((s, code(&v)), (409, Some("turn_active")), "{v}");
    let (h, name): (String, String) =
        sqlx::query_as("SELECT harness, name FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!((h.as_str(), name.as_str()), ("claude-code", "sw-busy"));
    let (epoch, sref): (i64, Option<String>) = sqlx::query_as(
        "SELECT harness_session_epoch, harness_session_ref FROM conversations WHERE id = $1",
    )
    .bind(conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!((epoch, sref.as_deref()), (0, Some("sess-x")));
}

#[tokio::test]
async fn model_readiness_follows_the_harness_dialects_and_a_switch_drops_incompatible_overrides() {
    let r = rig(&["claude-code", "codex"]).await;
    let ck = owner(&r).await;
    let agent = hosted_agent(&r, "dial", "codex").await;
    grant_llm(&r, agent, "oai-chat", "k-chat").await;
    grant_llm(&r, agent, "oai-both", "k-both").await;
    let (s, v) = call(
        &r,
        &ck,
        Method::GET,
        &format!("/v1/agents/{agent}/model-readiness"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["harness"], "codex");
    let by = |n: &str| {
        v["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["name"] == n)
            .unwrap_or_else(|| panic!("{v}"))
            .clone()
    };
    // The pinned Codex speaks `openai_responses` only: a chat-only endpoint is not usable.
    assert_eq!(by("k-chat")["reason"], "no_shared_dialect");
    assert_eq!(by("k-both")["usable"], true);
    assert_eq!(v["default"]["endpoint_id"], "oai-both");

    // Switching to claude-code: neither OpenAI-dialect endpoint serves it any more.
    sqlx::query("UPDATE agents SET model_endpoint_id = 'oai-both', model_id = 'm-x', model_provider = 'litellm' WHERE id = $1")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, _) = call(
        &r,
        &ck,
        Method::PATCH,
        &format!("/v1/agents/{agent}"),
        Some(json!({"harness": "claude-code"})),
    )
    .await;
    assert_eq!(s, 200);
    let (_, v) = call(
        &r,
        &ck,
        Method::GET,
        &format!("/v1/agents/{agent}/model-readiness"),
        None,
    )
    .await;
    assert_eq!(v["harness"], "claude-code");
    assert!(
        v["keys"]
            .as_array()
            .unwrap()
            .iter()
            .all(|k| k["reason"] == "no_shared_dialect"),
        "{v}"
    );
    assert_eq!(
        v["default"],
        Value::Null,
        "no compatible granted key: no_model_key"
    );
}

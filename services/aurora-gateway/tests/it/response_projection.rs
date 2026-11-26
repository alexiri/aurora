//! (lane review 2026-11-25) A tool that declares a response projection has every answer
//! reduced to the projected `$`-paths for every caller: the gateway applies it on the
//! response path, so a raw send sees the same reduced answer the agent CLI shows —
//! settings and secrets in the upstream answer reach no caller. Fail closed: an answer the
//! gateway cannot project (truncated, not JSON, not the projected shape) is withheld.
//!
//! A child module of `apps` (it reuses that file's pipeline harness).
use super::*;

fn projection_tools() -> Value {
    json!({"read": {"items": [
        {"id": "cps", "name": "c", "host": "127.0.0.1", "path": "/cps", "methods": ["GET"],
         "project": ["$.name", "$.type"]},
        // The same answer on a tool that declares no projection: unchanged (the projection is
        // per tool, not per app).
        {"id": "raw", "name": "r", "host": "127.0.0.1", "path": "/raw", "methods": ["GET"]},
        // A projected tool whose upstream answer is not the shape it projects.
        {"id": "cpsbad", "name": "b", "host": "127.0.0.1", "path": "/cpsbad", "methods": ["GET"],
         "project": ["$.name", "$.type"]}
    ]}})
}

async fn start_projection() -> Env {
    let snap = snapshot(
        vec![app_json("demo", projection_tools(), false)],
        vec![host_json(
            "demo",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"T"}"#)], |w| {
        let c = conn(w, "demo", "c1", None);
        let mut r = Rows::default();
        r.ws.default_action = "block".into();
        r.ws.derived.push((
            Uuid::now_v7(),
            json!({"name":"d1","enabled":true,"identities":[],"action":"allow",
                   "targets":[{"type":"connection","id":c.id,"tools":["cps","raw","cpsbad"]}]}),
        ));
        r.conns.push(c);
        r
    })
    .await;
    env
}

#[tokio::test]
async fn a_projected_tool_answers_with_names_and_types_only_on_the_raw_path() {
    let env = start_projection().await;
    let r = get(&env, "/cps", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("the answer is the projected JSON");
    assert_eq!(
        v,
        json!([{"name": "hook", "type": "hook"}, {"name": "mail", "type": "email"}]),
        "settings (url, addresses, token) never reach the caller: {}",
        r.body
    );
    // Names and types come through; nothing else of the item does.
    assert!(!r.body.contains("settings"), "{}", r.body);
    assert!(!r.body.contains("addresses"), "{}", r.body);
    assert!(!r.body.contains("hooks.example.org"), "{}", r.body);
    assert!(!r.body.contains("uid"), "{}", r.body);
}

#[tokio::test]
async fn a_tool_without_a_projection_answers_unchanged() {
    let env = start_projection().await;
    let r = get(&env, "/raw", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("settings"), "{}", r.body);
    assert!(r.body.contains("addresses"), "{}", r.body);
}

#[tokio::test]
async fn an_answer_the_gateway_cannot_project_is_withheld() {
    let env = start_projection().await;
    let r = get(&env, "/cpsbad", "").await;
    assert_eq!(r.status, 502, "fail closed: {}", r.body);
    assert_eq!(r.code(), "tool_response_unprojectable", "{}", r.body);
    assert!(!r.body.contains("not json"), "{}", r.body);
}

// A request whose URL carries a connection value (the host injects it as a query parameter)
// sets the echo needles; the answer path that scrubs them is the same one the projection runs
// on, so a projected tool answers projected there too (a person-pinned server's answers are
// scrubbed the same way).
fn keyed_tools() -> Value {
    json!({"read": {"items": [
        {"id": "cpskey", "name": "k", "host": "127.0.0.1", "path": "/cpskey", "methods": ["GET"],
         "project": ["$.name", "$.type"]},
        // The same projection on an upstream that echoes the query back: withheld.
        {"id": "cpsecho", "name": "e", "host": "127.0.0.1", "path": "/cpsecho",
         "methods": ["GET"], "project": ["$.name", "$.type"]}
    ]}})
}

async fn start_keyed() -> Env {
    let snap = snapshot(
        vec![app_json("demo", keyed_tools(), false)],
        vec![host_json(
            "demo",
            None,
            json!({"inject": [
                {"bearer": "{credentials.tok}"},
                {"query": {"name": "api_key", "value": "{credentials.tok}"}}
            ]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"s3cr3t"}"#)], |w| {
        let c = conn(w, "demo", "c1", None);
        let mut r = Rows::default();
        r.ws.default_action = "block".into();
        r.ws.derived.push((
            Uuid::now_v7(),
            json!({"name":"d1","enabled":true,"identities":[],"action":"allow",
                   "targets":[{"type":"connection","id":c.id,"tools":["cpskey","cpsecho"]}]}),
        ));
        r.conns.push(c);
        r
    })
    .await;
    env
}

#[tokio::test]
async fn a_projected_tool_with_a_url_credential_still_answers_projected() {
    let env = start_keyed().await;
    let r = get(&env, "/cpskey", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("the answer is the projected JSON");
    assert_eq!(
        v,
        json!([{"name": "hook", "type": "hook"}, {"name": "mail", "type": "email"}]),
        "the needles do not turn the projection off: {}",
        r.body
    );
    assert!(!r.body.contains("settings"), "{}", r.body);
    assert!(!r.body.contains("addresses"), "{}", r.body);
}

#[tokio::test]
async fn an_upstream_echoing_the_url_credential_is_still_withheld() {
    let env = start_keyed().await;
    let r = get(&env, "/cpsecho", "").await;
    assert_eq!(r.status, 502, "{}", r.body);
    assert_eq!(r.code(), "upstream_echoed_credential", "{}", r.body);
    assert!(!r.body.contains("s3cr3t"), "{}", r.body);
}

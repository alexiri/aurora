#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-MODELS-2 (D-304, C-879, SEC-123): usage per model and the trust rule. The harness's claim is
//! reduced at turn end: `provider`/`endpoint_id` come from the turn's model choice, an entry outside what
//! the turn could have used is folded into the default model's entry, and the totals are the sum of the
//! kept entries.

use crate::model_roles::{agent_on, rig};
use serde_json::{Value, json};

fn entry(model: &str, input: i64, output: i64) -> Value {
    json!({"model": model, "input_tokens": input, "output_tokens": output,
           "cache_read_tokens": 1, "cache_write_tokens": 2})
}

/// Boots a `claude-code` agent on `endpoint`, lets its turn finish with `claim`, returns `turns.usage`.
async fn usage_after(endpoint: &str, claim: Value) -> Value {
    let r = rig().await;
    let a = agent_on(&r, "meter", endpoint).await;
    let lv = r.boot_agent(a).await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    let s = r
        .report(
            &lv,
            json!({"kind": "turn.result", "turn_id": turn, "status": "done", "usage": claim}),
        )
        .await;
    assert_eq!(s, "applied");
    sqlx::query_scalar("SELECT usage FROM turns WHERE id = $1::uuid")
        .bind(&turn)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

fn models(u: &Value) -> Vec<(String, i64)> {
    u["by_model"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["model"].as_str().unwrap().to_owned(),
                e["input_tokens"].as_i64().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_default_a_role_and_an_allow_listed_model_are_kept_and_provider_is_stamped() {
    // endpoint `listed`: default m-def, role large = m-big, allow-list {m-def, m-big, m-other}
    let u = usage_after(
        "listed",
        json!({"input_tokens": 999, "output_tokens": 999, "provider": "anthropic", "endpoint_id": "evil",
               "model": "claimed", "by_model": [
                   entry("m-def", 100, 10), entry("m-big", 200, 20), entry("m-other", 300, 30)]}),
    )
    .await;
    assert_eq!(
        models(&u),
        [
            ("m-def".into(), 100),
            ("m-big".into(), 200),
            ("m-other".into(), 300)
        ]
    );
    assert_eq!(
        u["input_tokens"], 600,
        "the totals are the sum of the entries: {u}"
    );
    assert_eq!(u["output_tokens"], 60);
    assert_eq!(u["cache_read_tokens"], 3);
    assert_eq!(u["cache_write_tokens"], 6);
    assert_eq!(u.get("unverified_models"), None, "{u}");
    // never what the harness claims
    assert_eq!(
        (
            u["provider"].as_str(),
            u["endpoint_id"].as_str(),
            u["model"].as_str()
        ),
        (Some("litellm"), Some("listed"), Some("m-def"))
    );
}

#[tokio::test]
async fn a_foreign_model_is_folded_into_the_default_and_marks_the_turn() {
    let u = usage_after(
        "listed",
        json!({"by_model": [entry("m-def", 100, 10), entry("m-not-allowed", 500, 50)]}),
    )
    .await;
    assert_eq!(models(&u), [("m-def".into(), 600)], "{u}");
    assert_eq!(u["output_tokens"], 60);
    assert_eq!(u["unverified_models"], true);
    // only foreign entries: the default's entry is created to hold them
    let u = usage_after("listed", json!({"by_model": [entry("m-nope", 7, 1)]})).await;
    assert_eq!(models(&u), [("m-def".into(), 7)], "{u}");
    assert_eq!(u["unverified_models"], true);
}

#[tokio::test]
async fn more_than_sixteen_entries_fold_the_rest_into_the_default() {
    // no allow-list on `roled`: every pattern-valid id is trusted, so only the limit applies
    let mut by_model = vec![entry("m-def", 1, 1)];
    by_model.extend((0..20).map(|i| entry(&format!("m-x{i:02}"), 1, 1)));
    let u = usage_after("roled", json!({ "by_model": by_model })).await;
    let ms = models(&u);
    assert_eq!(ms.len(), aurora_core::limits::USAGE_BY_MODEL_MAX, "{u}");
    assert_eq!(ms[0].0, "m-def");
    assert_eq!(u["input_tokens"], 21, "nothing is lost by the fold: {u}");
    assert_eq!(
        ms[0].1,
        1 + (21 - ms.len() as i64),
        "the overflow lands on the default"
    );
    assert_eq!(u["unverified_models"], true);
}

#[tokio::test]
async fn with_no_allow_list_a_valid_id_is_kept_and_an_invalid_one_is_folded() {
    let u = usage_after(
        "roled",
        json!({"by_model": [entry("m-def", 100, 10), entry("glm-4.6", 200, 20),
                            entry("bad id; rm -rf", 300, 30), entry("$(id)", 400, 40)]}),
    )
    .await;
    assert_eq!(
        models(&u),
        [("m-def".into(), 100 + 300 + 400), ("glm-4.6".into(), 200)],
        "{u}"
    );
    assert_eq!(u["unverified_models"], true);
}

#[tokio::test]
async fn a_harness_that_reports_no_breakdown_gets_one_entry_for_the_default() {
    let u = usage_after(
        "listed",
        json!({"input_tokens": 12, "output_tokens": 3, "cache_read_tokens": 4}),
    )
    .await;
    assert_eq!(models(&u), [("m-def".into(), 12)], "{u}");
    assert_eq!(u["output_tokens"], 3);
    assert_eq!(u["cache_read_tokens"], 4);
    assert_eq!(u.get("unverified_models"), None);
}

/// (C-1100) Boots an agent, lets `shape` change the key between dispatch and the end of the turn, finishes the turn,
/// and returns the payload of its durable `turn.finished` event.
async fn finished_payload(shape: impl AsyncFn(&crate::hosted::Rig, uuid::Uuid)) -> Value {
    let r = rig().await;
    let a = agent_on(&r, "keyed", "plain").await;
    let lv = r.boot_agent(a).await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    let key: uuid::Uuid =
        sqlx::query_scalar("SELECT (model_basis->>'key_id')::uuid FROM turns WHERE id = $1::uuid")
            .bind(&turn)
            .fetch_one(r.h.pool())
            .await
            .expect("the dispatcher records the key in the basis");
    shape(&r, key).await;
    let s = r
        .report(
            &lv,
            json!({"kind": "turn.result", "turn_id": turn, "status": "done"}),
        )
        .await;
    assert_eq!(s, "applied");
    sqlx::query_scalar(
        "SELECT payload FROM transcript_events WHERE turn_id = $1::uuid AND type = 'turn.finished'",
    )
    .bind(&turn)
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn turn_finished_names_the_class_of_the_key_it_ran_on() {
    // An ordinary organization key.
    let p = finished_payload(async |_, _| {}).await;
    assert_eq!(p["ai_key"], "organization", "{p}");
    // The instance's starter grant for that organization: the Company key.
    let p = finished_payload(async |r, key| {
        sqlx::query(
            "INSERT INTO instance_placement (mode, default_org_id, personal_enabled, starter_grants)
             VALUES ('shared_org', $1, true, $2)",
        )
        .bind(r.org)
        .bind(json!([{"kind": "secret", "id": key}]))
        .execute(r.h.pool())
        .await
        .unwrap();
    })
    .await;
    assert_eq!(p["ai_key"], "starter", "{p}");
    // A key of the workspace.
    let p = finished_payload(async |r, key| {
        sqlx::query("UPDATE secrets SET workspace_id = $2 WHERE id = $1")
            .bind(key)
            .bind(r.ws)
            .execute(r.h.pool())
            .await
            .unwrap();
    })
    .await;
    assert_eq!(p["ai_key"], "workspace", "{p}");
    // Never a name or an id.
    assert!(!p.to_string().contains("k-keyed"), "{p}");
}

#[tokio::test]
async fn turn_finished_has_no_class_when_the_key_is_gone() {
    let p = finished_payload(async |r, key| {
        sqlx::query("DELETE FROM grants WHERE secret_id = $1")
            .bind(key)
            .execute(r.h.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM secrets WHERE id = $1")
            .bind(key)
            .execute(r.h.pool())
            .await
            .unwrap();
    })
    .await;
    assert_eq!(p.get("ai_key"), None, "{p}");
}

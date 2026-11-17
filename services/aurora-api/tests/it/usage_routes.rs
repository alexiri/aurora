#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-USAGE-1 (D-359, D-360, C-1000, C-1003, C-1006): the workspace, instance and organization usage
//! routes read the `usage_daily` ledger and `runtime_daily`, per day, agent, workspace and model, and never
//! another scope's rows.

use crate::common::*;
use chrono::{Duration, NaiveDate, Utc};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn today() -> NaiveDate {
    Utc::now().date_naive()
}

async fn call(h: &Harness, tok: &str, uri: &str) -> (u16, Value) {
    let c = cookie(tok);
    let (s, _, v) = h
        .send(
            Method::GET,
            uri,
            &[("cookie", &c), ("origin", ORIGIN)],
            None,
        )
        .await;
    (s.as_u16(), v)
}

async fn agent(h: &Harness, org: Uuid, ws: Uuid, name: &str) -> Uuid {
    let a = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,$4,$4,'hosted')")
        .bind(a)
        .bind(org)
        .bind(ws)
        .bind(name)
        .execute(h.pool())
        .await
        .unwrap();
    a
}

/// One ledger row: `days_ago`, model, turns, seconds, input, output.
#[allow(clippy::too_many_arguments)]
async fn seed(
    h: &Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    days_ago: i64,
    model: Option<&str>,
    turns: i64,
    secs: i64,
    input: i64,
    output: i64,
) {
    sqlx::query(
        "INSERT INTO usage_daily (org_id, workspace_id, agent_id, day, provider, endpoint_id, model, turns,
                                  turn_seconds, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens)
         VALUES ($1,$2,$3,$4,'litellm','ep',$5,$6,$7,$8,$9,1,2)",
    )
    .bind(org)
    .bind(ws)
    .bind(agent)
    .bind(today() - Duration::days(days_ago))
    .bind(model)
    .bind(turns)
    .bind(secs)
    .bind(input)
    .bind(output)
    .execute(h.pool())
    .await
    .unwrap();
}

async fn awake(h: &Harness, org: Uuid, ws: Uuid, agent: Uuid, days_ago: i64, secs: i64) {
    sqlx::query("INSERT INTO runtime_daily (org_id, workspace_id, agent_id, day, awake_seconds) VALUES ($1,$2,$3,$4,$5)")
        .bind(org)
        .bind(ws)
        .bind(agent)
        .bind(today() - Duration::days(days_ago))
        .bind(secs)
        .execute(h.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_workspace_route_aggregates_filters_and_stays_in_its_workspace() {
    let h = Harness::new().await;
    let org = h.org().await;
    let (ws, other_ws) = (h.workspace(org).await, h.workspace(org).await);
    let member = h.user("m@x.io", false).await;
    h.member(org, member, "member", "active").await;
    h.bind_ws(ws, member, "member").await;
    let (_, mt) = h.good_session(member).await;
    let outsider = h.user("o@x.io", false).await;
    let (_, ot) = h.good_session(outsider).await;
    let (a1, a2) = (
        agent(&h, org, ws, "alpha").await,
        agent(&h, org, ws, "beta").await,
    );
    let gone = Uuid::now_v7(); // a deleted agent: its ledger rows stay
    let elsewhere = agent(&h, org, other_ws, "away").await;
    seed(&h, org, ws, a1, 1, Some("m-def"), 2, 30, 100, 10).await;
    seed(&h, org, ws, a1, 1, Some("m-big"), 0, 0, 50, 5).await;
    seed(&h, org, ws, a2, 3, Some("m-def"), 1, 7, 20, 2).await;
    seed(&h, org, ws, gone, 3, None, 1, 4, 1, 1).await;
    seed(
        &h,
        org,
        other_ws,
        elsewhere,
        1,
        Some("m-def"),
        9,
        99,
        9999,
        999,
    )
    .await;
    awake(&h, org, ws, a1, 1, 600).await;
    awake(&h, org, other_ws, elsewhere, 1, 12345).await;

    let base = format!("/v1/workspaces/{ws}/usage");
    let (s, v) = call(&h, &mt, &base).await;
    assert_eq!(s, 200, "{v}");
    // the default range is the last 30 days, every day present
    assert_eq!(v["to"], today().to_string());
    assert_eq!(v["from"], (today() - Duration::days(29)).to_string());
    assert_eq!(v["by_day"].as_array().unwrap().len(), 30);
    assert_eq!(v["by_day"][0]["day"], v["from"]);
    assert_eq!(v["by_day"][29]["day"], v["to"]);
    assert_eq!(v["by_day"][29]["turns"], 0, "a zero day is present");
    let t = &v["totals"];
    assert_eq!(
        (
            t["turns"].as_i64(),
            t["turn_seconds"].as_i64(),
            t["awake_seconds"].as_i64(),
            t["input_tokens"].as_i64(),
            t["output_tokens"].as_i64(),
            t["cache_read_tokens"].as_i64(),
            t["cache_write_tokens"].as_i64()
        ),
        (
            Some(4),
            Some(41),
            Some(600),
            Some(171),
            Some(18),
            Some(4),
            Some(8)
        ),
        "the other workspace's rows never appear: {v}"
    );
    // by_agent: heaviest first; a deleted agent has a null name; the other workspace's agent is absent
    let ag = v["by_agent"].as_array().unwrap();
    assert_eq!(
        ag.iter()
            .map(|a| a["agent_name"].clone())
            .collect::<Vec<_>>(),
        [json!("alpha"), json!("beta"), Value::Null]
    );
    assert_eq!(ag[0]["awake_seconds"], 600);
    assert_eq!(ag[2]["agent_id"], json!(gone));
    // by_model: tokens desc; turns and seconds only under the default's key; no awake time
    let md = v["by_model"].as_array().unwrap();
    assert_eq!(md[0]["model"], "m-def");
    assert_eq!(
        (md[0]["turns"].as_i64(), md[0]["input_tokens"].as_i64()),
        (Some(3), Some(120))
    );
    assert_eq!(md[1]["model"], "m-big");
    assert_eq!(
        md[1]["turns"], 0,
        "a model used only by subagents: tokens, no turns"
    );
    assert_eq!(md[0]["endpoint_id"], "ep");
    assert!(md[0].get("awake_seconds").is_none());
    assert!(v["runtime_as_of"].is_null());

    // agent filter: also an agent of another workspace or a deleted one: zeros, no 404
    let (s, v) = call(&h, &mt, &format!("{base}?agent_id={a2}")).await;
    assert_eq!((s, v["totals"]["turns"].as_i64()), (200, Some(1)));
    let (s, v) = call(&h, &mt, &format!("{base}?agent_id={elsewhere}")).await;
    assert_eq!(
        (s, v["totals"]["input_tokens"].as_i64()),
        (200, Some(0)),
        "{v}"
    );
    assert!(v["by_agent"].as_array().unwrap().is_empty());
    let (s, v) = call(&h, &mt, &format!("{base}?agent_id={gone}")).await;
    assert_eq!((s, v["totals"]["turns"].as_i64()), (200, Some(1)));
    // an explicit range
    let day = (today() - Duration::days(3)).to_string();
    let (s, v) = call(&h, &mt, &format!("{base}?from={day}&to={day}")).await;
    assert_eq!(s, 200);
    assert_eq!(v["by_day"].as_array().unwrap().len(), 1);
    assert_eq!(v["totals"]["turns"], 2);

    // errors
    let (s, v) = call(&h, &mt, &format!("{base}?from=2026-11-10&to=2026-11-09")).await;
    assert_eq!(
        (s, v["error"]["details"]["reason"].clone()),
        (422, json!("from_after_to")),
        "{v}"
    );
    assert_eq!(v["error"]["code"], "usage_range_invalid");
    let (s, v) = call(&h, &mt, &format!("{base}?from=2024-01-01&to=2026-01-01")).await;
    assert_eq!(
        (s, v["error"]["details"]["reason"].clone()),
        (422, json!("too_long")),
        "{v}"
    );
    let (s, _) = call(&h, &mt, &format!("{base}?from=2026-02-30")).await;
    assert_eq!(s, 400);
    let (s, _) = call(&h, &mt, &format!("{base}?to=yesterday")).await;
    assert_eq!(s, 400);
    // exactly 366 days is fine
    let (s, _) = call(
        &h,
        &mt,
        &format!(
            "{base}?from={}&to={}",
            today() - Duration::days(365),
            today()
        ),
    )
    .await;
    assert_eq!(s, 200);
    // a non-member gets what the request log gives them
    let (s_act, _) = call(&h, &ot, &format!("/v1/workspaces/{ws}/activity")).await;
    let (s, _) = call(&h, &ot, &base).await;
    assert_eq!((s, s_act), (404, 404));

    // an API key works; runtime_as_of is the rollup's watermark
    let (_, key) = h.api_key(member, org, Some(ws), false).await;
    let kh = bearer(&key);
    let (s, v) = h.get(&base, &[("authorization", kh.as_str())]).await;
    assert_eq!(s, 200, "{v}");
    let at = Utc::now();
    sqlx::query("UPDATE usage_rollup_state SET runtime_watermark = $1")
        .bind(at)
        .execute(h.pool())
        .await
        .unwrap();
    let (_, v) = call(&h, &mt, &base).await;
    let got = chrono::DateTime::parse_from_rfc3339(v["runtime_as_of"].as_str().unwrap()).unwrap();
    assert_eq!(got.timestamp_micros(), at.timestamp_micros());
}

#[tokio::test]
async fn the_instance_route_lists_the_top_fifty_workspaces_and_sums_the_rest() {
    let h = Harness::new().await;
    let root = h.user("root@x.io", true).await;
    let plain = h.user("p@x.io", false).await;
    let (_, rt) = h.good_session(root).await;
    let (_, pt) = h.good_session(plain).await;
    let (o1, o2) = (h.org().await, h.org().await);
    let ghost_org = Uuid::now_v7(); // a deleted organization
    // 51 workspaces: ws i has i+1 input tokens; the smallest is the 51st
    let mut first = None;
    for i in 0..51_i64 {
        let org = if i % 2 == 0 { o1 } else { o2 };
        let ws = h.workspace(org).await;
        let a = Uuid::now_v7();
        seed(&h, org, ws, a, 1, Some("m"), 1, 1, i + 1, 0).await;
        first.get_or_insert(ws);
    }
    let ghost_ws = Uuid::now_v7();
    seed(
        &h,
        ghost_org,
        ghost_ws,
        Uuid::now_v7(),
        2,
        None,
        1,
        1,
        1000,
        0,
    )
    .await;

    assert_eq!(call(&h, &pt, "/v1/admin/usage").await.0, 403);
    let (s, v) = call(&h, &rt, "/v1/admin/usage").await;
    assert_eq!(s, 200, "{v}");
    let ws = v["by_workspace"].as_array().unwrap();
    assert_eq!(
        ws.len(),
        50,
        "52 workspaces have rows; the top 50 are listed"
    );
    assert_eq!(ws[0]["workspace_id"], json!(ghost_ws));
    assert!(ws[0]["workspace_name"].is_null() && ws[0]["org_name"].is_null());
    assert_eq!(ws[0]["org_id"], json!(ghost_org));
    let other = &v["other_workspaces"];
    assert_eq!(
        other["input_tokens"],
        1 + 2,
        "the two smallest workspaces are summed"
    );
    let listed: i64 = ws.iter().map(|w| w["input_tokens"].as_i64().unwrap()).sum();
    assert_eq!(
        listed + other["input_tokens"].as_i64().unwrap(),
        v["totals"]["input_tokens"].as_i64().unwrap()
    );
    // by_org: every organization with a counter; names null once deleted
    let orgs = v["by_org"].as_array().unwrap();
    assert_eq!(orgs.len(), 3);
    assert_eq!(orgs[0]["org_id"], json!(ghost_org));
    assert!(orgs[0]["org_name"].is_null());
    assert!(orgs[1]["org_name"].is_string());
    // the org filter, also a deleted organization's
    let (_, v) = call(&h, &rt, &format!("/v1/admin/usage?org_id={o1}")).await;
    assert_eq!(v["by_org"].as_array().unwrap().len(), 1);
    assert!(
        v["other_workspaces"].is_null(),
        "26 workspaces fit in the top: {v}"
    );
    assert_eq!(v["by_workspace"].as_array().unwrap().len(), 26);
    let (_, v) = call(&h, &rt, &format!("/v1/admin/usage?org_id={ghost_org}")).await;
    assert_eq!(v["totals"]["input_tokens"], 1000);
    assert_eq!(v["by_model"][0]["model"], Value::Null);
    let (s, _) = call(&h, &rt, "/v1/admin/usage?from=2026-11-10&to=2026-11-09").await;
    assert_eq!(s, 422);
}

#[tokio::test]
async fn the_org_route_is_for_org_admins_and_never_reads_another_org() {
    let h = Harness::new().await;
    let (o1, o2) = (h.org().await, h.org().await);
    let (w1, w1b, w2) = (
        h.workspace(o1).await,
        h.workspace(o1).await,
        h.workspace(o2).await,
    );
    let admin = h.user("a@x.io", false).await;
    h.member(o1, admin, "admin", "active").await;
    let plain = h.user("m@x.io", false).await;
    h.member(o1, plain, "member", "active").await;
    let outsider = h.user("o@x.io", false).await;
    let (_, at) = h.good_session(admin).await;
    let (_, pt) = h.good_session(plain).await;
    let (_, ot) = h.good_session(outsider).await;
    let a = Uuid::now_v7();
    seed(&h, o1, w1, a, 1, Some("m"), 1, 5, 10, 1).await;
    seed(&h, o1, w1b, a, 1, Some("m"), 1, 5, 30, 3).await;
    seed(&h, o2, w2, a, 1, Some("m"), 7, 70, 7000, 700).await;

    // the same answers as the storage route gives
    let storage = format!("/v1/orgs/{o1}/storage");
    for tok in [&pt, &ot] {
        let want = call(&h, tok, &storage).await.0;
        let got = call(&h, tok, &format!("/v1/orgs/{o1}/usage")).await.0;
        assert_eq!(got, want);
        assert!(got == 403 || got == 404);
    }
    let (s, v) = call(&h, &at, &format!("/v1/orgs/{o1}/usage")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["totals"]["input_tokens"], 40);
    assert!(v["other_workspaces"].is_null());
    let ws = v["by_workspace"].as_array().unwrap();
    assert_eq!(ws.len(), 2);
    assert_eq!(ws[0]["workspace_id"], json!(w1b));
    assert!(ws[0]["workspace_name"].is_string());
    assert!(
        ws[0].get("org_id").is_none(),
        "no org fields in the org view"
    );
    assert!(v.get("by_agent").is_none() && v.get("by_org").is_none());
    // the workspace filter; another organization's workspace yields zeros, never its rows
    let (_, v) = call(&h, &at, &format!("/v1/orgs/{o1}/usage?workspace_id={w1}")).await;
    assert_eq!(v["totals"]["input_tokens"], 10);
    let (s, v) = call(&h, &at, &format!("/v1/orgs/{o1}/usage?workspace_id={w2}")).await;
    assert_eq!(
        (
            s,
            v["totals"]["input_tokens"].as_i64(),
            v["totals"]["turns"].as_i64()
        ),
        (200, Some(0), Some(0))
    );
    assert!(v["by_workspace"].as_array().unwrap().is_empty());
    assert_eq!(v["by_model"], json!([]));
    // 51 workspaces: the 51st is summed
    for i in 0..50_i64 {
        let w = h.workspace(o1).await;
        seed(&h, o1, w, Uuid::now_v7(), 1, Some("m"), 1, 1, 100 + i, 0).await;
    }
    let (_, v) = call(&h, &at, &format!("/v1/orgs/{o1}/usage")).await;
    assert_eq!(v["by_workspace"].as_array().unwrap().len(), 50);
    assert_eq!(
        v["other_workspaces"]["input_tokens"], 40,
        "the two original workspaces (10 + 30 tokens) fall below the 50 new ones: {v}"
    );
    let listed: i64 = v["by_workspace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["input_tokens"].as_i64().unwrap())
        .sum();
    assert_eq!(
        listed + v["other_workspaces"]["input_tokens"].as_i64().unwrap(),
        v["totals"]["input_tokens"].as_i64().unwrap()
    );
}

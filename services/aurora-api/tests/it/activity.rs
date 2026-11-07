#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use uuid::Uuid;

async fn log(
    h: &Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    host: &str,
    decision: &str,
    age_s: i64,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, credential_kind, lane, method, host, path,
            app_id, decision, status, latency_ms)
         VALUES ($1, now() - make_interval(secs => $7), $2,$3,$4,'agent','mitm','GET',$5,'/p',
            CASE WHEN $5 = 'a.com' THEN 'app1' END, $6, 200, 5)",
    )
    .bind(id).bind(org).bind(ws).bind(agent).bind(host).bind(decision).bind(age_s as f64)
    .execute(h.pool()).await.unwrap();
    id
}

#[tokio::test]
async fn activity_filters_and_pagination() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let member = h.user("m@x.io", false).await;
    let outsider = h.user("o@x.io", false).await;
    h.member(org, member, "member", "active").await;
    h.bind_ws(ws, member, "member").await;
    let (_, mt) = h.good_session(member).await;
    let (_, ot) = h.good_session(outsider).await;
    let a1 = Uuid::now_v7();
    let a2 = Uuid::now_v7();
    let newest = log(&h, org, ws, a1, "a.com", "allow", 1).await;
    log(&h, org, ws, a1, "b.com", "blocked", 2).await;
    log(&h, org, ws, a2, "a.com", "allow", 3).await;
    log(&h, org, ws2, a1, "a.com", "allow", 4).await;
    let c = cookie(&mt);
    let hd = [("cookie", c.as_str())];
    let base = format!("/v1/workspaces/{ws}/activity");

    let (s, v) = h.get(&base, &hd).await;
    assert_eq!(s, 200);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["id"], serde_json::json!(newest));
    assert_eq!(items[0]["app_id"], "app1");
    assert!(items[1]["app_id"].is_null());
    for (q, n) in [
        (format!("agent_id={a2}"), 1),
        ("decision=blocked".to_owned(), 1),
        ("host=a.com".to_owned(), 2),
        ("app_id=app1".to_owned(), 2),
        (format!("conversation_id={}", Uuid::now_v7()), 0),
    ] {
        let (_, v) = h.get(&format!("{base}?{q}"), &hd).await;
        assert_eq!(v["items"].as_array().unwrap().len(), n, "{q}");
    }
    let (_, p1) = h.get(&format!("{base}?limit=2"), &hd).await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = h.get(&format!("{base}?limit=2&cursor={cur}"), &hd).await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert!(p2["next_cursor"].is_null());

    let oc = cookie(&ot);
    let (s, _) = h.get(&base, &[("cookie", oc.as_str())]).await;
    assert_eq!(s, 404);
    let (s, _) = h.get(&format!("{base}?limit=0"), &hd).await;
    assert_eq!(s, 400);
}

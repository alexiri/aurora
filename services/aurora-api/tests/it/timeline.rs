//! P5-REFINE-9 (D-415; C-1083, C-1084, SEC-143): one live timeline per agent. Every kind from a
//! fixture, folding, who sees what, paging stability, filters, BYO agents, and the stream.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_db::bus::{self, Channel};
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct Fx {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    gw: Uuid,
    /// Bob: a member with a direct chat.
    bob: Uuid,
    bob_c: String,
    /// Alice: another plain member.
    alice: Uuid,
    alice_c: String,
    /// The workspace's owner.
    owner_c: String,
    /// Bob's direct conversation and a group conversation.
    direct: Uuid,
    group: Uuid,
}

async fn fx() -> Fx {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = h.hosted_agent(org, ws, "dani").await;
    sqlx::query("UPDATE agents SET name = 'Dani', harness = 'claude-code' WHERE id = $1")
        .bind(agent)
        .execute(h.pool())
        .await
        .unwrap();
    let gw = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','0',now(),now())")
        .bind(gw).execute(h.pool()).await.unwrap();
    let mut people = Vec::new();
    for (email, role) in [
        ("bob@x.io", "member"),
        ("alice@x.io", "member"),
        ("owner@x.io", "owner"),
    ] {
        let u = h.user(email, false).await;
        h.member(org, u, "member", "active").await;
        h.bind_ws(ws, u, role).await;
        let (_, t) = h.good_session(u).await;
        people.push((u, cookie(&t)));
    }
    sqlx::query("UPDATE users SET display_name = 'Bob Person' WHERE id = $1")
        .bind(people[0].0)
        .execute(h.pool())
        .await
        .unwrap();
    let direct = conv(&h, agent, ws, "direct", Some(people[0].0)).await;
    let group = conv(&h, agent, ws, "group", None).await;
    let [(bob, bob_c), (alice, alice_c), (_, owner_c)] =
        <[(Uuid, String); 3]>::try_from(people).unwrap();
    Fx {
        h,
        org,
        ws,
        agent,
        gw,
        bob,
        bob_c,
        alice,
        alice_c,
        owner_c,
        direct,
        group,
    }
}

async fn conv(h: &Harness, agent: Uuid, ws: Uuid, kind: &str, user: Option<Uuid>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at)
         VALUES ($1,$2,$3,$4,$5,$6, now())",
    )
    .bind(id).bind(agent).bind(ws).bind(kind).bind(format!("{kind}:{id}")).bind(user)
    .execute(h.pool()).await.unwrap();
    id
}

/// A finished (or running) turn `age_s` seconds ago.
async fn turn(
    h: &Harness,
    f: &Fx,
    conv: Uuid,
    status: &str,
    speaker: Option<Uuid>,
    age_s: f64,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_user_id,
            speaker_label, text, created_at, started_at, finished_at)
         VALUES ($1,$2,$3,'turn',$4,'{}',$5,$6,'bob@x.io','hello secret message text',
            now() - make_interval(secs => $7), now() - make_interval(secs => $7),
            CASE WHEN $4 IN ('done','failed','aborted') THEN now() - make_interval(secs => $7) + interval '2 seconds' END)",
    )
    .bind(id).bind(conv).bind(f.agent).bind(status)
    .bind(if speaker.is_some() { "person" } else { "system" }).bind(speaker).bind(age_s)
    .execute(h.pool()).await.unwrap();
    id
}

async fn event(h: &Harness, conv: Uuid, turn: Uuid, ty: &str, payload: Value) -> i64 {
    let mut tx = h.pool().begin().await.unwrap();
    let seq =
        aurora_db::repo::transcript::append_durable_event(&mut tx, conv, Some(turn), ty, &payload)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    seq
}

async fn tool(
    h: &Harness,
    conv: Uuid,
    turn: Uuid,
    call: &str,
    name: &str,
    summary: &str,
    ok: Option<bool>,
) {
    event(
        h,
        conv,
        turn,
        "tool.started",
        json!({"call_id": call, "name": name, "summary": summary}),
    )
    .await;
    if let Some(ok) = ok {
        event(
            h,
            conv,
            turn,
            "tool.finished",
            json!({"call_id": call, "ok": ok, "summary": "OUTPUT-SECRET"}),
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn log(
    h: &Harness,
    f: &Fx,
    agent: Uuid,
    turn: Option<(Uuid, Uuid)>,
    host: &str,
    app: Option<&str>,
    decision: &str,
    rule: Option<(Uuid, &str)>,
    model: Option<&str>,
    age_s: f64,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, conversation_id, turn_id, credential_kind,
            lane, method, host, path, app_id, decision, rule_logical_id, rule_name, status, latency_ms, llm_model)
         VALUES ($1, now() - make_interval(secs => $2), $3,$4,$5,$6,$7,'agent','mitm','POST',$8,
            '/repos/x/secret-path',$9,$10,$11,$12,200,5,$13)",
    )
    .bind(id).bind(age_s).bind(f.org).bind(f.ws).bind(agent)
    .bind(turn.map(|t| t.0)).bind(turn.map(|t| t.1)).bind(host).bind(app).bind(decision)
    .bind(rule.map(|r| r.0)).bind(rule.map(|r| r.1)).bind(model)
    .execute(h.pool()).await.unwrap();
    id
}

async fn approval(
    h: &Harness,
    f: &Fx,
    conv: Option<Uuid>,
    turn: Option<Uuid>,
    host: &str,
    age_s: f64,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, turn_id, gateway_instance_id,
            method, host, path, summary, status, deadline, created_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,'PUT',$8,'/x','{}','pending', now() + interval '5 minutes',
            now() - make_interval(secs => $9))",
    )
    .bind(id).bind(f.org).bind(f.ws).bind(f.agent).bind(conv).bind(turn).bind(f.gw).bind(host).bind(age_s)
    .execute(h.pool()).await.unwrap();
    id
}

async fn schedule(h: &Harness, agent: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone) VALUES ($1,$2,$3,'p','0 * * * *','UTC')")
        .bind(id).bind(agent).bind(name).execute(h.pool()).await.unwrap();
    id
}

async fn run(h: &Harness, sched: Uuid, turn: Option<Uuid>, outcome: &str, age_s: f64) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO schedule_runs (id, schedule_id, turn_id, outcome, fired_at) VALUES ($1,$2,$3,$4, now() - make_interval(secs => $5))")
        .bind(id).bind(sched).bind(turn).bind(outcome).bind(age_s).execute(h.pool()).await.unwrap();
    id
}

async fn webhook(h: &Harness, agent: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO webhooks (id, agent_id, name, instructions, token_hash, display_prefix)
         VALUES ($1,$2,'Deploys','i', $3, 'whk_')",
    )
    .bind(id)
    .bind(agent)
    .bind(id.as_bytes().repeat(2))
    .execute(h.pool())
    .await
    .unwrap();
    id
}

async fn get(f: &Fx, cookie: &str, q: &str) -> (u16, Value) {
    let (s, v) =
        f.h.get(
            &format!("/v1/agents/{}/timeline{q}", f.agent),
            &[("cookie", cookie)],
        )
        .await;
    (s.as_u16(), v)
}

fn items(v: &Value) -> Vec<Value> {
    v["items"].as_array().unwrap().clone()
}

fn by_id<'a>(items: &'a [Value], id: &str) -> Option<&'a Value> {
    items.iter().find(|i| i["id"] == id)
}

fn ids(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

/// No row carries an e-mail, message text, tool output, request path or secret.
fn assert_clean(v: &Value) {
    let t = v.to_string();
    for bad in [
        "@x.io",
        "hello secret message text",
        "OUTPUT-SECRET",
        "secret-path",
        "sk-live",
        "ghp_",
    ] {
        assert!(!t.contains(bad), "{bad} leaked: {t}");
    }
}

/// Acceptance 1 and 2, and every kind from a fixture: newest first, structured names, no
/// sentence; the three rows of UXR5 workspace-2 can be written from the fields.
#[tokio::test]
async fn every_kind_newest_first_with_structured_names() {
    let f = fx().await;
    let h = &f.h;
    // An answered turn in the group conversation: one model call, one GitHub call, Bash ls and a
    // write of notes.txt; a Read and an unknown tool give no row.
    let t1 = turn(h, &f, f.group, "done", Some(f.bob), 100.0).await;
    log(
        h,
        &f,
        f.agent,
        Some((f.group, t1)),
        "llm.example",
        None,
        "allow",
        None,
        Some("claude-sonnet-x"),
        99.0,
    )
    .await;
    log(
        h,
        &f,
        f.agent,
        Some((f.group, t1)),
        "api.github.com",
        Some("github"),
        "allow",
        None,
        None,
        98.0,
    )
    .await;
    tool(h, f.group, t1, "c1", "Bash", "Bash: ls", Some(true)).await;
    tool(
        h,
        f.group,
        t1,
        "c2",
        "Write",
        "Write: notes.txt",
        Some(true),
    )
    .await;
    tool(
        h,
        f.group,
        t1,
        "c3",
        "Read",
        "Read: /workspace/x",
        Some(true),
    )
    .await;
    tool(
        h,
        f.group,
        t1,
        "c4",
        "FancyTool",
        "FancyTool: rm -rf /",
        Some(true),
    )
    .await;
    // A blocked request decided by a rule.
    let t2 = turn(h, &f, f.group, "done", Some(f.bob), 80.0).await;
    let rule = Uuid::now_v7();
    log(
        h,
        &f,
        f.agent,
        Some((f.group, t2)),
        "evil.example.org",
        None,
        "blocked",
        Some((rule, "Block evil.example.org")),
        None,
        79.0,
    )
    .await;
    // A held approval, then allowed.
    let held = approval(h, &f, Some(f.group), Some(t2), "docs.example.org", 70.0).await;
    let waiting = approval(h, &f, Some(f.group), None, "docs.example.org", 60.0).await;
    sqlx::query("UPDATE approvals SET status = 'approved', decided_at = now() WHERE id = $1")
        .bind(held)
        .execute(h.pool())
        .await
        .unwrap();
    // A schedule run with its own thread and turn.
    let sched = schedule(h, f.agent, "Morning summary").await;
    let thread = conv(h, f.agent, f.ws, "schedule", None).await;
    sqlx::query(
        "UPDATE conversations SET automation_kind = 'schedule', automation_id = $2 WHERE id = $1",
    )
    .bind(thread)
    .bind(sched)
    .execute(h.pool())
    .await
    .unwrap();
    let t3 = turn(h, &f, thread, "done", None, 50.0).await;
    let r = run(h, sched, Some(t3), "fired", 50.0).await;
    // A webhook delivery refused for its signature, a peer task, an access request.
    let wh = webhook(h, f.agent).await;
    let d = Uuid::now_v7();
    sqlx::query("INSERT INTO webhook_deliveries (id, webhook_id, status, size_bytes, reason, created_at) VALUES ($1,$2,'rejected',31,'rejected_signature', now() - interval '40 seconds')")
        .bind(d).bind(wh).execute(h.pool()).await.unwrap();
    let other = h.hosted_agent(f.org, f.ws, "ops").await;
    let (a, b) = if f.agent < other {
        (f.agent, other)
    } else {
        (other, f.agent)
    };
    let link = Uuid::now_v7();
    sqlx::query("INSERT INTO agent_links (id, agent_a_id, agent_b_id, policy_a, policy_b) VALUES ($1,$2,$3,'allow','allow')")
        .bind(link).bind(a).bind(b).execute(h.pool()).await.unwrap();
    let task_conv = conv(h, other, f.ws, "peer", None).await;
    let task = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO peer_tasks (id, link_id, from_agent_id, to_agent_id, origin_conversation_id, task_conversation_id,
            status, message_budget, expires_at, created_at, report)
         VALUES ($1,$2,$3,$4,$5,$6,'completed',5, now() + interval '1 hour', now() - interval '30 seconds', 'REPORT-TEXT')",
    )
    .bind(task).bind(link).bind(f.agent).bind(other).bind(f.group).bind(task_conv)
    .execute(h.pool()).await.unwrap();
    let access = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agent_access_requests (id, org_id, workspace_id, agent_id, conversation_id, kind, app_id, reason, created_at)
         VALUES ($1,$2,$3,$4,$5,'app','github','REASON-TEXT', now() - interval '20 seconds')",
    )
    .bind(access).bind(f.org).bind(f.ws).bind(f.agent).bind(f.group)
    .execute(h.pool()).await.unwrap();

    let (s, v) = get(&f, &f.owner_c, "").await;
    assert_eq!(s, 200, "{v}");
    let it = items(&v);
    assert_clean(&v);
    assert!(!v.to_string().contains("REPORT-TEXT") && !v.to_string().contains("REASON-TEXT"));
    // Newest first.
    let ats: Vec<&str> = it.iter().map(|i| i["at"].as_str().unwrap()).collect();
    let mut sorted = ats.clone();
    sorted.sort_by(|x, y| y.cmp(x));
    assert_eq!(ats, sorted);

    // "Dani answered you · Used <model> · 1 call": answered with the model and the tool count.
    let ans = by_id(&it, &format!("turn:{t1}")).unwrap();
    assert_eq!(ans["kind"], "answered");
    assert_eq!(ans["outcome"], "answered");
    assert_eq!(ans["state"], "done");
    assert_eq!(ans["model"], "claude-sonnet-x");
    assert_eq!(ans["count"], 4, "every tool call of the turn");
    assert_eq!(ans["agent"]["name"], "Dani");
    assert_eq!(ans["person"]["display_name"], "Bob Person");
    assert_eq!(ans["link"]["type"], "conversation");
    assert!(ans.get("sentence").is_none());
    // The computer row: `ls` and notes.txt, linked to the file; Read and FancyTool give nothing.
    let comp: Vec<&Value> = it.iter().filter(|i| i["kind"] == "computer").collect();
    assert_eq!(comp.len(), 1, "{comp:?}");
    assert_eq!(comp[0]["command"], "ls");
    assert_eq!(comp[0]["files"], json!(["notes.txt"]));
    assert_eq!(comp[0]["count"], 1);
    assert_eq!(comp[0]["outcome"], "ok");
    assert_eq!(
        comp[0]["link"],
        json!({"type": "file", "id": f.agent.to_string(), "path": "/workspace/notes.txt"})
    );
    // The model call is never a row; the GitHub call is.
    assert!(it.iter().all(|i| i["host"] != "llm.example"));
    let gh = by_id(&it, &format!("call:{t1}:github:allowed")).unwrap();
    assert_eq!(
        (gh["app"]["id"].as_str(), gh["count"].as_i64()),
        (Some("github"), Some(1))
    );
    // "Dani called … · Blocked by your rule 'Block evil.example.org'".
    let blocked = by_id(&it, &format!("call:{t2}:evil.example.org:blocked_by_rule")).unwrap();
    assert_eq!(blocked["state"], "blocked");
    assert_eq!(
        blocked["rule"],
        json!({"logical_id": rule, "name": "Block evil.example.org"})
    );
    assert_eq!(blocked["host"], "evil.example.org");
    // "Dani asked before changing docs.example.org · Waiting for you in Approvals".
    let w = by_id(&it, &format!("approval:{waiting}")).unwrap();
    assert_eq!(
        (w["kind"].as_str(), w["state"].as_str(), w["host"].as_str()),
        (
            Some("asked_before"),
            Some("waiting"),
            Some("docs.example.org")
        )
    );
    assert!(w["outcome"].is_null());
    let hd = by_id(&it, &format!("approval:{held}")).unwrap();
    assert_eq!(
        (hd["state"].as_str(), hd["outcome"].as_str()),
        (Some("done"), Some("allowed_once"))
    );
    // The run: no separate answered row for its turn; the run carries its outcome and link.
    assert!(by_id(&it, &format!("turn:{t3}")).is_none());
    let rr = by_id(&it, &format!("run:{r}")).unwrap();
    assert_eq!(
        (rr["kind"].as_str(), rr["outcome"].as_str()),
        (Some("ran_schedule"), Some("answered"))
    );
    assert_eq!(rr["schedule"]["name"], "Morning summary");
    assert_eq!(rr["link"]["type"], "conversation");
    assert_eq!(rr["link"]["id"], thread.to_string());
    let dd = by_id(&it, &format!("delivery:{d}")).unwrap();
    assert_eq!(
        (dd["outcome"].as_str(), dd["state"].as_str()),
        (Some("refused_signature"), Some("blocked"))
    );
    assert_eq!(dd["webhook"]["name"], "Deploys");
    let pt = by_id(&it, &format!("peer:{task}")).unwrap();
    assert_eq!(
        (
            pt["kind"].as_str(),
            pt["outcome"].as_str(),
            pt["peer"]["name"].as_str()
        ),
        (Some("handed_task"), Some("completed"), Some("ops"))
    );
    let ac = by_id(&it, &format!("access:{access}")).unwrap();
    assert_eq!(
        (
            ac["kind"].as_str(),
            ac["state"].as_str(),
            ac["app"]["id"].as_str()
        ),
        (Some("asked_for_access"), Some("waiting"), Some("github"))
    );

    // The other agent sees the same task as `got_task`.
    let (s, v) =
        f.h.get(
            &format!("/v1/agents/{other}/timeline"),
            &[("cookie", f.owner_c.as_str())],
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        by_id(&items(&v), &format!("peer:{task}")).unwrap()["kind"],
        "got_task"
    );
}

/// Folding: five calls of one app in one turn are one `called` row of five; model calls never.
#[tokio::test]
async fn one_app_in_one_turn_folds_into_one_row() {
    let f = fx().await;
    let h = &f.h;
    let t = turn(h, &f, f.group, "done", Some(f.bob), 30.0).await;
    for i in 0..5 {
        log(
            h,
            &f,
            f.agent,
            Some((f.group, t)),
            "api.github.com",
            Some("github"),
            "allow",
            None,
            None,
            29.0 - f64::from(i),
        )
        .await;
    }
    for _ in 0..3 {
        log(
            h,
            &f,
            f.agent,
            Some((f.group, t)),
            "llm.example",
            None,
            "allow",
            None,
            Some("m-1"),
            20.0,
        )
        .await;
    }
    log(
        h,
        &f,
        f.agent,
        Some((f.group, t)),
        "llm.example",
        None,
        "allow",
        None,
        Some("m-2"),
        20.0,
    )
    .await;
    let (_, v) = get(&f, &f.owner_c, "?kind=called").await;
    let it = items(&v);
    assert_eq!(it.len(), 1, "{v}");
    assert_eq!(it[0]["count"], 5);
    assert!(it[0]["ended_at"].is_string());
    let (_, v) = get(&f, &f.owner_c, "?kind=answered").await;
    assert_eq!(items(&v)[0]["model"], "m-1", "the most used model");
}

/// SEC-143 / acceptance 3: Alice never receives rows, commands, hosts or counts from Bob's direct
/// chat; Bob and the owner do; schedule rows reach every member.
#[tokio::test]
async fn a_direct_chat_is_seen_only_by_its_person_and_owners() {
    let f = fx().await;
    let h = &f.h;
    let t = turn(h, &f, f.direct, "done", Some(f.bob), 30.0).await;
    tool(
        h,
        f.direct,
        t,
        "c1",
        "Bash",
        "Bash: cat bob-private.txt",
        Some(true),
    )
    .await;
    log(
        h,
        &f,
        f.agent,
        Some((f.direct, t)),
        "bob-private.example",
        None,
        "allow",
        None,
        None,
        29.0,
    )
    .await;
    log(
        h,
        &f,
        f.agent,
        Some((f.direct, t)),
        "llm.example",
        None,
        "allow",
        None,
        Some("m"),
        29.0,
    )
    .await;
    approval(h, &f, Some(f.direct), Some(t), "bob-approval.example", 28.0).await;
    let sched = schedule(h, f.agent, "Nightly").await;
    let r = run(h, sched, None, "gated_quiet", 10.0).await;

    let (s, v) = get(&f, &f.alice_c, "").await;
    assert_eq!(s, 200);
    let txt = v.to_string();
    for bad in [
        "bob-private",
        "bob-approval",
        &t.to_string(),
        "\"computer\"",
        "\"called\"",
    ] {
        assert!(!txt.contains(bad), "{bad} reached Alice: {txt}");
    }
    let it = items(&v);
    assert_eq!(ids(&it), vec![format!("run:{r}")], "only the schedule run");
    assert_eq!(it[0]["outcome"], "skipped_by_gate");

    for c in [&f.bob_c, &f.owner_c] {
        let (_, v) = get(&f, c, "").await;
        let it = items(&v);
        assert!(by_id(&it, &format!("turn:{t}")).is_some());
        let comp = it.iter().find(|i| i["kind"] == "computer").unwrap();
        assert_eq!(comp["command"], "cat bob-private.txt");
        assert!(it.iter().any(|i| i["host"] == "bob-private.example"));
        assert!(it.iter().any(|i| i["host"] == "bob-approval.example"));
        assert_clean(&v);
    }

    // A caller without `can_use` gets 404; an unknown agent too.
    let stranger = h.user("s@y.io", false).await;
    let (_, st) = h.good_session(stranger).await;
    assert_eq!(get(&f, &cookie(&st), "").await.0, 404);
    let (s, _) = h
        .get(
            &format!("/v1/agents/{}/timeline", Uuid::now_v7()),
            &[("cookie", f.alice_c.as_str())],
        )
        .await;
    assert_eq!(s, 404);
    let _ = (f.alice, f.org);
}

/// Acceptance 5: three pages of 50 over 150 mixed rows give every row exactly once, with a row
/// inserted between pages.
#[tokio::test]
async fn paging_is_stable_under_concurrent_inserts() {
    let f = fx().await;
    let h = &f.h;
    let mut want: HashSet<String> = HashSet::new();
    for i in 0..75 {
        let id = log(
            h,
            &f,
            f.agent,
            None,
            "byo.example",
            None,
            "allow",
            None,
            None,
            1000.0 - f64::from(i) * 2.0,
        )
        .await;
        want.insert(format!("call:{id}"));
        let a = approval(h, &f, None, None, "ask.example", 999.0 - f64::from(i) * 2.0).await;
        want.insert(format!("approval:{a}"));
    }
    let mut got: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0..10 {
        let q = match &cursor {
            Some(c) => format!("?limit=50&cursor={c}"),
            None => "?limit=50".to_owned(),
        };
        let (s, v) = get(&f, &f.owner_c, &q).await;
        assert_eq!(s, 200, "{v}");
        got.extend(ids(&items(&v)));
        if page == 0 {
            // Newer than every row read so far: never in a later page.
            log(
                h,
                &f,
                f.agent,
                None,
                "late.example",
                None,
                "allow",
                None,
                None,
                0.0,
            )
            .await;
        }
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    let set: HashSet<String> = got.iter().cloned().collect();
    assert_eq!(set.len(), got.len(), "no row twice");
    assert_eq!(set, want, "every row once");
}

/// Filters, a BYO agent (request log and approvals only) and refusals.
#[tokio::test]
async fn filters_byo_and_refusals() {
    let f = fx().await;
    let h = &f.h;
    let byo = h.byo_agent(f.org, f.ws, "laptop", "Laptop").await;
    log(h, &f, byo, None, "pypi.org", None, "allow", None, None, 5.0).await;
    log(
        h,
        &f,
        byo,
        None,
        "bad.example",
        None,
        "default_denied",
        None,
        None,
        4.0,
    )
    .await;
    let (s, v) = h
        .get(
            &format!("/v1/agents/{byo}/timeline"),
            &[("cookie", f.alice_c.as_str())],
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(items(&v).len(), 2);
    let (_, v) = h
        .get(
            &format!("/v1/agents/{byo}/timeline?outcome=blocked_default"),
            &[("cookie", f.alice_c.as_str())],
        )
        .await;
    let it = items(&v);
    assert_eq!(it.len(), 1);
    assert_eq!(it[0]["host"], "bad.example");
    for bad in ["?limit=0", "?limit=101", "?kind=nope", "?cursor=%%%"] {
        assert_eq!(get(&f, &f.alice_c, bad).await.0, 400, "{bad}");
    }
    let (_, v) = get(&f, &f.alice_c, "?kind=answered,computer").await;
    assert!(items(&v).is_empty());
}

// ------------------------------------------------------------------------------------- stream

async fn open(app: &axum::Router, uri: &str, cookie: &str) -> (u16, Body) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    (res.status().as_u16(), Body::new(res.into_body()))
}

/// Read SSE text until `pred` holds or `wait` passes; returns what was read.
async fn read_until(body: &mut Body, wait: Duration, pred: impl Fn(&str) -> bool) -> String {
    let deadline = tokio::time::Instant::now() + wait;
    let mut seen = String::new();
    while !pred(&seen) {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Ok(Some(Ok(fr))) => {
                if let Ok(d) = fr.into_data() {
                    seen.push_str(&String::from_utf8_lossy(&d));
                }
            }
            _ => break,
        }
    }
    seen
}

/// The `row` events in `text`.
fn rows(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter(|b| b.contains("event: row"))
        .filter_map(|b| b.lines().find_map(|l| l.strip_prefix("data: ")))
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .map(|v| v["item"].clone())
        .collect()
}

fn has_row(text: &str, id: &str, state: &str) -> bool {
    rows(text)
        .iter()
        .any(|r| r["id"] == id && r["state"] == state)
}

/// Acceptance 4: a gateway request, a turn's tool step, an approval and its decision (the same
/// id, in place) and a schedule run each arrive within 2 s; `technical` adds `request` events.
#[tokio::test]
async fn the_stream_pushes_each_source_and_updates_in_place() {
    let f = fx().await;
    let h = &f.h;
    let app = test_router(h.state.clone());
    let uri = format!("/v1/agents/{}/timeline/stream", f.agent);
    let (s, mut body) = open(&app, &uri, &f.owner_c).await;
    assert_eq!(s, 200);
    let (s, mut tech) = open(&app, &format!("{uri}?technical=true"), &f.owner_c).await;
    assert_eq!(s, 200);
    let two = Duration::from_secs(2);

    // A held approval (the gateway's NOTIFY) and its decision: same id, waiting → done.
    let a = approval(h, &f, Some(f.group), None, "docs.example.org", 0.0).await;
    bus::notify(
        h.pool(),
        Channel::Approval,
        &json!({"id": a, "workspace_id": f.ws, "event": "created"}),
    )
    .await
    .unwrap();
    let id = format!("approval:{a}");
    let t = read_until(&mut body, two, |s| has_row(s, &id, "waiting")).await;
    assert!(has_row(&t, &id, "waiting"), "{t}");
    sqlx::query("UPDATE approvals SET status = 'denied', decided_at = now() WHERE id = $1")
        .bind(a)
        .execute(h.pool())
        .await
        .unwrap();
    bus::notify(
        h.pool(),
        Channel::Approval,
        &json!({"id": a, "workspace_id": f.ws, "event": "decided"}),
    )
    .await
    .unwrap();
    let t = read_until(&mut body, two, |s| has_row(s, &id, "done")).await;
    let r = rows(&t)
        .into_iter()
        .find(|r| r["id"] == id.as_str())
        .unwrap();
    assert_eq!(r["outcome"], "blocked");

    // A turn's tool step (the conversation channel).
    let turn_id = turn(h, &f, f.group, "running", Some(f.bob), 0.0).await;
    tool(h, f.group, turn_id, "c1", "Bash", "Bash: make test", None).await;
    let t = read_until(&mut body, two, |s| {
        rows(s).iter().any(|r| r["kind"] == "computer")
    })
    .await;
    let comp = rows(&t)
        .into_iter()
        .find(|r| r["kind"] == "computer")
        .unwrap();
    assert_eq!(
        (comp["command"].as_str(), comp["state"].as_str()),
        (Some("make test"), Some("running"))
    );

    // A gateway request batch (the gateway's hint after its flush).
    let rid = log(
        h, &f, f.agent, None, "pypi.org", None, "allow", None, None, 0.0,
    )
    .await;
    let row = aurora_db::repo::gateway::RequestLogRow {
        id: rid,
        ts: chrono::Utc::now() - chrono::Duration::seconds(1),
        org_id: f.org,
        workspace_id: f.ws,
        agent_id: f.agent,
        conversation_id: None,
        turn_id: None,
        credential_kind: "agent",
        lane: "mitm",
        method: "GET".into(),
        host: "pypi.org".into(),
        path: "/".into(),
        app_id: None,
        tool_id: None,
        connection_id: None,
        secret_id: None,
        decision: "allow",
        rule_logical_id: None,
        rule_name: None,
        upstream_status: None,
        status: 200,
        error_code: None,
        injected: 0,
        latency_ms: 1,
        bytes_up: 0,
        bytes_down: 0,
        context_label: None,
        speaker_user_id: None,
        approval_id: None,
        resumed: false,
        llm_model: None,
    };
    aurora_db::repo::timeline::notify_requests(h.pool(), std::slice::from_ref(&row)).await;
    let cid = format!("call:{rid}");
    let t = read_until(&mut body, two, |s| has_row(s, &cid, "done")).await;
    assert!(has_row(&t, &cid, "done"), "{t}");
    assert!(
        !t.contains("event: request"),
        "no request events without technical"
    );
    let tt = read_until(&mut tech, two, |s| s.contains("event: request")).await;
    assert!(
        tt.contains("event: request") && tt.contains(&rid.to_string()),
        "{tt}"
    );

    // A schedule run (its repo writer sends the hint).
    let sched = schedule(h, f.agent, "Morning summary").await;
    let mut tx = h.pool().begin().await.unwrap();
    let run = aurora_db::repo::schedules::insert_run(&mut tx, sched, None, "gated_quiet")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let rid = format!("run:{}", run.id);
    let t = read_until(&mut body, two, |s| has_row(s, &rid, "skipped")).await;
    assert!(has_row(&t, &rid, "skipped"), "{t}");
    assert!(!t.contains("\nid:"), "no SSE ids");
}

/// SEC-143 on the stream: Alice's stream gets nothing from Bob's direct chat.
#[tokio::test]
async fn the_stream_omits_rows_the_reader_may_not_see() {
    let f = fx().await;
    let h = &f.h;
    let app = test_router(h.state.clone());
    let uri = format!("/v1/agents/{}/timeline/stream", f.agent);
    let (_, mut alice) = open(&app, &uri, &f.alice_c).await;
    let (_, mut bob) = open(&app, &uri, &f.bob_c).await;
    let t = turn(h, &f, f.direct, "running", Some(f.bob), 0.0).await;
    tool(
        h,
        f.direct,
        t,
        "c1",
        "Bash",
        "Bash: cat bob-private.txt",
        None,
    )
    .await;
    let b = read_until(&mut bob, Duration::from_secs(2), |s| {
        s.contains("bob-private")
    })
    .await;
    assert!(b.contains("bob-private"), "{b}");
    // Then something Alice may see, to prove her stream is live.
    let sched = schedule(h, f.agent, "Nightly").await;
    let mut tx = h.pool().begin().await.unwrap();
    let run = aurora_db::repo::schedules::insert_run(&mut tx, sched, None, "gated_quiet")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let id = format!("run:{}", run.id);
    let a = read_until(&mut alice, Duration::from_secs(3), |s| s.contains(&id)).await;
    assert!(a.contains(&id), "{a}");
    assert!(
        !a.contains("bob-private") && !a.contains(&t.to_string()),
        "{a}"
    );
}

/// Revoking the reader's access ends the stream; the sixth stream of a person is 429; an API key
/// is refused (session only).
#[tokio::test]
async fn the_stream_ends_on_lost_access_and_is_limited() {
    let f = fx().await;
    let h = &f.h;
    let app = test_router(
        h.state
            .clone()
            .with_stream_recheck(Duration::from_millis(200)),
    );
    let uri = format!("/v1/agents/{}/timeline/stream", f.agent);
    let (s, mut body) = open(&app, &uri, &f.alice_c).await;
    assert_eq!(s, 200);
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
        .bind(f.ws)
        .bind(f.alice)
        .execute(h.pool())
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let ended = loop {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Err(_) => break false,
            Ok(None) | Ok(Some(Err(_))) => break true,
            Ok(Some(Ok(_))) => {}
        }
    };
    assert!(ended, "the stream ended when access went");

    let mut open_ones = Vec::new();
    for _ in 0..aurora_core::limits::TIMELINE_STREAMS_PER_USER {
        let (s, b) = open(&app, &uri, &f.bob_c).await;
        assert_eq!(s, 200);
        open_ones.push(b);
    }
    let (s, _) = open(&app, &uri, &f.bob_c).await;
    assert_eq!(s, 429);
    drop(open_ones);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (s, _) = open(&app, &uri, &f.bob_c).await;
    assert_eq!(s, 200, "a closed stream frees its place");

    let (_, key) = h.api_key(f.bob, f.org, Some(f.ws), false).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header("authorization", bearer(&key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

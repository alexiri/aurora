#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use async_trait::async_trait;
use aurora_api::domains::tools::{
    PlatformTool, Registry, ToolCallContext, ToolContext, ToolError, catalogue,
};
use aurora_protocol::tools::ToolContent;
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    tok: String,
    replica: Uuid,
    runner_id: Uuid,
}

async fn rig() -> Rig {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| c.gateway_ca_dir = d).await;
    let admin = h.user("admin@x.io", true).await;
    let (_, sess) = h.good_session(admin).await;
    let c = cookie(&sess);
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/runners",
            &[("cookie", &c), ("origin", ORIGIN)],
            Some(json!({"name": "r1"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let runner_id = v["runner"]["id"].as_str().unwrap().parse().unwrap();
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let mut r = Rig {
        h,
        org,
        ws,
        tok,
        replica: Uuid::nil(),
        runner_id,
    };
    let caps = json!({
        "backend": "fake", "home_durability": "resident",
        "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                      "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
        "max_sandboxes": 4, "harnesses": ["fake"], "max_attachment_bytes": 1048576
    });
    let (s, v) = r
        .post(
            None,
            "/v1/runner/register",
            json!({"version": "0.1", "backend": "fake", "capabilities": caps,
                   "advertised_host": "h1"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    r.replica = v["replica_id"].as_str().unwrap().parse().unwrap();
    r
}

struct Booted {
    agent: Uuid,
    sandbox: Uuid,
    boot: Uuid,
    conv: Uuid,
    turn: Uuid,
}

impl Rig {
    async fn post(&self, replica: Option<Uuid>, uri: &str, body: Value) -> (u16, Value) {
        let b = bearer(&self.tok);
        let rid = replica.map(|r| r.to_string());
        let mut hd = vec![("authorization", b.as_str())];
        if let Some(r) = &rid {
            hd.push(("x-aurora-replica", r.as_str()));
        }
        let (s, _, v) = self.h.send(Method::POST, uri, &hd, Some(body)).await;
        (s.as_u16(), v)
    }

    async fn tool(&self, req: Value) -> (u16, Value) {
        self.post(Some(self.replica), "/v1/runner/tool-calls", req)
            .await
    }

    /// Running sandbox + conversation + running turn for a fresh agent.
    async fn booted(&self, handle: &str) -> Booted {
        let agent = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
             VALUES ($1,$2,$3,$4,$4,'hosted','fake')",
        )
        .bind(agent)
        .bind(self.org)
        .bind(self.ws)
        .bind(handle)
        .execute(self.h.pool())
        .await
        .unwrap();
        let (sandbox, boot, conv, turn) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::query(
            "INSERT INTO sandboxes (id, agent_id, runner_id, state, boot_id)
             VALUES ($1,$2,$3,'running',$4)",
        )
        .bind(sandbox)
        .bind(agent)
        .bind(self.runner_id)
        .bind(boot)
        .execute(self.h.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
             VALUES ($1,$2,$3,'api',$1::text, now())",
        )
        .bind(conv)
        .bind(agent)
        .bind(self.ws)
        .execute(self.h.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text)
             VALUES ($1,$2,$3,'turn','running','{}','system','s','hi')",
        )
        .bind(turn)
        .bind(conv)
        .bind(agent)
        .execute(self.h.pool())
        .await
        .unwrap();
        Booted {
            agent,
            sandbox,
            boot,
            conv,
            turn,
        }
    }
}

fn call(b: &Booted, tool: &str, args: Value) -> Value {
    json!({"sandbox_id": b.sandbox, "boot_id": b.boot, "call_id": "c1",
           "conversation_id": b.conv, "turn_id": b.turn, "tool": tool, "args": args})
}

fn call_in(b: &Booted, conv: Uuid, turn: Uuid, tool: &str, args: Value) -> Value {
    json!({"sandbox_id": b.sandbox, "boot_id": b.boot, "call_id": "c1",
           "conversation_id": conv, "turn_id": turn, "tool": tool, "args": args})
}

#[tokio::test]
async fn catalogue_lists_available_tools_with_schemas() {
    let r = rig().await;
    let b = r.booted("a1").await;
    let cat = catalogue(&r.h.state, b.agent).await.unwrap();
    let names: Vec<&str> = cat
        .iter()
        .map(|t| t.name.as_str())
        .filter(|n| !n.starts_with("webhook_"))
        .collect();
    assert_eq!(
        names,
        [
            "list_agents",
            "list_apps",
            "memory_save",
            "memory_get",
            "memory_list",
            "memory_search",
            "memory_delete",
            "skill_list",
            "skill_get",
            "skill_create",
            "skill_update",
            "skill_delete",
            "schedule_list",
            "schedule_create",
            "schedule_update",
            "schedule_delete",
            "message_agent",
            "complete_task",
            "list_peer_tasks",
            "cancel_peer_task",
            "propose_custom_connection",
            "request_app_connection",
            "conversation_search",
            "conversation_read"
        ]
    );
    for t in &cat {
        assert_eq!(t.input_schema["type"], "object");
        assert!(t.description.as_deref().is_some_and(|d| !d.is_empty()));
    }
}

/// (P5-GUIDE-7, C-654, SEC-119) Every descriptor the API syncs names its family and exposure; the
/// core set is exactly `complete_task` (the supervisor adds its local `send_file`); the
/// `render_inputs.platform_tools` families cover every on-demand tool. Records the size of
/// `tools/list` as the bridge would answer it, everything vs core only (chars / 4).
#[tokio::test]
async fn catalogue_descriptors_carry_family_and_exposure_and_core_is_small() {
    use aurora_protocol::tools::Exposure;
    let r = rig().await;
    let b = r.booted("a1").await;
    let cat = catalogue(&r.h.state, b.agent).await.unwrap();
    assert!(cat.len() > 20, "{}", cat.len());
    for t in &cat {
        assert!(
            t.family.as_deref().is_some_and(|f| !f.is_empty()),
            "{} has no family",
            t.name
        );
        assert!(t.exposure.is_some(), "{} has no exposure", t.name);
    }
    let core: Vec<&str> = cat
        .iter()
        .filter(|t| t.exposure == Some(Exposure::Core))
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(core, ["complete_task"]);
    let size = |tools: &[&aurora_protocol::tools::ToolDescriptor]| -> usize {
        // The bridge's MCP shape: name, description, inputSchema.
        tools
            .iter()
            .map(|t| {
                json!({"name": t.name, "description": t.description, "inputSchema": t.input_schema})
                    .to_string()
                    .chars()
                    .count()
            })
            .sum()
    };
    let all: Vec<_> = cat.iter().collect();
    let core_only: Vec<_> = cat
        .iter()
        .filter(|t| t.exposure == Some(Exposure::Core))
        .collect();
    let (a, c) = (size(&all), size(&core_only));
    eprintln!(
        "MEASURE tools/list (API tools only, no supervisor-local): all = {} tools, {a} chars (~{} tokens); core-only = {} tools, {c} chars (~{} tokens)",
        all.len(),
        a / 4,
        core_only.len(),
        c / 4
    );
    assert!(c * 5 < a, "core-only is a small fraction: {c} vs {a}");
}

#[tokio::test]
async fn list_agents_and_apps() {
    let r = rig().await;
    let a = r.booted("alpha").await;
    let _b = r.booted("beta").await;
    let (s, v) = r.tool(call(&a, "list_agents", json!({}))).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["call_id"], "c1");
    let agents = v["content"][0]["value"]["agents"].as_array().unwrap();
    let mut handles: Vec<&str> = agents
        .iter()
        .map(|x| x["handle"].as_str().unwrap())
        .collect();
    handles.sort_unstable();
    assert!(handles.contains(&"alpha") && handles.contains(&"beta"));
    let me = agents.iter().find(|x| x["handle"] == "alpha").unwrap();
    assert_eq!(me["is_self"], true);

    // list_apps: only connections granted to the calling agent.
    let conn = Uuid::now_v7();
    let other = Uuid::now_v7();
    for id in [conn, other] {
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x')")
            .bind(id).bind(r.org).bind(r.ws).execute(r.h.pool()).await.unwrap();
    }
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(Uuid::now_v7()).bind(r.ws).bind(a.agent).bind(conn).execute(r.h.pool()).await.unwrap();
    let (s, v) = r.tool(call(&a, "list_apps", json!({}))).await;
    assert_eq!(s, 200, "{v}");
    let apps = v["content"][0]["value"]["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 1, "{v}");
    assert_eq!(apps[0]["connection_id"], conn.to_string());
    assert_eq!(apps[0]["app_id"], "acme_app");

    // Arguments are validated.
    let (s, v) = r.tool(call(&a, "list_apps", json!({"x": 1}))).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "invalid_arguments");
}

#[tokio::test]
async fn unknown_tool_is_structured_error() {
    let r = rig().await;
    let a = r.booted("a1").await;
    let (s, v) = r.tool(call(&a, "no_such_tool", json!({}))).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "unknown_tool");
    assert_eq!(v["call_id"], "c1");
}

#[tokio::test]
async fn identity_comes_from_authenticated_facts() {
    let r = rig().await;
    let a = r.booted("a1").await;
    let b = r.booted("b1").await;

    // Forged turn of another agent (and its conversation) -> error, no tool run.
    let mut forged = call(&a, "list_agents", json!({}));
    forged["turn_id"] = json!(b.turn);
    let (s, v) = r.tool(forged).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "turn_not_active");

    let mut forged = call(&a, "list_agents", json!({}));
    forged["conversation_id"] = json!(b.conv);
    forged["turn_id"] = json!(b.turn);
    let (s, _) = r.tool(forged).await;
    assert_eq!(s, 404);

    // Stale boot is fenced.
    let mut stale = call(&a, "list_agents", json!({}));
    stale["boot_id"] = json!(Uuid::now_v7());
    let (s, v) = r.tool(stale).await;
    assert_eq!(s, 409, "{v}");

    // Finished turn is not active.
    sqlx::query("UPDATE turns SET status = 'done' WHERE id = $1")
        .bind(a.turn)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, _) = r.tool(call(&a, "list_agents", json!({}))).await;
    assert_eq!(s, 403);

    // (FIX-110, SEC-24) The turn id is required: a call without one is refused, as is one
    // naming a finished turn, and neither runs the tool.
    let mut no_turn = call(&a, "list_agents", json!({}));
    no_turn.as_object_mut().unwrap().remove("turn_id");
    let (s, v) = r.tool(no_turn).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "no_active_turn");

    // Unknown sandbox / missing replica / no token.
    let mut unk = call(&a, "list_agents", json!({}));
    unk["sandbox_id"] = json!(Uuid::now_v7());
    assert_eq!(r.tool(unk).await.0, 404);
    let (s, _) = r
        .post(
            None,
            "/v1/runner/tool-calls",
            call(&a, "list_agents", json!({})),
        )
        .await;
    assert_eq!(s, 403);
}

/// (FIX-110, SEC-45) An agent that is itself frozen cannot use its tools: the identity check
/// refuses (423), so neither `message_agent` nor `schedule_create` runs.
#[tokio::test]
async fn a_frozen_agent_cannot_use_platform_tools() {
    let r = rig().await;
    let a = r.booted("frozen").await;
    let _b = r.booted("peer").await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'drill', frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(a.agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    for (tool, args) in [
        ("message_agent", json!({"agent": "@peer", "text": "hi"})),
        (
            "schedule_create",
            json!({"name": "n", "prompt": "p", "cron": "0 9 * * *"}),
        ),
        ("list_agents", json!({})),
    ] {
        let (s, v) = r.tool(call(&a, tool, args)).await;
        assert_eq!(s, 423, "{tool}: {v}");
    }
    sqlx::query("UPDATE agents SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL WHERE id = $1")
        .bind(a.agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r.tool(call(&a, "list_agents", json!({}))).await;
    assert_eq!(s, 200, "{v}");
}

/// (FIX-253, D-490) `schedule_create` from a delegated task's conversation (a peer turn, no
/// speaker): the schedule belongs to the task's requester and its origin is the task
/// conversation, and the wake runs there while the task is not closed. Without a requester, or
/// with a requester who cannot use the agent, it refuses.
#[tokio::test]
async fn schedule_create_from_a_peer_turn_belongs_to_the_requester() {
    let r = rig().await;
    let a = r.booted("a1").await;
    let requester = r.h.user("req@x.io", false).await;
    r.h.member(r.org, requester, "member", "active").await;
    r.h.bind_ws(r.ws, requester, "member").await;

    // A peer task conversation of a1 with an agent-speaker turn, and the open task behind it.
    let peer = |status, requester: Option<Uuid>| {
        let r = &r;
        async move { peer_task_conv(r, a.agent, requester, status).await }
    };

    // The wake: a one-time schedule from the peer turn belongs to the requester, origin the task
    // conversation, and `origin_for_run` sends the fire there while the task is open.
    let (conv, turn) = peer("open", Some(requester)).await;
    let (s, v) = r
        .tool(call_in(
            &a,
            conv,
            turn,
            "schedule_create",
            json!({"name": "check", "prompt": "Look again.", "run_at": "2026-12-01T09:00"}),
        ))
        .await;
    assert_eq!(s, 200, "{v}");
    let row: (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT creator_user_id, origin_conversation_id FROM schedules ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(row, (Some(requester), Some(conv)));
    let schedule: Uuid =
        sqlx::query_scalar("SELECT id FROM schedules ORDER BY created_at DESC LIMIT 1")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    let mut conn = r.h.pool().acquire().await.unwrap();
    let row = aurora_db::repo::schedules::get(&mut *conn, schedule)
        .await
        .unwrap()
        .unwrap();
    let fire_at = aurora_db::repo::schedules::origin_for_run(&mut conn, &row)
        .await
        .unwrap();
    assert_eq!(fire_at, Some(conv));
    drop(conn);
    // A closed task's conversation is nobody's chat any more.
    sqlx::query("UPDATE peer_tasks SET status = 'completed' WHERE task_conversation_id = $1")
        .bind(conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    let mut conn = r.h.pool().acquire().await.unwrap();
    let row = aurora_db::repo::schedules::get(&mut *conn, schedule)
        .await
        .unwrap()
        .unwrap();
    let fire_at = aurora_db::repo::schedules::origin_for_run(&mut conn, &row)
        .await
        .unwrap();
    assert_eq!(fire_at, None);
    drop(conn);

    // A peer turn with no requester still refuses.
    let (conv, turn) = peer("open", None).await;
    let (s, v) = r
        .tool(call_in(
            &a,
            conv,
            turn,
            "schedule_create",
            json!({"name": "n", "prompt": "p", "run_at": "2026-12-01T09:00"}),
        ))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], false, "{v}");

    // A requester who cannot use the agent never gets the schedule.
    let stranger = r.h.user("stranger@x.io", false).await;
    r.h.member(r.org, stranger, "member", "active").await;
    let (conv, turn) = peer("open", Some(stranger)).await;
    sqlx::query("UPDATE peer_tasks SET requester_user_id = $1 WHERE task_conversation_id = $2")
        .bind(stranger)
        .bind(conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r
        .tool(call_in(
            &a,
            conv,
            turn,
            "schedule_create",
            json!({"name": "n", "prompt": "p", "run_at": "2026-12-01T09:00"}),
        ))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert!(
        v.to_string().contains("can no longer use this agent"),
        "{v}"
    );
}

/// (FIX-253 review) Only a peer turn of a task that is still open may schedule for the
/// requester: a closed, cancelled or expired task refuses, and a speakerless watch or schedule
/// turn fired in the task conversation refuses (a wake never schedules itself again).
#[tokio::test]
async fn only_a_peer_turn_of_an_open_task_schedules_for_the_requester() {
    let r = rig().await;
    let a = r.booted("a1").await;
    let requester = r.h.user("req2@x.io", false).await;
    r.h.member(r.org, requester, "member", "active").await;
    r.h.bind_ws(r.ws, requester, "member").await;

    // A closed, cancelled or expired task's conversation is nobody's chat any more.
    for status in ["completed", "cancelled", "expired"] {
        let (conv, turn) = peer_task_conv(&r, a.agent, Some(requester), status).await;
        let (s, v) = r
            .tool(call_in(
                &a,
                conv,
                turn,
                "schedule_create",
                json!({"name": "n", "prompt": "p", "run_at": "2026-12-01T09:00"}),
            ))
            .await;
        assert_eq!(s, 200, "{status}: {v}");
        assert_eq!(v["ok"], false, "{status}: {v}");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM schedules")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
        assert_eq!(n, 0, "{status}");
    }

    // A watch or schedule turn fired in the task conversation has no speaker: it must not
    // schedule itself again.
    let (conv, _) = peer_task_conv(&r, a.agent, Some(requester), "open").await;
    sqlx::query("UPDATE turns SET status = 'done' WHERE conversation_id = $1")
        .bind(conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    let fired = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text)
         VALUES ($1,$2,$3,'turn','running','{}','system','Watch','A watch you set fired.')",
    )
    .bind(fired)
    .bind(conv)
    .bind(a.agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = r
        .tool(call_in(
            &a,
            conv,
            fired,
            "schedule_create",
            json!({"name": "n", "prompt": "p", "run_at": "2026-12-01T09:00"}),
        ))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM schedules")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

/// A `peer` conversation of `agent` with an agent-speaker turn, and an open peer task behind it
/// (requester `None` = the refusal case).
async fn peer_task_conv(
    r: &Rig,
    agent: Uuid,
    requester: Option<Uuid>,
    status: &str,
) -> (Uuid, Uuid) {
    let conv = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
         VALUES ($1,$2,$3,'peer',$1::text, now())",
    )
    .bind(conv)
    .bind(agent)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let turn = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text)
         VALUES ($1,$2,$3,'turn','running','{}','agent','a1','working')",
    )
    .bind(turn)
    .bind(conv)
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let other =
        r.h.hosted_agent(r.org, r.ws, &format!("peer-{}", Uuid::now_v7().simple()))
            .await;
    let (x, y) = if agent < other {
        (agent, other)
    } else {
        (other, agent)
    };
    let link = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agent_links (id, agent_a_id, agent_b_id, policy_a, policy_b)
         VALUES ($1,$2,$3,'allow','allow')",
    )
    .bind(link)
    .bind(x)
    .bind(y)
    .execute(r.h.pool())
    .await
    .unwrap();
    let task = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO peer_tasks (id, link_id, from_agent_id, to_agent_id, requester_user_id,
            origin_conversation_id, task_conversation_id, status, message_budget, expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,5, now() + interval '1 hour')",
    )
    .bind(task)
    .bind(link)
    .bind(other)
    .bind(agent)
    .bind(requester)
    .bind(conv)
    .bind(conv)
    .bind(status)
    .execute(r.h.pool())
    .await
    .unwrap();
    (conv, turn)
}

struct Slow;
struct Echo(bool);

#[async_trait]
impl PlatformTool for Slow {
    fn name(&self) -> &'static str {
        "slow"
    }
    fn family(&self) -> &'static str {
        "test"
    }
    fn description(&self, _: &ToolContext) -> String {
        "slow".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(&self, _: &ToolCallContext, _: Value) -> Result<Vec<ToolContent>, ToolError> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(vec![])
    }
}

#[async_trait]
impl PlatformTool for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn family(&self) -> &'static str {
        "test"
    }
    fn description(&self, _: &ToolContext) -> String {
        "echo".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn available(&self, _: &ToolContext) -> bool {
        self.0
    }
    fn unavailable_reason(&self) -> &'static str {
        "your presence was removed"
    }
    async fn call(&self, _: &ToolCallContext, _: Value) -> Result<Vec<ToolContent>, ToolError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn registry_timeout_unavailable_and_completeness() {
    let r = rig().await;
    let b = r.booted("a1").await;
    let ctx = ToolCallContext {
        agent: ToolContext {
            pool: r.h.pool().clone(),
            agent_id: b.agent,
            workspace_id: r.ws,
            org_id: r.org,
        },
        conversation_id: b.conv,
        turn_id: Some(b.turn),
        call_id: "k".into(),
        state: r.h.state.clone(),
    };
    let reg = Registry::new(vec![Box::new(Slow), Box::new(Echo(false))]).unwrap();
    let res = reg
        .call(&ctx, "slow", json!({}), Duration::from_millis(50))
        .await;
    assert!(!res.ok);
    assert_eq!(res.error.unwrap().code.as_str(), "tool_timeout");
    // Unavailable tools are hidden from the catalogue but answer with an explanation.
    assert_eq!(reg.catalogue(&ctx.agent).len(), 1);
    let res = reg
        .call(&ctx, "echo", json!({}), Duration::from_secs(1))
        .await;
    let e = res.error.unwrap();
    assert_eq!(e.code.as_str(), "tool_unavailable");
    assert_eq!(e.message, "your presence was removed");
    // Load-time completeness.
    assert!(Registry::new(vec![Box::new(Slow), Box::new(Slow)]).is_err());
}

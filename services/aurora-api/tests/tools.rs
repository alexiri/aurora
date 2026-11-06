#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use async_trait::async_trait;
use aurora_api::domains::tools::{
    PlatformTool, Registry, ToolCallContext, ToolContext, ToolError, catalogue,
};
use aurora_protocol::tools::ToolContent;
use common::*;
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

#[tokio::test]
async fn catalogue_lists_available_tools_with_schemas() {
    let r = rig().await;
    let b = r.booted("a1").await;
    let cat = catalogue(r.h.pool(), b.agent).await.unwrap();
    let names: Vec<&str> = cat.iter().map(|t| t.name.as_str()).collect();
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
            "skill_delete"
        ]
    );
    for t in &cat {
        assert_eq!(t.input_schema["type"], "object");
        assert!(t.description.as_deref().is_some_and(|d| !d.is_empty()));
    }
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

    // Turn id is optional.
    let mut no_turn = call(&a, "list_agents", json!({}));
    no_turn.as_object_mut().unwrap().remove("turn_id");
    let (s, v) = r.tool(no_turn).await;
    assert_eq!(s, 200, "{v}");

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

struct Slow;
struct Echo(bool);

#[async_trait]
impl PlatformTool for Slow {
    fn name(&self) -> &'static str {
        "slow"
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

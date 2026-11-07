#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-061: skills API, limits, permissions, generation bump, tools and projection.
mod common;

use common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    tok: String,
    replica: Uuid,
    runner_id: Uuid,
    cookie: String,
}

async fn rig() -> Rig {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| c.gateway_ca_dir = d).await;
    let admin = h.user("admin@x.io", true).await;
    let (_, sess) = h.good_session(admin).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/runners",
            &[("cookie", &cookie(&sess)), ("origin", ORIGIN)],
            Some(json!({"name": "r1"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let runner_id = v["runner"]["id"].as_str().unwrap().parse().unwrap();
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let user = h.user("o@x.io", false).await;
    h.member(org, user, "member", "active").await;
    h.bind_ws(ws, user, "owner").await;
    let (_, st) = h.good_session(user).await;
    let mut r = Rig {
        h,
        org,
        ws,
        tok,
        replica: Uuid::nil(),
        runner_id,
        cookie: cookie(&st),
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

    async fn user(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(
                m,
                uri,
                &[("cookie", &self.cookie), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn agent(&self, handle: &str) -> Uuid {
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
        agent
    }

    async fn booted(&self, handle: &str) -> Booted {
        let agent = self.agent(handle).await;
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

    async fn tool(&self, b: &Booted, tool: &str, args: Value) -> Value {
        let (s, v) = self
            .post(
                Some(self.replica),
                "/v1/runner/tool-calls",
                json!({"sandbox_id": b.sandbox, "boot_id": b.boot, "call_id": "c1",
                       "conversation_id": b.conv, "turn_id": b.turn, "tool": tool, "args": args}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
    async fn raw_put(&self, cookie: &str, uri: &str, body: &[u8]) -> (u16, Value) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let req = http::Request::builder()
            .method(Method::PUT)
            .uri(uri)
            .header("cookie", cookie)
            .header("origin", ORIGIN)
            .header("content-type", "application/octet-stream")
            .body(axum::body::Body::from(body.to_vec()))
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn as_user(&self, email: &str, role: &str) -> String {
        let u = self.h.user(email, false).await;
        self.h.member(self.org, u, role, "active").await;
        let (_, st) = self.h.good_session(u).await;
        cookie(&st)
    }

    async fn call(&self, cookie: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", cookie), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn gen_of(&self, agent: Uuid) -> i64 {
        sqlx::query_scalar("SELECT home_sync_generation FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn code(v: &Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

#[tokio::test]
async fn crud_limits_and_names() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let base = format!("/v1/agents/{a}/skills");
    let g0 = r.gen_of(a).await;
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"name": "deploy", "description": "How to deploy", "body": "# Steps"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (v["scope"].as_str(), v["agent_id"].as_str()),
        (Some("agent"), Some(a.to_string().as_str()))
    );
    assert_eq!(v["total_size"], 7);
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(r.gen_of(a).await, g0 + 1);
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"name": "deploy", "description": "d", "body": ""})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, Some("skill_exists")));
    for bad in [
        "Bad",
        "a--b",
        "aurora-core",
        "aurora-x",
        "",
        &"x".repeat(65),
    ] {
        let (s, _) = r
            .user(
                Method::POST,
                &base,
                Some(json!({"name": bad, "description": "d", "body": "b"})),
            )
            .await;
        assert_eq!(s, 400, "{bad}");
    }
    for d in ["", "two\nlines", &"d".repeat(501)] {
        let (s, _) = r
            .user(
                Method::POST,
                &base,
                Some(json!({"name": "okay", "description": d, "body": "b"})),
            )
            .await;
        assert_eq!(s, 400, "{d:?}");
    }
    let (s, _) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"name": "okay", "description": "d", "body": "b".repeat(24_001)})),
        )
        .await;
    assert_eq!(s, 400);

    // Total limit: body 24k + files.
    let uri = format!("/v1/skills/{id}");
    let (s, _) = r
        .user(
            Method::PATCH,
            &uri,
            Some(json!({"body": "b".repeat(24_000)})),
        )
        .await;
    assert_eq!(s, 200);
    let f = format!("{uri}/files/scripts/run.sh");
    let (s, v) = r.raw_put(&r.cookie, &f, "x".repeat(8_000).as_bytes()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["total_size"], 32_000);
    let (s, v) = r.raw_put(&r.cookie, &f, "x".repeat(8_001).as_bytes()).await;
    assert_eq!((s, code(&v)), (422, Some("skill_too_large")));
    // Replacing a file accounts for the old size.
    assert_eq!(r.raw_put(&r.cookie, &f, b"small").await.0, 200);

    let (s, v) = r.user(Method::GET, &base, None).await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
    assert!(v["items"][0]["files"][0].get("content").is_none());
    let (s, _) = r.user(Method::DELETE, &uri, None).await;
    assert_eq!(s, 204);
    assert_eq!(r.user(Method::GET, &uri, None).await.0, 404);
}

#[tokio::test]
async fn file_paths_and_executable_bit() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let (_, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(json!({"name": "tool", "description": "d", "body": "b"})),
        )
        .await;
    let uri = format!("/v1/skills/{}/files", v["id"].as_str().unwrap());
    for bad in [
        "..%2Fx",
        "a/../b",
        "a/./b",
        "a//b",
        "a/b/c/d/e",
        "sp%20ace",
        "SKILL.md",
        "dir/skill.MD",
        "caf%C3%A9",
    ] {
        let (s, v) = r.raw_put(&r.cookie, &format!("{uri}/{bad}"), b"x").await;
        assert!(matches!(s, 400 | 404), "{bad}: {s} {v}");
        assert_ne!(s, 200, "{bad}");
    }
    let (s, _) = r.raw_put(&r.cookie, &format!("{uri}/a/b/c/d"), b"x").await;
    assert_eq!(s, 200);
    let (s, v) = r
        .raw_put(
            &r.cookie,
            &format!("{uri}/bin/run.sh?executable=true"),
            b"#!/bin/sh",
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let exec = |v: &Value, p: &str| {
        v["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == p)
            .map(|f| f["executable"].clone())
    };
    assert_eq!(exec(&v, "bin/run.sh"), Some(json!(true)));
    assert_eq!(exec(&v, "a/b/c/d"), Some(json!(false)));
    // Omitted flag keeps the bit; explicit false clears it.
    let (_, v) = r
        .raw_put(&r.cookie, &format!("{uri}/bin/run.sh"), b"#!/bin/sh\n")
        .await;
    assert_eq!(exec(&v, "bin/run.sh"), Some(json!(true)));
    let (_, v) = r
        .raw_put(
            &r.cookie,
            &format!("{uri}/bin/run.sh?executable=false"),
            b"x",
        )
        .await;
    assert_eq!(exec(&v, "bin/run.sh"), Some(json!(false)));
    // Binary counts as base64 length.
    let (_, v) = r
        .raw_put(&r.cookie, &format!("{uri}/blob.bin"), &[0xff, 0xfe, 0xfd])
        .await;
    let size = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "blob.bin")
        .unwrap()["size"]
        .clone();
    assert_eq!(size, 4);
    let (s, _) = r
        .user(Method::DELETE, &format!("{uri}/blob.bin"), None)
        .await;
    assert_eq!(s, 204);
    let (s, _) = r
        .user(Method::DELETE, &format!("{uri}/blob.bin"), None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn org_permissions_and_generation_bump() {
    let r = rig().await;
    let a1 = r.agent("a1").await;
    let a2 = r.agent("a2").await;
    let member = r.cookie.clone();
    let admin = r.as_user("adm@x.io", "admin").await;
    let base = format!("/v1/orgs/{}/skills", r.org);
    let body = json!({"name": "policy", "description": "Org policy", "body": "Be nice"});
    let (s, v) = r
        .call(&member, Method::POST, &base, Some(body.clone()))
        .await;
    assert_eq!((s, code(&v)), (403, Some("forbidden")));
    let (g1, g2) = (r.gen_of(a1).await, r.gen_of(a2).await);
    let (s, v) = r.call(&admin, Method::POST, &base, Some(body)).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["scope"], "organization");
    assert!(v["agent_id"].is_null());
    assert_eq!((r.gen_of(a1).await, r.gen_of(a2).await), (g1 + 1, g2 + 1));
    let id = v["id"].as_str().unwrap().to_owned();
    // Members read but cannot write.
    assert_eq!(r.call(&member, Method::GET, &base, None).await.0, 200);
    assert_eq!(
        r.call(&member, Method::GET, &format!("/v1/skills/{id}"), None)
            .await
            .0,
        200
    );
    let (s, _) = r
        .call(
            &member,
            Method::PATCH,
            &format!("/v1/skills/{id}"),
            Some(json!({"body": "x"})),
        )
        .await;
    assert_eq!(s, 403);
    assert_eq!(
        r.raw_put(&member, &format!("/v1/skills/{id}/files/x.txt"), b"x")
            .await
            .0,
        403
    );
    assert_eq!(
        r.call(&member, Method::DELETE, &format!("/v1/skills/{id}"), None)
            .await
            .0,
        403
    );
    // Admin edits bump every agent; file changes too.
    let (s, _) = r
        .call(
            &admin,
            Method::PATCH,
            &format!("/v1/skills/{id}"),
            Some(json!({"body": "Be kind"})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(r.gen_of(a2).await, g2 + 2);
    assert_eq!(
        r.raw_put(&admin, &format!("/v1/skills/{id}/files/x.txt"), b"x")
            .await
            .0,
        200
    );
    assert_eq!(r.gen_of(a1).await, g1 + 3);
    // Same name at agent tier is allowed (agent wins in the projection).
    let (s, _) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a1}/skills"),
            Some(json!({"name": "policy", "description": "Mine", "body": "Mine"})),
        )
        .await;
    assert_eq!(s, 201);
    // Deleting bumps again.
    assert_eq!(
        r.call(&admin, Method::DELETE, &format!("/v1/skills/{id}"), None)
            .await
            .0,
        204
    );
    assert_eq!(r.gen_of(a2).await, g2 + 4);
    // A user without access to the org sees nothing.
    let u = r.h.user("out@x.io", false).await;
    let (_, st) = r.h.good_session(u).await;
    assert_eq!(r.call(&cookie(&st), Method::GET, &base, None).await.0, 404);
    assert_eq!(
        r.call(
            &cookie(&st),
            Method::GET,
            &format!("/v1/agents/{a1}/skills"),
            None
        )
        .await
        .0,
        404
    );
}

#[tokio::test]
async fn skill_tools_via_tool_calls() {
    let r = rig().await;
    let b = r.booted("t1").await;
    // An org skill, created by an admin through the DB.
    let org_skill = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skills (id, org_id, name, description, body, created_by_kind)
         VALUES ($1,$2,'orgskill','From org','org body','user')",
    )
    .bind(org_skill)
    .bind(r.org)
    .execute(r.h.pool())
    .await
    .unwrap();
    let g0 = r.gen_of(b.agent).await;
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "mine", "description": "My skill", "body": "do it"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(r.gen_of(b.agent).await, g0 + 1);
    let v = r.tool(&b, "skill_list", json!({})).await;
    let names: Vec<&str> = v["content"][0]["value"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["mine", "orgskill"]);
    let v = r.tool(&b, "skill_get", json!({"name": "orgskill"})).await;
    assert_eq!(v["content"][0]["value"]["body"], "org body");
    let v = r.tool(&b, "skill_get", json!({"name": "mine"})).await;
    assert_eq!(v["content"][0]["value"]["scope"], "agent");
    // Org skills are read-only for the agent.
    for (t, a) in [
        ("skill_update", json!({"name": "orgskill", "body": "x"})),
        ("skill_delete", json!({"name": "orgskill"})),
    ] {
        let v = r.tool(&b, t, a).await;
        assert_eq!(v["ok"], false, "{t}: {v}");
    }
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM skills WHERE id = $1 AND body = 'org body'")
            .bind(org_skill)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 1);
    let v = r
        .tool(
            &b,
            "skill_update",
            json!({"name": "mine", "body": "better", "enabled": false}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "aurora-x", "description": "d", "body": "b"}),
        )
        .await;
    assert_eq!(v["ok"], false);
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "x", "description": "d", "body": "b", "bogus": 1}),
        )
        .await;
    assert_eq!(v["error"]["code"], "invalid_arguments");
    // Created by the agent.
    let k: String = sqlx::query_scalar("SELECT created_by_kind FROM skills WHERE name = 'mine'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(k, "agent");
    let v = r.tool(&b, "skill_delete", json!({"name": "mine"})).await;
    assert_eq!(v["ok"], true, "{v}");
    // Another agent never sees it.
    let o = r.booted("t2").await;
    let v = r.tool(&o, "skill_get", json!({"name": "mine"})).await;
    assert_eq!(v["ok"], false);
}

#[tokio::test]
async fn projection_entries_in_home_sync() {
    let r = rig().await;
    let b = r.booted("p1").await;
    let mk = |agent: Option<Uuid>, name: &str, desc: &str, enabled: bool| {
        let (org, name, desc) = (r.org, name.to_owned(), desc.to_owned());
        let pool = r.h.pool().clone();
        async move {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO skills (id, org_id, agent_id, name, description, body, enabled, created_by_kind)
                 VALUES ($1,$2,$3,$4,$5,'body',$6,'user')",
            )
            .bind(id).bind(org).bind(agent).bind(name).bind(desc).bind(enabled)
            .execute(&pool).await.unwrap();
            id
        }
    };
    mk(None, "shared", "Org shared", true).await;
    mk(Some(b.agent), "shared", "Agent shared", true).await;
    mk(Some(b.agent), "off", "Disabled agent", false).await;
    mk(None, "off", "Org off", true).await;
    // Files go through the API so the exec flag is exercised end to end.
    let (_, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{}/skills", b.agent),
            Some(json!({"name": "withfiles", "description": "d", "body": "b"})),
        )
        .await;
    let uri = format!("/v1/skills/{}/files", v["id"].as_str().unwrap());
    r.raw_put(
        &r.cookie,
        &format!("{uri}/run.sh?executable=true"),
        b"#!/bin/sh",
    )
    .await;
    r.raw_put(&r.cookie, &format!("{uri}/notes.txt"), b"hi")
        .await;

    let claim = aurora_db::repo::hosted::HomeSyncClaim {
        sandbox_id: b.sandbox,
        boot_id: b.boot,
        agent_id: b.agent,
        generation: 1,
        harness_id: Some("fake".into()),
    };
    let mut conn = r.h.pool().acquire().await.unwrap();
    let parts = aurora_api::domains::hosted::home::render(&r.h.state, &mut conn, &claim)
        .await
        .unwrap();
    let entries: Vec<Value> = parts
        .iter()
        .filter(|p| p.part.kind.as_str() == "skills")
        .flat_map(|p| p.part.entries.clone())
        .collect();
    let find = |path: &str| entries.iter().find(|e| e["path"] == path).cloned();
    let shared = find(".claude/skills/shared/SKILL.md").unwrap();
    assert!(shared["content"].as_str().unwrap().contains("Agent shared"));
    assert_eq!(
        entries
            .iter()
            .filter(|e| e["path"] == ".claude/skills/shared/SKILL.md")
            .count(),
        1
    );
    // A disabled agent skill does not veto the enabled org skill of the same name.
    let off = find(".claude/skills/off/SKILL.md").unwrap();
    assert!(off["content"].as_str().unwrap().contains("Org off"));
    assert_eq!(
        find(".claude/skills/withfiles/run.sh").unwrap()["exec"],
        true
    );
    assert_eq!(
        find(".claude/skills/withfiles/notes.txt").unwrap()["exec"],
        false
    );
}

/// TEST-014: parts stay within `HOME_SYNC_PART_MAX` at worst-case encodings (docs/12 §Sync
/// protocol: CJK x3 bytes, JSON-escaped controls x6) with every entry delivered exactly once.
#[tokio::test]
async fn home_sync_parts_pack_within_the_wire_max_at_worst_case_encodings() {
    let r = rig().await;
    let b = r.booted("pk1").await;
    // Skill bodies of control characters: each is escaped to `\u0001` (6 bytes) on the wire.
    let ctrl = "\u{1}".repeat(aurora_core::limits::SKILL_BODY_MAX);
    for i in 0..3 {
        sqlx::query(
            "INSERT INTO skills (id, org_id, agent_id, name, description, body, enabled, created_by_kind)
             VALUES ($1,$2,$3,$4,'d',$5,true,'user')",
        )
        .bind(Uuid::now_v7())
        .bind(r.org)
        .bind(b.agent)
        .bind(format!("ctrl-{i}"))
        .bind(&ctrl)
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    // Memories at the tool content cap in 3-byte CJK.
    let cjk = "漢".repeat(aurora_core::limits::MEMORY_TOOL_CONTENT_MAX);
    for i in 0..30 {
        sqlx::query(
            "INSERT INTO memories (id, agent_id, key, content, revision) VALUES ($1,$2,$3,$4,1)",
        )
        .bind(Uuid::now_v7())
        .bind(b.agent)
        .bind(format!("cjk-{i}"))
        .bind(&cjk)
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    let claim = aurora_db::repo::hosted::HomeSyncClaim {
        sandbox_id: b.sandbox,
        boot_id: b.boot,
        agent_id: b.agent,
        generation: 7,
        harness_id: Some("fake".into()),
    };
    let mut conn = r.h.pool().acquire().await.unwrap();
    let parts = aurora_api::domains::hosted::home::render(&r.h.state, &mut conn, &claim)
        .await
        .unwrap();
    let count = u32::try_from(parts.len()).unwrap();
    for (i, p) in parts.iter().enumerate() {
        let wire = serde_json::to_string(p).unwrap();
        assert!(
            wire.len() <= aurora_protocol::runner::MAX_HOME_SYNC_PART,
            "part {i} ({}) is {} bytes",
            p.part.kind.as_str(),
            wire.len()
        );
        assert_eq!(
            (p.generation, p.part_index, p.part_count),
            (7, u32::try_from(i).unwrap(), count)
        );
    }
    assert_eq!(parts.last().unwrap().part.kind.as_str(), "final");
    let paths = |kind: &str| -> Vec<String> {
        parts
            .iter()
            .filter(|p| p.part.kind.as_str() == kind)
            .flat_map(|p| p.part.entries.iter())
            .map(|e| e["path"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(paths("skills").len(), 3);
    assert!(
        parts
            .iter()
            .filter(|p| p.part.kind.as_str() == "skills")
            .count()
            >= 3
    );
    // 30 memories + index, each once, and the manifest names every managed path.
    let mem = paths("memory");
    assert_eq!(mem.len(), 31);
    let manifest = paths("manifest");
    for p in paths("skills").iter().chain(mem.iter()) {
        assert!(manifest.contains(p), "{p} missing from the manifest");
    }
}

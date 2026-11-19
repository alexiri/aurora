#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A fake-runner rig with a booted sandbox seeded straight into the database: one admin-registered
//! runner, one workspace owner with a session, and an agent with a running sandbox, a conversation
//! and a running turn. Shared by the memory, skills and webhooks tests (P5-DEDUP-1 T2); the
//! turn-driving rig is [`crate::hosted::Rig`].

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct Rig {
    pub(crate) h: Harness,
    pub(crate) org: Uuid,
    pub(crate) ws: Uuid,
    pub(crate) tok: String,
    pub(crate) replica: Uuid,
    pub(crate) runner_id: Uuid,
    pub(crate) cookie: String,
}

pub(crate) async fn rig() -> Rig {
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

pub(crate) struct Booted {
    pub(crate) agent: Uuid,
    pub(crate) sandbox: Uuid,
    pub(crate) boot: Uuid,
    pub(crate) conv: Uuid,
    pub(crate) turn: Uuid,
}

impl Rig {
    pub(crate) async fn post(&self, replica: Option<Uuid>, uri: &str, body: Value) -> (u16, Value) {
        let b = bearer(&self.tok);
        let rid = replica.map(|r| r.to_string());
        let mut hd = vec![("authorization", b.as_str())];
        if let Some(r) = &rid {
            hd.push(("x-aurora-replica", r.as_str()));
        }
        let (s, _, v) = self.h.send(Method::POST, uri, &hd, Some(body)).await;
        (s.as_u16(), v)
    }

    pub(crate) async fn user(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
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

    pub(crate) async fn agent(&self, handle: &str) -> Uuid {
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

    pub(crate) async fn booted(&self, handle: &str) -> Booted {
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

    pub(crate) async fn tool(&self, b: &Booted, tool: &str, args: Value) -> Value {
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
}

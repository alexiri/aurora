#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Shared rig for the BE-065 tests (`action_approvals.rs`, `peer_tasks.rs`): one org with two
//! workspaces, agents in each, people with different access, and direct tool invocation.

use crate::common::*;
use aurora_api::domains::tools::{ToolCallContext, ToolContext, registry};
use aurora_protocol::tools::{ToolContent, ToolResult};
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

pub struct Person {
    pub id: Uuid,
    pub cookie: String,
}

pub struct Rig {
    pub h: Harness,
    pub org: Uuid,
    pub ws1: Uuid,
    pub ws2: Uuid,
    /// Member of ws1 only.
    pub p: Person,
    /// Member of ws2 only.
    pub q: Person,
    /// Org member without any workspace binding.
    pub r: Person,
}

pub async fn rig() -> Rig {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws1 = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let mut people = Vec::new();
    for (email, ws) in [
        ("p@x.io", Some(ws1)),
        ("q@x.io", Some(ws2)),
        ("r@x.io", None),
    ] {
        let id = h.user(email, false).await;
        h.member(org, id, "member", "active").await;
        if let Some(ws) = ws {
            h.bind_ws(ws, id, "member").await;
        }
        let (_, tok) = h.good_session(id).await;
        people.push(Person {
            id,
            cookie: cookie(&tok),
        });
    }
    let r = people.pop().unwrap();
    let q = people.pop().unwrap();
    let p = people.pop().unwrap();
    Rig {
        h,
        org,
        ws1,
        ws2,
        p,
        q,
        r,
    }
}

impl Rig {
    pub async fn agent(&self, ws: Uuid, handle: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
             VALUES ($1,$2,$3,$4,$5,'hosted','fake')",
        )
        .bind(id)
        .bind(self.org)
        .bind(ws)
        .bind(handle)
        .bind(format!("Agent {handle}"))
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    pub async fn agent_ws(&self, agent: Uuid) -> Uuid {
        sqlx::query_scalar("SELECT workspace_id FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// An `api` conversation of `agent`.
    pub async fn conversation(&self, agent: Uuid) -> Uuid {
        let id = Uuid::now_v7();
        let ws = self.agent_ws(agent).await;
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
             VALUES ($1,$2,$3,'api',$1::text, now())",
        )
        .bind(id)
        .bind(agent)
        .bind(ws)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    /// A running turn spoken by `speaker_kind` (`person` needs `user`).
    pub async fn turn(
        &self,
        conv: Uuid,
        agent: Uuid,
        speaker_kind: &str,
        user: Option<Uuid>,
    ) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind,
                speaker_user_id, speaker_label, text)
             VALUES ($1,$2,$3,'turn','running','{\"kind\":\"web\"}',$4,$5,'Speaker','hello')",
        )
        .bind(id)
        .bind(conv)
        .bind(agent)
        .bind(speaker_kind)
        .bind(user)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    pub async fn finish_turn(&self, turn: Uuid) {
        sqlx::query("UPDATE turns SET status = 'done', finished_at = now() WHERE id = $1")
            .bind(turn)
            .execute(self.h.pool())
            .await
            .unwrap();
    }

    /// A conversation of `agent` with a running person turn by `user`: `(conv, turn)`.
    pub async fn person_turn(&self, agent: Uuid, user: Uuid) -> (Uuid, Uuid) {
        let c = self.conversation(agent).await;
        let t = self.turn(c, agent, "person", Some(user)).await;
        (c, t)
    }

    pub async fn tool(
        &self,
        agent: Uuid,
        conv: Uuid,
        turn: Option<Uuid>,
        name: &str,
        args: Value,
    ) -> ToolResult {
        let ws = self.agent_ws(agent).await;
        let ctx = ToolCallContext {
            agent: ToolContext {
                pool: self.h.pool().clone(),
                agent_id: agent,
                workspace_id: ws,
                org_id: self.org,
            },
            conversation_id: conv,
            turn_id: turn,
            call_id: "c1".into(),
            state: self.h.state.clone(),
        };
        registry()
            .call(&ctx, name, args, Duration::from_secs(20))
            .await
    }

    /// A successful tool result's JSON payload.
    pub fn ok(res: &ToolResult) -> Value {
        assert!(res.ok, "tool failed: {:?}", res.error);
        match &res.content.as_ref().unwrap()[0] {
            ToolContent::Json(j) => j.value.clone(),
            other => panic!("unexpected content {other:?}"),
        }
    }

    /// A failed tool result's message.
    pub fn err(res: &ToolResult) -> String {
        assert!(!res.ok, "tool unexpectedly succeeded: {:?}", res.content);
        res.error.as_ref().unwrap().message.clone()
    }

    pub async fn post(&self, who: &Person, uri: &str, body: Option<Value>) -> (u16, Value) {
        let c = who.cookie.as_str();
        let (s, _, v) = self
            .h
            .send(
                Method::POST,
                uri,
                &[("cookie", c), ("origin", ORIGIN)],
                body.or(Some(json!({}))),
            )
            .await;
        (s.as_u16(), v)
    }

    pub async fn put(&self, who: &Person, uri: &str, body: Value) -> (u16, Value) {
        let c = who.cookie.as_str();
        let (s, _, v) = self
            .h
            .send(
                Method::PUT,
                uri,
                &[("cookie", c), ("origin", ORIGIN)],
                Some(body),
            )
            .await;
        (s.as_u16(), v)
    }

    pub async fn get(&self, who: &Person, uri: &str) -> (u16, Value) {
        let c = who.cookie.as_str();
        let (s, v) = self.h.get(uri, &[("cookie", c), ("origin", ORIGIN)]).await;
        (s.as_u16(), v)
    }

    pub async fn decide(&self, who: &Person, approval: Uuid, decision: &str) -> (u16, Value) {
        self.post(
            who,
            &format!("/v1/action-approvals/{approval}/decision"),
            Some(json!({"decision": decision})),
        )
        .await
    }

    /// Durable `(type, payload)` events of a conversation in seq order.
    pub async fn events(&self, conv: Uuid) -> Vec<(String, Value)> {
        sqlx::query_as(
            "SELECT type, payload FROM transcript_events WHERE conversation_id = $1 ORDER BY seq",
        )
        .bind(conv)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    /// Notices with `code` in a conversation.
    pub async fn notices(&self, conv: Uuid, code: &str) -> Vec<Value> {
        self.events(conv)
            .await
            .into_iter()
            .filter(|(t, p)| t == "notice" && p["code"] == code)
            .map(|(_, p)| p)
            .collect()
    }

    /// `(role, status, speaker_kind, text)` of every turn row of a conversation, oldest first.
    pub async fn turns(&self, conv: Uuid) -> Vec<(String, String, String, String)> {
        sqlx::query_as(
            "SELECT role, status, speaker_kind, text FROM turns WHERE conversation_id = $1
             ORDER BY created_at, id",
        )
        .bind(conv)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    pub async fn peer_conversation(&self, task: Uuid) -> Uuid {
        sqlx::query_scalar("SELECT task_conversation_id FROM peer_tasks WHERE id = $1")
            .bind(task)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    pub async fn task_status(&self, task: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM peer_tasks WHERE id = $1")
            .bind(task)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    pub async fn running_turn(&self, conv: Uuid) -> Option<Uuid> {
        sqlx::query_scalar(
            "SELECT id FROM turns WHERE conversation_id = $1 AND role = 'turn'
               AND status IN ('queued','dispatched','running') LIMIT 1",
        )
        .bind(conv)
        .fetch_optional(self.h.pool())
        .await
        .unwrap()
    }

    pub fn id(v: &Value) -> Uuid {
        v.as_str().unwrap().parse().unwrap()
    }
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-073: interactive cards, action tokens, clicker authorization and approval fan-out
//! (docs/13 §4.4, §4.5, §5.5, §5.6). The connector side is played by SQL: ops are read from
//! `channel_ops`, "delivered" by marking them done, and clicks arrive as `action.invoked` inbox
//! rows processed by the real handlers.

use crate::common::*;
use aurora_api::domains::channels::actions::ActionHandler;
use aurora_api::domains::channels::card_requests::{
    BindingChoose, DmDefaultChoose, ReachDecide, card_requests,
};
use aurora_api::domains::channels::cards::{self, fanout};
use aurora_api::domains::connectors::inbox::{self, InboxHandler};
use aurora_channels::{ActionKind, ActionToken, Claims, Decision};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    integ: Uuid,
    agent: Uuid,
    agent2: Uuid,
    conv: Uuid,
    owner: Uuid,
    member: Uuid,
    gw: Uuid,
}

const FULL: [&str; 7] = [
    "threads",
    "edits",
    "buttons",
    "menus",
    "ephemeral",
    "commands",
    "reactions",
];

fn caps(names: &[&str]) -> Value {
    let mut m = serde_json::Map::new();
    for n in names {
        m.insert((*n).to_owned(), json!(true));
    }
    json!({ "capabilities": m })
}

impl W {
    async fn new(names: &[&str]) -> W {
        let h = Harness::new().await;
        let org = h.org().await;
        let ws = h.workspace(org).await;
        let owner = h.user("owner@x.io", false).await;
        let member = h.user("member@x.io", false).await;
        let outsider = h.user("out@x.io", false).await;
        let stranger = h.user("stranger@x.io", false).await;
        for (u, n) in [(owner, "Olivia"), (member, "Mark")] {
            sqlx::query("UPDATE users SET display_name=$2 WHERE id=$1")
                .bind(u)
                .bind(n)
                .execute(h.pool())
                .await
                .unwrap();
        }
        h.member(org, owner, "owner", "active").await;
        h.member(org, member, "member", "active").await;
        h.member(org, outsider, "member", "active").await;
        h.bind_ws(ws, member, "member").await;
        let p = h.pool();
        sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('chat', $1, 'chat', $2, 'active')")
            .bind(vec![7u8; 32]).bind(caps(names)).execute(p).await.unwrap();
        let integ = Self::integration(&h, org, "chat", "T1").await;
        let agent = Self::agent(&h, org, ws, "bot").await;
        let agent2 = Self::agent(&h, org, ws, "bot2").await;
        let conv = Uuid::now_v7();
        sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api',$1::text, now())")
            .bind(conv).bind(agent).bind(ws).execute(p).await.unwrap();
        sqlx::query("INSERT INTO thread_links (id, integration_id, agent_id, conversation_id, kind, space_ref, thread_ref, sender) VALUES ($1,$2,$3,$4,'group','C1','T1','\"shared\"')")
            .bind(Uuid::now_v7()).bind(integ).bind(agent).bind(conv).execute(p).await.unwrap();
        for (u, r) in [
            (owner, "U_OWNER"),
            (member, "U_MEMBER"),
            (outsider, "U_OUT"),
            (stranger, "U_STR"),
        ] {
            sqlx::query("INSERT INTO user_links (id, integration_id, platform_user_ref, user_id, method) VALUES ($1,$2,$3,$4,'admin')")
                .bind(Uuid::now_v7()).bind(integ).bind(r).bind(u).execute(p).await.unwrap();
        }
        let gw = Uuid::now_v7();
        sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','0',now(),now())")
            .bind(gw).execute(p).await.unwrap();
        W {
            h,
            org,
            ws,
            integ,
            agent,
            agent2,
            conv,
            owner,
            member,
            gw,
        }
    }

    async fn integration(h: &Harness, org: Uuid, connector: &str, tenant: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,$3,$4,$4,'{}','x','active')")
            .bind(id).bind(org).bind(connector).bind(tenant).execute(h.pool()).await.unwrap();
        id
    }

    async fn agent(h: &Harness, org: Uuid, ws: Uuid, handle: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,$4,'byo','medium','')")
            .bind(id).bind(org).bind(ws).bind(handle).execute(h.pool()).await.unwrap();
        id
    }

    fn pool(&self) -> &sqlx::PgPool {
        self.h.pool()
    }

    async fn approval(&self, batch: Option<(&str, i32)>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, gateway_instance_id,
                method, host, path, summary, batch_id, batch_total, status, deadline)
             VALUES ($1,$2,$3,$4,$5,$6,'POST','api.example.com','/x',
                '{\"title\":\"Send **mail**\",\"fields\":[{\"label\":\"to\",\"value\":\"a@b.c\"}],\"text\":null,\"link\":null,\"group\":null,\"source\":\"generic\"}',
                $7,$8,'pending', now() + interval '5 minutes')",
        )
        .bind(id).bind(self.org).bind(self.ws).bind(self.agent).bind(self.conv).bind(self.gw)
        .bind(batch.map(|b| b.0)).bind(batch.map(|b| b.1))
        .execute(self.pool()).await.unwrap();
        id
    }

    async fn action_approval(&self) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO action_approvals (id, workspace_id, agent_id, conversation_id, action_key, payload, summary, status, expires_at) VALUES ($1,$2,$3,$4,'message_agent','{}','Talk to bot2','pending', now() + interval '1 hour')")
            .bind(id).bind(self.ws).bind(self.agent).bind(self.conv).execute(self.pool()).await.unwrap();
        id
    }

    async fn fanout(&self) {
        fanout::run_once(&self.h.state).await.unwrap();
    }

    /// Ops of a kind, oldest first.
    async fn ops(&self, kind: &str) -> Vec<(Uuid, Value, Option<String>)> {
        sqlx::query_as(
            "SELECT id, payload, dedupe_key FROM channel_ops WHERE kind=$1 ORDER BY created_at, id",
        )
        .bind(kind)
        .fetch_all(self.pool())
        .await
        .unwrap()
    }

    /// The connector "delivers" every pending post: marks it done with a message ref.
    async fn deliver(&self) {
        sqlx::query("UPDATE channel_ops SET status='done', result=jsonb_build_object('message_ref', 'm-'||id::text, 'thread_ref', 'T1') WHERE kind='message.post' AND status='pending'")
            .execute(self.pool()).await.unwrap();
        fanout::sync_posting(&self.h.state).await.unwrap();
    }

    fn tokens(op: &Value) -> Vec<(String, String)> {
        let mut out = vec![];
        for b in op["content"]["card"]["blocks"]
            .as_array()
            .unwrap_or(&vec![])
        {
            if let Some(items) = b["items"].as_array() {
                for i in items {
                    out.push((
                        i["label"].as_str().unwrap().to_owned(),
                        i["token"].as_str().unwrap().to_owned(),
                    ));
                }
            }
            if b["type"] == "select" {
                out.push(("select".into(), b["token"].as_str().unwrap().to_owned()));
            }
        }
        out
    }

    fn token(op: &Value, label: &str) -> String {
        Self::tokens(op)
            .into_iter()
            .find(|(l, _)| l == label)
            .unwrap_or_else(|| panic!("no {label} in {op}"))
            .1
    }

    async fn click(&self, integ: Uuid, token: &str, clicker: &str, value: Option<&str>) -> Value {
        self.click_in(integ, token, clicker, value, None).await
    }

    async fn click_in(
        &self,
        integ: Uuid,
        token: &str,
        clicker: &str,
        value: Option<&str>,
        space: Option<&str>,
    ) -> Value {
        let id = Uuid::now_v7();
        let mut ev = json!({"token": token, "clicker_ref": clicker});
        if let Some(v) = value {
            ev["value"] = json!(v);
        }
        if let Some(s) = space {
            ev["space_ref"] = json!(s);
        }
        sqlx::query("INSERT INTO channel_inbox (id, connector_id, integration_id, event_id, kind, payload, status, next_attempt_at) VALUES ($1,'chat',$2,$1::text,'action.invoked',$3,'pending', now())")
            .bind(id).bind(integ).bind(ev).execute(self.pool()).await.unwrap();
        let handlers: Vec<Arc<dyn InboxHandler>> = vec![Arc::new(ActionHandler)];
        inbox::process_batch(&self.h.state, &handlers)
            .await
            .unwrap();
        let (status, outcome): (String, Option<Value>) =
            sqlx::query_as("SELECT status, outcome FROM channel_inbox WHERE id=$1")
                .bind(id)
                .fetch_one(self.pool())
                .await
                .unwrap();
        assert_eq!(status, "done");
        outcome.unwrap()
    }

    async fn status(&self, table: &str, id: Uuid) -> String {
        sqlx::query_scalar(&format!("SELECT status FROM {table} WHERE id=$1"))
            .bind(id)
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    async fn token_uses(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM action_token_uses")
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    fn minter(&self, integ: Uuid) -> cards::Minter<'_> {
        cards::minter(&self.h.state, integ)
    }

    /// Post a fresh approval card and deliver it; returns (approval id, card op payload).
    async fn approval_card(&self) -> (Uuid, Value) {
        let id = self.approval(None).await;
        self.fanout().await;
        self.deliver().await;
        let op = self.ops("message.post").await.pop().unwrap().1;
        (id, op)
    }
}

#[tokio::test]
async fn approval_created_posts_one_card_and_click_approves_and_settles() {
    let w = W::new(&FULL).await;
    let id = w.approval(None).await;
    w.fanout().await;
    w.fanout().await; // catch-up is idempotent
    let posts = w.ops("message.post").await;
    assert_eq!(posts.len(), 1);
    let (_, op, dedupe) = &posts[0];
    assert_eq!(dedupe.as_deref(), Some(format!("approval:{id}").as_str()));
    assert_eq!(op["target"], json!({"space_ref": "C1", "thread_ref": "T1"}));
    assert_eq!(op["sender"], json!("shared"));
    assert_eq!(op["op_marker"], json!(posts[0].0.to_string()));
    let blocks = op["content"]["card"]["blocks"].as_array().unwrap();
    assert_eq!(
        blocks[0],
        json!({"type": "header", "text": "Approval needed"})
    );
    let labels: Vec<_> = W::tokens(op).into_iter().map(|t| t.0).collect();
    assert_eq!(labels, ["Approve", "Deny"]);
    let card: (String, String) =
        sqlx::query_as("SELECT subject_kind, state FROM channel_cards WHERE subject_id=$1")
            .bind(id)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert_eq!(card, ("approval".into(), "posting".into()));

    w.deliver().await;
    let state: String = sqlx::query_scalar("SELECT state FROM channel_cards WHERE subject_id=$1")
        .bind(id)
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(state, "posted");

    let out = w
        .click(w.integ, &W::token(op, "Approve"), "U_MEMBER", None)
        .await;
    assert_eq!(out["result"], "done", "{out}");
    let (st, by, via): (String, Option<Uuid>, Option<String>) =
        sqlx::query_as("SELECT status, decided_by_user_id, decided_via FROM approvals WHERE id=$1")
            .bind(id)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert_eq!(
        (st.as_str(), by, via.as_deref()),
        ("approved", Some(w.member), Some("channel"))
    );
    // Settled in place with who/how.
    let upd = w.ops("message.update").await;
    assert_eq!(upd.len(), 1);
    assert_eq!(upd[0].1["space_ref"], "C1");
    assert!(
        upd[0].1["content"]["card"]["blocks"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Approved by Mark")
    );
    let cs: String = sqlx::query_scalar("SELECT state FROM channel_cards WHERE subject_id=$1")
        .bind(id)
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(cs, "settled");
}

#[tokio::test]
async fn dashboard_decision_and_expiry_settle_the_card() {
    let w = W::new(&FULL).await;
    let (id, _) = w.approval_card().await;
    sqlx::query("UPDATE approvals SET status='denied', decided_via='dashboard', decided_at=now(), decided_by_user_id=$2 WHERE id=$1")
        .bind(id).bind(w.owner).execute(w.pool()).await.unwrap();
    w.fanout().await;
    let upd = w.ops("message.update").await;
    assert_eq!(upd.len(), 1);
    let t = upd[0].1["content"]["card"]["blocks"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        t.starts_with("Denied by Olivia") && t.contains("dashboard"),
        "{t}"
    );
    w.fanout().await;
    assert_eq!(w.ops("message.update").await.len(), 1, "settled once");

    let (id2, _) = w.approval_card().await;
    sqlx::query("UPDATE approvals SET status='expired' WHERE id=$1")
        .bind(id2)
        .execute(w.pool())
        .await
        .unwrap();
    w.fanout().await;
    let upd = w.ops("message.update").await;
    assert_eq!(upd.len(), 2);
    assert!(
        upd[1].1["content"]["card"]["blocks"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Expired")
    );
}

#[tokio::test]
async fn unlinked_clicker_gets_a_link_prompt_and_nothing_happens() {
    let w = W::new(&FULL).await;
    let (id, op) = w.approval_card().await;
    let tok = W::token(&op, "Approve");
    let out = w.click_in(w.integ, &tok, "U_NONE", None, Some("C1")).await;
    assert_eq!(out["result"], "link_prompt");
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.token_uses().await, 0, "the token is not burnt");
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM link_requests WHERE platform_user_ref='U_NONE'")
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert_eq!(n, 1);
    // Ephemeral notice with the link.
    let notice = w.ops("message.post").await.pop().unwrap().1;
    assert_eq!(notice["ephemeral_to"], "U_NONE");
    assert!(
        notice["content"]["text"]
            .as_str()
            .unwrap()
            .contains("/link/channel?token=")
    );
    // After linking, the same token works.
    let u = w.h.user("late@x.io", false).await;
    w.h.member(w.org, u, "member", "active").await;
    w.h.bind_ws(w.ws, u, "member").await;
    sqlx::query("INSERT INTO user_links (id, integration_id, platform_user_ref, user_id, method) VALUES ($1,$2,'U_NONE',$3,'admin')")
        .bind(Uuid::now_v7()).bind(w.integ).bind(u).execute(w.pool()).await.unwrap();
    assert_eq!(
        w.click(w.integ, &tok, "U_NONE", None).await["result"],
        "done"
    );
    assert_eq!(w.status("approvals", id).await, "approved");
}

#[tokio::test]
async fn non_members_are_refused_without_a_decision() {
    let w = W::new(&FULL).await;
    let (id, op) = w.approval_card().await;
    let tok = W::token(&op, "Approve");
    // U_STR is not in the org; U_OUT is an org member with no workspace access.
    for who in ["U_STR", "U_OUT"] {
        let out = w.click_in(w.integ, &tok, who, None, Some("C1")).await;
        assert_eq!(
            (out["result"].as_str(), out["reason"].as_str()),
            (Some("refused"), Some("forbidden")),
            "{who}: {out}"
        );
    }
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.token_uses().await, 0);
    // The refusal reached the clicker as an ephemeral notice.
    let notices: Vec<_> = w
        .ops("message.post")
        .await
        .into_iter()
        .filter(|o| o.1["ephemeral_to"].is_string())
        .collect();
    assert_eq!(notices.len(), 2);
}

#[tokio::test]
async fn reused_token_is_refused() {
    let w = W::new(&FULL).await;
    let (id, op) = w.approval_card().await;
    let tok = W::token(&op, "Approve");
    assert_eq!(
        w.click(w.integ, &tok, "U_MEMBER", None).await["result"],
        "done"
    );
    let out = w.click(w.integ, &tok, "U_OWNER", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("already_used")),
        "{out}"
    );
    // The sibling button of an already decided approval does not flip the decision.
    let deny = W::token(&op, "Deny");
    let out = w.click(w.integ, &deny, "U_OWNER", None).await;
    assert_eq!(out["result"], "refused");
    assert_eq!(w.status("approvals", id).await, "approved");
}

#[tokio::test]
async fn expired_token_is_refused() {
    let w = W::new(&FULL).await;
    let id = w.approval(None).await;
    let exp = chrono::Utc::now().timestamp() - 5;
    let tok = ActionToken::sign(
        &w.h.state.derived,
        &Claims::new(
            w.integ,
            id,
            exp,
            ActionKind::ApprovalDecide {
                decision: Decision::Approve,
            },
        ),
    )
    .unwrap();
    let out = w.click(w.integ, &tok, "U_MEMBER", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("expired"))
    );
    assert_eq!(w.status("approvals", id).await, "pending");
    // A forged token is refused too.
    let out = w.click(w.integ, "v1.e30.AAAA", "U_MEMBER", None).await;
    assert_eq!(out["reason"], "invalid_token");
}

#[tokio::test]
async fn token_from_another_integration_is_refused() {
    let w = W::new(&FULL).await;
    let other = W::integration(&w.h, w.org, "chat", "T2").await;
    let id = w.approval(None).await;
    let tok = w
        .minter(other)
        .mint(
            id,
            ActionKind::ApprovalDecide {
                decision: Decision::Approve,
            },
        )
        .unwrap();
    let out = w.click(w.integ, &tok, "U_MEMBER", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("wrong_integration"))
    );
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.token_uses().await, 0);
}

#[tokio::test]
async fn batch_cards_decide_all_of_the_agents_batch() {
    let w = W::new(&FULL).await;
    let a = w.approval(Some(("b1", 2))).await;
    let b = w.approval(Some(("b1", 2))).await;
    w.fanout().await;
    w.deliver().await;
    let posts = w.ops("message.post").await;
    assert_eq!(posts.len(), 2);
    let op = &posts
        .iter()
        .find(|p| p.2.as_deref() == Some(format!("approval:{a}").as_str()))
        .unwrap()
        .1;
    let labels: Vec<_> = W::tokens(op).into_iter().map(|t| t.0).collect();
    assert_eq!(labels, ["Approve", "Deny", "Approve all", "Deny all"]);
    let out = w
        .click(w.integ, &W::token(op, "Approve all"), "U_MEMBER", None)
        .await;
    assert_eq!(out["result"], "done", "{out}");
    assert_eq!(w.status("approvals", a).await, "approved");
    assert_eq!(w.status("approvals", b).await, "approved");
    assert_eq!(w.ops("message.update").await.len(), 2, "both cards settled");
}

#[tokio::test]
async fn action_approval_card_deny() {
    let w = W::new(&FULL).await;
    let id = w.action_approval().await;
    w.fanout().await;
    w.deliver().await;
    let op = w.ops("message.post").await.pop().unwrap();
    assert_eq!(
        op.2.as_deref(),
        Some(format!("action_approval:{id}").as_str())
    );
    let out = w
        .click(w.integ, &W::token(&op.1, "Deny"), "U_MEMBER", None)
        .await;
    assert_eq!(out["result"], "done", "{out}");
    assert_eq!(w.status("action_approvals", id).await, "rejected");
    let upd = w.ops("message.update").await;
    assert_eq!(upd.len(), 1);
    assert!(
        upd[0].1["content"]["card"]["blocks"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Denied by Mark")
    );
    // A non-member cannot decide a fresh one.
    let id2 = w.action_approval().await;
    w.fanout().await;
    let op2 = w.ops("message.post").await.pop().unwrap();
    let out = w
        .click(w.integ, &W::token(&op2.1, "Approve"), "U_STR", None)
        .await;
    assert_eq!(out["reason"], "forbidden");
    assert_eq!(w.status("action_approvals", id2).await, "pending");
}

async fn reach_grant(w: &W) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO reach_grants (id, agent_id, integration_id, subject_kind, subject_ref, subject_label, state) VALUES ($1,$2,$3,'person','U_X','Xavier','pending')")
        .bind(id).bind(w.agent).bind(w.integ).execute(w.pool()).await.unwrap();
    id
}

#[tokio::test]
async fn reach_decide_needs_can_own() {
    let w = W::new(&FULL).await;
    let grant = reach_grant(&w).await;
    let mut tx = w.pool().begin().await.unwrap();
    card_requests(&w.h.state)
        .reach_decide(
            &mut tx,
            ReachDecide {
                integration_id: w.integ,
                grant_id: grant,
                agent_id: w.agent,
                subject_label: "Xavier".into(),
                owners: vec![(w.owner, "U_OWNER".into())],
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let op = w.ops("message.post").await.pop().unwrap().1;
    assert_eq!(op["target"], json!({"user_ref": "U_OWNER"}));
    let tok = W::token(&op, "Allow");
    // can_use is not enough.
    let out = w.click(w.integ, &tok, "U_MEMBER", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("forbidden"))
    );
    let st: String = sqlx::query_scalar("SELECT state FROM reach_grants WHERE id=$1")
        .bind(grant)
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(st, "pending");
    // The owner can; the card (DM) is settled by a new message until its space is learned.
    w.deliver().await;
    let out = w.click_in(w.integ, &tok, "U_OWNER", None, Some("D1")).await;
    assert_eq!(out["result"], "done", "{out}");
    let (st, by): (String, Option<Uuid>) =
        sqlx::query_as("SELECT state, decided_by_user_id FROM reach_grants WHERE id=$1")
            .bind(grant)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert_eq!((st.as_str(), by), ("approved", Some(w.owner)));
    assert_eq!(
        w.ops("message.update").await.len() + w.ops("message.post").await.len(),
        3
    );
}

async fn shared_presence(w: &W, agent: Uuid, bot: &str) {
    sqlx::query("INSERT INTO channel_presences (id, agent_id, integration_id, mode, status, platform_user_ref, handle) VALUES ($1,$2,$3,'shared','active',$4,$4)")
        .bind(Uuid::now_v7()).bind(agent).bind(w.integ).bind(bot).execute(w.pool()).await.unwrap();
}

#[tokio::test]
async fn binding_choose_creates_the_binding_for_owners_only() {
    let w = W::new(&FULL).await;
    shared_presence(&w, w.agent, "B1").await;
    let mut tx = w.pool().begin().await.unwrap();
    card_requests(&w.h.state)
        .binding_choose(
            &mut tx,
            BindingChoose {
                integration_id: w.integ,
                actor_user_id: w.owner,
                actor_ref: "U_OWNER".into(),
                space_ref: "C9".into(),
                space_label: "C9".into(),
                allowed_agent_ids: vec![w.agent, w.agent2],
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let op = w.ops("message.post").await.pop().unwrap().1;
    // 13 §5.3: the select is DMed to the person who added the bot.
    assert_eq!(op["target"], json!({"user_ref": "U_OWNER"}));
    let tok = W::token(&op, "select");
    let a = w.agent.to_string();
    // can_use only: refused.
    let out = w.click(w.integ, &tok, "U_MEMBER", Some(&a)).await;
    assert_eq!(out["reason"], "forbidden");
    // Not one of the offered agents: refused.
    let out = w
        .click(w.integ, &tok, "U_OWNER", Some(&Uuid::now_v7().to_string()))
        .await;
    assert_eq!(out["reason"], "invalid_choice");
    // An offered agent without a shared presence: refused.
    let out = w
        .click(w.integ, &tok, "U_OWNER", Some(&w.agent2.to_string()))
        .await;
    assert_eq!(out["reason"], "unavailable");
    assert_eq!(w.token_uses().await, 0);
    w.deliver().await;
    // Clicked in the owner's DM with the bot: the card's space is learned and it is updated in place.
    let out = w
        .click_in(w.integ, &tok, "U_OWNER", Some(&a), Some("D_OWNER"))
        .await;
    assert_eq!(out["result"], "done", "{out}");
    let (agent, by): (Uuid, Option<Uuid>) = sqlx::query_as("SELECT agent_id, bound_by_user_id FROM space_bindings WHERE integration_id=$1 AND space_ref='C9' AND active").bind(w.integ).fetch_one(w.pool()).await.unwrap();
    assert_eq!((agent, by), (w.agent, Some(w.owner)));
    // A DM card is settled in place once its space is known, otherwise by a new message.
    let mut settled = w.ops("message.update").await;
    if settled.is_empty() {
        settled = w.ops("message.post").await;
    }
    let content = &settled.last().unwrap().1["content"];
    let text = content["card"]["blocks"][0]["text"]
        .as_str()
        .or_else(|| content["text"].as_str())
        .unwrap_or_default();
    assert!(text.starts_with("Bound to bot by Olivia"), "{content}");
}

#[tokio::test]
async fn dm_default_choose_sets_the_default_for_the_addressee_only() {
    let w = W::new(&FULL).await;
    let mut tx = w.pool().begin().await.unwrap();
    card_requests(&w.h.state)
        .dm_default_choose(
            &mut tx,
            DmDefaultChoose {
                integration_id: w.integ,
                user_id: w.member,
                user_ref: "U_MEMBER".into(),
                space_ref: "D_MEMBER".into(),
                thread_ref: String::new(),
                allowed_agent_ids: vec![w.agent2],
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let op = w.ops("message.post").await.pop().unwrap().1;
    assert_eq!(op["target"], json!({"user_ref": "U_MEMBER"}));
    let tok = W::token(&op, "select");
    let v = w.agent2.to_string();
    // Somebody else holding the token cannot set the member's default.
    let out = w.click(w.integ, &tok, "U_OWNER", Some(&v)).await;
    assert_eq!(out["reason"], "forbidden");
    let out = w.click(w.integ, &tok, "U_MEMBER", Some(&v)).await;
    assert_eq!(out["result"], "done", "{out}");
    let a: Uuid = sqlx::query_scalar(
        "SELECT agent_id FROM dm_defaults WHERE integration_id=$1 AND platform_user_ref='U_MEMBER'",
    )
    .bind(w.integ)
    .fetch_one(w.pool())
    .await
    .unwrap();
    assert_eq!(a, w.agent2);
}

#[tokio::test]
async fn turn_stop_and_send_now() {
    let w = W::new(&FULL).await;
    let turn = Uuid::now_v7();
    sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text) VALUES ($1,$2,$3,'turn','running','{}','system','s','hi')")
        .bind(turn).bind(w.conv).bind(w.agent).execute(w.pool()).await.unwrap();
    let m = w.minter(w.integ);
    let stop = m
        .mint(turn, ActionKind::TurnStop { turn_id: turn })
        .unwrap();
    let now = m
        .mint(turn, ActionKind::FollowupSendNow { turn_id: turn })
        .unwrap();
    // No workspace access: refused.
    let out = w.click(w.integ, &stop, "U_OUT", None).await;
    assert_eq!(out["reason"], "forbidden");
    let out = w.click(w.integ, &now, "U_MEMBER", None).await;
    assert_eq!(out["result"], "done", "{out}");
    let sn: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT send_now_requested_at FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert!(sn.is_some());
    let out = w.click(w.integ, &stop, "U_MEMBER", None).await;
    assert_eq!(out["result"], "done", "{out}");
    let ab: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT abort_requested_at FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert!(ab.is_some());
}

#[tokio::test]
async fn agent_link_decide() {
    let w = W::new(&FULL).await;
    let (a, b) = if w.agent < w.agent2 {
        (w.agent, w.agent2)
    } else {
        (w.agent2, w.agent)
    };
    let link = Uuid::now_v7();
    sqlx::query("INSERT INTO agent_links (id, agent_a_id, agent_b_id, policy_a, policy_b) VALUES ($1,$2,$3,'ask','ask')")
        .bind(link).bind(a).bind(b).execute(w.pool()).await.unwrap();
    let tok = w
        .minter(w.integ)
        .mint(
            link,
            ActionKind::AgentLinkDecide {
                side: aurora_protocol::vocab::BoundedString::new("a").unwrap(),
                decision: Decision::Approve,
            },
        )
        .unwrap();
    let out = w.click(w.integ, &tok, "U_STR", None).await;
    assert_eq!(out["reason"], "forbidden");
    let out = w.click(w.integ, &tok, "U_MEMBER", None).await;
    assert_eq!(out["result"], "done", "{out}");
    let p: String = sqlx::query_scalar("SELECT policy_a FROM agent_links WHERE id=$1")
        .bind(link)
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(p, "allow");
}

#[tokio::test]
async fn without_buttons_or_edits_cards_fall_back() {
    // threads + commands only: no buttons, no edits.
    let w = W::new(&["threads", "commands"]).await;
    let id = w.approval(None).await;
    w.fanout().await;
    w.deliver().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    assert!(W::tokens(&op).is_empty(), "no buttons");
    let text = op["content"]["text"].as_str().unwrap();
    let code = cards::approval_code(id);
    assert!(text.contains("/approvals"), "{text}");
    assert!(
        text.contains(&format!("approve {code}")) && text.contains(&format!("deny {code}")),
        "{text}"
    );
    // Decided elsewhere: settled by a new message, not an edit.
    sqlx::query("UPDATE approvals SET status='approved', decided_via='dashboard', decided_at=now(), decided_by_user_id=$2 WHERE id=$1")
        .bind(id).bind(w.owner).execute(w.pool()).await.unwrap();
    w.fanout().await;
    assert!(w.ops("message.update").await.is_empty());
    let posts = w.ops("message.post").await;
    assert_eq!(posts.len(), 2);
    assert_eq!(
        posts[1].1["target"],
        json!({"space_ref": "C1", "thread_ref": "T1"})
    );
    assert!(
        posts[1].1["content"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Approved by Olivia")
    );
    assert_eq!(
        posts[1].2.as_deref().map(|d| d.starts_with("settle:")),
        Some(true)
    );
}

#[test]
fn approval_code_is_eight_base32_chars_and_spreads_uuidv7() {
    let c = cards::approval_code(Uuid::nil());
    assert_eq!(c.len(), 8);
    assert!(
        c.bytes()
            .all(|b| b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".contains(&b))
    );
    // S-12: ids minted back to back (same timestamp prefix) get distinct codes.
    let codes: std::collections::HashSet<String> = (0..200)
        .map(|_| cards::approval_code(Uuid::now_v7()))
        .collect();
    assert_eq!(codes.len(), 200);
}

/// SEC-21/SEC-19: a suspended member is a non-member on the channel surface too: their
/// click on a valid card is refused, decides nothing and burns no token; reinstating
/// them makes the same token work.
#[tokio::test]
async fn suspended_member_click_is_refused_everywhere() {
    let w = W::new(&FULL).await;
    let (id, op) = w.approval_card().await;
    let tok = W::token(&op, "Approve");
    w.h.set_member_status(w.org, w.member, "suspended").await;
    let out = w
        .click_in(w.integ, &tok, "U_MEMBER", None, Some("C1"))
        .await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("forbidden")),
        "{out}"
    );
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.token_uses().await, 0);
    w.h.set_member_status(w.org, w.member, "active").await;
    assert_eq!(
        w.click(w.integ, &tok, "U_MEMBER", None).await["result"],
        "done"
    );
    assert_eq!(w.status("approvals", id).await, "approved");
}

/// RFE-009 / SEC-43: a card for an approval naming a direct conversation can be acted on only by
/// that person or a workspace owner; a plain member who can otherwise use the workspace is
/// refused before the token is consumed.
#[tokio::test]
async fn dm_approval_card_click_by_non_participant_is_refused() {
    let w = W::new(&FULL).await;
    let (id, op) = w.approval_card().await;
    // The conversation turns out to be a direct conversation of a third person.
    let other = w.h.user("dm@x.io", false).await;
    w.h.member(w.org, other, "member", "active").await;
    w.h.bind_ws(w.ws, other, "member").await;
    sqlx::query("UPDATE conversations SET kind = 'direct', direct_user_id = $2, external_ref = 'user:x' WHERE id = $1")
        .bind(w.conv)
        .bind(other)
        .execute(w.pool())
        .await
        .unwrap();
    let tok = W::token(&op, "Approve");
    let out = w
        .click_in(w.integ, &tok, "U_MEMBER", None, Some("C1"))
        .await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("forbidden")),
        "{out}"
    );
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.token_uses().await, 0);
    // The owner (org owner = can_own) may act.
    let out = w.click_in(w.integ, &tok, "U_OWNER", None, Some("C1")).await;
    assert_eq!(out["result"].as_str(), Some("done"), "{out}");
    assert_eq!(w.status("approvals", id).await, "approved");
}

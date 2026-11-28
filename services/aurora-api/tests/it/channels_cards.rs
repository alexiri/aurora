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
    outsider: Uuid,
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
        // RFE-012: the clicking `member` of these fixtures is an approver.
        h.bind_ws(ws, member, "approver").await;
        // (FIX-234, D-477) The org owner's workspace reach is an own-right owner binding: an org
        // role grants no workspace right, and the card clicks act in the workspace.
        h.bind_ws(ws, owner, "owner").await;
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
            outsider,
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
        h.byo_agent(org, ws, handle, handle).await
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
                '{\"title\":\"Send **mail**\",\"fields\":[{\"label\":\"to\",\"value\":\"a@b.c\"}],\"text\":null,\"link\":null,\"group\":null,\"source\":\"template\"}',
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
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT status FROM {table} WHERE id=$1"
        )))
        .bind(id)
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    /// The status of a peer task, by its task conversation.
    async fn task_status(&self, task_conversation: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM peer_tasks WHERE task_conversation_id=$1")
            .bind(task_conversation)
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

    /// The platform lines sent privately to clickers, oldest first (D-470: every click is answered).
    async fn notice_lines(&self) -> Vec<String> {
        self.ops("message.post")
            .await
            .into_iter()
            .filter(|o| o.1["ephemeral_to"].is_string())
            .filter_map(|o| o.1["content"]["text"].as_str().map(str::to_owned))
            .collect()
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
    assert_eq!(labels, ["Allow once", "Block"]);
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
        .click(w.integ, &W::token(op, "Allow once"), "U_MEMBER", None)
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

/// All the text a card op carries, flattened (blocks, fields, context, fallback text).
fn card_text(op: &Value) -> String {
    op["content"].to_string()
}

/// D-178 (Q-R5-9): a card in a shared channel carries the title, agent, rule and expiry only; the
/// request's details are not shown to everyone in the channel. A 1:1 conversation keeps them.
#[tokio::test]
async fn shared_channel_approval_card_hides_request_details() {
    let w = W::new(&FULL).await;
    let id = w.approval(None).await;
    sqlx::query("UPDATE approvals SET rule_name='Mail needs a human' WHERE id=$1")
        .bind(id)
        .execute(w.pool())
        .await
        .unwrap();
    w.fanout().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let text = card_text(&op);
    assert!(text.contains("mail"), "{text}");
    assert!(text.contains("Agent: bot"), "{text}");
    assert!(text.contains("Mail needs a human"), "{text}");
    assert!(text.contains("Expires"), "{text}");
    for secret in ["a@b.c", "api.example.com", "\"fields\""] {
        assert!(!text.contains(secret), "shared card leaks {secret}: {text}");
    }
    let blocks = op["content"]["card"]["blocks"].as_array().unwrap();
    assert!(blocks.iter().all(|b| b["type"] != "fields"), "{blocks:?}");
    // Still decidable from the card, and the dashboard link is there.
    assert!(!W::tokens(&op).is_empty());
}

#[tokio::test]
async fn direct_conversation_approval_card_keeps_the_request_fields() {
    let w = W::new(&FULL).await;
    sqlx::query("UPDATE conversations SET kind='direct', direct_user_id=$1")
        .bind(w.member)
        .execute(w.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE thread_links SET kind='direct'")
        .execute(w.pool())
        .await
        .unwrap();
    w.approval(None).await;
    w.fanout().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let text = card_text(&op);
    assert!(text.contains("a@b.c"), "{text}");
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

/// Marks the approval as parked (the columns the gateway sets when it parks).
async fn make_parked(w: &W, id: Uuid) {
    sqlx::query(
        "UPDATE approvals SET parked=true, fp_salt='\\x0102'::bytea, fingerprint='\\x0304'::bytea \
         WHERE id=$1",
    )
    .bind(id)
    .execute(w.pool())
    .await
    .unwrap();
}

fn update_text(op: &Value) -> String {
    op["content"]["card"]["blocks"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// P4-011B-2 (C-825, `13 §5.6`): a parked approval's card carries the parked sentence and no
/// request detail in a shared space; it settles as "will be told"; when the grant ends unused or
/// revoked after that, the settled card is edited once more (never a second time).
#[tokio::test]
async fn a_parked_approval_card_says_so_and_is_edited_when_its_grant_ends() {
    let w = W::new(&FULL).await;
    let id = w.approval(None).await;
    make_parked(&w, id).await;
    w.fanout().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let text = card_text(&op);
    assert!(
        text.contains("The agent has stopped waiting and will resume when this is approved")
            && text.contains("valid for 30 min after approval"),
        "{text}"
    );
    assert!(!text.contains("api.example.com"), "{text}");
    w.deliver().await;
    // Approved: the card settles with "will be told".
    sqlx::query("UPDATE approvals SET status='approved', decided_via='dashboard', decided_at=now(), decided_by_user_id=$2, resume_state='ready', resume_expires_at=now()+interval '30 minutes' WHERE id=$1")
        .bind(id).bind(w.owner).execute(w.pool()).await.unwrap();
    w.fanout().await;
    let upd = w.ops("message.update").await;
    assert_eq!(upd.len(), 1);
    let t = update_text(&upd[0].1);
    assert!(
        t.starts_with("Approved by Olivia") && t.ends_with("— bot will be told."),
        "{t}"
    );
    // The window ends unused: the settled card is edited once.
    sqlx::query("UPDATE approvals SET resume_state='expired', resolved_at=now() WHERE id=$1")
        .bind(id)
        .execute(w.pool())
        .await
        .unwrap();
    w.fanout().await;
    w.fanout().await;
    let upd = w.ops("message.update").await;
    assert_eq!(upd.len(), 2, "one more edit, exactly once");
    assert!(update_text(&upd[1].1).starts_with("Approval window ended"));
}

#[tokio::test]
async fn a_revoked_grant_and_an_abandoned_parked_approval_settle_their_cards() {
    let w = W::new(&FULL).await;
    let id = w.approval(None).await;
    make_parked(&w, id).await;
    let id2 = w.approval(None).await;
    make_parked(&w, id2).await;
    w.fanout().await;
    w.deliver().await;
    sqlx::query("UPDATE approvals SET status='approved', decided_via='dashboard', decided_at=now(), decided_by_user_id=$2, resume_state='ready', resume_expires_at=now()+interval '30 minutes' WHERE id=$1")
        .bind(id).bind(w.owner).execute(w.pool()).await.unwrap();
    sqlx::query("UPDATE approvals SET status='abandoned', resolved_at=now() WHERE id=$1")
        .bind(id2)
        .execute(w.pool())
        .await
        .unwrap();
    w.fanout().await;
    let texts: Vec<String> = w
        .ops("message.update")
        .await
        .iter()
        .map(|(_, p, _)| update_text(p))
        .collect();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(texts.iter().any(|t| t == "No longer needed."), "{texts:?}");
    sqlx::query("UPDATE approvals SET resume_state='revoked', resolved_at=now() WHERE id=$1")
        .bind(id)
        .execute(w.pool())
        .await
        .unwrap();
    w.fanout().await;
    let texts: Vec<String> = w
        .ops("message.update")
        .await
        .iter()
        .map(|(_, p, _)| update_text(p))
        .collect();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert!(
        texts.iter().any(|t| t.starts_with("Revoked")),
        "the settled card is edited to Revoked: {texts:?}"
    );
}

#[tokio::test]
async fn unlinked_clicker_gets_a_link_prompt_and_nothing_happens() {
    let w = W::new(&FULL).await;
    let (id, op) = w.approval_card().await;
    let tok = W::token(&op, "Allow once");
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
    w.h.bind_ws(w.ws, u, "approver").await;
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
    let tok = W::token(&op, "Allow once");
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
    let tok = W::token(&op, "Allow once");
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
    let deny = W::token(&op, "Block");
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
    assert_eq!(
        labels,
        ["Allow once", "Block", "Allow all once", "Block all"]
    );
    let out = w
        .click(w.integ, &W::token(op, "Allow all once"), "U_MEMBER", None)
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

/// FIX-112 (CHAN-8): once a click has taught us the DM channel of a menu card, it is still the
/// addressee's menu: another linked user holding the token is refused.
#[tokio::test]
async fn learned_dm_menu_still_belongs_to_its_addressee() {
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
    // What a click does to a card DMed to a person: its space becomes the real DM channel.
    sqlx::query("UPDATE channel_cards SET space_ref='D_MEMBER', message_ref='m1', state='posted' WHERE subject_kind='binding'")
        .execute(w.pool())
        .await
        .unwrap();
    let tok = W::token(&op, "select");
    let v = w.agent2.to_string();
    let out = w.click(w.integ, &tok, "U_OWNER", Some(&v)).await;
    assert_eq!(out["reason"], "forbidden", "{out}");
    let set: i64 = sqlx::query_scalar("SELECT count(*) FROM dm_defaults")
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(set, 0);
    let out = w.click(w.integ, &tok, "U_MEMBER", Some(&v)).await;
    assert_eq!(out["result"], "done", "{out}");
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
    // (FIX-225, D-470) An old card's Send now on an agent that cannot do it is answered with a
    // visible line, queues nothing, and does not burn the token.
    for _ in 0..2 {
        let out = w
            .click_in(w.integ, &now, "U_MEMBER", None, Some("C1"))
            .await;
        assert_eq!(
            (out["result"].as_str(), out["reason"].as_str()),
            (Some("refused"), Some("unsupported")),
            "{out}"
        );
    }
    let sn: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT send_now_requested_at FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert!(sn.is_none());
    assert_eq!(w.token_uses().await, 0);
    assert!(
        w.notice_lines().await.iter().any(|l| l
            == "This agent can't take messages mid-turn; they run when the current turn ends."),
        "{:?}",
        w.notice_lines().await
    );
    let out = w
        .click_in(w.integ, &stop, "U_MEMBER", None, Some("C1"))
        .await;
    assert_eq!(out["result"], "done", "{out}");
    let ab: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT abort_requested_at FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert!(ab.is_some());
    assert!(
        w.notice_lines()
            .await
            .contains(&"Stopping this turn.".to_owned())
    );
}

/// (FIX-225, D-469(c), D-470) The status card offers Stop always, Steer only for a harness that
/// steers, Send now only for one that backgrounds a tool and does not steer.
#[tokio::test]
async fn the_turn_status_card_offers_only_the_controls_the_harness_supports() {
    use aurora_api::domains::channels::cards::build;
    use aurora_protocol::runner::TurnControls;
    let w = W::new(&FULL).await;
    let m = w.minter(w.integ);
    let labels = |steer, background_tool| -> Vec<String> {
        let b = build::turn_controls(
            &m,
            Uuid::now_v7(),
            Uuid::now_v7(),
            TurnControls {
                steer,
                background_tool,
            },
        )
        .unwrap();
        let v = serde_json::to_value(&b).unwrap();
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(labels(false, false), ["Stop"]);
    assert_eq!(labels(true, false), ["Stop", "Steer"]);
    assert_eq!(labels(false, true), ["Stop", "Send now"]);
    // A harness that steers uses Steer even when it could also background (`fake`).
    assert_eq!(labels(true, true), ["Stop", "Steer"]);
}

#[tokio::test]
async fn the_steer_button_steers_the_waiting_messages_and_says_so_and_says_when_there_are_none() {
    let w = W::new(&FULL).await;
    // A hosted agent on the harness that steers, a running turn and two waiting follow-ups.
    let agent = w.h.hosted_agent(w.org, w.ws, "steerer").await;
    let conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api',$1::text, now())")
        .bind(conv).bind(agent).bind(w.ws).execute(w.pool()).await.unwrap();
    let turn = Uuid::now_v7();
    sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text) VALUES ($1,$2,$3,'turn','running','{}','system','s','hi')")
        .bind(turn).bind(conv).bind(agent).execute(w.pool()).await.unwrap();
    let mut followups = Vec::new();
    for t in ["one", "two"] {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, parent_turn_id, door, speaker_kind, speaker_label, text) VALUES ($1,$2,$3,'followup','joining',$4,'{}','person','Mark',$5)")
            .bind(id).bind(conv).bind(agent).bind(turn).bind(t).execute(w.pool()).await.unwrap();
        followups.push(id);
    }
    let m = w.minter(w.integ);
    let steer = || {
        m.mint(turn, ActionKind::FollowupSteer { turn_id: turn })
            .unwrap()
    };
    // No access: refused, nothing steered.
    let out = w.click(w.integ, &steer(), "U_OUT", None).await;
    assert_eq!(out["reason"], "forbidden");
    let out = w
        .click_in(w.integ, &steer(), "U_MEMBER", None, Some("C1"))
        .await;
    assert_eq!(out["result"], "done", "{out}");
    let requested: Vec<bool> = sqlx::query_scalar(
        "SELECT steer_requested_at IS NOT NULL FROM turns WHERE id = ANY($1) ORDER BY created_at, id",
    )
    .bind(&followups)
    .fetch_all(w.pool())
    .await
    .unwrap();
    assert_eq!(requested, [true, true]);
    assert!(
        w.notice_lines()
            .await
            .contains(&"Steering 2 messages in at the next step.".to_owned()),
        "{:?}",
        w.notice_lines().await
    );
    // Nothing left to steer: a visible line, and the card stays usable (the token is not burned).
    let again = steer();
    let uses = w.token_uses().await;
    for _ in 0..2 {
        let out = w
            .click_in(w.integ, &again, "U_MEMBER", None, Some("C1"))
            .await;
        assert_eq!(
            (out["result"].as_str(), out["reason"].as_str()),
            (Some("refused"), Some("nothing_waiting")),
            "{out}"
        );
    }
    assert_eq!(w.token_uses().await, uses);
    assert!(
        w.notice_lines()
            .await
            .contains(&"No message is waiting to be steered.".to_owned())
    );
}

/// (FIX-225, D-470) A control that did not do what it promised (a steer the harness missed) is
/// posted to the conversation's chat thread as one short platform line, once per `dedupe`.
#[tokio::test]
async fn a_failed_control_is_posted_to_the_chat_thread_as_a_platform_note_once() {
    use aurora_api::domains::channels::mirror;
    let w = W::new(&FULL).await;
    let text =
        "Couldn't steer your message in (the agent missed it). It will run after the current turn.";
    for _ in 0..2 {
        let mut conn = w.pool().acquire().await.unwrap();
        mirror::post_platform_note(&mut conn, w.conv, "steer-failed:one", text).await;
    }
    let posts: Vec<_> = w
        .ops("message.post")
        .await
        .into_iter()
        .filter(|o| o.1["content"]["text"] == text)
        .collect();
    assert_eq!(posts.len(), 1, "one note per dedupe key");
    assert_eq!(posts[0].1["target"]["thread_ref"], "T1");
    // A conversation without a chat thread posts nothing and fails nothing.
    let mut conn = w.pool().acquire().await.unwrap();
    mirror::post_platform_note(&mut conn, Uuid::now_v7(), "steer-failed:two", text).await;
    assert_eq!(
        w.ops("message.post")
            .await
            .into_iter()
            .filter(|o| o.1["content"]["text"] == text)
            .count(),
        1
    );
}

#[tokio::test]
async fn the_steer_button_on_an_agent_that_cannot_steer_says_so() {
    let w = W::new(&FULL).await;
    // The fixture's agent is bring-your-own: its harness is not ours and takes nothing mid-turn.
    let turn = Uuid::now_v7();
    sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text) VALUES ($1,$2,$3,'turn','running','{}','system','s','hi')")
        .bind(turn).bind(w.conv).bind(w.agent).execute(w.pool()).await.unwrap();
    let tok = w
        .minter(w.integ)
        .mint(turn, ActionKind::FollowupSteer { turn_id: turn })
        .unwrap();
    let out = w
        .click_in(w.integ, &tok, "U_MEMBER", None, Some("C1"))
        .await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("unsupported")),
        "{out}"
    );
    assert!(w.notice_lines().await.contains(
        &"This agent can't take messages mid-turn; they run when the current turn ends.".to_owned()
    ));
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
    let tok = W::token(&op, "Allow once");
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

/// RFE-012 / SEC-28: a linked plain member who clicks an approval card is refused with the
/// ephemeral, the token is not consumed and the card stays pending; once promoted the same
/// click works.
#[tokio::test]
async fn member_click_on_approval_is_refused_and_card_stays_pending() {
    let w = W::new(&FULL).await;
    let (id, op) = w.approval_card().await;
    let tok = W::token(&op, "Allow once");
    let ctx = op["content"]["card"]["blocks"].to_string();
    assert!(ctx.contains("Needs an approver"), "{ctx}");
    sqlx::query(
        "UPDATE workspace_access SET role = 'member' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(w.ws)
    .bind(w.member)
    .execute(w.pool())
    .await
    .unwrap();
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
    let told = w.ops("message.post").await;
    assert!(
        told.iter()
            .any(|(_, p, _)| p.to_string().contains("Only approvers can decide this.")),
        "{told:?}"
    );
    sqlx::query(
        "UPDATE workspace_access SET role = 'approver' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(w.ws)
    .bind(w.member)
    .execute(w.pool())
    .await
    .unwrap();
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
    let tok = W::token(&op, "Allow once");
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

// ---------------------------------------------------------------- FIX-252: the origin card

impl W {
    /// (FIX-252) An approval raised in a peer task conversation: `task` (the receiver's `peer`
    /// conversation) has no thread link; `origin` (where the person asked) has one of `origin_kind`.
    /// Delegating agent: `agent2`; receiving agent (the approval's): `agent`; `requester` is the
    /// person who asked. Returns `(approval id, task conversation, origin conversation)`.
    async fn peer_approval(&self, origin_kind: &str, requester: Uuid) -> (Uuid, Uuid, Uuid) {
        let p = self.pool();
        let origin = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
             VALUES ($1,$2,$3,'api',$1::text, now())",
        )
        .bind(origin)
        .bind(self.agent2)
        .bind(self.ws)
        .execute(p)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO thread_links (id, integration_id, agent_id, conversation_id, kind, space_ref, thread_ref, sender)
             VALUES ($1,$2,$3,$4,$5,'C9','T9','\"shared\"')",
        )
        .bind(Uuid::now_v7())
        .bind(self.integ)
        .bind(self.agent2)
        .bind(origin)
        .bind(origin_kind)
        .execute(p)
        .await
        .unwrap();
        let task = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
             VALUES ($1,$2,$3,'peer',$1::text, now())",
        )
        .bind(task)
        .bind(self.agent)
        .bind(self.ws)
        .execute(p)
        .await
        .unwrap();
        let (a, b) = if self.agent < self.agent2 {
            (self.agent, self.agent2)
        } else {
            (self.agent2, self.agent)
        };
        let link = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agent_links (id, agent_a_id, agent_b_id, policy_a, policy_b)
             VALUES ($1,$2,$3,'allow','allow')",
        )
        .bind(link)
        .bind(a)
        .bind(b)
        .execute(p)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO peer_tasks (id, link_id, from_agent_id, to_agent_id, requester_user_id,
                origin_conversation_id, task_conversation_id, status, message_budget, expires_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,'open',20, now() + interval '1 day')",
        )
        .bind(Uuid::now_v7())
        .bind(link)
        .bind(self.agent2)
        .bind(self.agent)
        .bind(requester)
        .bind(origin)
        .bind(task)
        .execute(p)
        .await
        .unwrap();
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, gateway_instance_id,
                method, host, path, summary, status, deadline)
             VALUES ($1,$2,$3,$4,$5,$6,'POST','api.example.com','/x',
                '{\"title\":\"Push to GitLab\",\"fields\":[{\"label\":\"repo\",\"value\":\"x/y\"}],\"source\":\"generic\"}',
                'pending', now() + interval '5 minutes')",
        )
        .bind(id)
        .bind(self.org)
        .bind(self.ws)
        .bind(self.agent)
        .bind(task)
        .bind(self.gw)
        .execute(p)
        .await
        .unwrap();
        (id, task, origin)
    }
}

impl W {
    /// (FIX-288) Puts a task above the one `peer_approval` made: its `origin` becomes the
    /// conversation of a task `requester` also asked for, and the thread link moves up to the new
    /// first conversation, which is returned.
    async fn add_hop_above(&self, origin: Uuid, requester: Uuid) -> Uuid {
        let p = self.pool();
        let root = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
             VALUES ($1,$2,$3,'api',$1::text, now())",
        )
        .bind(root)
        .bind(self.agent)
        .bind(self.ws)
        .execute(p)
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET kind = 'peer' WHERE id = $1")
            .bind(origin)
            .execute(p)
            .await
            .unwrap();
        sqlx::query("UPDATE thread_links SET conversation_id = $2, agent_id = $3 WHERE conversation_id = $1")
            .bind(origin)
            .bind(root)
            .bind(self.agent)
            .execute(p)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO peer_tasks (id, link_id, from_agent_id, to_agent_id, requester_user_id,
                origin_conversation_id, task_conversation_id, status, message_budget, expires_at)
             SELECT $1, id, $2, $3, $4, $5, $6, 'open', 20, now() + interval '1 day'
               FROM agent_links LIMIT 1",
        )
        .bind(Uuid::now_v7())
        .bind(self.agent)
        .bind(self.agent2)
        .bind(requester)
        .bind(root)
        .bind(origin)
        .execute(p)
        .await
        .unwrap();
        root
    }
}

/// (FIX-288) An approval two levels down posts one card in the thread of the first conversation of
/// the chain, with D-489's buttons: Approve and Deny for a requester who may decide in the
/// approval's workspace, otherwise Ask an approver and Cancel the task. D-178 follows that
/// thread's kind.
#[tokio::test]
async fn peer_approval_two_levels_down_cards_the_persons_thread_with_their_buttons() {
    for (may_decide, labels) in [
        (true, ["Approve", "Deny"]),
        (false, ["Ask an approver", "Cancel the task"]),
    ] {
        let w = W::new(&FULL).await;
        let requester = if may_decide { w.member } else { w.outsider };
        let (id, _task, origin) = w.peer_approval("group", requester).await;
        w.add_hop_above(origin, requester).await;
        if !may_decide {
            // Can use the workspace of the first conversation, cannot approve there.
            sqlx::query(
                "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,'member')",
            )
            .bind(w.ws)
            .bind(requester)
            .execute(w.pool())
            .await
            .unwrap();
        }
        w.fanout().await;
        w.fanout().await;
        let posts = w.ops("message.post").await;
        assert_eq!(posts.len(), 1, "may_decide={may_decide}");
        let (_, op, dedupe) = &posts[0];
        assert_eq!(dedupe.as_deref(), Some(format!("approval:{id}").as_str()));
        assert_eq!(op["target"], json!({"space_ref": "C9", "thread_ref": "T9"}));
        let got: Vec<_> = W::tokens(op).into_iter().map(|t| t.0).collect();
        assert_eq!(got, labels);
        assert!(!serde_json::to_string(&op).unwrap().contains("x/y"));
    }
}

/// (FIX-288, D-509) A thread above the point where the requester changes gets no card.
#[tokio::test]
async fn peer_approval_card_does_not_pass_a_change_of_requester() {
    let w = W::new(&FULL).await;
    let (_id, _task, origin) = w.peer_approval("direct", w.outsider).await;
    w.add_hop_above(origin, w.member).await;
    w.fanout().await;
    assert_eq!(w.ops("message.post").await.len(), 0);
}

/// (FIX-252) An approval raised in a peer task conversation is surfaced at the origin thread the
/// person asked in — not at the peer conversation (which has no thread link of its own) — with the
/// delegating agent named. In a shared space the card carries no request details (D-178).
#[tokio::test]
async fn peer_approval_card_goes_to_the_origin_thread_and_names_the_delegating_agent() {
    let w = W::new(&FULL).await;
    let (id, _task, _origin) = w.peer_approval("group", w.member).await;
    w.fanout().await;
    w.fanout().await; // idempotent: still exactly one card
    let posts = w.ops("message.post").await;
    assert_eq!(posts.len(), 1);
    let (_, op, dedupe) = &posts[0];
    assert_eq!(dedupe.as_deref(), Some(format!("approval:{id}").as_str()));
    assert_eq!(op["target"], json!({"space_ref": "C9", "thread_ref": "T9"}));
    let blocks = op["content"]["card"]["blocks"].as_array().unwrap();
    assert_eq!(
        blocks[0],
        json!({"type": "header", "text": "Approval needed"})
    );
    let title = blocks[1]["text"].as_str().unwrap();
    assert!(
        title.contains("is waiting for your approval to send a POST request to api.example.com"),
        "{title}"
    );
    assert!(title.contains("(asked for by "), "{title}");
    // D-178: a shared origin shows no request fields. The requester (`member`) is an approver in
    // the approval's workspace, so the card offers Approve and Deny.
    assert!(W::tokens(op).iter().map(|t| &t.0).eq(["Approve", "Deny"]));
    assert!(!serde_json::to_string(&op).unwrap().contains("x/y"));
    let card: (String, String) =
        sqlx::query_as("SELECT subject_kind, state FROM channel_cards WHERE subject_id=$1")
            .bind(id)
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert_eq!(card, ("approval".into(), "posting".into()));
}

/// (FIX-252) A direct (1:1) origin conversation gets the request's details on the card (D-178).
#[tokio::test]
async fn peer_approval_card_in_a_direct_origin_carries_the_details() {
    let w = W::new(&FULL).await;
    w.peer_approval("direct", w.member).await;
    w.fanout().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let blocks = op["content"]["card"]["blocks"].as_array().unwrap();
    let fields = blocks
        .iter()
        .find(|b| b["type"] == "fields")
        .expect("fields on a direct card");
    assert!(serde_json::to_string(fields).unwrap().contains("x/y"));
}

/// (FIX-252) Without a thread link at the origin nothing is posted (the card waits for one).
#[tokio::test]
async fn peer_approval_without_an_origin_link_posts_nothing() {
    let w = W::new(&FULL).await;
    let (_id, _task, origin) = w.peer_approval("group", w.member).await;
    sqlx::query("DELETE FROM thread_links WHERE conversation_id = $1")
        .bind(origin)
        .execute(w.pool())
        .await
        .unwrap();
    w.fanout().await;
    assert_eq!(w.ops("message.post").await.len(), 0);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_cards")
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

/// (FIX-252) A peer approval whose origin link sits on a SECOND integration that is not active
/// posts nothing (the same rule as every card).
#[tokio::test]
async fn peer_approval_card_needs_an_active_integration() {
    let w = W::new(&FULL).await;
    w.peer_approval("group", w.member).await;
    sqlx::query("UPDATE channel_integrations SET status='disabled'")
        .execute(w.pool())
        .await
        .unwrap();
    w.fanout().await;
    assert_eq!(w.ops("message.post").await.len(), 0);
}

/// (FIX-252) A requester **with** `can_approve` at the approval's workspace decides from the
/// origin card (Approve); being the requester grants nothing beyond what they already had.
#[tokio::test]
async fn peer_requester_with_can_approve_decides_from_the_origin_card() {
    let w = W::new(&FULL).await;
    let (id, _task, _origin) = w.peer_approval("group", w.member).await;
    w.fanout().await;
    w.deliver().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let out = w
        .click(w.integ, &W::token(&op, "Approve"), "U_MEMBER", None)
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
}

/// (FIX-252) A requester **without** `can_approve` gets Ask an approver and Cancel the task
/// instead of Approve/Deny; deciding from the card is refused and grants nothing.
#[tokio::test]
async fn peer_requester_without_can_approve_gets_ask_an_approver_and_cancel() {
    let w = W::new(&FULL).await;
    let (id, task, _origin) = w.peer_approval("group", w.outsider).await;
    w.fanout().await;
    w.deliver().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let labels: Vec<_> = W::tokens(&op).into_iter().map(|t| t.0).collect();
    assert_eq!(labels, ["Ask an approver", "Cancel the task"]);
    let ctx = op["content"]["card"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "context")
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(ctx.contains("Ask an approver of"), "{ctx}");
    // The decide route still refuses a plain member (RFE-012): no right gained by the card. The
    // card shows no Approve button, so the token is minted directly.
    let tok = w
        .minter(w.integ)
        .mint(
            id,
            ActionKind::ApprovalDecide {
                decision: Decision::Approve,
            },
        )
        .unwrap();
    let out = w.click(w.integ, &tok, "U_OUT", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("forbidden")),
        "{out}"
    );
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.task_status(task).await, "open");
    assert_eq!(w.token_uses().await, 0);
}

/// (FIX-252) Cancel the task: the requester's click closes the task (its close line goes to the
/// origin) and the pending approval is abandoned; a different person's click is refused.
#[tokio::test]
async fn peer_cancel_the_task_is_the_requesters_only_and_abandons_the_approval() {
    let w = W::new(&FULL).await;
    let (id, task, _origin) = w.peer_approval("group", w.outsider).await;
    w.fanout().await;
    w.deliver().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let tok = W::token(&op, "Cancel the task");
    // Not the requester: refused, nothing burned.
    let out = w.click(w.integ, &tok, "U_MEMBER", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("forbidden")),
        "{out}"
    );
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.task_status(task).await, "open");
    // The requester cancels.
    let out = w.click(w.integ, &tok, "U_OUT", None).await;
    assert_eq!(out["result"], "done", "{out}");
    assert_eq!(w.task_status(task).await, "cancelled");
    assert_eq!(w.status("approvals", id).await, "abandoned");
    // The close line reached the origin conversation.
    let origin: Uuid = sqlx::query_scalar(
        "SELECT origin_conversation_id FROM peer_tasks WHERE task_conversation_id = $1",
    )
    .bind(task)
    .fetch_one(w.pool())
    .await
    .unwrap();
    let lines: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM transcript_events WHERE conversation_id = $1 AND type = 'notice'
          AND payload->>'code' = 'peer_task_closed' ORDER BY seq",
    )
    .bind(origin)
    .fetch_all(w.pool())
    .await
    .unwrap();
    assert_eq!(lines.len(), 1);
    // The origin card settles to the abandoned line.
    w.fanout().await;
    let upd = w.ops("message.update").await;
    assert!(
        upd.iter()
            .any(|(_, p, _)| serde_json::to_string(p).unwrap().contains("abandoned"))
    );
}

/// (FIX-252) Ask an approver: the click e-mails the people with `can_approve` in the approval's
/// workspace at once (claimed, so the notifier's delayed mail does not repeat it) and is audited.
#[tokio::test]
async fn peer_ask_an_approver_emails_the_deciders_once() {
    let w = W::new(&FULL).await;
    let (id, _task, _origin) = w.peer_approval("group", w.outsider).await;
    w.fanout().await;
    w.deliver().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let tok = W::token(&op, "Ask an approver");
    let out = w.click(w.integ, &tok, "U_OUT", None).await;
    assert_eq!(out["result"], "done", "{out}");
    let to: Vec<String> = w.h.mail.sent().into_iter().map(|m| m.to).collect();
    // Both people with `can_approve` at the approval's workspace: the owner binding and the
    // approver binding (D-477).
    assert_eq!(to, vec!["owner@x.io".to_owned(), "member@x.io".to_owned()]);
    assert!(w.h.mail.sent()[0].subject.contains("Approval needed"));
    let claimed: Option<String> =
        sqlx::query_scalar("SELECT delivery FROM approval_notifications WHERE approval_id = $1")
            .bind(id)
            .fetch_optional(w.pool())
            .await
            .unwrap();
    assert_eq!(claimed.as_deref(), Some("mail"));
    // Single use: the button cannot ask again.
    let out = w.click(w.integ, &tok, "U_OUT", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("already_used")),
        "{out}"
    );
    assert_eq!(w.h.mail.sent().len(), 2);
    // One row, on this approval: the refused second click writes none.
    w.h.wait_audit_rows("approval.ask_approver", Some(id), 1)
        .await;
    assert_eq!(w.h.audit_rows("approval.ask_approver", Some(id)).await, 1);
}

/// (FIX-252 review, D-178 Q-R5-9) The decided line keeps the approver's name out of a shared
/// origin space (a card there shows the facts only, no names); a direct origin conversation
/// keeps it.
#[tokio::test]
async fn peer_decision_line_shows_the_approvers_name_only_in_a_direct_origin() {
    for (kind, named) in [("group", false), ("direct", true)] {
        let w = W::new(&FULL).await;
        let (id, _task, origin) = w.peer_approval(kind, w.outsider).await;
        sqlx::query("UPDATE users SET display_name = 'Decider Person' WHERE id = $1")
            .bind(w.member)
            .execute(w.pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE approvals SET status='approved', decided_by_user_id=$2, \
             decided_via='dashboard', decided_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(w.member)
        .execute(w.pool())
        .await
        .unwrap();
        aurora_api::domains::approvals::transcript::write_current(&w.h.state, id)
            .await
            .unwrap();
        let lines: Vec<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM transcript_events WHERE conversation_id = $1 \
             AND type = 'notice' AND payload->>'code' = 'peer_approval_decided'",
        )
        .bind(origin)
        .fetch_all(w.pool())
        .await
        .unwrap();
        assert_eq!(lines.len(), 1, "{kind}");
        assert_eq!(
            lines[0].contains("Decider Person"),
            named,
            "{kind}: {}",
            lines[0]
        );
    }
}

/// (FIX-252 review) Ask an approver is the requester's button, like Cancel: any other linked
/// person's click is an ephemeral refusal and the single-use button stays usable.
#[tokio::test]
async fn peer_ask_an_approver_is_the_requesters_only() {
    let w = W::new(&FULL).await;
    let (id, _task, _origin) = w.peer_approval("group", w.outsider).await;
    w.fanout().await;
    w.deliver().await;
    let op = w.ops("message.post").await.pop().unwrap().1;
    let tok = W::token(&op, "Ask an approver");
    // Not the requester: refused, nothing burned, nobody e-mailed.
    let out = w.click(w.integ, &tok, "U_MEMBER", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("forbidden")),
        "{out}"
    );
    assert_eq!(w.h.mail.sent().len(), 0);
    assert_eq!(w.token_uses().await, 0);
    assert_eq!(w.status("approvals", id).await, "pending");
    // The requester still can ask (the button was not consumed).
    let out = w.click(w.integ, &tok, "U_OUT", None).await;
    assert_eq!(out["result"], "done", "{out}");
    assert_eq!(w.h.mail.sent().len(), 2);
}

/// (FIX-252) Ask an approver on a card that is not a peer-origin card is not valid.
#[tokio::test]
async fn peer_ask_an_approver_needs_a_peer_origin_card() {
    let w = W::new(&FULL).await;
    let (id, _op) = w.approval_card().await;
    // Mint an ask token bound to this (non-peer) approval and click it.
    let tok = w
        .minter(w.integ)
        .mint(id, aurora_channels::ActionKind::PeerTaskAskApprover)
        .unwrap();
    let out = w.click(w.integ, &tok, "U_MEMBER", None).await;
    assert_eq!(
        (out["result"].as_str(), out["reason"].as_str()),
        (Some("refused"), Some("invalid_token")),
        "{out}"
    );
    assert_eq!(w.status("approvals", id).await, "pending");
    assert_eq!(w.token_uses().await, 0);
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-10 (D-411, C-1085, SEC-142): people create agent links. Who may link which agents,
//! the starting policies, the direction and where it is enforced, and what a delete ends.

use crate::common::*;
use crate::peers_rig::{Person, Rig, rig};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

/// The rig plus people with every mix of rights on ws1 and ws2.
struct Fx {
    rig: Rig,
    /// Plain member of ws1 and of ws2.
    members: Person,
    /// Approver of ws1, plain member of ws2.
    mixed: Person,
    /// Approver of ws1 and ws2.
    approvers: Person,
    /// Organization admin, also an own-right owner of both workspaces.
    admin: Person,
}

async fn person(r: &Rig, email: &str, org_role: &str, binds: &[(Uuid, &str)]) -> Person {
    let id = r.h.user(email, false).await;
    r.h.member(r.org, id, org_role, "active").await;
    for (ws, role) in binds {
        r.h.bind_ws(*ws, id, role).await;
    }
    sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
        .bind(id)
        .bind(email.split('@').next().unwrap())
        .execute(r.h.pool())
        .await
        .unwrap();
    let (_, tok) = r.h.good_session(id).await;
    Person {
        id,
        cookie: cookie(&tok),
    }
}

async fn fx() -> Fx {
    let rig = rig().await;
    let (ws1, ws2) = (rig.ws1, rig.ws2);
    for (ws, name) in [(ws1, "Alpha team"), (ws2, "Bravo team")] {
        sqlx::query("UPDATE workspaces SET name = $2 WHERE id = $1")
            .bind(ws)
            .bind(name)
            .execute(rig.h.pool())
            .await
            .unwrap();
    }
    let members = person(
        &rig,
        "members@x.io",
        "member",
        &[(ws1, "member"), (ws2, "member")],
    )
    .await;
    let mixed = person(
        &rig,
        "mixed@x.io",
        "member",
        &[(ws1, "approver"), (ws2, "member")],
    )
    .await;
    let approvers = person(
        &rig,
        "approvers@x.io",
        "member",
        &[(ws1, "approver"), (ws2, "approver")],
    )
    .await;
    // (FIX-234, D-477) The org admin also holds own-right owner bindings on both workspaces:
    // linking is a can-use feature, so an org role alone no longer reaches the agents.
    let admin = person(
        &rig,
        "admin@x.io",
        "admin",
        &[(ws1, "owner"), (ws2, "owner")],
    )
    .await;
    Fx {
        rig,
        members,
        mixed,
        approvers,
        admin,
    }
}

impl Fx {
    async fn link(&self, who: &Person, agent: Uuid, peer: Uuid, direction: &str) -> (u16, Value) {
        self.rig
            .post(
                who,
                &format!("/v1/agents/{agent}/links"),
                Some(json!({"peer_agent_id": peer, "direction": direction})),
            )
            .await
    }

    async fn delete(&self, who: &Person, link: Uuid) -> u16 {
        let (s, _, _) = self
            .rig
            .h
            .send(
                Method::DELETE,
                &format!("/v1/agent-links/{link}"),
                &[("cookie", who.cookie.as_str()), ("origin", ORIGIN)],
                None,
            )
            .await;
        s.as_u16()
    }

    async fn direction(
        &self,
        who: &Person,
        link: &Value,
        direction: &str,
        revision: i64,
    ) -> (u16, Value) {
        self.rig
            .put(
                who,
                &format!("/v1/agent-links/{}/direction", link["id"].as_str().unwrap()),
                json!({"agent_id": link["self"]["agent_id"], "direction": direction,
                    "revision": revision}),
            )
            .await
    }

    async fn rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM agent_links")
            .fetch_one(self.rig.h.pool())
            .await
            .unwrap()
    }

    async fn audit(&self, action: &str, link: Uuid) -> Vec<Value> {
        sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = $1 AND target_id = $2 AND critical
             ORDER BY created_at",
        )
        .bind(action)
        .bind(link.to_string())
        .fetch_all(self.rig.h.pool())
        .await
        .unwrap()
    }

    async fn agent_in(&self, org: Uuid, ws: Uuid, handle: &str, kind: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
             VALUES ($1,$2,$3,$4,$5,$6, CASE WHEN $6 = 'hosted' THEN 'fake' END)",
        )
        .bind(id)
        .bind(org)
        .bind(ws)
        .bind(handle)
        .bind(format!("Agent {handle}"))
        .bind(kind)
        .execute(self.rig.h.pool())
        .await
        .unwrap();
        id
    }

    /// `agent` sends `message_agent` to `@handle` in a fresh conversation where `by` speaks.
    async fn message(
        &self,
        agent: Uuid,
        by: Uuid,
        handle: &str,
    ) -> aurora_protocol::tools::ToolResult {
        let (conv, turn) = self.rig.person_turn(agent, by).await;
        self.rig
            .tool(
                agent,
                conv,
                Some(turn),
                "message_agent",
                json!({"agent": format!("@{handle}"), "text": "please do the thing"}),
            )
            .await
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(self.rig.h.pool())
            .await
            .unwrap()
    }
}

fn id(v: &Value) -> Uuid {
    Rig::id(&v["id"])
}

#[tokio::test]
async fn a_person_links_two_agents_and_each_side_starts_by_the_creators_approval_rights() {
    let f = fx().await;
    let r = &f.rig;
    // (creator, agent in ws1, agent in ws2, ws1 side, ws2 side)
    for (who, h1, h2, want1, want2) in [
        (&f.approvers, "daneel", "rex", "allow", "allow"),
        (&f.mixed, "daneel2", "rex2", "allow", "ask"),
        (&f.members, "daneel3", "rex3", "ask", "ask"),
    ] {
        let a = r.agent(r.ws1, h1).await;
        let b = r.agent(r.ws2, h2).await;
        let (s, v) = f.link(who, a, b, "both").await;
        assert_eq!(s, 201, "{v}");
        assert_eq!(v["self"]["agent_id"], json!(a));
        assert_eq!(v["self"]["policy"], want1, "{h1}");
        assert_eq!(v["peer"]["agent_id"], json!(b));
        assert_eq!(v["peer"]["policy"], want2, "{h2}");
        assert_eq!(v["peer"]["workspace_name"], "Bravo team");
        assert_eq!(v["peer"]["handle"], h2);
        assert_eq!(v["direction"], "both");
        assert_eq!(v["created_by"]["kind"], "person");
        let name = v["created_by"]["display_name"].as_str().unwrap();
        assert!(["approvers", "mixed", "members"].contains(&name), "{name}");
        // Seen from the other agent: same link, sides swapped.
        let (s, l) = r.get(who, &format!("/v1/agents/{b}/links")).await;
        assert_eq!(s, 200);
        let other = &l["items"][0];
        assert_eq!(other["id"], v["id"]);
        assert_eq!(other["self"]["policy"], want2);
        assert_eq!(other["peer"]["policy"], want1);
        assert_eq!(other["peer"]["workspace_name"], "Alpha team");
        // The creator is stored, and the change is a critical audit row.
        let creator: Option<Uuid> =
            sqlx::query_scalar("SELECT created_by_user_id FROM agent_links WHERE id = $1")
                .bind(id(&v))
                .fetch_one(r.h.pool())
                .await
                .unwrap();
        assert_eq!(creator, Some(who.id));
        let rows = f.audit("agent_link.create", id(&v)).await;
        assert_eq!(rows.len(), 1);
        let m = &rows[0];
        assert_eq!(m["via"], "person");
        assert_eq!(m["direction"], "both");
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        assert_eq!(
            (m["agent_a_id"].clone(), m["agent_b_id"].clone()),
            (json!(lo), json!(hi))
        );
        let (pa, pb) = if a < b {
            (want1, want2)
        } else {
            (want2, want1)
        };
        assert_eq!(
            (m["policy_a"].as_str(), m["policy_b"].as_str()),
            (Some(pa), Some(pb))
        );
    }
}

#[tokio::test]
async fn linking_again_is_idempotent_and_another_direction_is_link_exists() {
    let f = fx().await;
    let r = &f.rig;
    let a = r.agent(r.ws1, "daneel").await;
    let b = r.agent(r.ws2, "rex").await;
    let (s, v) = f.link(&f.approvers, a, b, "to_peer").await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["direction"], "outgoing");
    // The same link asked from the other side with the same meaning is the same link.
    let (s, again) = f.link(&f.approvers, b, a, "from_peer").await;
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["id"], v["id"]);
    assert_eq!(again["direction"], "incoming");
    let (s, e) = f.link(&f.approvers, a, b, "both").await;
    assert_eq!(s, 409, "{e}");
    assert_eq!(e["error"]["code"], "link_exists");
    assert_eq!(e["error"]["details"]["link_id"], v["id"]);
    assert_eq!(f.rows().await, 1);
    assert_eq!(f.audit("agent_link.create", id(&v)).await.len(), 1);
}

#[tokio::test]
async fn sec142_refusals_write_no_row() {
    let f = fx().await;
    let r = &f.rig;
    let a = r.agent(r.ws1, "daneel").await;
    let b = r.agent(r.ws2, "rex").await;
    let other_org = r.h.org().await;
    let other_ws = r.h.workspace(other_org).await;
    let foreign = f.agent_in(other_org, other_ws, "stranger", "hosted").await;
    let byo = f.agent_in(r.org, r.ws2, "outside", "byo").await;
    let frozen = r.agent(r.ws2, "sleepy").await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 't', frozen_authority = 'org_admin'
         WHERE id = $1",
    )
    .bind(frozen)
    .execute(r.h.pool())
    .await
    .unwrap();

    let cases: [(&Person, Uuid, Value, u16, &str); 8] = [
        // can_use on one side only (P: approver of ws1): the other agent is a 404.
        (
            &r.p,
            a,
            json!({"peer_agent_id": b, "direction": "both"}),
            404,
            "not_found",
        ),
        // no side (R: no workspace): the agent itself is a 404.
        (
            &r.r,
            a,
            json!({"peer_agent_id": b, "direction": "both"}),
            404,
            "not_found",
        ),
        // another organization: 404, the same answer as "no such agent".
        (
            &f.admin,
            a,
            json!({"peer_agent_id": foreign, "direction": "both"}),
            404,
            "not_found",
        ),
        (
            &f.admin,
            a,
            json!({"peer_agent_id": Uuid::now_v7(), "direction": "both"}),
            404,
            "not_found",
        ),
        (
            &f.approvers,
            a,
            json!({"peer_agent_id": byo, "direction": "both"}),
            409,
            "not_hosted",
        ),
        (
            &f.approvers,
            a,
            json!({"peer_agent_id": frozen, "direction": "both"}),
            423,
            "frozen",
        ),
        (
            &f.approvers,
            a,
            json!({"peer_agent_id": a, "direction": "both"}),
            409,
            "link_self",
        ),
        (
            &f.approvers,
            a,
            json!({"peer_agent_id": b, "direction": "sideways"}),
            400,
            "",
        ),
    ];
    for (who, agent, body, status, code) in cases {
        let (s, v) = r
            .post(
                who,
                &format!("/v1/agents/{agent}/links"),
                Some(body.clone()),
            )
            .await;
        assert_eq!(s, status, "{body} -> {v}");
        if !code.is_empty() {
            assert_eq!(v["error"]["code"], code, "{body} -> {v}");
        }
        assert_eq!(f.rows().await, 0, "{body} wrote a row");
    }
    // The 404 for another organization's agent says nothing about it.
    let (_, v) = f.link(&f.admin, a, foreign, "both").await;
    let text = v.to_string();
    assert!(
        !text.contains("stranger") && !text.contains(&foreign.to_string()),
        "{text}"
    );
    assert_eq!(
        f.count("SELECT count(*) FROM audit_log WHERE action = 'agent_link.create'")
            .await,
        0
    );
}

#[tokio::test]
async fn linkable_lists_only_agents_of_workspaces_the_caller_can_use() {
    let f = fx().await;
    let r = &f.rig;
    let a = r.agent(r.ws1, "daneel").await;
    let a2 = r.agent(r.ws1, "anna").await;
    let b = r.agent(r.ws2, "rex").await;
    let byo = f.agent_in(r.org, r.ws2, "outside", "byo").await;
    let frozen = r.agent(r.ws2, "sleepy").await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 't', frozen_authority = 'org_admin'
         WHERE id = $1",
    )
    .bind(frozen)
    .execute(r.h.pool())
    .await
    .unwrap();
    let other_org = r.h.org().await;
    let other_ws = r.h.workspace(other_org).await;
    let foreign = f.agent_in(other_org, other_ws, "stranger", "hosted").await;

    // P uses ws1 only: ws1's other agent, nothing of ws2 or the other organization.
    let (s, v) = r.get(&r.p, &format!("/v1/agents/{a}/linkable")).await;
    assert_eq!(s, 200, "{v}");
    let ids: Vec<Value> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["agent_id"].clone())
        .collect();
    assert_eq!(ids, vec![json!(a2)]);
    let text = v.to_string();
    for hidden in [b, byo, frozen, foreign] {
        assert!(!text.contains(&hidden.to_string()), "{text}");
    }
    let ws2 = r.ws2.to_string();
    for word in [
        "rex",
        "outside",
        "sleepy",
        "stranger",
        "Bravo team",
        ws2.as_str(),
    ] {
        assert!(!text.contains(word), "{word} in {text}");
    }
    // An agent the caller cannot use: 404.
    let (s, _) = r.get(&r.p, &format!("/v1/agents/{b}/linkable")).await;
    assert_eq!(s, 404);

    // An organization admin sees every workspace of the organization, sorted by workspace name
    // then agent name; BYO and frozen agents are listed with their flags; not the agent itself.
    f.link(&f.admin, a, b, "both").await;
    let (s, v) = r.get(&f.admin, &format!("/v1/agents/{a}/linkable")).await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["Agent anna", "Agent outside", "Agent rex", "Agent sleepy"]
    );
    for i in items {
        let mut keys: Vec<&str> = i.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "agent_id",
                "frozen",
                "handle",
                "hosted",
                "linked",
                "name",
                "workspace_id",
                "workspace_name"
            ],
            "nothing else about another agent"
        );
        let id = Rig::id(&i["agent_id"]);
        assert_eq!(i["linked"], json!(id == b), "{i}");
        assert_eq!(i["hosted"], json!(id != byo), "{i}");
        assert_eq!(i["frozen"], json!(id == frozen), "{i}");
    }
    assert_eq!(items[1]["workspace_name"], "Bravo team");
    assert!(!v.to_string().contains("stranger"));
}

#[tokio::test]
async fn a_one_way_link_refuses_an_opening_against_it_before_any_approval() {
    let f = fx().await;
    let r = &f.rig;
    let a = r.agent(r.ws1, "daneel").await;
    let b = r.agent(r.ws2, "rex").await;
    // `mixed` approves ws1 only, so rex's side starts `ask`: an opening would need an approval.
    let (s, link) = f.link(&f.mixed, a, b, "to_peer").await;
    assert_eq!(s, 201, "{link}");
    assert_eq!(link["peer"]["policy"], "ask");

    // Rex (Q speaks for it) cannot hand Daneel a task: refused, no approval, no task.
    let res = f.message(b, r.q.id, "daneel").await;
    let msg = Rig::err(&res);
    assert!(
        msg.contains(
            "This link is one-way: Agent rex (@rex) only works on tasks that Agent daneel \
             (@daneel) sends it, so you cannot hand Agent daneel (@daneel) a task."
        ),
        "{msg}"
    );
    assert_eq!(f.count("SELECT count(*) FROM action_approvals").await, 0);
    assert_eq!(f.count("SELECT count(*) FROM peer_tasks").await, 0);

    // Daneel's way works: rex's `ask` side gets its approval.
    let res = f.message(a, r.p.id, "rex").await;
    assert_eq!(Rig::ok(&res)["status"], "pending_approval");

    // `from_peer` is the reverse; `both` works both ways (allow/allow: tasks open at once).
    let c = r.agent(r.ws1, "carol").await;
    let d = r.agent(r.ws2, "dave").await;
    let (s, l2) = f.link(&f.approvers, c, d, "from_peer").await;
    assert_eq!(s, 201, "{l2}");
    assert_eq!(l2["direction"], "incoming");
    let msg = Rig::err(&f.message(c, r.p.id, "dave").await);
    assert!(
        msg.contains("only works on tasks that Agent dave (@dave) sends it"),
        "{msg}"
    );
    assert_eq!(
        Rig::ok(&f.message(d, r.q.id, "carol").await)["status"],
        "sent"
    );
    let (s, l2) = f
        .direction(&f.approvers, &l2, "both", l2["revision"].as_i64().unwrap())
        .await;
    assert_eq!(s, 200, "{l2}");
    assert_eq!(
        Rig::ok(&f.message(c, r.p.id, "dave").await)["status"],
        "sent"
    );
}

#[tokio::test]
async fn an_approval_decided_after_the_direction_narrowed_fails() {
    let f = fx().await;
    let r = &f.rig;
    let a = r.agent(r.ws1, "daneel").await;
    let b = r.agent(r.ws2, "rex").await;
    let (_, link) = f.link(&f.mixed, a, b, "both").await;
    let v = Rig::ok(&f.message(a, r.p.id, "rex").await);
    assert_eq!(v["status"], "pending_approval");
    let approval = Rig::id(&v["approval_id"]);
    // Narrow to "Rex hands tasks to Daneel" (from Daneel's side: from_peer).
    let (s, _) = f
        .direction(
            &f.mixed,
            &link,
            "from_peer",
            link["revision"].as_i64().unwrap(),
        )
        .await;
    assert_eq!(s, 200);
    let (s, d) = r.decide(&r.q, approval, "approve").await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["status"], "failed", "{d}");
    assert_eq!(f.count("SELECT count(*) FROM peer_tasks").await, 0);
}

#[tokio::test]
async fn widening_needs_an_approver_of_both_sides_and_narrowing_any_user() {
    let f = fx().await;
    let r = &f.rig;
    let a = r.agent(r.ws1, "daneel").await;
    let b = r.agent(r.ws2, "rex").await;
    let (_, link) = f.link(&f.members, a, b, "to_peer").await;
    let rev = link["revision"].as_i64().unwrap();
    // Widening: plain members and an approver of one side only are refused.
    for who in [&f.members, &f.mixed] {
        let (s, e) = f.direction(who, &link, "both", rev).await;
        assert_eq!(s, 403, "{e}");
        assert_eq!(e["error"]["code"], "approver_required");
    }
    // Flipping the way round is a widening too.
    let (s, _) = f.direction(&f.members, &link, "from_peer", rev).await;
    assert_eq!(s, 403);
    // No access to the asked side: 404.
    let (s, _) = f.direction(&r.r, &link, "both", rev).await;
    assert_eq!(s, 404);
    let (s, wide) = f.direction(&f.approvers, &link, "both", rev).await;
    assert_eq!(s, 200, "{wide}");
    assert_eq!(wide["direction"], "both");
    let rev2 = wide["revision"].as_i64().unwrap();
    assert_eq!(rev2, rev + 1);
    // A stale revision is 409.
    let (s, e) = f.direction(&f.members, &link, "to_peer", rev).await;
    assert_eq!(s, 409, "{e}");
    assert_eq!(e["error"]["code"], "stale_revision");
    // Narrowing: a plain member of the asked side may.
    let (s, narrow) = f.direction(&f.members, &wide, "from_peer", rev2).await;
    assert_eq!(s, 200, "{narrow}");
    assert_eq!(narrow["direction"], "incoming");
    let rows = f.audit("agent_link.direction", id(&link)).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0]["to"].as_str(), rows[1]["to"].as_str()),
        (Some("both"), narrow_stored(a, b))
    );
    // The same direction is a no-op.
    let rev3 = narrow["revision"].as_i64().unwrap();
    let (s, same) = f.direction(&r.p, &narrow, "from_peer", rev3).await;
    assert_eq!(s, 200, "{same}");
    assert_eq!(same["revision"], json!(rev3));
    assert_eq!(f.audit("agent_link.direction", id(&link)).await.len(), 2);
}

/// Stored form of "Rex hands tasks to Daneel" for the pair (daneel = a, rex = b).
fn narrow_stored(a: Uuid, b: Uuid) -> Option<&'static str> {
    Some(if b < a { "a_to_b" } else { "b_to_a" })
}

#[tokio::test]
async fn delete_cancels_the_pairs_tasks_expires_its_approvals_and_an_agent_can_link_again() {
    let f = fx().await;
    let r = &f.rig;
    let a = r.agent(r.ws1, "daneel").await;
    let b = r.agent(r.ws2, "rex").await;
    let (_, link) = f.link(&f.approvers, a, b, "both").await;
    let lid = id(&link);
    // One open and one queued task from Daneel; their origin conversations get the close line.
    let (conv, turn) = r.person_turn(a, r.p.id).await;
    let mut tasks = Vec::new();
    for want in ["sent", "queued"] {
        let v = Rig::ok(
            &r.tool(
                a,
                conv,
                Some(turn),
                "message_agent",
                json!({"agent": "@rex", "text": "go"}),
            )
            .await,
        );
        assert_eq!(v["status"], want, "{v}");
        tasks.push(Rig::id(&v["peer_task_id"]));
    }
    // A pending approval: Rex's side back to `ask`, then Rex asks.
    let (s, _) = r
        .put(
            &r.q,
            &format!("/v1/agent-links/{lid}/policy"),
            json!({"agent_id": b, "policy": "ask", "revision": link["revision"]}),
        )
        .await;
    assert_eq!(s, 200);
    let (bconv, bturn) = r.person_turn(b, r.q.id).await;
    let v = Rig::ok(
        &r.tool(
            b,
            bconv,
            Some(bturn),
            "message_agent",
            json!({"agent": "@daneel", "text": "hi"}),
        )
        .await,
    );
    assert_eq!(v["status"], "pending_approval", "{v}");
    let approval = Rig::id(&v["approval_id"]);

    // Nobody who can use neither side can delete; P (ws1 only) can.
    assert_eq!(f.delete(&r.r, lid).await, 404);
    assert_eq!(f.delete(&r.p, lid).await, 204);
    assert_eq!(f.rows().await, 0);
    assert_eq!(
        f.count("SELECT count(*) FROM peer_tasks").await,
        0,
        "cascade"
    );
    let closed = r.notices(conv, "peer_task_closed").await;
    assert_eq!(closed.len(), 2, "{closed:?}");
    for n in &closed {
        assert_eq!(
            n["text"],
            "Task to Agent rex (@rex) was cancelled: the link between these agents was removed."
        );
        assert_eq!(n["status"], "cancelled");
    }
    let status: String = sqlx::query_scalar("SELECT status FROM action_approvals WHERE id = $1")
        .bind(approval)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(status, "expired");
    let ended = r.notices(bconv, "action_approval_expired").await;
    assert_eq!(ended.len(), 1);
    assert!(
        ended[0]["text"]
            .as_str()
            .unwrap()
            .ends_with("ended without a decision: the link between these agents was removed."),
        "{ended:?}"
    );
    let rows = f.audit("agent_link.delete", lid).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cancelled_tasks"], 2);
    assert_eq!(rows[0]["expired_approvals"], 1);
    assert_eq!(f.delete(&r.p, lid).await, 404);

    // The agent path is unchanged: a later message_agent creates the link again, by an agent.
    let v = Rig::ok(&f.message(a, r.p.id, "rex").await);
    assert_eq!(
        v["status"], "pending_approval",
        "cross-workspace links start ask"
    );
    let (s, l) = r.get(&r.p, &format!("/v1/agents/{a}/links")).await;
    assert_eq!(s, 200);
    let again = &l["items"][0];
    assert_ne!(again["id"], link["id"]);
    assert_eq!(
        again["created_by"],
        json!({"kind": "agent", "display_name": null})
    );
    assert_eq!(again["direction"], "both");
}

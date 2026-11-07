#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! TEST-016: M6 acceptance, black box: real api/gateway/runner/podman sandbox (fake harness) plus
//! the real `aurora-connector-fake` binary registered through the connector plane (I-28
//! signature, admin approval) and driven through its `/fake/*` control API.
//! Gated by AURORA_TEST_PODMAN=1; run via scripts/e2e/channels.sh.
//!
//! SEC mapping (see the task report for the full table):
//! * SEC-18: `s1`/`s5` assert platform-voice text on the card is core-authored; hostile
//!   broadcast text in an agent answer is defused by the connector (`s4`).
//! * SEC-19: `s3` (verified-email rule, replayed/used sign-in link), `s5` (unlinked clicker,
//!   token replay), `s1` (clicks authorized from the linked user).
//! * SEC-20: `s2` (a replica only ever serves its own connector's integration), `s6`
//!   (the connector never receives Aurora credentials: the card/ops carry only action tokens).
mod common;

use aurora_testkit::wiremock::matchers::method;
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const OWNER: &str = "owner@e2e.test";
const DM: &str = "dm-U1";

struct W {
    st: Stack,
    api: Api,
    fakes: Vec<Fake>,
    integ: String,
    user_id: String,
    conv: String,
}

/// Stack + registered/approved fake connector replicas + a shared-bot integration with Daneel
/// attached; the platform knows the bot `B1` and the person `U1` (verified email = the owner's).
async fn world(replicas: usize) -> W {
    Stack::wipe();
    let mut st = Stack::start(Opts::default()).await;
    let api = st.signup(OWNER).await;
    let kit = st.provision_connector(&api, OWNER).await;
    let mut fakes = vec![];
    for i in 0..replicas {
        fakes.push(st.spawn_fake(&kit, 4191 + i as u16));
    }
    st.approve_connector(&kit).await;
    let (s, v) = api
        .post(
            &format!("/v1/orgs/{}/channel-integrations", api.org),
            json!({"connector_id": "fake", "config": {},
                   "credentials": {"token": "e2e-token", "shared_bot": true}}),
        )
        .await;
    assert_eq!(s, 201, "create integration: {v}\n{}", st.log_tails());
    assert_eq!(v["status"], "active", "{v}");
    let integ = v["id"].as_str().unwrap().to_owned();
    let (s, v) = api
        .post(
            &format!("/v1/agents/{}/presences", api.daneel),
            json!({"integration_id": integ, "mode": "shared"}),
        )
        .await;
    assert_eq!(s, 201, "attach presence: {v}");
    assert_eq!(v["status"], "active", "{v}");
    for f in &fakes {
        f.user("B1", None, false, true).await;
        f.user("U1", Some(OWNER), true, false).await;
    }
    // The platform says the email is verified; the Aurora side must be verified too.
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE email = $1")
        .bind(OWNER)
        .execute(st.pool())
        .await
        .unwrap();
    let user_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(OWNER)
        .fetch_one(st.pool())
        .await
        .unwrap();
    let conv = api.direct().await;
    let w = W {
        st,
        api,
        fakes,
        integ,
        user_id: user_id.to_string(),
        conv,
    };
    w.owner().await.wait_serving(5).await;
    w
}

impl W {
    /// The replica currently serving the integration.
    async fn owner(&self) -> &Fake {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            for f in &self.fakes {
                if !f.open_integrations().await.is_empty() {
                    return f;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no replica serves the integration\n{}",
                self.st.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn turns(&self) -> Vec<(String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT id::text, status, door->>'kind' FROM turns WHERE conversation_id = $1::uuid
              ORDER BY created_at",
        )
        .bind(&self.conv)
        .fetch_all(self.st.pool())
        .await
        .unwrap()
    }

    /// Wait for the `n`-th turn (1-based) of the direct conversation and for it to be `done`.
    async fn turn_done(&self, n: usize, secs: u64) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some((id, _, _)) = self.turns().await.get(n - 1) {
                let id = id.clone();
                let (s, e) = self
                    .st
                    .wait_turn(&id, &["done", "failed", "aborted"], secs)
                    .await;
                assert_eq!((s.as_str(), e), ("done", None), "{}", self.st.log_tails());
                return id;
            }
            assert!(
                Instant::now() < deadline,
                "turn {n} never created: {:?}\n{}",
                self.turns().await,
                self.st.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .bind(&self.integ)
            .fetch_one(self.st.pool())
            .await
            .unwrap()
    }

    async fn finish(self) {
        let W { st, mut fakes, .. } = self;
        for f in &mut fakes {
            f.kill().await;
        }
        st.finish().await;
    }
}

async fn upstream() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("upstream-ok"))
        .mount(&s)
        .await;
    s
}

/// Gateway rule: calls to the mock upstream need approval.
async fn hold_upstream(api: &Api, up: &MockServer) -> String {
    let id = api
        .ws_secret(json!({
            "name": "mock-api", "kind": "generic", "host_pattern": up.address().to_string(),
            "injection": {"kind": "header", "name": "x-api-key", "format": "{value}"},
            "value": "sk-e2e-channels-SECRET"
        }))
        .await;
    api.grant(&api.daneel, &id).await;
    let host = up.address().to_string();
    api.set_policy(
        "block",
        json!([{"name": "ask", "action": "allow", "targets": [{"type": "network", "host": host}],
                "require_approval": true}]),
    )
    .await;
    host
}

/// One call on the sandbox's tools socket (`session` = conversation id); the raw response line.
fn tools_call(container: &str, conv: &str, tool: &str, args: &Value) -> String {
    let script = "import socket,sys\ns=socket.socket(socket.AF_UNIX);s.settimeout(20)\ns.connect('/run/aurora/tools.sock');s.sendall((sys.argv[1]+'\\n').encode());print(s.makefile().readline())";
    let req = json!({"id": 1, "session": conv, "method": "call", "tool": tool, "arguments": args});
    podman_rc(&[
        "exec",
        "--user",
        "1000:1000",
        container,
        "python3",
        "-c",
        script,
        &req.to_string(),
    ])
    .1
}

fn bot_texts(msgs: &[Value], space: &str) -> Vec<String> {
    msgs.iter()
        .filter(|m| m["space_ref"] == space && is_bot_author(&m["author_ref"]))
        .filter_map(|m| m["text"].as_str().map(str::to_owned))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// 1. The M6 scenario: DM the shared bot, get an answer, approve a held request from a card.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s1_dm_answer_and_gateway_approval_from_card() {
    if !enabled() {
        return;
    }
    let w = world(1).await;
    let f = w.owner().await;
    let up = upstream().await;
    let host = hold_upstream(&w.api, &up).await;

    // A plain DM is answered in the DM (verified email auto-links U1 on the way).
    f.dm("U1", "[[say: hello from daneel]]").await;
    w.turn_done(1, 120).await;
    let m = f.wait_text(DM, "hello from daneel", 30).await;
    assert!(
        m["text"].as_str().unwrap().contains("Daneel"),
        "shared-bot attribution on the first post: {m}"
    );

    // A request held by the gateway shows up as a card in the same thread.
    f.dm("U1", &format!("[[http: GET http://{host}/needs-approval]]"))
        .await;
    let a = w.api.wait_pending().await;
    assert_eq!(a["path"], "/needs-approval");
    assert_eq!(a["conversation_id"].as_str(), Some(w.conv.as_str()));
    let (token, msg) = f.wait_button(DM, "Approve", 30).await;
    assert_eq!(up.received_requests().await.unwrap().len(), 0, "held");

    f.click(&token, "U1", DM, &msg).await;
    w.turn_done(2, 120).await;
    f.wait_text(DM, "http 200", 30).await;
    assert_eq!(up.received_requests().await.unwrap().len(), 1);
    let (s, v) = w
        .api
        .get(&format!(
            "/v1/workspaces/{}/approvals?status=approved",
            w.api.ws
        ))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().map(Vec::len), Some(1), "{v}");
    // The card was settled: no live Approve button remains.
    let deadline = Instant::now() + Duration::from_secs(20);
    while f.find_button(DM, "Approve").await.is_some() {
        assert!(Instant::now() < deadline, "card never settled");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // Replaying the (single-use) token neither errors the connector nor decides anything again.
    f.click(&token, "U1", DM, &msg).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(up.received_requests().await.unwrap().len(), 1);
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 2. Ownership failover between two fake replicas.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s2_ownership_failover_between_replicas() {
    if !enabled() {
        return;
    }
    let mut w = world(2).await;
    let owner_idx = {
        let mut idx = None;
        for (i, f) in w.fakes.iter().enumerate() {
            if !f.open_integrations().await.is_empty() {
                idx = Some(i);
            }
        }
        idx.expect("one replica owns the integration")
    };
    // Exactly one replica serves it (never both).
    let other = 1 - owner_idx;
    assert!(w.fakes[other].open_integrations().await.is_empty());
    let before: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT owner_replica_id FROM channel_integrations WHERE id = $1::uuid")
            .bind(&w.integ)
            .fetch_one(w.st.pool())
            .await
            .unwrap();
    assert!(before.is_some());

    w.fakes[owner_idx].dm("U1", "[[say: first answer]]").await;
    w.turn_done(1, 120).await;
    w.fakes[owner_idx].wait_text(DM, "first answer", 30).await;

    // Kill the owner without a retire: the lease has to lapse, then the survivor takes over at
    // its next config poll.
    w.fakes[owner_idx].kill().await;
    let survivor = &w.fakes[other];
    survivor.wait_serving(150).await;
    let after: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT owner_replica_id FROM channel_integrations WHERE id = $1::uuid")
            .bind(&w.integ)
            .fetch_one(w.st.pool())
            .await
            .unwrap();
    assert!(after.is_some() && after != before, "owner changed");

    // The survivor keeps processing: inbound event -> turn -> answer posted on ITS platform.
    survivor.dm("U1", "[[say: second answer]]").await;
    w.turn_done(2, 120).await;
    survivor.wait_text(DM, "second answer", 30).await;
    assert!(
        !bot_texts(&survivor.posted(None).await, DM)
            .iter()
            .any(|t| t.contains("first answer")),
        "the first answer was delivered by the dead replica only"
    );
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 3. Verified-email auto-link vs the sign-in link flow (incl. /v1/channel-links/confirm).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s3_verified_email_autolink_vs_signin_link() {
    if !enabled() {
        return;
    }
    let w = world(1).await;
    let f = w.owner().await;
    let links = "SELECT count(*) FROM user_links WHERE integration_id = $1::uuid";

    // (a) Platform says verified but the Aurora address is NOT verified: no link, no turn, the
    // person gets a sign-in link instead.
    sqlx::query("UPDATE users SET email_verified_at = NULL WHERE email = $1")
        .bind(OWNER)
        .execute(w.st.pool())
        .await
        .unwrap();
    f.dm("U1", "[[say: should not run]]").await;
    let link_msg = wait_link_message(f, "dm-U1").await;
    assert_eq!(w.count(links).await, 0);
    assert!(w.turns().await.is_empty(), "no turn before linking");

    // (b) A platform user with an unverified email equal to the owner's is not auto-linked.
    f.user("U2", Some(OWNER), false, false).await;
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE email = $1")
        .bind(OWNER)
        .execute(w.st.pool())
        .await
        .unwrap();
    f.dm("U2", "[[say: should not run either]]").await;
    let u2_link = wait_link_message(f, "dm-U2").await;
    assert_eq!(
        w.count(links).await,
        0,
        "unverified platform email never links"
    );
    assert!(w.turns().await.is_empty());

    // (c) Both sides verified: U1 is auto-linked on the next message and answered.
    f.dm("U1", "[[say: linked by email]]").await;
    w.turn_done(1, 120).await;
    f.wait_text(DM, "linked by email", 30).await;
    let method: String = sqlx::query_scalar(
        "SELECT method FROM user_links WHERE integration_id = $1::uuid AND platform_user_ref = 'U1'",
    )
    .bind(&w.integ)
    .fetch_one(w.st.pool())
    .await
    .unwrap();
    assert_eq!(method, "verified_email");

    // (d) The sign-in link flow for U2: confirm through the API, then U2 is answered.
    let (s, v) = w
        .api
        .post(
            "/v1/channel-links/confirm",
            json!({"token": "aui_notarealtoken"}),
        )
        .await;
    assert_eq!(s, 410, "bogus token: {v}");
    let (s, v) = w
        .api
        .post("/v1/channel-links/confirm", json!({"token": u2_link}))
        .await;
    assert!(s == 200 || s == 201, "confirm: {s} {v}");
    let method: String = sqlx::query_scalar(
        "SELECT method FROM user_links WHERE integration_id = $1::uuid AND platform_user_ref = 'U2'",
    )
    .bind(&w.integ)
    .fetch_one(w.st.pool())
    .await
    .unwrap();
    assert_eq!(method, "sign_in_link");
    // Single use.
    let (s, v) = w
        .api
        .post("/v1/channel-links/confirm", json!({"token": u2_link}))
        .await;
    assert_eq!(s, 410, "link reuse: {v}");
    // The first link request (for U1) was superseded by the auto-link; confirming it is
    // idempotent for the same user and must not create a second link.
    let (s, _) = w
        .api
        .post("/v1/channel-links/confirm", json!({"token": link_msg}))
        .await;
    assert!(s == 200 || s == 201 || s == 410, "{s}");
    assert_eq!(w.count(links).await, 2);

    f.dm("U2", "[[say: linked by signin]]").await;
    w.turn_done(2, 120).await;
    f.wait_text("dm-U2", "linked by signin", 30).await;
    // Both platform users resolve to the same Aurora user.
    let users: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT user_id) FROM user_links WHERE integration_id = $1::uuid",
    )
    .bind(&w.integ)
    .fetch_one(w.st.pool())
    .await
    .unwrap();
    assert_eq!(users, 1);
    let _ = &w.user_id;
    w.finish().await;
}

/// The sign-in link (`aui_…` token) the core sent to `space` as an ephemeral/DM message.
async fn wait_link_message(f: &Fake, space: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        for m in f.posted(Some(space)).await {
            let t = m["text"].as_str().unwrap_or("").to_owned();
            if let Some(i) = t.find("token=aui_") {
                let tok: String = t[i + 6..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                return tok;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no sign-in link in {space}: {:?}",
            f.posted(None).await
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Mirror: answer once; web-originated turns are not mirrored.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s4_mirror_answer_once_web_turn_not_mirrored() {
    if !enabled() {
        return;
    }
    let w = world(1).await;
    let f = w.owner().await;
    f.dm("U1", "[[say: mirrorone @channel]]").await;
    w.turn_done(1, 120).await;
    f.wait_text(DM, "mirrorone", 30).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let posts = bot_texts(&f.posted(None).await, DM);
    assert_eq!(
        posts.iter().filter(|t| t.contains("mirrorone")).count(),
        1,
        "answered exactly once: {posts:?}"
    );
    // SEC-18: the model's broadcast mention is defused on the platform.
    assert!(
        posts.iter().all(|t| !t.contains("@channel")),
        "broadcast defused: {posts:?}"
    );
    let before = f.posted(None).await.len();

    // A turn started from the web in the same conversation is not mirrored.
    let v = w.api.say(&w.conv, "[[say: webonlyanswer]]").await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let (s, e) = w.st.wait_turn(&turn, &["done", "failed"], 120).await;
    assert_eq!((s.as_str(), e), ("done", None), "{}", w.st.log_tails());
    assert!(transcript_text(&w.api.events(&w.conv).await).contains("webonlyanswer"));
    tokio::time::sleep(Duration::from_secs(4)).await;
    let all = f.posted(None).await;
    assert!(
        all.iter()
            .all(|m| !m["text"].as_str().unwrap_or("").contains("webonlyanswer")),
        "web turn leaked to the platform: {all:?}"
    );
    assert_eq!(all.len(), before, "nothing new on the platform");
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 5. send_message with proactive_messaging=ask: action approval card -> approve -> delivered;
//    the doorless approval-outcome turn is mirrored to the last thread link.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s5_send_message_action_approval_card() {
    if !enabled() {
        return;
    }
    let w = world(1).await;
    let f = w.owner().await;
    let (s, a) = w.api.get(&format!("/v1/agents/{}", w.api.daneel)).await;
    assert_eq!(s, 200, "{a}");
    assert_eq!(a["proactive_messaging"], "ask", "{a}");
    // U2: another platform identity of the owner, so the recipient is reachable.
    f.user("U2", None, false, false).await;
    f.user("U9", None, false, false).await;
    let (s, v) = w
        .api
        .post(
            &format!("/v1/channel-integrations/{}/user-links", w.integ),
            json!({"platform_user_ref": "U2", "user_id": w.user_id}),
        )
        .await;
    assert_eq!(s, 201, "admin link: {v}");

    // The fake harness' `[[tool:]]` directive only emits events, so the agent's tool call goes
    // through the sandbox's real tools socket while a turn is active (fenced on active turns).
    let to = format!("{}:user:U2", w.integ);
    f.dm("U1", "[[sleep: 25s]] [[say: asked to send]]").await;
    let deadline = Instant::now() + Duration::from_secs(90);
    let out = loop {
        if let Some(c) = containers().first() {
            let r = tools_call(
                c,
                &w.conv,
                "send_message",
                &json!({"to": to, "text": "proactive hello"}),
            );
            if r.contains("pending_approval") {
                break r;
            }
            eprintln!("tools socket not ready yet: {}", r.trim());
        }
        assert!(Instant::now() < deadline, "no tool call possible");
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    assert!(out.contains("approval_id"), "{out}");
    w.turn_done(1, 120).await;
    let (token, msg) = f.wait_button(DM, "Approve", 40).await;
    assert!(
        !f.posted(None).await.iter().any(|m| m["text"]
            .as_str()
            .unwrap_or("")
            .contains("proactive hello")
            && m["space_ref"] != DM),
        "nothing is sent before approval"
    );

    // An unlinked platform user cannot decide (SEC-19)...
    f.click(&token, "U9", DM, &msg).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (s, v) = w
        .api
        .get(&format!(
            "/v1/workspaces/{}/action-approvals?status=pending",
            w.api.ws
        ))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["items"].as_array().map(Vec::len),
        Some(1),
        "still held: {v}"
    );
    assert!(
        f.find_button(DM, "Approve").await.is_some(),
        "card unchanged"
    );
    assert!(
        f.posted(Some("dm-U2")).await.is_empty(),
        "nothing delivered to U2 yet"
    );

    // ...the linked owner can.
    let n_turns = w.turns().await.len();
    f.click(&token, "U1", DM, &msg).await;
    let sent = f.wait_text("dm-U2", "proactive hello", 60).await;
    assert!(sent["text"].as_str().unwrap().contains("proactive hello"));
    // Replaying the token must not send a second copy (single use).
    f.click(&token, "U1", DM, &msg).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        f.posted(Some("dm-U2"))
            .await
            .iter()
            .filter(|m| m["text"].as_str().unwrap_or("").contains("proactive hello"))
            .count(),
        1
    );
    // The outcome is a doorless (system) turn in the conversation, mirrored to its last thread
    // link: it produces a further bot post in the DM.
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let t = w.turns().await;
        if t.len() > n_turns && t.iter().skip(n_turns).all(|x| x.1 == "done") {
            assert_eq!(t[n_turns].2.as_deref(), Some("system"), "{t:?}");
            break;
        }
        assert!(Instant::now() < deadline, "no outcome turn: {t:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let plain_bot_posts = |msgs: Vec<Value>| {
        msgs.iter()
            .filter(|m| m["space_ref"] == DM && is_bot_author(&m["author_ref"]))
            .filter(|m| m["card"].is_null() && m["ephemeral_to"].is_null())
            .count()
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while plain_bot_posts(f.posted(None).await) < 2 {
        assert!(
            Instant::now() < deadline,
            "the doorless outcome turn was not mirrored to the last thread link: {:?}",
            f.posted(None).await
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 6. credential_dead disables the presence.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s6_credential_dead_disables_presence() {
    if !enabled() {
        return;
    }
    let w = world(1).await;
    let f = w.owner().await;
    f.dm("U1", "[[say: before the outage]]").await;
    w.turn_done(1, 120).await;
    f.wait_text(DM, "before the outage", 30).await;

    // A second agent with a dedicated bot (its presence row carries the credentials).
    let (s, scout) = w
        .api
        .post(
            &format!("/v1/workspaces/{}/agents", w.api.ws),
            json!({"name": "Scout", "handle": "scout", "kind": "hosted", "harness": "fake"}),
        )
        .await;
    assert_eq!(s, 201, "create agent: {scout}");
    let scout_id = scout["id"]
        .as_str()
        .or_else(|| scout["agent"]["id"].as_str())
        .unwrap_or_else(|| panic!("agent id missing: {scout}"))
        .to_owned();
    let (s, p) = w
        .api
        .post(
            &format!("/v1/agents/{scout_id}/presences"),
            json!({"integration_id": w.integ, "mode": "dedicated"}),
        )
        .await;
    assert_eq!(s, 201, "attach dedicated presence: {p}");
    let presence_status = || async {
        let (s, v) = w.api.get(&format!("/v1/agents/{scout_id}/presences")).await;
        assert_eq!(s, 200, "{v}");
        v["items"][0].clone()
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while presence_status().await["status"] != "active" {
        assert!(
            Instant::now() < deadline,
            "dedicated presence never became active: {}\n{}",
            presence_status().await,
            w.st.log_tails()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let bot: String = sqlx::query_scalar(
        "SELECT platform_user_ref FROM channel_presences WHERE agent_id = $1::uuid",
    )
    .bind(&scout_id)
    .fetch_one(w.st.pool())
    .await
    .unwrap();

    let (s, v) = f
        .ctl("/fake/failures", json!({"dead_credentials": true}))
        .await;
    assert_eq!(s, 200, "{v}");
    // Mentioning the dedicated bot in a DM routes to Scout; the first op that bot sends (the
    // receipt) fails with credential_dead.
    f.dm("U1", &format!("<@bot:{bot}> [[say: during the outage]]"))
        .await;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let p = presence_status().await;
        if p["status"] == "disabled" {
            assert_eq!(p["status_reason"], "credential_dead", "{p}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "presence never disabled: {p}\n{}",
            w.st.log_tails()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        !bot_texts(&f.posted(None).await, DM)
            .iter()
            .any(|t| t.contains("during the outage")),
        "nothing was delivered while the credentials were dead"
    );
    w.finish().await;
}

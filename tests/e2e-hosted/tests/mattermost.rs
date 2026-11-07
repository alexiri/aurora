#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MM-002: the Mattermost connector against the **real core** and a **real Mattermost 11.7**
//! server (the container `scripts/mattermost/setup.sh` seeds): real api/gateway/runner/podman
//! sandboxes (fake harness) plus the real `aurora-connector-mattermost` binary, registered through
//! the connector plane (I-28 signature, admin approval). People are real Mattermost users acting
//! through the REST API with their own sessions.
//! Gated by AURORA_TEST_PODMAN=1; run via scripts/e2e/mattermost.sh.
//!
//! Scenarios: (1) DM + the sign-in link flow, (2) channel mention + thread follow-up, (3) approval
//! click, (4) the unlinked-user link flow and auto-link (never triggers on Mattermost, S-15),
//! (5) bot removal: the shared bot leaves a channel; a dedicated bot is revoked, then detached.
mod common;

use aurora_connector_mattermost::{MattermostConnector, Settings};
use aurora_connector_sdk::Connector;
use aurora_testkit::wiremock::matchers::method;
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use common::*;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const OWNER: &str = "owner@e2e.test";
const CONNECTOR_PORT: u16 = 4310;
const CONNECTOR_OPS: u16 = 4391;
const PASSWORD: &str = "Aur0ra-E2E-Pass-1!";

fn mm_env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} not set: run scripts/e2e/mattermost.sh"))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// Mattermost side
// ---------------------------------------------------------------------------------------------

struct Mm {
    http: reqwest::Client,
    url: String,
    admin: String,
    team_id: String,
    sfx: String,
}

/// A person on Mattermost: their own session, their ephemeral posts (never stored by the server).
struct Human {
    id: String,
    username: String,
    session: String,
    ephemerals: Arc<Mutex<Vec<Value>>>,
}

struct Bot {
    id: String,
    username: String,
    token: String,
    token_id: String,
}

impl Mm {
    fn new() -> Self {
        Mm {
            http: reqwest::Client::new(),
            url: mm_env("AURORA_MM_URL").trim_end_matches('/').to_owned(),
            admin: mm_env("AURORA_MM_ADMIN_TOKEN"),
            team_id: mm_env("AURORA_MM_TEAM_ID"),
            sfx: uuid::Uuid::now_v7().simple().to_string()[20..30].to_owned(),
        }
    }

    async fn call(
        &self,
        token: &str,
        m: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut rb = self
            .http
            .request(m, format!("{}/api/v4{path}", self.url))
            .bearer_auth(token)
            .timeout(Duration::from_secs(30));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.unwrap();
        let s = r.status().as_u16();
        let t = r.text().await.unwrap_or_default();
        (s, serde_json::from_str(&t).unwrap_or(Value::String(t)))
    }

    async fn ok(&self, token: &str, m: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let (s, v) = self.call(token, m.clone(), path, body).await;
        assert!((200..300).contains(&s), "{m} {path} -> {s}: {v}");
        v
    }

    async fn admin_ok(&self, m: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        self.ok(&self.admin, m, path, body).await
    }

    async fn bot(&self, name: &str) -> Bot {
        let name = format!("{name}{}", self.sfx);
        let b = self
            .admin_ok(
                reqwest::Method::POST,
                "/bots",
                Some(json!({"username": name, "display_name": "Daneel", "description": "e2e"})),
            )
            .await;
        let id = b["user_id"].as_str().unwrap().to_owned();
        self.admin_ok(
            reqwest::Method::POST,
            &format!("/teams/{}/members", self.team_id),
            Some(json!({"team_id": self.team_id, "user_id": id})),
        )
        .await;
        let t = self
            .admin_ok(
                reqwest::Method::POST,
                &format!("/users/{id}/tokens"),
                Some(json!({"description": "e2e"})),
            )
            .await;
        Bot {
            id,
            username: name,
            token: t["token"].as_str().unwrap().to_owned(),
            token_id: t["id"].as_str().unwrap().to_owned(),
        }
    }

    async fn human(&self, name: &str, email: &str) -> Human {
        let username = format!("{name}{}", self.sfx);
        let u = self
            .admin_ok(
                reqwest::Method::POST,
                "/users",
                Some(json!({"email": email, "username": username, "password": PASSWORD})),
            )
            .await;
        let id = u["id"].as_str().unwrap().to_owned();
        self.admin_ok(
            reqwest::Method::POST,
            &format!("/users/{id}/email/verify/member"),
            None,
        )
        .await;
        self.admin_ok(
            reqwest::Method::POST,
            &format!("/teams/{}/members", self.team_id),
            Some(json!({"team_id": self.team_id, "user_id": id})),
        )
        .await;
        let resp = self
            .http
            .post(format!("{}/api/v4/users/login", self.url))
            .json(&json!({"login_id": username, "password": PASSWORD}))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "login {username}");
        let session = resp
            .headers()
            .get("token")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let ephemerals = Arc::new(Mutex::new(Vec::new()));
        self.watch_ephemerals(session.clone(), ephemerals.clone());
        Human {
            id,
            username,
            session,
            ephemerals,
        }
    }

    fn watch_ephemerals(&self, token: String, sink: Arc<Mutex<Vec<Value>>>) {
        let ws_url = format!("ws{}/api/v4/websocket", self.url.trim_start_matches("http"));
        tokio::spawn(async move {
            let Ok((mut ws, _)) = tokio_tungstenite::connect_async(ws_url.as_str()).await else {
                return;
            };
            let auth =
                json!({"seq": 1, "action": "authentication_challenge", "data": {"token": token}});
            if ws
                .send(tokio_tungstenite::tungstenite::Message::text(
                    auth.to_string(),
                ))
                .await
                .is_err()
            {
                return;
            }
            while let Some(Ok(msg)) = ws.next().await {
                let tokio_tungstenite::tungstenite::Message::Text(t) = msg else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else {
                    continue;
                };
                if v["event"] != "ephemeral_message" {
                    continue;
                }
                let post = match &v["data"]["post"] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                    other => other.clone(),
                };
                if !post.is_null() {
                    lock(&sink).push(post);
                }
            }
        });
    }

    async fn dm(&self, h: &Human, with: &str) -> String {
        let v = self
            .ok(
                &h.session,
                reqwest::Method::POST,
                "/channels/direct",
                Some(json!([h.id, with])),
            )
            .await;
        v["id"].as_str().unwrap().to_owned()
    }

    async fn post(&self, h: &Human, channel: &str, text: &str, root: &str) -> String {
        let v = self
            .ok(
                &h.session,
                reqwest::Method::POST,
                "/posts",
                Some(json!({"channel_id": channel, "message": text, "root_id": root})),
            )
            .await;
        v["id"].as_str().unwrap().to_owned()
    }

    /// Posts of a channel, oldest first (read with the admin token).
    async fn posts(&self, channel: &str) -> Vec<Value> {
        let v = self
            .admin_ok(
                reqwest::Method::GET,
                &format!("/channels/{channel}/posts?per_page=200"),
                None,
            )
            .await;
        let mut posts: Vec<Value> = v["posts"]
            .as_object()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        posts.retain(|p| p["type"].as_str().unwrap_or("").is_empty() && p["delete_at"] == 0);
        posts.sort_by_key(|p| p["create_at"].as_i64().unwrap_or(0));
        posts
    }

    /// First post by `author` in `channel` whose text contains `needle`.
    async fn wait_text(
        &self,
        channel: &str,
        author: &str,
        needle: &str,
        secs: u64,
        ctx: &str,
    ) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            for p in self.posts(channel).await {
                if p["user_id"] == author
                    && p["message"].as_str().is_some_and(|t| t.contains(needle))
                {
                    return p;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no post by {author} containing {needle:?} in {channel}: {:?}\n{ctx}",
                self.posts(channel)
                    .await
                    .iter()
                    .map(|p| p["message"].clone())
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// The interactive action called `label` on a card by `author` in `channel`:
    /// `(post id, action id)`.
    async fn find_action(
        &self,
        channel: &str,
        author: &str,
        label: &str,
    ) -> Option<(String, String)> {
        for p in self.posts(channel).await.iter().rev() {
            if p["user_id"] != author {
                continue;
            }
            for a in p["props"]["attachments"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                for act in a["actions"].as_array().cloned().unwrap_or_default() {
                    if act["name"] == label {
                        return Some((
                            p["id"].as_str()?.to_owned(),
                            act["id"].as_str()?.to_owned(),
                        ));
                    }
                }
            }
        }
        None
    }

    async fn wait_action(
        &self,
        channel: &str,
        author: &str,
        label: &str,
        secs: u64,
    ) -> (String, String) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(a) = self.find_action(channel, author, label).await {
                return a;
            }
            assert!(
                Instant::now() < deadline,
                "no {label:?} action in {channel}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// A person clicks a button: Mattermost calls the connector's `/mm/actions`.
    async fn click(&self, h: &Human, post: &str, action: &str) -> u16 {
        self.call(
            &h.session,
            reqwest::Method::POST,
            &format!("/posts/{post}/actions/{action}"),
            Some(json!({})),
        )
        .await
        .0
    }

    async fn channel(&self, h: &Human, name: &str) -> String {
        let v = self
            .ok(
                &h.session,
                reqwest::Method::POST,
                "/channels",
                Some(
                    json!({"team_id": self.team_id, "name": format!("{name}{}", self.sfx),
                            "display_name": name, "type": "O"}),
                ),
            )
            .await;
        v["id"].as_str().unwrap().to_owned()
    }
}

/// The sign-in link (`aui_…` token) an ephemeral post to `h` carries.
async fn wait_link(h: &Human, secs: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let found = lock(&h.ephemerals).iter().find_map(|p| {
            let t = p["message"].as_str()?;
            let i = t.find("token=aui_")?;
            Some(
                t[i + 6..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect::<String>(),
            )
        });
        if let Some(t) = found {
            return t;
        }
        assert!(
            Instant::now() < deadline,
            "no sign-in link for {}: {:?}",
            h.username,
            lock(&h.ephemerals)
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

// ---------------------------------------------------------------------------------------------
// The world: core + connector + a Mattermost bot/integration of its own
// ---------------------------------------------------------------------------------------------

struct W {
    st: Stack,
    api: Api,
    mm: Mm,
    bot: Bot,
    alice: Human,
    integ: String,
    conv: String,
}

async fn world() -> W {
    Stack::wipe();
    let mut st = Stack::start(Opts::default()).await;
    let api = st.signup(OWNER).await;
    let mm = Mm::new();
    let bot = mm.bot("daneel").await;
    // The person on Mattermost has the same (verified) email as the Aurora owner.
    let alice = mm.human("alice", OWNER).await;

    // The manifest the binary announces (it depends on nothing but the build and the publisher).
    let manifest = serde_json::to_value(
        MattermostConnector::new(Settings {
            public_url: None,
            hmac_key: vec![0; 32],
            catchup_window: Duration::from_secs(600),
            api_url: None,
            token_check: Duration::from_secs(2),
            capabilities: None,
            test_control: false,
        })
        .with_publisher("e2e-mm")
        .manifest(),
    )
    .unwrap();
    let kit = st
        .provision_connector_with(&api, OWNER, "mattermost", "e2e-mm", &manifest)
        .await;
    st.spawn_extra(
        "mattermost-connector",
        "aurora-connector-mattermost",
        &[
            ("AURORA_API_URL".into(), format!("http://{API_IP}:4101")),
            ("AURORA_CONNECTOR_TOKEN".into(), kit.token.clone()),
            (
                "AURORA_CONNECTOR_LISTEN".into(),
                format!("127.0.0.1:{CONNECTOR_PORT}"),
            ),
            (
                "AURORA_CONNECTOR_OPS_PORT".into(),
                CONNECTOR_OPS.to_string(),
            ),
            (
                "AURORA_CONNECTOR_PUBLIC_URL".into(),
                format!("http://127.0.0.1:{CONNECTOR_PORT}"),
            ),
            (
                "AURORA_CONNECTOR_CALLBACK_HMAC_KEY".into(),
                format!(
                    "{}{}",
                    uuid::Uuid::now_v7().simple(),
                    uuid::Uuid::now_v7().simple()
                ),
            ),
            (
                "AURORA_CONNECTOR_MANIFEST_SIGNATURE".into(),
                kit.signature.clone(),
            ),
            ("AURORA_CONNECTOR_PUBLISHER".into(), "e2e-mm".into()),
            ("AURORA_CONNECTOR_TOKEN_CHECK_SECS".into(), "2".into()),
            ("AURORA_LOG_LEVEL".into(), "debug".into()),
        ],
    );
    st.approve_connector_named(&kit, "mattermost").await;

    let (s, v) = api
        .post(
            &format!("/v1/orgs/{}/channel-integrations", api.org),
            json!({"connector_id": "mattermost",
                   "config": {"server_url": mm.url, "team": mm_env("AURORA_MM_TEAM")},
                   "credentials": {"bot_token": bot.token, "admin_token": mm.admin}}),
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
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE email = $1")
        .bind(OWNER)
        .execute(st.pool())
        .await
        .unwrap();
    let conv = api.direct().await;
    let w = W {
        st,
        api,
        mm,
        bot,
        alice,
        integ,
        conv,
    };
    w.wait_connector_ready().await;
    w
}

impl W {
    async fn wait_connector_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(90);
        let url = format!("http://127.0.0.1:{CONNECTOR_PORT}/mm/ready");
        loop {
            if let Ok(r) = reqwest::get(&url).await
                && r.status().is_success()
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the connector never served the integration\n{}",
                self.st.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    fn logs(&self) -> String {
        self.st.log_tails()
    }

    async fn turns(&self) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT id::text, status FROM turns WHERE conversation_id = $1::uuid ORDER BY created_at",
        )
        .bind(&self.conv)
        .fetch_all(self.st.pool())
        .await
        .unwrap()
    }

    async fn turn_done(&self, n: usize, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some((id, _)) = self.turns().await.get(n - 1) {
                let id = id.clone();
                let (s, e) = self
                    .st
                    .wait_turn(&id, &["done", "failed", "aborted"], secs)
                    .await;
                assert_eq!((s.as_str(), e), ("done", None), "{}", self.logs());
                return;
            }
            assert!(
                Instant::now() < deadline,
                "turn {n} never created: {:?}\n{}",
                self.turns().await,
                self.logs()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    async fn links(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM user_links WHERE integration_id = $1::uuid")
            .bind(&self.integ)
            .fetch_one(self.st.pool())
            .await
            .unwrap()
    }

    /// Alice DMs the bot, gets the sign-in link, and the owner confirms it through the API.
    async fn link_alice(&self, dm: &str) {
        self.mm.post(&self.alice, dm, "hello", "").await;
        let token = wait_link(&self.alice, 60).await;
        let (s, v) = self
            .api
            .post("/v1/channel-links/confirm", json!({"token": token}))
            .await;
        assert!(s == 200 || s == 201, "confirm: {s} {v}");
        assert_eq!(self.links().await, 1);
    }

    async fn finish(self) {
        self.st.finish().await;
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

async fn hold_upstream(api: &Api, up: &MockServer) -> String {
    let id = api
        .ws_secret(json!({
            "name": "mock-api", "kind": "generic", "host_pattern": up.address().to_string(),
            "injection": {"kind": "header", "name": "x-api-key", "format": "{value}"},
            "value": "sk-e2e-mattermost-SECRET"
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

// ---------------------------------------------------------------------------------------------
// 1. DM: unlinked first (sign-in link), then answered in the DM.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s1_dm_sign_in_link_then_answer() {
    if !enabled() {
        return;
    }
    let w = world().await;
    let dm = w.mm.dm(&w.alice, &w.bot.id).await;
    // Nobody is linked: the first DM gets a private sign-in link and no turn.
    w.link_alice(&dm).await;
    assert!(w.turns().await.is_empty(), "no turn before linking");

    w.mm.post(&w.alice, &dm, "[[say: hello from daneel]]", "")
        .await;
    w.turn_done(1, 120).await;
    let m =
        w.mm.wait_text(&dm, &w.bot.id, "hello from daneel", 30, &w.logs())
            .await;
    assert!(
        m["message"].as_str().unwrap().contains("Daneel"),
        "shared-bot attribution on the first post: {m}"
    );
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 2. Channel: bind, mention, thread follow-up without a mention.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s2_channel_mention_and_thread_follow_up() {
    if !enabled() {
        return;
    }
    let w = world().await;
    let dm = w.mm.dm(&w.alice, &w.bot.id).await;
    w.link_alice(&dm).await;

    let channel = w.mm.channel(&w.alice, "team-room").await;
    // Alice (a channel admin) adds the bot: the core learns it from `bot.added`.
    w.mm.ok(
        &w.alice.session,
        reqwest::Method::POST,
        &format!("/channels/{channel}/members"),
        Some(json!({"user_id": w.bot.id})),
    )
    .await;
    // A mention in a space nobody bound is refused politely and starts nothing.
    let at_bot = format!("@{}", w.bot.username);
    w.mm.post(
        &w.alice,
        &channel,
        &format!("{at_bot} [[say: too early]]"),
        "",
    )
    .await;
    w.mm.wait_text(&channel, &w.bot.id, "", 40, &w.logs()).await;

    // Bind from inside the space (an owner is needed: Alice is linked to the owner).
    w.mm.post(&w.alice, &channel, &format!("{at_bot} bind daneel"), "")
        .await;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let active: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM space_bindings WHERE integration_id = $1::uuid AND space_ref = $2 AND active",
        )
        .bind(&w.integ)
        .bind(&channel)
        .fetch_one(w.st.pool())
        .await
        .unwrap();
        if active == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "no binding\n{}", w.logs());
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Mention: answered in a thread rooted at the mention.
    let root =
        w.mm.post(
            &w.alice,
            &channel,
            &format!("{at_bot} [[say: channel answer]]"),
            "",
        )
        .await;
    let a =
        w.mm.wait_text(&channel, &w.bot.id, "channel answer", 120, &w.logs())
            .await;
    assert_eq!(a["root_id"], root, "answer is in the mention's thread: {a}");
    assert!(
        !a["message"].as_str().unwrap().contains("too early"),
        "the early mention started nothing"
    );

    // Thread follow-up without a mention continues the same thread.
    w.mm.post(&w.alice, &channel, "[[say: followup answer]]", &root)
        .await;
    let f =
        w.mm.wait_text(&channel, &w.bot.id, "followup answer", 120, &w.logs())
            .await;
    assert_eq!(f["root_id"], root, "{f}");

    // A top-level message without a mention is ignored (listen = mentions).
    let before = w.mm.posts(&channel).await.len();
    w.mm.post(&w.alice, &channel, "[[say: nobody asked]]", "")
        .await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    let after = w.mm.posts(&channel).await;
    assert_eq!(after.len(), before + 1, "only Alice's own post was added");
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 3. Approval click: a held gateway request becomes a card; the click decides it.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s3_approval_card_click() {
    if !enabled() {
        return;
    }
    let w = world().await;
    let up = upstream().await;
    let host = hold_upstream(&w.api, &up).await;
    let dm = w.mm.dm(&w.alice, &w.bot.id).await;
    w.link_alice(&dm).await;

    w.mm.post(&w.alice, &dm, "[[say: ready]]", "").await;
    w.turn_done(1, 120).await;
    w.mm.wait_text(&dm, &w.bot.id, "ready", 30, &w.logs()).await;

    w.mm.post(
        &w.alice,
        &dm,
        &format!("[[http: GET http://{host}/needs-approval]]"),
        "",
    )
    .await;
    let a = w.api.wait_pending().await;
    assert_eq!(a["path"], "/needs-approval");
    assert_eq!(a["conversation_id"].as_str(), Some(w.conv.as_str()));
    let (post, action) = w.mm.wait_action(&dm, &w.bot.id, "Approve", 60).await;
    assert_eq!(up.received_requests().await.unwrap().len(), 0, "held");

    let s = w.mm.click(&w.alice, &post, &action).await;
    assert_eq!(s, 200, "click answered {s}\n{}", w.logs());
    w.turn_done(2, 120).await;
    w.mm.wait_text(&dm, &w.bot.id, "http 200", 30, &w.logs())
        .await;
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
    // The card was settled in place: no live Approve button remains.
    let deadline = Instant::now() + Duration::from_secs(20);
    while w.mm.find_action(&dm, &w.bot.id, "Approve").await.is_some() {
        assert!(Instant::now() < deadline, "card never settled");
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    // A replay of the single-use click decides nothing again.
    let _ = w.mm.click(&w.alice, &post, &action).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(up.received_requests().await.unwrap().len(), 1);
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 4. The unlinked-user flow, and auto-link (S-15: Mattermost never reports other users'
//    `email_verified`, so it never triggers there).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s4_unlinked_user_link_flow_and_no_autolink() {
    if !enabled() {
        return;
    }
    let w = world().await;
    let dm = w.mm.dm(&w.alice, &w.bot.id).await;

    // Alice's email equals the verified owner's: on Mattermost that still never auto-links.
    w.mm.post(&w.alice, &dm, "[[say: should not run]]", "")
        .await;
    let link = wait_link(&w.alice, 60).await;
    assert_eq!(
        w.links().await,
        0,
        "no auto-link without a platform-verified flag"
    );
    assert!(w.turns().await.is_empty(), "no turn before linking");

    // A bogus token is refused; the real one links (single use).
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
        .post("/v1/channel-links/confirm", json!({"token": link}))
        .await;
    assert!(s == 200 || s == 201, "confirm: {s} {v}");
    let method: String = sqlx::query_scalar(
        "SELECT method FROM user_links WHERE integration_id = $1::uuid AND platform_user_ref = $2",
    )
    .bind(&w.integ)
    .bind(&w.alice.id)
    .fetch_one(w.st.pool())
    .await
    .unwrap();
    assert_eq!(method, "sign_in_link");
    let (s, v) = w
        .api
        .post("/v1/channel-links/confirm", json!({"token": link}))
        .await;
    assert_eq!(s, 410, "link reuse: {v}");

    // Now linked: answered.
    w.mm.post(&w.alice, &dm, "[[say: linked by signin]]", "")
        .await;
    w.turn_done(1, 120).await;
    w.mm.wait_text(&dm, &w.bot.id, "linked by signin", 30, &w.logs())
        .await;

    // A second person with the same email on Mattermost is not auto-linked either.
    let bob = w.mm.human("bob", "bob-same-owner@e2e.test").await;
    let bdm = w.mm.dm(&bob, &w.bot.id).await;
    w.mm.post(&bob, &bdm, "[[say: nope]]", "").await;
    let _ = wait_link(&bob, 60).await;
    assert_eq!(w.links().await, 1, "still one link");
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// 5. Bot removal: the shared bot leaves a channel; a dedicated bot is revoked, then detached.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn s5_bot_removal_shared_and_dedicated() {
    if !enabled() {
        return;
    }
    let w = world().await;
    let dm = w.mm.dm(&w.alice, &w.bot.id).await;
    w.link_alice(&dm).await;

    // (a) The shared bot is removed from a bound channel: the binding is deactivated.
    let channel = w.mm.channel(&w.alice, "removal").await;
    w.mm.ok(
        &w.alice.session,
        reqwest::Method::POST,
        &format!("/channels/{channel}/members"),
        Some(json!({"user_id": w.bot.id})),
    )
    .await;
    let at_bot = format!("@{}", w.bot.username);
    w.mm.post(&w.alice, &channel, &format!("{at_bot} bind daneel"), "")
        .await;
    let bindings = |active: bool| {
        let (integ, channel, st) = (w.integ.clone(), channel.clone(), w.st.pool().clone());
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM space_bindings WHERE integration_id = $1::uuid AND space_ref = $2 AND active = $3",
            )
            .bind(integ)
            .bind(channel)
            .bind(active)
            .fetch_one(&st)
            .await
            .unwrap()
        }
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while bindings(true).await != 1 {
        assert!(Instant::now() < deadline, "no binding\n{}", w.logs());
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    w.mm.ok(
        &w.alice.session,
        reqwest::Method::DELETE,
        &format!("/channels/{channel}/members/{}", w.bot.id),
        None,
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(60);
    while bindings(true).await != 0 {
        assert!(
            Instant::now() < deadline,
            "binding stayed active after the bot left\n{}",
            w.logs()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // (b) A dedicated bot: provisioned on Mattermost, speaks as itself, then its token is
    // revoked on the server: the presence is disabled; detaching removes the bot.
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
    let presence_id = p["id"].as_str().unwrap().to_owned();
    let presence = || async {
        let (s, v) = w.api.get(&format!("/v1/agents/{scout_id}/presences")).await;
        assert_eq!(s, 200, "{v}");
        v["items"][0].clone()
    };
    let deadline = Instant::now() + Duration::from_secs(90);
    while presence().await["status"] != "active" {
        assert!(
            Instant::now() < deadline,
            "dedicated presence never became active: {}\n{}",
            presence().await,
            w.logs()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let bot_id: String = sqlx::query_scalar(
        "SELECT platform_user_ref FROM channel_presences WHERE agent_id = $1::uuid",
    )
    .bind(&scout_id)
    .fetch_one(w.st.pool())
    .await
    .unwrap();
    let profile =
        w.mm.admin_ok(reqwest::Method::GET, &format!("/bots/{bot_id}"), None)
            .await;
    assert_eq!(profile["display_name"], "Scout", "{profile}");
    assert_eq!(profile["delete_at"], 0);

    // Alice reaches Scout through its own bot (a DM with it); it answers as that bot.
    let sdm = w.mm.dm(&w.alice, &bot_id).await;
    w.mm.post(&w.alice, &sdm, "[[say: scout here]]", "").await;
    let m =
        w.mm.wait_text(&sdm, &bot_id, "scout here", 120, &w.logs())
            .await;
    assert_eq!(
        m["user_id"],
        bot_id.as_str(),
        "posted by the dedicated bot itself"
    );

    // The server revokes the bot's token: the connector notices and the core disables it.
    let tokens =
        w.mm.admin_ok(
            reqwest::Method::GET,
            &format!("/users/{bot_id}/tokens"),
            None,
        )
        .await;
    for t in tokens.as_array().unwrap() {
        w.mm.admin_ok(
            reqwest::Method::POST,
            "/users/tokens/revoke",
            Some(json!({"token_id": t["id"]})),
        )
        .await;
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let p = presence().await;
        if p["status"] == "disabled" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "presence never disabled: {p}\n{}",
            w.logs()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Detaching enqueues presence.deprovision: the bot account is disabled on the server.
    let (s, v) = w
        .api
        .call(
            reqwest::Method::DELETE,
            &format!("/v1/presences/{presence_id}"),
            None,
        )
        .await;
    assert!(s == 200 || s == 204, "detach: {s} {v}");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let b =
            w.mm.admin_ok(
                reqwest::Method::GET,
                &format!("/bots/{bot_id}?include_deleted=true"),
                None,
            )
            .await;
        if b["delete_at"].as_i64().unwrap_or(0) > 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the bot was not deprovisioned: {b}\n{}",
            w.logs()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = (&w.bot.token, &w.bot.token_id);
    w.finish().await;
}

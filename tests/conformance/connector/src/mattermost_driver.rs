//! The Mattermost [`PlatformDriver`]: the conformance scenarios against a real
//! Mattermost server (the container `scripts/mattermost/setup.sh` starts).
//!
//! Everything platform-side goes through Mattermost's REST API: people post as
//! themselves (their own session), buttons are clicked through the post-action
//! endpoint, the bot is added to and removed from channels, and what "the
//! platform shows" is read back with the system admin's token.
//!
//! Mattermost picks its own ids, so the scenarios' `U1` / `C1` / `B1` / `F1`
//! are aliases registered with the fake core ([`FakeCore::set_alias`]); every
//! scenario run gets its own users, channels and bot (suffix `sfx`), so runs do
//! not see each other's posts.
//!
//! Two things the REST API hides are read from the server's database through
//! `podman exec` into the container: the `integration` block (action tokens) of
//! message attachments, which Mattermost strips from every API answer.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use aurora_protocol::connector::{
    AgentRef, EventKind, Integration, Presence, PresenceMode, SharedBot,
};
use futures::{SinkExt, StreamExt};
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::driver::*;
use crate::fake_core::{CoreProbe, FakeCore, eventually};
use crate::target::{BIN_ENV, ConnectorUnderTest, Harness, StartOptions, TOKEN};

/// Set to `mattermost` to run the scenarios against Mattermost (see `scripts/conformance.sh --mattermost`).
pub const PLATFORM_ENV: &str = "CONFORMANCE_PLATFORM";

const WAIT: Duration = Duration::from_secs(30);

/// Progress on stderr (`cargo test -- --nocapture`).
macro_rules! vlog {
    ($($a:tt)*) => { eprintln!("mattermost-driver: {}", format!($($a)*)) };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn env(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .with_context(|| format!("{name} is not set (run scripts/conformance.sh --mattermost)"))
}

fn free_port() -> anyhow::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

#[derive(Debug, Clone)]
struct Person {
    id: String,
    username: String,
    token: String,
}

#[derive(Default)]
struct State {
    people: HashMap<String, Person>,
    spaces: HashMap<String, String>,
    files: HashMap<String, FileSpec>,
    /// scenario event id → the post whose delivery it names.
    event_posts: HashMap<String, String>,
    ephemerals: Vec<Post>,
}

pub struct MattermostDriver {
    http: reqwest::Client,
    url: String,
    team_id: String,
    admin_token: String,
    container: String,
    probe: CoreProbe,
    ready_url: String,
    control_url: String,
    sfx: String,
    integration: Uuid,
    bot_id: String,
    bot_name: String,
    bot_token_id: String,
    /// A private channel for files "hosted" before any message mentions them.
    files_channel: String,
    bot_token: String,
    st: Arc<Mutex<State>>,
}

/// Starts the Mattermost target: a fresh bot, an integration for it, the
/// connector binary, and the driver.
pub async fn start(opts: StartOptions) -> anyhow::Result<Harness> {
    let url = env("AURORA_MM_URL")?;
    let team_id = env("AURORA_MM_TEAM_ID")?;
    let team = env("AURORA_MM_TEAM")?;
    let admin_token = env("AURORA_MM_ADMIN_TOKEN")?;
    let container =
        std::env::var("AURORA_MM_CONTAINER").unwrap_or_else(|_| "aurora-mm-server".into());
    let bin = std::env::var(BIN_ENV)
        .ok()
        .filter(|b| !b.is_empty())
        .with_context(|| format!("{BIN_ENV} must name the aurora-connector-mattermost binary"))?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let sfx = Uuid::now_v7().simple().to_string()[20..28].to_owned();

    vlog!("run {sfx}: starting the fake core");
    let core = FakeCore::start(TOKEN).await?;
    let probe = core.probe();

    // A bot of its own for this run, so token revocation in one scenario cannot affect another.
    let mut d = MattermostDriver {
        http,
        url: url.trim_end_matches('/').to_owned(),
        team_id: team_id.clone(),
        admin_token: admin_token.clone(),
        container,
        probe,
        ready_url: String::new(),
        control_url: String::new(),
        sfx: sfx.clone(),
        integration: Uuid::now_v7(),
        bot_id: String::new(),
        bot_name: format!("cb{sfx}"),
        bot_token_id: String::new(),
        files_channel: String::new(),
        bot_token: String::new(),
        st: Arc::new(Mutex::new(State::default())),
    };
    let bot = d
        .admin(
            Method::POST,
            "/bots",
            Some(json!({"username": d.bot_name, "display_name": "Daneel", "description": "conformance"})),
        )
        .await?;
    vlog!("bot {} created", d.bot_name);
    d.bot_id = bot["user_id"].as_str().context("bot id")?.to_owned();
    d.admin(
        Method::POST,
        &format!("/teams/{team_id}/members"),
        Some(json!({"team_id": team_id, "user_id": d.bot_id})),
    )
    .await?;
    let tok = d
        .admin(
            Method::POST,
            &format!("/users/{}/tokens", d.bot_id),
            Some(json!({"description": "conformance"})),
        )
        .await?;
    d.bot_token = tok["token"].as_str().context("bot token")?.to_owned();
    d.bot_token_id = tok["id"].as_str().context("bot token id")?.to_owned();
    let files = d
        .admin(
            Method::POST,
            "/channels",
            Some(json!({"team_id": team_id, "name": format!("files{sfx}"),
                        "display_name": "files", "type": "P"})),
        )
        .await?;
    d.files_channel = files["id"].as_str().context("files channel")?.to_owned();
    d.admin(
        Method::POST,
        &format!("/channels/{}/members", d.files_channel),
        Some(json!({"user_id": d.bot_id})),
    )
    .await?;
    core.set_alias(crate::target::BOT_REF, &d.bot_id);

    let host = d
        .url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_owned();
    let integration = d.integration;
    core.add_integration_with(Integration {
        id: integration,
        org_id: Uuid::now_v7(),
        tenant_ref: format!("{host}/{team_id}"),
        tenant_label: format!("Conformance on {host}"),
        config: json!({"server_url": d.url, "team": team}),
        credentials: json!({"bot_token": d.bot_token, "admin_token": admin_token}),
        credentials_version: 1,
        shared_bot: Some(SharedBot {
            platform_user_ref: d.bot_id.clone(),
            handle: d.bot_name.clone(),
            display_name: "Daneel".into(),
        }),
        status: "active".into(),
    });

    let (listen, ops) = (free_port()?, free_port()?);
    d.ready_url = format!("http://127.0.0.1:{listen}/mm/ready");
    d.control_url = format!("http://127.0.0.1:{listen}/mm/test");
    let mut envs = vec![
        (
            "AURORA_CONNECTOR_LISTEN".to_owned(),
            format!("127.0.0.1:{listen}"),
        ),
        ("AURORA_CONNECTOR_OPS_PORT".to_owned(), ops.to_string()),
        (
            "AURORA_CONNECTOR_PUBLIC_URL".to_owned(),
            format!("http://127.0.0.1:{listen}"),
        ),
        (
            "AURORA_CONNECTOR_CALLBACK_HMAC_KEY".to_owned(),
            format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple()),
        ),
        ("AURORA_MM_TEST_CONTROL".to_owned(), "true".to_owned()),
        (
            "AURORA_CONNECTOR_TOKEN_CHECK_SECS".to_owned(),
            "2".to_owned(),
        ),
    ];
    if let Some(c) = &opts.capabilities {
        envs.push(("AURORA_MM_CAPABILITIES".into(), c.join(",")));
    }
    let cut = ConnectorUnderTest::Binary {
        path: bin,
        args: vec!["serve".into()],
        env: envs,
    };
    vlog!("spawning the connector on {listen}");
    Harness::start(core, integration, cut, Arc::new(d)).await
}

impl MattermostDriver {
    async fn call(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let mut rb = self
            .http
            .request(method.clone(), format!("{}/api/v4{path}", self.url))
            .bearer_auth(token);
        if let Some(b) = &body {
            rb = rb.json(b);
        }
        let resp = rb
            .send()
            .await
            .with_context(|| format!("{method} {path}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if std::env::var_os("CONFORMANCE_VERBOSE").is_some() {
            vlog!("{method} {path} -> {status}");
        }
        if !status.is_success() {
            bail!("{method} {path} -> {status}: {text}");
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn admin(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        self.call(&self.admin_token, method, path, body).await
    }

    fn person(&self, alias: &str) -> anyhow::Result<Person> {
        lock(&self.st)
            .people
            .get(alias)
            .cloned()
            .ok_or_else(|| anyhow!("no such user {alias}: add_user first"))
    }

    fn space_id(&self, alias: &str) -> String {
        lock(&self.st)
            .spaces
            .get(alias)
            .cloned()
            .unwrap_or_else(|| alias.to_owned())
    }

    /// Alias → real user id (the bot, a person, or the id unchanged).
    fn user_id(&self, alias: &str) -> String {
        if alias == crate::target::BOT_REF {
            return self.bot_id.clone();
        }
        lock(&self.st)
            .people
            .get(alias)
            .map_or_else(|| alias.to_owned(), |p| p.id.clone())
    }

    /// `<@bot:B1>` / `<@user:U1>` → `@username`, as a person would type it.
    fn humanize(&self, text: &str) -> String {
        let mut out = text.to_owned();
        let people: Vec<(String, String)> = lock(&self.st)
            .people
            .iter()
            .map(|(a, p)| (a.clone(), p.username.clone()))
            .collect();
        for (alias, name) in people {
            for tag in ["user", "bot"] {
                out = out.replace(&format!("<@{tag}:{alias}>"), &format!("@{name}"));
            }
        }
        out = out.replace(
            &format!("<@bot:{}>", crate::target::BOT_REF),
            &format!("@{}", self.bot_name),
        );
        out
    }

    async fn login(&self, username: &str, password: &str) -> anyhow::Result<String> {
        let resp = self
            .http
            .post(format!("{}/api/v4/users/login", self.url))
            .json(&json!({"login_id": username, "password": password}))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("login {username}: {}", resp.status());
        }
        resp.headers()
            .get("token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .context("login answered without a session token")
    }

    /// The connector's live view of the world: has the core seen `real_event`?
    async fn wait_delivery(
        &self,
        real_event: &str,
        duplicates_before: Option<usize>,
    ) -> anyhow::Result<Ack> {
        let alias = self.probe.alias_of(real_event);
        let seen = eventually(WAIT, || {
            // A redelivery (`duplicates_before` is set) is answered by the core as a duplicate.
            if duplicates_before.is_none()
                && self.probe.events().iter().any(|e| e.event_id == alias)
            {
                return Some(Ack::Accepted);
            }
            let dups = self
                .probe
                .duplicates()
                .iter()
                .filter(|d| **d == alias)
                .count();
            (dups > duplicates_before.unwrap_or(usize::MAX)).then_some(Ack::Duplicate)
        })
        .await;
        seen.ok_or_else(|| anyhow!("the core never answered for event {alias}"))
    }

    /// Reads the post's props straight from the database: unlike the API it
    /// keeps the attachments' `integration` blocks (the action tokens).
    fn db_props(&self, post_id: &str) -> anyhow::Result<Value> {
        if !post_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            bail!("bad post id");
        }
        let out = std::process::Command::new("podman")
            .args([
                "exec",
                &self.container,
                "psql",
                "-U",
                "mmuser",
                "-d",
                "mattermost_test",
                "-tAc",
                &format!("select props from posts where id = '{post_id}'"),
            ])
            .stderr(Stdio::inherit())
            .output()
            .context("podman exec psql")?;
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(text.trim()).with_context(|| format!("props of {post_id}: {text}"))
    }

    fn to_post(&self, p: &Value, with_integration: bool) -> anyhow::Result<Post> {
        let id = p["id"].as_str().unwrap_or_default().to_owned();
        let channel = p["channel_id"].as_str().unwrap_or_default();
        let root = p["root_id"].as_str().unwrap_or_default();
        let mut props = p["props"].clone();
        let has_cards = props["attachments"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        if has_cards && with_integration {
            props = self.db_props(&id)?;
        }
        let files: Vec<String> = p["metadata"]["files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| f["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let reactions: Vec<(String, String)> = p["metadata"]["reactions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| {
                        (
                            r["emoji_name"].as_str().unwrap_or_default().to_owned(),
                            self.probe
                                .alias_of(r["user_id"].as_str().unwrap_or_default()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Post {
            message_ref: id,
            space_ref: self.probe.alias_of(channel),
            thread_ref: root.to_owned(),
            author_ref: self
                .probe
                .alias_of(p["user_id"].as_str().unwrap_or_default()),
            text: p["message"].as_str().unwrap_or_default().to_owned(),
            card: has_cards.then(|| props["attachments"].clone()),
            ephemeral_to: None,
            op_marker: props["aurora_op"].as_str().map(str::to_owned),
            files,
            deleted: p["delete_at"].as_i64().unwrap_or(0) > 0,
            reactions,
        })
    }

    async fn channel_posts(&self, channel: &str) -> anyhow::Result<Vec<Value>> {
        let v = self
            .admin(
                Method::GET,
                &format!("/channels/{channel}/posts?per_page=200"),
                None,
            )
            .await?;
        let mut posts: Vec<Value> = v["posts"]
            .as_object()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        posts.retain(|p| p["type"].as_str().unwrap_or("").is_empty());
        posts.sort_by_key(|p| p["create_at"].as_i64().unwrap_or(0));
        Ok(posts)
    }

    async fn connector_control(&self, path: &str) -> anyhow::Result<()> {
        let r = self
            .http
            .post(format!("{}/{path}", self.control_url))
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("connector control {path}: {}", r.status());
        }
        Ok(())
    }

    /// Captures ephemeral posts addressed to `alias` (they are never stored by Mattermost).
    fn watch_ephemerals(&self, alias: String, token: String) {
        let ws_url = format!("ws{}/api/v4/websocket", self.url.trim_start_matches("http"));
        let st = self.st.clone();
        let probe = self.probe.clone();
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
                let raw = &v["data"]["post"];
                let post: Value = match raw {
                    Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                    other => other.clone(),
                };
                if post.is_null() {
                    continue;
                }
                let channel = post["channel_id"].as_str().unwrap_or_default();
                lock(&st).ephemerals.push(Post {
                    message_ref: post["id"].as_str().unwrap_or_default().to_owned(),
                    space_ref: probe.alias_of(channel),
                    thread_ref: post["root_id"].as_str().unwrap_or_default().to_owned(),
                    author_ref: probe.alias_of(post["user_id"].as_str().unwrap_or_default()),
                    text: post["message"].as_str().unwrap_or_default().to_owned(),
                    card: post["props"]["attachments"]
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .map(|_| post["props"]["attachments"].clone()),
                    ephemeral_to: Some(alias.clone()),
                    op_marker: post["props"]["aurora_op"].as_str().map(str::to_owned),
                    files: vec![],
                    deleted: false,
                    reactions: vec![],
                });
            }
        });
    }

    async fn upload(
        &self,
        token: &str,
        channel: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<String> {
        let resp = self
            .http
            .post(format!("{}/api/v4/files", self.url))
            .bearer_auth(token)
            .query(&[("channel_id", channel), ("filename", name)])
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .await?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("upload {name}: {status}: {v}");
        }
        v["file_infos"][0]["id"]
            .as_str()
            .map(str::to_owned)
            .context("upload answered without a file id")
    }
}

/// `@channel`, `@all`, `@here` outside code spans and fences (what would notify everyone).
fn live_broadcast_words(text: &str) -> Vec<String> {
    let mut visible = String::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // A run of backticks toggles a code span once, whatever its length.
        let mut in_code = false;
        let mut prev_tick = false;
        for c in line.chars() {
            if c == '`' {
                if !prev_tick {
                    in_code = !in_code;
                }
                prev_tick = true;
                continue;
            }
            prev_tick = false;
            if !in_code {
                visible.push(c);
            }
        }
        visible.push('\n');
    }
    let lower = visible.to_lowercase();
    let mut found = Vec::new();
    for (i, _) in lower.match_indices('@') {
        let rest = &lower[i + 1..];
        for w in ["channel", "all", "here"] {
            if let Some(after) = rest.strip_prefix(w)
                && !after
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                found.push(format!("@{w}"));
            }
        }
    }
    found
}

fn strings_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
        Value::Object(m) => m
            .iter()
            .filter(|(k, _)| *k != "integration")
            .for_each(|(_, x)| strings_of(x, out)),
        _ => {}
    }
}

#[async_trait]
impl PlatformDriver for MattermostDriver {
    fn quirks(&self) -> PlatformQuirks {
        PlatformQuirks {
            // Mattermost 11.7 does not expose other users' `email_verified` over REST.
            email_verified_visible: false,
            // Every Mattermost account has an email address.
            users_without_email: false,
            // Dedicated bots (MM-002): the admin token is part of the integration.
            dedicated_presences: true,
            // No Mattermost action makes the connector call `POST /link-start` in shared mode.
            link_start_action: false,
        }
    }

    async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        vlog!("waiting for the connector at {}", self.ready_url);
        let end = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(r) = self.http.get(&self.ready_url).send().await
                && r.status().is_success()
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= end {
                bail!("the connector does not serve an integration");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn capabilities(&self) -> Result<Vec<String>> {
        let regs = self.probe.registrations();
        let m = &regs
            .first()
            .context("connector has not registered")?
            .manifest;
        Ok(m.enabled_capabilities()
            .into_iter()
            .map(str::to_owned)
            .collect())
    }

    async fn add_user(&self, u: UserSpec) -> Result<()> {
        if u.is_bot || u.reference == crate::target::BOT_REF {
            return Ok(()); // the run's shared bot exists already
        }
        let username = format!("{}{}", u.reference.to_lowercase(), self.sfx);
        let password = format!("Aur0ra-{}-Pass!", Uuid::now_v7().simple());
        let email = u
            .email
            .clone()
            .unwrap_or_else(|| format!("{username}@aurora-conf.test"));
        // Emails are unique per server and the suite reuses the same ones every run:
        // retire whoever holds this address from an earlier run.
        if let Ok(old) = self
            .admin(Method::GET, &format!("/users/email/{email}"), None)
            .await
            && let Some(old_id) = old["id"].as_str()
        {
            let _ = self
                .admin(
                    Method::PUT,
                    &format!("/users/{old_id}/patch"),
                    Some(json!({"email": format!("retired-{}@aurora-conf.test", Uuid::now_v7().simple())})),
                )
                .await;
        }
        let created = self
            .admin(
                Method::POST,
                "/users",
                Some(json!({"email": email, "username": username, "password": password})),
            )
            .await?;
        let id = created["id"].as_str().context("user id")?.to_owned();
        if u.email_verified {
            // Best effort: Mattermost 11.7 does not show the flag to other users anyway.
            let _ = self
                .admin(
                    Method::PUT,
                    &format!("/users/{id}/patch"),
                    Some(json!({"email_verified": true})),
                )
                .await;
        }
        self.admin(
            Method::POST,
            &format!("/teams/{}/members", self.team_id),
            Some(json!({"team_id": self.team_id, "user_id": id})),
        )
        .await?;
        let token = self.login(&username, &password).await?;
        lock(&self.st).people.insert(
            u.reference.clone(),
            Person {
                id: id.clone(),
                username,
                token: token.clone(),
            },
        );
        self.probe.set_alias(&u.reference, &id);
        self.watch_ephemerals(u.reference, token);
        Ok(())
    }

    async fn add_space(&self, s: SpaceSpec) -> Result<()> {
        let kind = match s.kind.as_str() {
            "channel" => "O",
            "private" => "P",
            other => bail!("the Mattermost driver cannot create a {other} space"),
        };
        let name = format!("{}{}", s.reference.to_lowercase(), self.sfx);
        let ch = self
            .admin(
                Method::POST,
                "/channels",
                Some(json!({"team_id": self.team_id, "name": name, "display_name": s.label, "type": kind})),
            )
            .await?;
        let id = ch["id"].as_str().context("channel id")?.to_owned();
        self.admin(
            Method::POST,
            &format!("/channels/{id}/members"),
            Some(json!({"user_id": self.bot_id})),
        )
        .await?;
        lock(&self.st)
            .spaces
            .insert(s.reference.clone(), id.clone());
        self.probe.set_alias(&s.reference, &id);
        Ok(())
    }

    async fn host_file(&self, f: FileSpec) -> Result<()> {
        // Hosted by the bot first so a file no message ever attaches is still readable by it;
        // a message that attaches the file re-uploads it as the poster (see `post_message`).
        let id = self
            .upload(
                &self.bot_token,
                &self.files_channel,
                &f.name,
                f.content.clone().into_bytes(),
            )
            .await?;
        self.probe.set_alias(&f.reference, &id);
        lock(&self.st).files.insert(f.reference.clone(), f);
        Ok(())
    }

    async fn post_message(&self, m: MessageSpec) -> Result<Delivered> {
        let author = self.person(&m.author)?;
        vlog!("{} posts {:?}", m.author, m.text);
        // Redelivery of an event the core already has: Mattermost replays through the
        // connector's catch-up, which the test profile can run on demand.
        if let Some(ev) = &m.event_id {
            let known = lock(&self.st).event_posts.get(ev).cloned();
            if let Some(post_id) = known {
                let before = self.probe.duplicates().iter().filter(|d| *d == ev).count();
                self.connector_control("redeliver").await?;
                let ack = self
                    .wait_delivery(&format!("posted:{post_id}"), Some(before))
                    .await?;
                return Ok(Delivered {
                    ack,
                    message_ref: post_id,
                });
            }
        }
        let channel = match &m.space {
            Some(alias) => {
                let id = self.space_id(alias);
                self.admin(
                    Method::POST,
                    &format!("/channels/{id}/members"),
                    Some(json!({"user_id": author.id})),
                )
                .await?;
                id
            }
            None => {
                let ch = self
                    .call(
                        &author.token,
                        Method::POST,
                        "/channels/direct",
                        Some(json!([author.id, self.bot_id])),
                    )
                    .await?;
                ch["id"].as_str().context("dm channel id")?.to_owned()
            }
        };
        let mut file_ids = Vec::new();
        for (fref, name, _size, _ct) in &m.files {
            let content = lock(&self.st)
                .files
                .get(fref)
                .map(|f| f.content.clone())
                .ok_or_else(|| anyhow!("file {fref} was never hosted"))?;
            let id = self
                .upload(&author.token, &channel, name, content.into_bytes())
                .await?;
            self.probe.set_alias(fref, &id);
            file_ids.push(id);
        }
        let post = self
            .call(
                &author.token,
                Method::POST,
                "/posts",
                Some(json!({
                    "channel_id": channel,
                    "message": self.humanize(&m.text),
                    "root_id": m.thread_ref.clone().unwrap_or_default(),
                    "file_ids": file_ids,
                })),
            )
            .await?;
        let post_id = post["id"].as_str().context("post id")?.to_owned();
        if let Some(ev) = &m.event_id {
            lock(&self.st)
                .event_posts
                .insert(ev.clone(), post_id.clone());
            self.probe.set_alias(ev, &format!("posted:{post_id}"));
        }
        let ack = self
            .wait_delivery(&format!("posted:{post_id}"), None)
            .await?;
        Ok(Delivered {
            ack,
            message_ref: post_id,
        })
    }

    async fn click(&self, c: Click) -> Result<Ack> {
        let clicker = self.person(&c.clicker)?;
        let post_id = c
            .message_ref
            .clone()
            .context("a Mattermost click names the post")?;
        let props = self.db_props(&post_id)?;
        let action = props["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|a| a["actions"].as_array().into_iter().flatten())
            .find(|a| a["integration"]["context"]["token"].as_str() == Some(c.token.as_str()))
            .context("no action carries that token")?;
        let action_id = action["id"].as_str().context("action id")?.to_owned();
        let body = match &c.value {
            Some(v) => json!({"selected_option": v}),
            None => json!({}),
        };
        match self
            .call(
                &clicker.token,
                Method::POST,
                &format!("/posts/{post_id}/actions/{action_id}"),
                Some(body),
            )
            .await
        {
            Ok(_) => Ok(Ack::Accepted),
            Err(e) => Ok(Ack::Rejected(e.to_string())),
        }
    }

    async fn bot_added(&self, space_ref: &str, bot_ref: &str, actor_ref: &str) -> Result<Ack> {
        let channel = self.space_id(space_ref);
        let bot = self.user_id(bot_ref);
        let actor = self.person(actor_ref)?;
        self.admin(
            Method::POST,
            &format!("/channels/{channel}/members"),
            Some(json!({"user_id": actor.id})),
        )
        .await?;
        // Start from "the bot is not in the channel", then the person adds it.
        let _ = self
            .admin(
                Method::DELETE,
                &format!("/channels/{channel}/members/{bot}"),
                None,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.call(
            &actor.token,
            Method::POST,
            &format!("/channels/{channel}/members"),
            Some(json!({"user_id": bot})),
        )
        .await?;
        let alias = self.probe.alias_of(&channel);
        let seen = eventually(WAIT, || {
            self.probe
                .events()
                .iter()
                .any(|e| {
                    e.kind == EventKind::BotAdded
                        && e.payload["space"]["ref"].as_str() == Some(alias.as_str())
                })
                .then_some(())
        })
        .await;
        seen.context("the core never received bot.added")?;
        Ok(Ack::Accepted)
    }

    async fn presence_removed(
        &self,
        bot_ref: &str,
        presence_id: Option<Uuid>,
        reason: &str,
    ) -> Result<Ack> {
        // A dedicated bot the core knows (the scenario's `B2`, presence `presence_id`).
        let pid = presence_id.context("a presence id is needed")?;
        let name = format!("db{}", self.sfx);
        let bot = self
            .admin(
                Method::POST,
                "/bots",
                Some(json!({"username": name, "display_name": "Scout", "description": "conformance"})),
            )
            .await?;
        let bot_id = bot["user_id"].as_str().context("bot id")?.to_owned();
        self.admin(
            Method::POST,
            &format!("/teams/{}/members", self.team_id),
            Some(json!({"team_id": self.team_id, "user_id": bot_id})),
        )
        .await?;
        let tok = self
            .admin(
                Method::POST,
                &format!("/users/{bot_id}/tokens"),
                Some(json!({"description": "conformance"})),
            )
            .await?;
        self.probe.set_alias(bot_ref, &bot_id);
        self.probe.add_presence(Presence {
            id: pid,
            integration_id: self.integration,
            mode: PresenceMode::Dedicated,
            agent: AgentRef {
                id: Uuid::now_v7(),
                handle: "scout".into(),
                name: "Scout".into(),
                avatar_url: None,
            },
            platform_user_ref: Some(bot_id.clone()),
            handle: Some(name),
            credentials: Some(json!({"token": tok["token"], "token_id": tok["id"]})),
            credentials_version: Some(1),
            status: "active".into(),
        });
        // The connector opens the bot's socket at its next config poll.
        let end = tokio::time::Instant::now() + WAIT;
        loop {
            let seen = match self
                .http
                .get(format!("{}/bots", self.control_url))
                .send()
                .await
            {
                Ok(r) => r.text().await.unwrap_or_default().contains(&bot_id),
                Err(_) => false,
            };
            if seen {
                break;
            }
            if tokio::time::Instant::now() >= end {
                bail!("the connector never picked up the dedicated bot");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        if reason == "token_revoked" {
            self.admin(
                Method::POST,
                "/users/tokens/revoke",
                Some(json!({"token_id": tok["id"]})),
            )
            .await?;
        } else {
            self.admin(Method::POST, &format!("/bots/{bot_id}/disable"), None)
                .await?;
        }
        Ok(Ack::Accepted)
    }

    async fn bot_profile(&self, bot_ref: &str) -> Result<Option<BotProfile>> {
        let id = self.user_id(bot_ref);
        let bot = self
            .admin(
                Method::GET,
                &format!("/bots/{id}?include_deleted=true"),
                None,
            )
            .await?;
        let user = self
            .admin(Method::GET, &format!("/users/{id}"), None)
            .await?;
        Ok(Some(BotProfile {
            username: bot["username"].as_str().unwrap_or_default().to_owned(),
            display_name: bot["display_name"].as_str().unwrap_or_default().to_owned(),
            active: bot["delete_at"].as_i64().unwrap_or(0) == 0,
            has_icon: user["last_picture_update"].as_i64().unwrap_or(0) > 0
                || bot["last_icon_update"].as_i64().unwrap_or(0) > 0,
        }))
    }

    async fn inject(&self, f: Failure) -> Result<()> {
        match f {
            Failure::DeadCredentials => {
                self.admin(
                    Method::POST,
                    "/users/tokens/disable",
                    Some(json!({"token_id": self.bot_token_id})),
                )
                .await?;
                Ok(())
            }
            Failure::LoseResults(n) => self.connector_control(&format!("lose-results?n={n}")).await,
        }
    }

    async fn request_link(&self, _user_ref: &str) -> Result<String> {
        bail!(
            "no Mattermost action triggers POST /link-start in shared mode (PlatformQuirks::link_start_action)"
        )
    }

    async fn posts(&self, space_ref: Option<&str>) -> Result<Vec<Post>> {
        // Ephemeral posts reach the recipient's websocket a moment after the op result.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let channels: Vec<String> = match space_ref {
            Some(s) => vec![self.space_id(s)],
            None => {
                let v = self
                    .admin(
                        Method::GET,
                        &format!("/users/{}/teams/{}/channels", self.bot_id, self.team_id),
                        None,
                    )
                    .await?;
                v.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["id"].as_str().map(str::to_owned))
                    .collect()
            }
        };
        let mut out = Vec::new();
        for ch in &channels {
            for p in self.channel_posts(ch).await? {
                out.push(self.to_post(&p, true)?);
            }
        }
        let alias_filter = space_ref.map(str::to_owned);
        out.extend(
            lock(&self.st)
                .ephemerals
                .iter()
                .filter(|p| alias_filter.as_deref().is_none_or(|s| p.space_ref == s))
                .cloned(),
        );
        Ok(out)
    }

    async fn attachment(&self, message_ref: &str, name: &str) -> Result<Option<Vec<u8>>> {
        let post = self
            .admin(Method::GET, &format!("/posts/{message_ref}"), None)
            .await?;
        let infos = post["metadata"]["files"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let Some(info) = infos.iter().find(|f| f["name"].as_str() == Some(name)) else {
            return Ok(None);
        };
        let id = info["id"].as_str().context("file id")?;
        let bytes = self
            .http
            .get(format!("{}/api/v4/files/{id}", self.url))
            .bearer_auth(&self.admin_token)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(Some(bytes.to_vec()))
    }

    fn buttons(&self, post: &Post) -> Vec<Button> {
        post.card
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|a| a["actions"].as_array().into_iter().flatten())
            .filter(|a| a["type"] == "button")
            .map(|a| Button {
                label: a["name"].as_str().unwrap_or_default().to_owned(),
                token: a["integration"]["context"]["token"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect()
    }

    fn select(&self, post: &Post) -> Option<Select> {
        post.card
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|a| a["actions"].as_array().into_iter().flatten())
            .find(|a| a["type"] == "select")
            .map(|a| Select {
                token: a["integration"]["context"]["token"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                options: a["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| {
                        (
                            o["text"].as_str().unwrap_or_default().to_owned(),
                            o["value"].as_str().unwrap_or_default().to_owned(),
                        )
                    })
                    .collect(),
            })
    }

    fn live_broadcasts(&self, post: &Post) -> Vec<String> {
        let mut texts = vec![post.text.clone()];
        if let Some(card) = &post.card {
            strings_of(card, &mut texts);
        }
        let mut found = Vec::new();
        for t in texts {
            for w in live_broadcast_words(&t) {
                if !found.contains(&w) {
                    found.push(w);
                }
            }
        }
        found
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_detector_ignores_code_and_longer_words() {
        assert_eq!(
            live_broadcast_words("hi @here and @ALL!"),
            ["@here", "@all"]
        );
        assert!(live_broadcast_words("`@channel` and ``@all`` ok").is_empty());
        assert!(live_broadcast_words("```\n@here\n```").is_empty());
        assert!(live_broadcast_words("@allison @channels @here_x").is_empty());
        assert_eq!(live_broadcast_words("x\n`a`\n@channel"), ["@channel"]);
    }
}

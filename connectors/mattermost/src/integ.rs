//! One served integration: its Mattermost client, caches, event emission, the
//! WebSocket loop with catch-up, and the inbound event handlers (docs/20 §Transport).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use aurora_connector_sdk::{EventAck, EventSink, IntegrationHandle};
use aurora_protocol::connector::{
    BotMembership, CommandInvoked, EventKind, InboundEvent, Integration, MessageChanged, OpError,
    Presence, PresenceRemoved, PresenceRemovedReason, ReactionAdded,
};
use reqwest::Method;
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::{Mm, R, is_mm_id};
use crate::events::{
    Bots, MmChannel, MmFileInfo, MmPost, MmUser, WsEvent, command_from_text, dedupe_posts,
    mentioned_usernames, message_posted, now_ms, rfc3339_ms, space,
};
use crate::render::{Lookups, MentionRefs, mention_refs};
use crate::ws::{WsConn, WsError};

const USER_TTL: Duration = Duration::from_secs(300);
const CHANNEL_TTL: Duration = Duration::from_secs(600);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A dedicated bot of the integration (MM-002 provisions them; here they are
/// only used as senders and for the echo guard).
#[derive(Debug, Clone)]
pub struct PresenceBot {
    pub token: String,
    pub user_id: String,
    pub username: String,
}

/// The parts of an integration that change on `update_integration`.
#[derive(Debug, Clone)]
pub struct Dynamic {
    pub bot_token: String,
    pub admin_token: Option<String>,
    pub command_token: Option<String>,
    pub credentials_version: u64,
    pub caps: BTreeSet<String>,
    pub bot_id: String,
    pub bot_username: String,
    pub presences: HashMap<Uuid, PresenceBot>,
}

/// Deployment settings shared by all integrations of this process.
#[derive(Debug, Clone)]
pub struct Settings {
    /// `https://…` URL Mattermost reaches this connector at (no trailing slash).
    pub public_url: Option<String>,
    /// Key of the interactive-action signatures.
    pub hmac_key: Vec<u8>,
    pub catchup_window: Duration,
    /// Base URL of the Aurora API (the only origin avatars are fetched from).
    pub api_url: Option<String>,
    /// How often a socket's token is checked with a REST call: Mattermost keeps a
    /// socket open after its token is revoked.
    pub token_check: Duration,
    /// Test profile: declare only these capabilities.
    pub capabilities: Option<Vec<String>>,
    pub test_control: bool,
}

impl Settings {
    pub fn actions_url(&self) -> Option<String> {
        self.public_url
            .as_ref()
            .map(|u| format!("{}/mm/actions", u.trim_end_matches('/')))
    }
}

pub struct Integ {
    pub id: Uuid,
    pub org_id: Uuid,
    pub mm: Mm,
    pub team_id: String,
    pub dynamic: RwLock<Dynamic>,
    pub sink: EventSink,
    pub handle: IntegrationHandle,
    pub settings: Settings,
    users: Mutex<HashMap<String, (Instant, MmUser)>>,
    names: Mutex<HashMap<String, (Instant, Option<MmUser>)>>,
    channels: Mutex<HashMap<String, (Instant, MmChannel)>>,
    ready: AtomicBool,
    catching_up: AtomicBool,
    /// Bots this connector disabled itself (`presence.deprovision`): their
    /// `user_updated` is not a `presence.removed`.
    retired: Mutex<HashSet<String>>,
    /// Dedicated bots with a running socket task: presence id → token it uses.
    bot_sockets: Mutex<HashMap<Uuid, String>>,
    /// Bumped whenever the presences change (wakes the per-bot socket tasks).
    bots_gen: tokio::sync::watch::Sender<u64>,
    /// `(kind, channel, user)` of recent membership events: the same change is seen
    /// on every bot's socket.
    recent_membership: Mutex<HashMap<String, (Instant, Option<Uuid>)>>,
}

/// Capabilities left after the integration's own narrowing (`config._capabilities`).
pub fn effective_caps(declared: &BTreeSet<String>, config: &Value) -> BTreeSet<String> {
    let mut caps = declared.clone();
    if let Some(n) = config.get("_capabilities").and_then(Value::as_object) {
        for (name, v) in n {
            let on = v.as_bool().unwrap_or_else(|| v.is_object());
            if !on {
                caps.remove(name);
            }
        }
    }
    caps
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub fn presence_bots(presences: &[Presence]) -> HashMap<Uuid, PresenceBot> {
    presences
        .iter()
        .filter_map(|p| {
            let token = p
                .credentials
                .as_ref()
                .and_then(|c| str_field(c, "token").or_else(|| str_field(c, "bot_token")))?;
            Some((
                p.id,
                PresenceBot {
                    token,
                    user_id: p.platform_user_ref.clone()?,
                    username: p.handle.clone().unwrap_or_default(),
                },
            ))
        })
        .collect()
}

impl Integ {
    /// Builds the integration from the core's `config` entry.
    pub fn from_config(
        integration: &Integration,
        presences: &[Presence],
        declared: &BTreeSet<String>,
        sink: EventSink,
        handle: IntegrationHandle,
        settings: Settings,
    ) -> Result<Self, String> {
        let server =
            str_field(&integration.config, "server_url").ok_or("config.server_url missing")?;
        let mm = Mm::new(&server)?;
        let bot_token = str_field(&integration.credentials, "bot_token")
            .ok_or("credentials.bot_token missing")?;
        let bot = integration
            .shared_bot
            .as_ref()
            .ok_or("integration has no shared bot")?;
        let team_id = integration
            .tenant_ref
            .rsplit_once('/')
            .map(|(_, t)| t.to_owned())
            .filter(|t| !t.is_empty())
            .ok_or("tenant_ref has no team id")?;
        let dynamic = Dynamic {
            bot_token,
            admin_token: str_field(&integration.credentials, "admin_token"),
            command_token: str_field(&integration.credentials, "command_token"),
            credentials_version: integration.credentials_version,
            caps: effective_caps(declared, &integration.config),
            bot_id: bot.platform_user_ref.clone(),
            bot_username: bot.handle.clone(),
            presences: presence_bots(presences),
        };
        Ok(Self {
            id: integration.id,
            org_id: integration.org_id,
            mm,
            team_id,
            dynamic: RwLock::new(dynamic),
            sink,
            handle,
            settings,
            users: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
            ready: AtomicBool::new(false),
            catching_up: AtomicBool::new(false),
            retired: Mutex::new(HashSet::new()),
            bot_sockets: Mutex::new(HashMap::new()),
            bots_gen: tokio::sync::watch::channel(0).0,
            recent_membership: Mutex::new(HashMap::new()),
        })
    }

    pub fn update(
        &self,
        integration: &Integration,
        presences: &[Presence],
        declared: &BTreeSet<String>,
    ) {
        let mut d = self.dynamic.write().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = str_field(&integration.credentials, "bot_token") {
            d.bot_token = t;
        }
        d.admin_token = str_field(&integration.credentials, "admin_token");
        d.command_token = str_field(&integration.credentials, "command_token");
        d.credentials_version = integration.credentials_version;
        d.caps = effective_caps(declared, &integration.config);
        d.presences = presence_bots(presences);
    }

    pub fn snapshot(&self) -> Dynamic {
        self.dynamic
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn retire_bot(&self, user_id: &str) {
        lock(&self.retired).insert(user_id.to_owned());
    }

    /// GET with the shared bot's token; a bot that is not in the channel (a DM of a
    /// dedicated bot) gets 403/404, which the admin token (required for dedicated
    /// mode) can still read.
    pub async fn read(&self, path: &str, q: &[(&str, &str)]) -> R<Value> {
        let d = self.snapshot();
        match self.mm.get_q(&d.bot_token, path, q).await {
            Err(e)
                if matches!(
                    e.code,
                    aurora_protocol::connector::ErrorCode::Forbidden
                        | aurora_protocol::connector::ErrorCode::NotFound
                ) =>
            {
                match d.admin_token {
                    Some(a) => self.mm.get_q(&a, path, q).await,
                    None => Err(e),
                }
            }
            other => other,
        }
    }

    /// Starts a socket task for every dedicated bot that has none (or whose token
    /// changed), and wakes the running tasks so removed bots' tasks end.
    pub fn sync_bot_sockets(self: &Arc<Self>) {
        let wanted: HashMap<Uuid, String> = self
            .snapshot()
            .presences
            .iter()
            .map(|(id, p)| (*id, p.token.clone()))
            .collect();
        let mut running = lock(&self.bot_sockets);
        running.retain(|id, _| wanted.contains_key(id));
        for (id, token) in wanted {
            if running.get(&id) != Some(&token) {
                running.insert(id, token.clone());
                tokio::spawn(self.clone().run_bot_socket(id, token));
            }
        }
        drop(running);
        self.bots_gen.send_modify(|g| *g += 1);
    }

    pub fn has(&self, cap: &str) -> bool {
        self.snapshot().caps.contains(cap)
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    pub fn bots(&self) -> Bots {
        let d = self.snapshot();
        let mut list = vec![(d.bot_id, d.bot_username)];
        list.extend(
            d.presences
                .values()
                .map(|p| (p.user_id.clone(), p.username.clone())),
        );
        Bots { list }
    }

    // ------------------------------------------------------------ lookups

    pub async fn user(&self, id: &str) -> R<MmUser> {
        if !is_mm_id(id) {
            return Err(OpError::new(
                aurora_protocol::connector::ErrorCode::NotFound,
                "no such user",
            ));
        }
        if let Some((at, u)) = lock(&self.users).get(id)
            && at.elapsed() < USER_TTL
        {
            return Ok(u.clone());
        }
        let v = self.read(&format!("/users/{id}"), &[]).await?;
        let u: MmUser = serde_json::from_value(v).map_err(bad_shape)?;
        lock(&self.users).insert(id.to_owned(), (Instant::now(), u.clone()));
        Ok(u)
    }

    /// Users by lowercase username (negative answers are cached too).
    pub async fn users_by_names(&self, names: &[String]) -> HashMap<String, MmUser> {
        let mut found: HashMap<String, MmUser> = HashMap::new();
        let mut missing: Vec<String> = Vec::new();
        {
            let cache = lock(&self.names);
            for n in names {
                match cache.get(n) {
                    Some((at, u)) if at.elapsed() < USER_TTL => {
                        if let Some(u) = u {
                            found.insert(n.clone(), u.clone());
                        }
                    }
                    _ => missing.push(n.clone()),
                }
            }
        }
        if missing.is_empty() {
            return found;
        }
        let token = self.snapshot().bot_token;
        match self
            .mm
            .post(&token, "/users/usernames", &json!(missing))
            .await
            .and_then(|v| serde_json::from_value::<Vec<MmUser>>(v).map_err(bad_shape))
        {
            Ok(users) => {
                let mut cache = lock(&self.names);
                for n in &missing {
                    let hit = users.iter().find(|u| u.username.eq_ignore_ascii_case(n));
                    cache.insert(n.clone(), (Instant::now(), hit.cloned()));
                    if let Some(u) = hit {
                        found.insert(n.clone(), u.clone());
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e.message, "username lookup failed"),
        }
        found
    }

    pub async fn channel(&self, id: &str) -> R<MmChannel> {
        if !is_mm_id(id) {
            return Err(OpError::new(
                aurora_protocol::connector::ErrorCode::NotFound,
                "no such channel",
            ));
        }
        if let Some((at, c)) = lock(&self.channels).get(id)
            && at.elapsed() < CHANNEL_TTL
        {
            return Ok(c.clone());
        }
        let v = self.read(&format!("/channels/{id}"), &[]).await?;
        let c: MmChannel = serde_json::from_value(v).map_err(bad_shape)?;
        lock(&self.channels).insert(id.to_owned(), (Instant::now(), c.clone()));
        Ok(c)
    }

    pub async fn lookups(&self, refs: &MentionRefs) -> Lookups {
        let mut lk = Lookups::default();
        for id in &refs.users {
            if let Ok(u) = self.user(id).await {
                lk.users.insert(id.clone(), u.username);
            }
        }
        for id in &refs.spaces {
            if let Ok(c) = self.channel(id).await {
                lk.spaces.insert(id.clone(), c.name);
            }
        }
        lk
    }

    pub async fn lookups_for(&self, neutral: &str) -> Lookups {
        self.lookups(&mention_refs(neutral)).await
    }

    async fn file_info(&self, id: &str) -> Option<MmFileInfo> {
        let v = self.read(&format!("/files/{id}/info"), &[]).await.ok()?;
        serde_json::from_value(v).ok()
    }

    // ------------------------------------------------------------- events

    /// Posts an event through the SDK sink and waits for the core's answer.
    /// The platform needs no ack here; the wait keeps ordering honest and logs rejections.
    pub async fn emit<T: Serialize>(
        &self,
        event_id: String,
        kind: EventKind,
        payload: &T,
    ) -> Option<EventAck> {
        let ev = match InboundEvent::new(
            event_id.clone(),
            self.id,
            kind,
            rfc3339_ms(now_ms()),
            payload,
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(error = %e, "event payload does not serialize");
                return None;
            }
        };
        match self.sink.post(ev).await {
            Ok(ack) => {
                if let EventAck::Rejected(code) = &ack {
                    tracing::warn!(%event_id, code = code.as_str(), "core rejected an event");
                }
                Some(ack)
            }
            Err(_) => None,
        }
    }

    /// `posted` (and catch-up): `message.posted`, or `command.invoked` for text commands.
    pub async fn handle_post(&self, post: &MmPost) {
        if !post.kind.is_empty() || post.delete_at > 0 || self.bots().contains(&post.user_id) {
            return;
        }
        let channel = match self.channel(&post.channel_id).await {
            Ok(c) => c,
            Err(e) => return tracing::warn!(error = %e.message, "post in an unknown channel"),
        };
        if !channel.team_id.is_empty() && channel.team_id != self.team_id {
            return;
        }
        let author = match self.user(&post.user_id).await {
            Ok(u) => u,
            Err(e) => return tracing::warn!(error = %e.message, "post by an unknown user"),
        };
        let d = self.snapshot();
        if let Some((name, args)) = command_from_text(&post.message, &channel.kind, &d.bot_username)
        {
            let payload = CommandInvoked {
                name,
                args,
                space: space(&channel),
                thread_ref: (!post.root_id.is_empty()).then(|| post.root_id.clone()),
                user_ref: author.id.clone(),
                response_ref: Some(post.id.clone()),
            };
            self.emit(
                format!("command:{}", post.id),
                EventKind::CommandInvoked,
                &payload,
            )
            .await;
            return;
        }
        let known = self
            .users_by_names(&mentioned_usernames(&post.message))
            .await;
        let mut files = Vec::new();
        for id in &post.file_ids {
            if let Some(f) = self.file_info(id).await {
                files.push(f);
            }
        }
        let payload = message_posted(post, &channel, &author, &files, &known, &self.bots());
        self.emit(
            format!("posted:{}", post.id),
            EventKind::MessagePosted,
            &payload,
        )
        .await;
    }

    async fn handle_changed(&self, post: &MmPost, deleted: bool) {
        if !post.kind.is_empty() || self.bots().contains(&post.user_id) {
            return;
        }
        let Ok(channel) = self.channel(&post.channel_id).await else {
            return;
        };
        if !channel.team_id.is_empty() && channel.team_id != self.team_id {
            return;
        }
        let payload = MessageChanged {
            space_ref: post.channel_id.clone(),
            thread_ref: (!post.root_id.is_empty()).then(|| post.root_id.clone()),
            message_ref: post.id.clone(),
            text: (!deleted).then(|| crate::render::neutralize_tokens(&post.message)),
        };
        let (id, kind) = if deleted {
            (format!("deleted:{}", post.id), EventKind::MessageDeleted)
        } else {
            (
                format!("edited:{}:{}", post.id, post.edit_at.max(post.update_at)),
                EventKind::MessageEdited,
            )
        };
        self.emit(id, kind, &payload).await;
    }

    async fn handle_membership(&self, ev: &WsEvent, added: bool, via: Option<Uuid>) {
        let event_at = now_ms();
        let Some(user_id) = ev.str_field("user_id") else {
            return;
        };
        if !self.bots().contains(user_id) {
            return;
        }
        let Some(channel_id) = ev.channel_id().or_else(|| ev.str_field("channel_id")) else {
            return;
        };
        let Ok(channel) = self.channel(channel_id).await else {
            return;
        };
        if matches!(channel.kind.as_str(), "D" | "G") {
            return;
        }
        let actor_ref = if added {
            self.adder_of(channel_id, user_id, event_at).await
        } else {
            ev.str_field("remover_id")
                .filter(|r| *r != user_id)
                .map(str::to_owned)
        };
        {
            // Every bot in the channel sees the same change on its own socket.
            let key = format!("{added}:{channel_id}:{user_id}");
            let mut recent = lock(&self.recent_membership);
            recent.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(5));
            // Only a copy seen on a different socket is a duplicate; the same change
            // repeated on one socket is a new one (the bot left and came back).
            match recent.get(&key) {
                Some((_, first)) if *first != via => return,
                _ => {
                    recent.insert(key, (Instant::now(), via));
                }
            }
        }
        tracing::debug!(%channel_id, ?actor_ref, added, "bot membership change");
        let payload = BotMembership {
            space: space(&channel),
            bot_ref: user_id.to_owned(),
            actor_ref,
        };
        let (tag, kind) = if added {
            ("bot_added", EventKind::BotAdded)
        } else {
            ("bot_removed", EventKind::BotRemoved)
        };
        self.emit(
            format!("{tag}:{channel_id}:{user_id}:{}", now_ms()),
            kind,
            &payload,
        )
        .await;
    }

    /// Who added the bot: the channel's `system_add_to_channel` post names the adder.
    /// The post is written just after the membership change; the one nearest to
    /// the event wins (the bot may have been added to the same channel before).
    async fn adder_of(&self, channel_id: &str, bot_id: &str, event_at: i64) -> Option<String> {
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let since = (event_at - 60_000).to_string();
            let Ok(v) = self
                .read(
                    &format!("/channels/{channel_id}/posts"),
                    &[("since", &since)],
                )
                .await
            else {
                continue;
            };
            let hit = posts_of(&v)
                .into_iter()
                .filter(|p| p.kind == "system_add_to_channel")
                .filter(|p| p.props.get("addedUserId").and_then(Value::as_str) == Some(bot_id))
                .filter(|p| (p.create_at - event_at).abs() <= 5_000)
                .min_by_key(|p| (p.create_at - event_at).abs());
            if let Some(p) = hit {
                return p
                    .props
                    .get("userId")
                    .and_then(Value::as_str)
                    .filter(|a| *a != bot_id)
                    .map(str::to_owned);
            }
        }
        None
    }

    async fn handle_reaction(&self, ev: &WsEvent) {
        let raw = ev.data.get("reaction");
        let r: Value = match raw {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
            Some(v) => v.clone(),
            None => return,
        };
        let (Some(user), Some(post), Some(emoji)) = (
            r.get("user_id").and_then(Value::as_str),
            r.get("post_id").and_then(Value::as_str),
            r.get("emoji_name").and_then(Value::as_str),
        ) else {
            return;
        };
        if self.bots().contains(user) {
            return;
        }
        let Ok(p) = self.read(&format!("/posts/{post}"), &[]).await else {
            return;
        };
        let Some(channel) = p.get("channel_id").and_then(Value::as_str) else {
            return;
        };
        let at = r
            .get("create_at")
            .and_then(Value::as_i64)
            .unwrap_or_else(now_ms);
        let payload = ReactionAdded {
            space_ref: channel.to_owned(),
            message_ref: post.to_owned(),
            user_ref: user.to_owned(),
            emoji: emoji.to_owned(),
        };
        self.emit(
            format!("reaction:{post}:{user}:{emoji}:{at}"),
            EventKind::ReactionAdded,
            &payload,
        )
        .await;
    }

    async fn handle_user_updated(&self, ev: &WsEvent) {
        let Some(user) = ev.data.get("user") else {
            return;
        };
        let id = user.get("id").and_then(Value::as_str).unwrap_or("");
        if user.get("delete_at").and_then(Value::as_i64).unwrap_or(0) <= 0
            || !self.bots().contains(id)
        {
            return;
        }
        if lock(&self.retired).contains(id) {
            return;
        }
        let presence_id = self
            .snapshot()
            .presences
            .iter()
            .find(|(_, p)| p.user_id == id)
            .map(|(pid, _)| *pid);
        let payload = PresenceRemoved {
            presence_id,
            bot_ref: id.to_owned(),
            reason: PresenceRemovedReason::Deleted,
        };
        self.emit(
            format!("presence_removed:{id}:deleted"),
            EventKind::PresenceRemoved,
            &payload,
        )
        .await;
    }

    pub async fn handle_ws_event(self: &Arc<Self>, ev: WsEvent, via: Option<Uuid>) {
        tracing::debug!(event = %ev.event, seq = ev.seq, "websocket event");
        match ev.event.as_str() {
            "posted" => {
                if let Some(p) = ev.post() {
                    self.handle_post(&p).await;
                }
            }
            "post_edited" => {
                if let Some(p) = ev.post() {
                    self.handle_changed(&p, false).await;
                }
            }
            "post_deleted" => {
                if let Some(p) = ev.post() {
                    self.handle_changed(&p, true).await;
                }
            }
            "user_added" => self.handle_membership(&ev, true, via).await,
            "user_removed" => self.handle_membership(&ev, false, via).await,
            "reaction_added" => self.handle_reaction(&ev).await,
            "user_updated" => self.handle_user_updated(&ev).await,
            _ => {}
        }
    }

    // ----------------------------------------------------------- catch-up

    /// Re-reads recent posts of the bot's channels and DMs and emits them again;
    /// the core's deduplication on `event_id` makes replays harmless.
    pub async fn catch_up(&self) {
        let window = self.settings.catchup_window;
        let since = now_ms() - i64::try_from(window.as_millis()).unwrap_or(i64::MAX / 2);
        let d = self.snapshot();
        // The shared bot and every dedicated bot, each with its own channels and DMs.
        let mut bots: Vec<(String, String)> = vec![(d.bot_id.clone(), d.bot_token.clone())];
        bots.extend(
            d.presences
                .values()
                .map(|p| (p.user_id.clone(), p.token.clone())),
        );
        let mut posts: Vec<MmPost> = Vec::new();
        for (bot_id, token) in &bots {
            let path = format!("/users/{bot_id}/teams/{}/channels", self.team_id);
            let channels: Vec<MmChannel> = match self
                .mm
                .get(token, &path)
                .await
                .and_then(|v| serde_json::from_value(v).map_err(bad_shape))
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e.message, "catch-up could not list channels");
                    continue;
                }
            };
            for ch in channels
                .iter()
                .filter(|c| c.kind != "O" || !c.id.is_empty())
            {
                let s = since.to_string();
                match self
                    .mm
                    .get_q(token, &format!("/channels/{}/posts", ch.id), &[("since", &s)])
                    .await
                {
                    Ok(v) => posts.extend(posts_of(&v)),
                    Err(e) => {
                        tracing::warn!(error = %e.message, channel = %ch.id, "catch-up fetch failed")
                    }
                }
            }
        }
        let posts = dedupe_posts(posts, since);
        tracing::info!(count = posts.len(), "catch-up replays recent posts");
        for p in &posts {
            self.handle_post(p).await;
        }
    }

    async fn catch_up_once(&self) {
        if self.catching_up.swap(true, Ordering::AcqRel) {
            return;
        }
        self.catch_up().await;
        self.catching_up.store(false, Ordering::Release);
    }

    // -------------------------------------------------------------- socket

    async fn report_token_revoked(&self, bot: &Option<(Uuid, String)>, user_id: &str) {
        if lock(&self.retired).contains(user_id) {
            return;
        }
        let payload = PresenceRemoved {
            presence_id: bot.as_ref().map(|(id, _)| *id),
            bot_ref: user_id.to_owned(),
            reason: PresenceRemovedReason::TokenRevoked,
        };
        self.emit(
            format!("presence_removed:{user_id}:token_revoked:{}", now_ms()),
            EventKind::PresenceRemoved,
            &payload,
        )
        .await;
    }

    /// The shared bot's socket; runs until ownership is lost.
    pub async fn run_socket(self: Arc<Self>) {
        self.socket_loop(None).await;
    }

    /// A dedicated bot's socket; ends when its presence is gone or its token changed.
    async fn run_bot_socket(self: Arc<Self>, presence: Uuid, token: String) {
        self.socket_loop(Some((presence, token))).await;
    }

    /// Connect, authenticate, read; reconnect with jittered backoff; catch up after
    /// every (re)connect and on `seq` gaps. `bot` is `None` for the shared bot.
    async fn socket_loop(self: Arc<Self>, bot: Option<(Uuid, String)>) {
        let mut backoff = Duration::from_millis(250);
        let mut auth_reported = false;
        let url = self.mm.websocket_url();
        let mut gen_rx = self.bots_gen.subscribe();
        loop {
            if self.handle.is_closed() {
                return;
            }
            // (user id, token) this socket authenticates as.
            let (user_id, token) = match &bot {
                None => {
                    let d = self.snapshot();
                    (d.bot_id, d.bot_token)
                }
                Some((id, mine)) => match self.snapshot().presences.get(id) {
                    Some(p) if &p.token == mine => (p.user_id.clone(), p.token.clone()),
                    _ => return,
                },
            };
            let conn = tokio::select! {
                c = WsConn::connect(&url, &token) => c,
                () = self.handle.closed() => return,
            };
            match conn {
                Ok(mut conn) => {
                    backoff = Duration::from_millis(250);
                    auth_reported = false;
                    if bot.is_none() {
                        self.ready.store(true, Ordering::Relaxed);
                    }
                    let me = self.clone();
                    tokio::spawn(async move { me.catch_up_once().await });
                    let mut last_seq: Option<i64> = None;
                    let mut check = tokio::time::interval(self.settings.token_check);
                    check.reset();
                    loop {
                        let next = tokio::select! {
                            _ = check.tick() => {
                                let dead = matches!(
                                    self.mm.get(&token, "/users/me").await,
                                    Err(e) if e.code == aurora_protocol::connector::ErrorCode::CredentialDead
                                );
                                if dead {
                                    if !auth_reported {
                                        auth_reported = true;
                                        self.report_token_revoked(&bot, &user_id).await;
                                    }
                                    break;
                                }
                                continue;
                            }
                            n = conn.next_event() => n,
                            () = self.handle.closed() => return,
                            _ = gen_rx.changed(), if bot.is_some() => {
                                // Presences changed: keep going only while ours is unchanged.
                                let still = bot.as_ref().is_some_and(|(id, mine)| {
                                    self.snapshot().presences.get(id).is_some_and(|p| &p.token == mine)
                                });
                                if still { continue } else { return }
                            }
                        };
                        match next {
                            Ok(Some(ev)) => {
                                if let Some(prev) = last_seq
                                    && ev.seq != prev + 1
                                {
                                    tracing::warn!(prev, seq = ev.seq, "websocket sequence gap");
                                    let me = self.clone();
                                    tokio::spawn(async move { me.catch_up_once().await });
                                }
                                last_seq = Some(ev.seq);
                                let me = self.clone();
                                let via = bot.as_ref().map(|(id, _)| *id);
                                tokio::spawn(async move { me.handle_ws_event(ev, via).await });
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(error = %e, "websocket read failed");
                                break;
                            }
                        }
                    }
                    if bot.is_none() {
                        self.ready.store(false, Ordering::Relaxed);
                    }
                }
                Err(WsError::Auth) => {
                    tracing::error!("mattermost refused a bot token on the websocket");
                    if !auth_reported {
                        auth_reported = true;
                        self.report_token_revoked(&bot, &user_id).await;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "websocket connect failed"),
            }
            let jitter = Duration::from_millis(u64::from(now_ms().rem_euclid(250) as u32));
            tokio::select! {
                () = tokio::time::sleep(backoff + jitter) => {}
                () = self.handle.closed() => return,
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }
}

pub fn bad_shape(e: serde_json::Error) -> OpError {
    OpError::new(
        aurora_protocol::connector::ErrorCode::Unavailable,
        format!("unexpected mattermost response: {e}"),
    )
}

/// The posts of a `{order, posts}` list answer.
pub fn posts_of(v: &Value) -> Vec<MmPost> {
    v.get("posts")
        .and_then(Value::as_object)
        .map(|m| {
            m.values()
                .filter_map(|p| serde_json::from_value(p.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// `PUT`/`POST` helper kept here so ops and registration share it.
pub async fn send_json(mm: &Mm, token: &str, method: Method, path: &str, body: &Value) -> R<Value> {
    mm.json(token, method, path, &[], Some(body)).await
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_only_narrow() {
        let declared: BTreeSet<String> = ["threads", "ephemeral", "files", "edits"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let cfg = json!({"_capabilities": {"ephemeral": false, "files": {"max_bytes": 1}, "history": true}});
        let caps = effective_caps(&declared, &cfg);
        assert!(!caps.contains("ephemeral"));
        assert!(caps.contains("files"));
        assert!(caps.contains("threads"));
        // `history` was never declared: the integration cannot widen the set.
        assert!(!caps.contains("history"));
        assert_eq!(effective_caps(&declared, &json!({})), declared);
    }

    #[test]
    fn posts_list_answers_are_flattened() {
        let v =
            json!({"order": ["a"], "posts": {"a": {"id": "a", "message": "x"}, "b": {"id": "b"}}});
        let mut ids: Vec<String> = posts_of(&v).into_iter().map(|p| p.id).collect();
        ids.sort();
        assert_eq!(ids, ["a", "b"]);
        assert!(posts_of(&json!({})).is_empty());
    }
}

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
use crate::lanes::Lanes;
use crate::render::{Lookups, MentionRefs, mention_refs};
use crate::ws::{WsConn, WsError};

const USER_TTL: Duration = Duration::from_secs(300);
const CHANNEL_TTL: Duration = Duration::from_secs(600);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Catch-up reads a channel this many posts at a time, for at most this many pages: 10 000 posts
/// per channel (docs/20 §Transport).
const CATCHUP_PER_PAGE: usize = 200;
const CATCHUP_MAX_PAGES: usize = 50;

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
    /// Which addresses the tenant-supplied server URL may point at (docs/20 §Platform URL).
    pub net: crate::api::NetPolicy,
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
    /// Per-channel ordered handling of socket events (docs/20 §Event ordering).
    lanes: Lanes<(WsEvent, Option<Uuid>)>,
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
        let mm = Mm::with_policy(&server, &settings.net)?;
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
            lanes: Lanes::default(),
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
        let Some(user_id) = ev.member_id() else {
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
        // Like posts and edits: a reaction in another team's channel is not ours.
        match self.channel(channel).await {
            Ok(c) if c.team_id.is_empty() || c.team_id == self.team_id => {}
            _ => return,
        }
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

    /// Queues a socket event on its channel's lane: events of one channel are handled in the
    /// order they arrived (a post and its deletion must not swap places while a lookup is slow),
    /// channels in parallel. Events that name no channel share one lane.
    pub fn dispatch_ws_event(self: &Arc<Self>, ev: WsEvent, via: Option<Uuid>) {
        let key = ev.lane_key();
        let me = self.clone();
        self.lanes.dispatch(&key, (ev, via), move |(ev, via)| {
            let me = me.clone();
            async move { me.handle_ws_event(ev, via).await }
        });
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
        self.catch_up_paged(CATCHUP_PER_PAGE, CATCHUP_MAX_PAGES)
            .await;
    }

    /// The posts of `channel` created at or after `since`, read newest-first a page at a time
    /// (`GET /channels/{id}/posts?page=&per_page=`; `?since=` is not used: Mattermost answers it
    /// with one capped batch and no way to continue, so a longer gap lost its oldest messages).
    /// Stops at a short page, at a page that reaches back past `since`, or after `max_pages`.
    /// Past the cap the oldest posts of the gap are not replayed (the newest `max_pages ×
    /// per_page` are), and that is logged: the window is bounded, not silently shortened.
    async fn channel_posts_since(
        &self,
        token: &str,
        channel: &str,
        since: i64,
        per_page: usize,
        max_pages: usize,
    ) -> Vec<MmPost> {
        let per = per_page.to_string();
        let mut out = Vec::new();
        for page in 0..max_pages {
            let p = page.to_string();
            let v = match self
                .mm
                .get_q(
                    token,
                    &format!("/channels/{channel}/posts"),
                    &[("page", &p), ("per_page", &per)],
                )
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e.message, %channel, page, "catch-up fetch failed");
                    return out;
                }
            };
            let in_page = v.get("order").and_then(Value::as_array).map_or(0, Vec::len);
            let batch = posts_of(&v);
            // The oldest post of the page decides whether the gap is covered: `posts` also holds
            // thread roots outside the page, so only the page's own `order` is looked at.
            let oldest = v
                .get("order")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|id| batch.iter().find(|b| b.id == id))
                .map(|b| b.create_at)
                .min();
            out.extend(batch);
            if in_page < per_page || oldest.is_none_or(|t| t < since) {
                return out;
            }
        }
        tracing::warn!(
            %channel,
            pages = max_pages,
            "catch-up reached its page cap: older posts of the gap are not replayed"
        );
        out
    }

    async fn catch_up_paged(&self, per_page: usize, max_pages: usize) {
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
                posts.extend(
                    self.channel_posts_since(token, &ch.id, since, per_page, max_pages)
                        .await,
                );
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
                                let via = bot.as_ref().map(|(id, _)| *id);
                                self.dispatch_ws_event(ev, via);
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

    // ------------------------------------------------ a fake Mattermost + a fake core (FIX-139)

    use aurora_connector_sdk::{CoreClient, RetryPolicy, spawn_event_poster};
    use aurora_protocol::connector::{EventsRequest, EventsResponse, SharedBot};
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, Request as WmRequest, Respond, ResponseTemplate};

    const TEAM: &str = "team0000000000000000000001";
    const CH: &str = "chan0000000000000000000001";
    const BOT: &str = "bot00000000000000000000001";
    const USER: &str = "user0000000000000000000001";

    /// The fake core accepts every event it is sent.
    struct AcceptAll;
    impl Respond for AcceptAll {
        fn respond(&self, req: &WmRequest) -> ResponseTemplate {
            let ev: EventsRequest = serde_json::from_slice(&req.body).unwrap();
            ResponseTemplate::new(200).set_body_json(EventsResponse {
                accepted: ev.events.iter().map(|e| e.event_id.clone()).collect(),
                duplicates: vec![],
                rejected: vec![],
            })
        }
    }

    /// `GET /channels/{id}/posts` over a fixed history, newest first, answering `page`/`per_page`
    /// like Mattermost (default 60 per page). `since` is ignored, as Mattermost ignores `page` with it.
    struct PagedPosts {
        newest_first: Vec<Value>,
    }
    impl Respond for PagedPosts {
        fn respond(&self, req: &WmRequest) -> ResponseTemplate {
            let q: HashMap<String, String> = req.url.query_pairs().into_owned().collect();
            let page: usize = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(0);
            let per: usize = q.get("per_page").and_then(|v| v.parse().ok()).unwrap_or(60);
            let slice: Vec<&Value> = self
                .newest_first
                .iter()
                .skip(page * per)
                .take(per)
                .collect();
            let order: Vec<&Value> = slice.iter().map(|p| &p["id"]).collect();
            let posts: serde_json::Map<String, Value> = slice
                .iter()
                .map(|p| (p["id"].as_str().unwrap().to_owned(), (*p).clone()))
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({"order": order, "posts": posts}))
        }
    }

    struct Rig {
        integ: Arc<Integ>,
        mm: MockServer,
        core: MockServer,
    }

    async fn rig() -> Rig {
        let core = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/connector/events"))
            .respond_with(AcceptAll)
            .mount(&core)
            .await;
        let mm = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v4/channels/{CH}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": CH, "team_id": TEAM, "type": "O", "name": "general", "display_name": "General"
            })))
            .mount(&mm)
            .await;
        let client =
            Arc::new(CoreClient::new(&core.uri(), "aun_test", RetryPolicy::default()).unwrap());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let sink = spawn_event_poster(client, rx);
        std::mem::forget(tx);
        let settings = Settings {
            public_url: Some("https://connector.test".into()),
            hmac_key: b"k".to_vec(),
            catchup_window: Duration::from_secs(60),
            api_url: None,
            token_check: Duration::from_secs(3600),
            capabilities: None,
            test_control: false,
            net: crate::api::NetPolicy::allow_all(),
        };
        let id = Uuid::from_u128(1);
        let integration = Integration {
            id,
            org_id: Uuid::nil(),
            tenant_ref: format!("mm.test/{TEAM}"),
            tenant_label: "T".into(),
            config: json!({"server_url": mm.uri()}),
            credentials: json!({"bot_token": "bot"}),
            credentials_version: 1,
            shared_bot: Some(SharedBot {
                platform_user_ref: BOT.into(),
                handle: "aurora".into(),
                display_name: "Aurora".into(),
            }),
            status: "active".into(),
        };
        let integ = Integ::from_config(
            &integration,
            &[],
            &BTreeSet::new(),
            sink,
            IntegrationHandle::new(id),
            settings,
        )
        .unwrap();
        Rig {
            integ: Arc::new(integ),
            mm,
            core,
        }
    }

    impl Rig {
        async fn user_lookup(&self, delay: Duration) {
            Mock::given(method("GET"))
                .and(path(format!("/api/v4/users/{USER}")))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_delay(delay)
                        .set_body_json(json!({"id": USER, "username": "alice"})),
                )
                .mount(&self.mm)
                .await;
        }

        /// `(kind, event_id)` of the events the core received, in arrival order.
        async fn core_events(&self) -> Vec<(String, String)> {
            let mut out = Vec::new();
            for r in self.core.received_requests().await.unwrap() {
                if let Ok(req) = serde_json::from_slice::<EventsRequest>(&r.body) {
                    out.extend(
                        req.events
                            .iter()
                            .map(|e| (e.kind.as_str().to_owned(), e.event_id.clone())),
                    );
                }
            }
            out
        }

        /// Waits (bounded) until the core has `n` events, then returns them.
        async fn core_events_n(&self, n: usize) -> Vec<(String, String)> {
            let end = Instant::now() + Duration::from_secs(30);
            loop {
                let ev = self.core_events().await;
                if ev.len() >= n || Instant::now() >= end {
                    return ev;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    fn mm_post(id: &str, create_at: i64, deleted: bool) -> Value {
        json!({"id": id, "create_at": create_at, "update_at": create_at,
               "delete_at": if deleted { create_at + 1 } else { 0 },
               "user_id": USER, "channel_id": CH, "message": "hello", "type": ""})
    }

    fn ws_post_event(kind: &str, post: &Value, seq: i64) -> WsEvent {
        serde_json::from_value(json!({
            "event": kind, "data": {"post": post.to_string()},
            "broadcast": {"channel_id": CH}, "seq": seq
        }))
        .unwrap()
    }

    /// A person posts and deletes straight away while the author lookup is slow: the core must
    /// see `message.posted` and then `message.deleted`, never the deletion first and a "posted"
    /// with the original text after it (a message that would stay visible).
    #[tokio::test]
    async fn a_post_and_its_immediate_deletion_reach_the_core_in_order() {
        let rig = rig().await;
        rig.user_lookup(Duration::from_millis(400)).await;
        let post = mm_post("post0000000000000000000001", now_ms(), false);
        rig.integ
            .dispatch_ws_event(ws_post_event("posted", &post, 1), None);
        rig.integ
            .dispatch_ws_event(ws_post_event("post_deleted", &post, 2), None);
        let ev = rig.core_events_n(2).await;
        let kinds: Vec<&str> = ev.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["message.posted", "message.deleted"], "{ev:?}");
    }

    /// A gap longer than one page: catch-up delivers every post of it, oldest first.
    #[tokio::test]
    async fn catch_up_pages_through_a_gap_longer_than_one_page() {
        const N: usize = 450;
        let rig = rig().await;
        let ids = mount_history(&rig, N).await;

        rig.integ.catch_up().await;

        let got = posted_ids(rig.core_events_n(N).await);
        assert_eq!(got.len(), N, "catch-up lost messages");
        assert_eq!(got, ids, "catch-up replayed out of order");
    }

    /// Past the page cap the oldest posts of the gap are dropped (and logged), the newest are
    /// delivered in order, and the fetch does not go on forever.
    #[tokio::test]
    async fn catch_up_stops_at_its_page_cap_keeping_the_newest_posts() {
        let rig = rig().await;
        let ids = mount_history(&rig, 20).await;

        rig.integ.catch_up_paged(5, 3).await;

        let got = posted_ids(rig.core_events_n(15).await);
        assert_eq!(got, ids[5..], "the newest 15 of 20, oldest first");
        let pages = rig
            .mm
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().ends_with("/posts"))
            .count();
        assert_eq!(pages, 3, "a fourth page was fetched");
    }

    /// Mounts a channel list and a history of `n` posts (ids ascending with age) and returns
    /// the `posted:` event ids the core should see, oldest first.
    async fn mount_history(rig: &Rig, n: usize) -> Vec<String> {
        rig.user_lookup(Duration::ZERO).await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v4/users/{BOT}/teams/{TEAM}/channels")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": CH, "team_id": TEAM, "type": "O", "name": "general"
            }])))
            .mount(&rig.mm)
            .await;
        let base = now_ms() - 30_000;
        let mut history: Vec<Value> = (0..n)
            .map(|i| {
                mm_post(
                    &format!("post{i:022}"),
                    base + i64::try_from(i).unwrap() * 10,
                    false,
                )
            })
            .collect();
        let ids = history
            .iter()
            .map(|p| format!("posted:{}", p["id"].as_str().unwrap()))
            .collect();
        history.reverse();
        Mock::given(method("GET"))
            .and(path_regex(format!("^/api/v4/channels/{CH}/posts$")))
            .respond_with(PagedPosts {
                newest_first: history,
            })
            .mount(&rig.mm)
            .await;
        ids
    }

    fn posted_ids(events: Vec<(String, String)>) -> Vec<String> {
        events.into_iter().map(|(_, id)| id).collect()
    }
}

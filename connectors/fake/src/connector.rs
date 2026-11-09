//! The fake connector: SDK [`Connector`] implementation over [`Platform`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use aurora_channels::{Capability, CapabilitySet, Content, FallbackContext, render_card};
use aurora_connector_sdk::{
    ClientError, Connector, ConnectorError, CoreClient, EventAck, EventSink, IntegrationHandle,
};
use aurora_protocol::common::Validate;
use aurora_protocol::connector::*;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::platform::{FakeUser, Platform, PostedMessage, to_platform_text};

/// Capability names the fake declares by default.
pub const DEFAULT_CAPABILITIES: &[&str] = &[
    "threads",
    "edits",
    "buttons",
    "menus",
    "text_input",
    "reactions",
    "ephemeral",
    "presence_dedicated",
    "presence_shared",
    "email_verified",
    "commands",
    "history",
    "files",
];

pub const MESSAGE_MAX_CHARS: u32 = 16_000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct FakeConnector {
    pub platform: Arc<Mutex<Platform>>,
    names: Vec<String>,
    caps: CapabilitySet,
    sinks: Arc<Mutex<BTreeMap<Uuid, EventSink>>>,
    core: Mutex<Option<Arc<CoreClient>>>,
    publisher: String,
}

impl Default for FakeConnector {
    fn default() -> Self {
        Self::new(DEFAULT_CAPABILITIES)
    }
}

impl FakeConnector {
    /// A connector declaring exactly the capabilities in `names`.
    pub fn new(names: &[&str]) -> Self {
        let caps = CapabilitySet::new(names.iter().filter_map(|n| Capability::from_wire(n).ok()));
        Self {
            platform: Arc::new(Mutex::new(Platform::default())),
            names: names.iter().map(|s| (*s).to_owned()).collect(),
            caps,
            sinks: Arc::new(Mutex::new(BTreeMap::new())),
            core: Mutex::new(None),
            publisher: "aurora".into(),
        }
    }

    /// Manifest publisher (default `aurora`, which the core reserves for the project key: a
    /// core that verifies the manifest signature needs another, trusted publisher).
    pub fn with_publisher(mut self, publisher: &str) -> Self {
        self.publisher = publisher.to_owned();
        self
    }

    /// The names this connector declares as capabilities.
    pub fn capability_names(&self) -> &[String] {
        &self.names
    }

    fn core_client(&self) -> Result<Arc<CoreClient>, OpError> {
        lock(&self.core)
            .clone()
            .ok_or_else(|| OpError::new(ErrorCode::Unavailable, "not connected to a core"))
    }

    /// Posts an inbound event through the SDK sink of `integration` (or the
    /// first open one) and waits for the core's answer.
    pub async fn post_event<T: serde::Serialize>(
        &self,
        integration: Option<Uuid>,
        kind: EventKind,
        payload: &T,
    ) -> Result<EventAck, String> {
        self.post_event_with_id(integration, kind, payload, None)
            .await
    }

    /// Like [`Self::post_event`] with a caller-chosen platform event id (a
    /// redelivered platform event carries the same id).
    pub async fn post_event_with_id<T: serde::Serialize>(
        &self,
        integration: Option<Uuid>,
        kind: EventKind,
        payload: &T,
        event_id: Option<String>,
    ) -> Result<EventAck, String> {
        let (id, sink) = {
            let g = lock(&self.sinks);
            let pick = match integration {
                Some(i) => g.get(&i).map(|s| (i, s.clone())),
                None => g.iter().next().map(|(i, s)| (*i, s.clone())),
            };
            pick.ok_or("no open integration")?
        };
        let event_id = match event_id {
            Some(id) => id,
            None => lock(&self.platform).next_id("fake-ev-"),
        };
        let ev =
            InboundEvent::new(event_id, id, kind, now(), payload).map_err(|e| e.to_string())?;
        sink.post(ev).await.map_err(|e| e.to_string())
    }

    /// A platform-side "link my account" action: asks the core for a sign-in
    /// link for `user_ref` (`POST /link-start`) on `integration` (or the first open one).
    pub async fn link_start(
        &self,
        integration: Option<Uuid>,
        user_ref: &str,
    ) -> Result<LinkStartResponse, String> {
        let id = match integration {
            Some(i) => i,
            None => *lock(&self.sinks)
                .keys()
                .next()
                .ok_or("no open integration")?,
        };
        let client = lock(&self.core).clone().ok_or("not connected to a core")?;
        client
            .link_start(&LinkStartRequest {
                integration_id: id,
                platform_user_ref: user_ref.to_owned(),
            })
            .await
            .map_err(|e| e.to_string())
    }

    pub fn open_integrations(&self) -> Vec<Uuid> {
        lock(&self.sinks).keys().copied().collect()
    }

    fn fail(&self, p: &mut Platform, kind: &OpKind) -> Option<OpError> {
        if p.failures.dead_credentials {
            return Some(OpError::new(
                ErrorCode::CredentialDead,
                "credentials are dead",
            ));
        }
        if p.failures.rate_limit_next > 0 {
            p.failures.rate_limit_next -= 1;
            let mut e = OpError::new(ErrorCode::RateLimited, "slow down");
            e.retry_after_secs = Some(p.failures.rate_limit_retry_after_secs.unwrap_or(1));
            return Some(e);
        }
        if p.failures.unavailable_next > 0 {
            p.failures.unavailable_next -= 1;
            return Some(OpError::new(
                ErrorCode::Unavailable,
                format!("{} unavailable", kind.as_str()),
            ));
        }
        None
    }

    fn need(&self, c: Capability) -> Result<(), OpError> {
        if self.caps.has(&c) {
            Ok(())
        } else {
            Err(OpError::new(
                ErrorCode::Unsupported,
                format!("capability {} not declared", c.as_str()),
            ))
        }
    }

    fn render(
        &self,
        p: &Platform,
        content: &Value,
    ) -> Result<(String, Option<aurora_channels::Card>), OpError> {
        let c: Content = serde_json::from_value(content.clone())
            .map_err(|e| OpError::new(ErrorCode::Invalid, e.to_string()))?;
        let mut text = c
            .text
            .as_deref()
            .map(|t| to_platform_text(t, p))
            .unwrap_or_default();
        let mut card = None;
        if let Some(cd) = &c.card {
            cd.validate()
                .map_err(|e| OpError::new(ErrorCode::Invalid, e.to_string()))?;
            let r = render_card(cd, &self.caps, &FallbackContext::default());
            if let Some(extra) = r.text {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&to_platform_text(&extra, p));
            }
            card = r.card.map(|c| defuse_card(c, p));
        }
        if text.chars().count() > MESSAGE_MAX_CHARS as usize {
            return Err(OpError::new(ErrorCode::TooLarge, "message too long"));
        }
        Ok((text, card))
    }

    /// Runs one op against the platform. `Ok(None)` = effect applied but the result is lost.
    fn run(
        &self,
        op: &Operation,
        downloads: Vec<(String, Vec<u8>)>,
    ) -> Result<Option<Value>, OpError> {
        let payload = op
            .parse()
            .map_err(|e| OpError::new(ErrorCode::Invalid, e.to_string()))?;
        let mut p = lock(&self.platform);
        if let Some(e) = self.fail(&mut p, &op.kind) {
            return Err(e);
        }
        let ok = |v: Value| Ok(Some(v));
        match payload {
            OpPayload::MessagePost(m) => {
                // At-most-once in practice: honour the invisible marker on re-claim.
                if op.maybe_sent
                    && let Some(prev) = p
                        .messages
                        .iter()
                        .find(|x| x.op_marker.as_deref() == Some(m.op_marker.as_str()))
                {
                    return ok(
                        json!({"message_ref": prev.message_ref, "thread_ref": prev.thread_ref}),
                    );
                }
                let (space_ref, thread_in) = match &m.target {
                    Target::Space {
                        space_ref,
                        thread_ref,
                    } => {
                        if !p.spaces.contains_key(space_ref) {
                            return Err(OpError::new(ErrorCode::NotFound, "no such space"));
                        }
                        (space_ref.clone(), thread_ref.clone().unwrap_or_default())
                    }
                    Target::User { user_ref } => {
                        if !p.users.contains_key(user_ref) {
                            return Err(OpError::new(ErrorCode::NotFound, "no such user"));
                        }
                        (p.dm_with(user_ref), String::new())
                    }
                };
                if m.ephemeral_to.is_some() {
                    self.need(Capability::Ephemeral)?;
                }
                let (text, card) = self.render(&p, &m.content)?;
                let message_ref = p.next_id("m");
                let thread_ref = self.caps.thread_ref(
                    if thread_in.is_empty()
                        && p.spaces.get(&space_ref).is_some_and(|s| s.kind != "dm")
                    {
                        message_ref.as_str()
                    } else {
                        thread_in.as_str()
                    },
                );
                let thread_ref = thread_ref.to_owned();
                let author = match &m.sender {
                    Sender::Presence { presence_id } => format!("presence-{presence_id}"),
                    Sender::Shared(_) => "shared-bot".to_owned(),
                };
                for (name, bytes) in downloads {
                    p.attachments.insert((message_ref.clone(), name), bytes);
                }
                p.messages.push(PostedMessage {
                    message_ref: message_ref.clone(),
                    space_ref,
                    thread_ref: thread_ref.clone(),
                    author_ref: author,
                    text,
                    card,
                    ephemeral_to: m.ephemeral_to,
                    op_marker: Some(m.op_marker),
                    files: m.files.iter().map(|f| f.name.clone()).collect(),
                    deleted: false,
                    reactions: vec![],
                    at: now(),
                });
                if p.failures.lose_results > 0 {
                    p.failures.lose_results -= 1;
                    return Ok(None);
                }
                ok(json!({"message_ref": message_ref, "thread_ref": thread_ref}))
            }
            OpPayload::MessageUpdate(m) => {
                self.need(Capability::Edits)?;
                let (text, card) = self.render(&p, &m.content)?;
                let msg = p
                    .find(&m.message_ref)
                    .filter(|x| !x.deleted && x.space_ref == m.space_ref)
                    .ok_or_else(|| OpError::new(ErrorCode::NotFound, "no such message"))?;
                msg.text = text;
                msg.card = card;
                ok(json!({}))
            }
            OpPayload::MessageDelete(m) => {
                let msg = p
                    .find(&m.message_ref)
                    .filter(|x| x.space_ref == m.space_ref)
                    .ok_or_else(|| OpError::new(ErrorCode::NotFound, "no such message"))?;
                msg.deleted = true;
                ok(json!({}))
            }
            OpPayload::ReactionAdd(r) | OpPayload::ReactionRemove(r) => {
                self.need(Capability::Reactions)?;
                let add = op.kind == OpKind::ReactionAdd;
                let by = match &r.sender {
                    Sender::Presence { presence_id } => format!("presence-{presence_id}"),
                    Sender::Shared(_) => "shared-bot".to_owned(),
                };
                let msg = p
                    .find(&r.message_ref)
                    .ok_or_else(|| OpError::new(ErrorCode::NotFound, "no such message"))?;
                let entry = (r.emoji, by);
                if add {
                    if !msg.reactions.contains(&entry) {
                        msg.reactions.push(entry);
                    }
                } else {
                    msg.reactions.retain(|x| *x != entry);
                }
                ok(json!({}))
            }
            OpPayload::StatusSet(_) | OpPayload::StatusClear(_) => {
                self.need(Capability::NativeStatus)?;
                ok(json!({}))
            }
            // Handled asynchronously in `execute` (needs the core client).
            OpPayload::FileFetch(_) => Err(OpError::new(
                ErrorCode::Invalid,
                "file.fetch is executed asynchronously",
            )),
            OpPayload::UserLookup(l) => {
                let users: Vec<Value> = p
                    .users
                    .values()
                    .filter(|u| match (&l.user_ref, &l.query) {
                        (Some(r), _) => &u.reference == r,
                        (None, Some(q)) => {
                            let q = q.to_lowercase();
                            u.handle.to_lowercase().contains(&q)
                                || u.display_name.to_lowercase().contains(&q)
                        }
                        (None, None) => true,
                    })
                    .take(l.limit.unwrap_or(25) as usize)
                    .map(user_json)
                    .collect();
                if l.user_ref.is_some() && users.is_empty() {
                    return Err(OpError::new(ErrorCode::NotFound, "no such user"));
                }
                ok(json!({"users": users}))
            }
            OpPayload::SpacesList(l) => {
                let spaces: Vec<Value> = p
                    .spaces
                    .values()
                    .filter(|s| {
                        l.query
                            .as_ref()
                            .is_none_or(|q| s.label.to_lowercase().contains(&q.to_lowercase()))
                    })
                    .take(l.limit as usize)
                    .map(|s| json!({"ref": s.reference, "kind": s.kind, "label": s.label}))
                    .collect();
                ok(json!({"spaces": spaces}))
            }
            OpPayload::SpaceMembers(m) => {
                let s = p
                    .spaces
                    .get(&m.space_ref)
                    .ok_or_else(|| OpError::new(ErrorCode::NotFound, "no such space"))?;
                let users: Vec<Value> = s
                    .members
                    .iter()
                    .filter_map(|r| p.users.get(r))
                    .map(user_json)
                    .collect();
                ok(json!({"users": users}))
            }
            OpPayload::HistoryRead(h) => {
                self.need(Capability::History)?;
                h.validate()
                    .map_err(|e| OpError::new(ErrorCode::Invalid, e.to_string()))?;
                if !p.spaces.contains_key(&h.space_ref) {
                    return Err(OpError::new(ErrorCode::NotFound, "no such space"));
                }
                let mut msgs: Vec<&PostedMessage> = p
                    .messages
                    .iter()
                    .filter(|m| {
                        m.space_ref == h.space_ref && !m.deleted && m.ephemeral_to.is_none()
                    })
                    .filter(|m| h.thread_ref.as_ref().is_none_or(|t| &m.thread_ref == t))
                    .collect();
                if let Some(b) = &h.before
                    && let Some(i) = msgs.iter().position(|m| &m.message_ref == b)
                {
                    msgs.truncate(i);
                }
                let start = msgs.len().saturating_sub(h.limit as usize);
                let messages: Vec<Value> = msgs[start..]
                    .iter()
                    .map(|m| {
                        json!({
                            "message_ref": m.message_ref,
                            "author": {"ref": m.author_ref, "kind": "bot", "display_name": m.author_ref},
                            "text": m.text, "at": m.at
                        })
                    })
                    .collect();
                ok(json!({"messages": messages}))
            }
            OpPayload::PresenceProvision(a) => {
                let reference = p.next_id("bot");
                let handle = a.agent.handle;
                p.upsert_user(FakeUser {
                    reference: reference.clone(),
                    display_name: a.agent.name,
                    handle: handle.clone(),
                    email: None,
                    email_verified: false,
                    is_bot: true,
                });
                ok(json!({
                    "platform_user_ref": reference, "handle": handle,
                    "credentials": {"token": format!("fake-bot-token-{reference}")}
                }))
            }
            OpPayload::PresenceDeprovision(c) => {
                // Self-contained (RFE-024): payload + `credentials` alone, no integration
                // lookup (it may be gone and never owned by this replica).
                let token = op
                    .credentials
                    .as_ref()
                    .and_then(|v| v.get("token"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if token.is_empty() {
                    return Err(OpError::new(
                        ErrorCode::Forbidden,
                        "the operation carries no credentials",
                    ));
                }
                if token == "dead" {
                    return Err(OpError::new(ErrorCode::CredentialDead, "token rejected"));
                }
                match p.users.remove(&c.platform_user_ref) {
                    Some(_) => ok(json!({})),
                    None => Err(OpError::new(ErrorCode::NotFound, "unknown bot")),
                }
            }
            OpPayload::PresenceRename(c) | OpPayload::PresenceSetIcon(c) => {
                let _ = c;
                ok(json!({}))
            }
            OpPayload::CredentialsValidate(c) => {
                let token = c
                    .credentials
                    .get("token")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if token.is_empty() || token == "dead" {
                    return Err(OpError::new(ErrorCode::CredentialDead, "token rejected"));
                }
                let mut res =
                    json!({"tenant_ref": "fake-tenant", "tenant_label": "Fake Workspace"});
                // `{"token": .., "shared_bot": true}` provisions the integration's shared bot.
                if c.credentials.get("shared_bot").and_then(Value::as_bool) == Some(true) {
                    res["shared_bot"] = json!({
                        "platform_user_ref": "B1", "handle": "daneel", "display_name": "Daneel"
                    });
                }
                ok(res)
            }
            OpPayload::Other => Err(OpError::new(
                ErrorCode::Unsupported,
                "unknown operation kind",
            )),
        }
    }
}

fn user_json(u: &FakeUser) -> Value {
    json!({
        "ref": u.reference, "display_name": u.display_name, "handle": u.handle,
        "email": u.email, "email_verified": u.email_verified, "is_bot": u.is_bot
    })
}

/// Seconds since the epoch as RFC 3339 (UTC).
pub fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // civil-from-days (Howard Hinnant)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[async_trait]
impl Connector for FakeConnector {
    fn manifest(&self) -> ConnectorManifest {
        let mut capabilities: BTreeMap<String, Value> = self
            .names
            .iter()
            .map(|n| (n.clone(), Value::Bool(true)))
            .collect();
        if capabilities.contains_key("files") {
            capabilities.insert(
                "files".into(),
                json!({"max_bytes": 52_428_800, "max_count": 10}),
            );
        }
        ConnectorManifest {
            package: ManifestPackage {
                id: "fake".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                contract: 1,
                publisher: self.publisher.clone(),
            },
            about: ManifestAbout {
                name: "Fake".into(),
                icon_png_b64: None,
                setup_guide_md: Some("Test-only simulated platform.".into()),
            },
            capabilities,
            integration_setup: json!({"fields": []}),
            presence_setup: json!({"dedicated": {"fields": []}}),
            limits: ManifestLimits {
                message_max_chars: Some(MESSAGE_MAX_CHARS),
                writes_per_space_per_minute: Some(600),
            },
            public_callbacks: vec![],
        }
    }

    async fn open_integration(
        &self,
        integration: Integration,
        _presences: Vec<Presence>,
        sink: EventSink,
    ) -> Result<IntegrationHandle, ConnectorError> {
        let handle = IntegrationHandle::new(integration.id);
        lock(&self.sinks).insert(integration.id, sink);
        let sinks = self.sinks.clone();
        let (h, id) = (handle.clone(), integration.id);
        tokio::spawn(async move {
            h.closed().await;
            lock(&sinks).remove(&id);
        });
        Ok(handle)
    }

    async fn update_integration(
        &self,
        _handle: &IntegrationHandle,
        _integration: Integration,
        _presences: Vec<Presence>,
    ) -> Result<(), ConnectorError> {
        Ok(())
    }

    async fn execute(&self, op: Operation) -> OpResult {
        if op.kind == OpKind::FileFetch {
            return match self.file_fetch(&op).await {
                Ok(data) => OpResult::ok(op.op_id, data),
                Err(e) => OpResult::err(op.op_id, e),
            };
        }
        let downloads = match self.download_outbound(&op).await {
            Ok(d) => d,
            Err(e) => return OpResult::err(op.op_id, e),
        };
        match self.run(&op, downloads) {
            Ok(Some(data)) => OpResult::ok(op.op_id, data),
            Ok(None) => std::future::pending().await,
            Err(e) => OpResult::err(op.op_id, e),
        }
    }

    fn attach_core_client(&self, client: Arc<CoreClient>) {
        *lock(&self.core) = Some(client);
    }
}

fn map_client_error(e: &ClientError) -> OpError {
    let code = match e.status() {
        Some(404) => ErrorCode::NotFound,
        Some(413) => ErrorCode::TooLarge,
        Some(400 | 422) => ErrorCode::Invalid,
        Some(403) => ErrorCode::Forbidden,
        _ => ErrorCode::Unavailable,
    };
    OpError::new(code, e.to_string())
}

impl FakeConnector {
    /// `file.fetch`: reads the platform file and uploads it to `PUT /files/{op_id}`.
    async fn file_fetch(&self, op: &Operation) -> Result<Value, OpError> {
        let OpPayload::FileFetch(f) = op
            .parse()
            .map_err(|e| OpError::new(ErrorCode::Invalid, e.to_string()))?
        else {
            return Err(OpError::new(ErrorCode::Invalid, "not a file.fetch payload"));
        };
        self.need(Capability::Files)?;
        let file = {
            let mut p = lock(&self.platform);
            if let Some(e) = self.fail(&mut p, &op.kind) {
                return Err(e);
            }
            p.files
                .get(&f.file_ref)
                .cloned()
                .ok_or_else(|| OpError::new(ErrorCode::NotFound, "no such file"))?
        };
        if file.bytes.len() as u64 > f.max_bytes {
            return Err(OpError::new(
                ErrorCode::TooLarge,
                "file exceeds the operation's max_bytes",
            ));
        }
        let client = self.core_client()?;
        let r = client
            .put_file(op.op_id, &file.name, &file.content_type, file.bytes)
            .await
            .map_err(|e| map_client_error(&e))?;
        Ok(json!(FileFetchResult {
            upload_url_used: true,
            attachment_id: r.attachment_id,
        }))
    }

    /// Downloads the outbound files of a `message.post` from `GET /files/{id}`.
    async fn download_outbound(&self, op: &Operation) -> Result<Vec<(String, Vec<u8>)>, OpError> {
        if op.kind != OpKind::MessagePost {
            return Ok(vec![]);
        }
        let Ok(OpPayload::MessagePost(m)) = op.parse() else {
            return Ok(vec![]);
        };
        if m.files.is_empty() {
            return Ok(vec![]);
        }
        self.need(Capability::Files)?;
        let client = self.core_client()?;
        let mut out = Vec::with_capacity(m.files.len());
        for f in &m.files {
            let bytes = client
                .get_file(f.attachment_id)
                .await
                .map_err(|e| map_client_error(&e))?;
            out.push((f.name.clone(), bytes.to_vec()));
        }
        Ok(out)
    }
}

/// Defuses platform broadcast syntax in every text of a card.
fn defuse_card(card: aurora_channels::Card, p: &Platform) -> aurora_channels::Card {
    use aurora_channels::Block;
    let t = |s: String| to_platform_text(&s, p);
    let blocks = card
        .blocks
        .into_iter()
        .map(|b| match b {
            Block::Header { text } => Block::Header { text: t(text) },
            Block::Text { text } => Block::Text { text: t(text) },
            Block::Context { text } => Block::Context { text: t(text) },
            Block::Fields { pairs } => Block::Fields {
                pairs: pairs
                    .into_iter()
                    .map(|mut f| {
                        f.label = t(f.label);
                        f.value = t(f.value);
                        f
                    })
                    .collect(),
            },
            Block::Link { label, url } => Block::Link {
                label: t(label),
                url,
            },
            Block::Buttons { items } => Block::Buttons {
                items: items
                    .into_iter()
                    .map(|mut i| {
                        i.label = t(i.label);
                        i
                    })
                    .collect(),
            },
            Block::Select {
                placeholder,
                options,
                token,
            } => Block::Select {
                placeholder: t(placeholder),
                options: options
                    .into_iter()
                    .map(|mut o| {
                        o.label = t(o.label);
                        o
                    })
                    .collect(),
                token,
            },
            Block::Input { label, token } => Block::Input {
                label: t(label),
                token,
            },
        })
        .collect();
    aurora_channels::Card { blocks }
}

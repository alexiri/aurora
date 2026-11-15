//! The SDK [`Connector`] implementation for Mattermost (shared mode).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use aurora_connector_sdk::{Connector, ConnectorError, CoreClient, EventSink, IntegrationHandle};
use aurora_protocol::connector::*;
use base64::Engine;
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::{Mm, R};
use crate::integ::{Integ, Settings, lock};
use crate::ops;

/// Capabilities of the manifest (`docs/20 §Manifest`). `presence_dedicated` is narrowed
/// away per integration when there is no working system-admin token.
pub const DEFAULT_CAPABILITIES: &[&str] = &[
    "threads",
    "edits",
    "reactions",
    "buttons",
    "menus",
    "files",
    "commands",
    "history",
    "presence_shared",
    "presence_dedicated",
    "email_verified",
    "ephemeral",
];

pub const MAX_FILE_BYTES: u64 = 52_428_800;

/// The slash-command trigger of integration `id` among the integrations `peers` that serve the
/// same Mattermost team (`peers` includes `id`). A team has one command per trigger, so two
/// integrations sharing `/aurora` would overwrite each other's command and token: the one with the
/// smallest id keeps `aurora`, the others get `aurora-<id prefix>`.
pub fn command_trigger(id: Uuid, peers: &[Uuid]) -> String {
    if peers.iter().all(|p| *p >= id) {
        "aurora".to_owned()
    } else {
        format!("aurora-{}", &id.simple().to_string()[24..])
    }
}

/// The bot's display name as Mattermost shows it: the bot record's `display_name`, or its username
/// when the record is unavailable, empty or shorter than two characters (a single letter is not a
/// name anyone can recognise).
pub fn bot_display_name(bot_record: Option<&Value>, username: &str) -> String {
    bot_record
        .and_then(|b| b.get("display_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| n.chars().count() >= 2)
        .unwrap_or(username)
        .to_owned()
}

/// State shared with the public callbacks.
pub struct Shared {
    pub settings: Settings,
    pub integrations: RwLock<BTreeMap<Uuid, Arc<Integ>>>,
    pub core: Mutex<Option<Arc<CoreClient>>>,
    /// Test profile: the next `n` successful `message.post` ops drop their result
    /// (the claim lease lapses), like a connector crashing after the platform call.
    pub lose_results: AtomicU32,
}

impl Shared {
    pub fn integration(&self, id: Uuid) -> Option<Arc<Integ>> {
        self.integrations
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
    }

    pub fn all(&self) -> Vec<Arc<Integ>> {
        self.integrations
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub fn core_client(&self) -> Option<Arc<CoreClient>> {
        lock(&self.core).clone()
    }
}

pub struct MattermostConnector {
    declared: BTreeSet<String>,
    pub shared: Arc<Shared>,
    publisher: String,
    /// Managed (connector host) instances declare `instance_setup`; the self-hosted binary does
    /// not, so its manifest and digest stay as they were (13 §3.1).
    managed: bool,
}

/// The official Mattermost glyph (Simple Icons 16.37.0, `plugins/builtin/ICONS.md`).
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

impl MattermostConnector {
    pub fn new(settings: Settings) -> Self {
        let declared: BTreeSet<String> = match &settings.capabilities {
            Some(names) => names.iter().cloned().collect(),
            None => DEFAULT_CAPABILITIES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        };
        Self {
            declared,
            shared: Arc::new(Shared {
                settings,
                integrations: RwLock::new(BTreeMap::new()),
                core: Mutex::new(None),
                lose_results: AtomicU32::new(0),
            }),
            publisher: "aurora".into(),
            managed: false,
        }
    }

    /// Marks this instance as run by the connector host: its manifest carries `instance_setup`.
    pub fn managed(mut self) -> Self {
        self.managed = true;
        self
    }

    /// `instance_setup` of a managed Mattermost connector (13 §3.1).
    pub fn instance_setup() -> InstanceSetup {
        InstanceSetup {
            fields: vec![
                json!({"name": "public_url", "label": "Address Mattermost uses to reach Aurora",
                    "type": "url", "required": false, "advanced": true,
                    "help": "Leave empty unless Mattermost reaches Aurora under another name \
                             (this Aurora's address plus /connectors/mattermost is used)."}),
                json!({"name": "callback_hmac_key", "label": "Callback signing key",
                    "type": "generated_secret", "required": false,
                    "help": "Generated by Aurora; signs the buttons and menus it posts."}),
                json!({"name": "catchup_window_secs", "label": "Catch-up window (seconds)",
                    "type": "number", "required": false, "advanced": true, "default": 600,
                    "min": 1, "max": 86400,
                    "help": "How far back events are re-read after a reconnect."}),
                json!({"name": "token_check_secs", "label": "Token check interval (seconds)",
                    "type": "number", "required": false, "advanced": true, "default": 30,
                    "min": 1, "max": 3600,
                    "help": "How often each bot token is checked, since Mattermost keeps a \
                             socket open after a token is revoked."}),
            ],
        }
    }

    pub fn with_publisher(mut self, publisher: &str) -> Self {
        self.publisher = publisher.to_owned();
        self
    }

    pub fn declared(&self) -> &BTreeSet<String> {
        &self.declared
    }

    fn setup_fields() -> Value {
        json!({"fields": [
            {"name": "server_url", "label": "Your Mattermost server", "type": "text",
             "required": true, "store": "config", "placeholder": "https://chat.example.com"},
            {"name": "team", "label": "Team", "type": "text", "required": true,
             "store": "config", "choices": "from_validate"},
            {"name": "bot_token", "label": "Bot access token", "type": "secret", "required": true,
             "store": "credentials"},
            {"name": "admin_token", "label": "System-admin personal access token (optional)",
             "type": "secret", "required": false, "store": "credentials",
             "help": "Enables ephemeral posts and the /aurora slash command."},
            {"name": "shared_bot_display_name", "label": "Shared bot display name", "type": "text",
             "required": false, "store": "config", "default": "Daneel"},
        ]})
    }

    fn setup_guide() -> String {
        "Create a bot account in System Console → Integrations → Bot Accounts, add it to your \
         team and to the channels it should serve, and paste its access token here. An optional \
         system-admin token enables private (ephemeral) replies and the `/aurora` slash command.\n\n\
         **Account linking.** Mattermost's email address only pre-fills the sign-in link flow. \
         Automatic linking by verified email happens only if an organization admin enables \
         `verified_email_autolink` on this integration (off by default).\n\n\
         **Network.** Mattermost must be able to reach this connector's public URL (`/mm/actions`, \
         `/mm/commands`) and the Mattermost setting *Allow untrusted internal connections* must \
         include it. Restrict that port to the Mattermost server's addresses."
            .into()
    }

    fn manifest_caps(&self) -> BTreeMap<String, Value> {
        let all = [
            "threads",
            "edits",
            "reactions",
            "buttons",
            "menus",
            "files",
            "commands",
            "history",
            "presence_shared",
            "presence_dedicated",
            "email_verified",
            "ephemeral",
            "text_input",
            "native_status",
            "user_tokens",
        ];
        all.iter()
            .map(|n| {
                let on = self.declared.contains(*n);
                let v = if *n == "files" && on {
                    json!({"max_bytes": MAX_FILE_BYTES, "max_count": ops::MAX_FILES})
                } else {
                    Value::Bool(on)
                };
                ((*n).to_owned(), v)
            })
            .collect()
    }

    /// Self-contained `presence.deprovision` (RFE-024).
    async fn deprovision(&self, op: &Operation) -> R<Value> {
        if !self.declared.contains("presence_dedicated") {
            return Err(OpError::new(
                ErrorCode::Unsupported,
                "capability presence_dedicated not declared",
            ));
        }
        let OpPayload::PresenceDeprovision(c) = op
            .parse()
            .map_err(|e| OpError::new(ErrorCode::Invalid, e.to_string()))?
        else {
            return Err(OpError::new(ErrorCode::Invalid, "bad payload"));
        };
        crate::presence::deprovision(&self.shared.all(), op, c).await
    }

    /// `credentials.validate` with `mode: "probe"` (C-710, docs/20): the bot's identity and the
    /// teams it belongs to. Read-only: no profile edit, no command registration.
    async fn probe(
        mm: &Mm,
        bot_token: &str,
        admin_token: Option<&str>,
        team: Option<String>,
    ) -> R<Value> {
        let me = mm.get(bot_token, "/users/me").await?;
        let bot_name = me
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let teams = mm.get(bot_token, "/users/me/teams").await?;
        let mut choices: Vec<ValidateChoice> = teams
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                // The team's URL name is the value: create resolves a name or an id, and the
                // dashboard builds `https://<server>/<name>/messages/@<bot>` from it.
                let value = t
                    .get("name")
                    .or_else(|| t.get("id"))
                    .and_then(Value::as_str)?
                    .to_owned();
                let label = t
                    .get("display_name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&value)
                    .chars()
                    .take(200)
                    .collect();
                Some(ValidateChoice { value, label })
            })
            .take(100)
            .collect();
        choices.sort_by_key(|c| c.label.to_lowercase());
        // A team that was typed must still resolve; an unset one is not an error.
        if let Some(t) = team {
            let path = if crate::api::is_mm_id(&t) {
                format!("/teams/{t}")
            } else {
                format!("/teams/name/{t}")
            };
            mm.get(bot_token, &path).await.map_err(|e| {
                if e.code == ErrorCode::NotFound {
                    OpError::new(
                        ErrorCode::Invalid,
                        "team not found, or the bot cannot see it: add the bot to the team",
                    )
                } else {
                    e
                }
            })?;
        }
        let mut warnings = Vec::new();
        match admin_token {
            None => warnings.push(
                "No admin token: replies to individuals arrive as DMs, and /aurora is not \
                 registered."
                    .to_owned(),
            ),
            Some(tok) => match mm.get(tok, "/users/me").await {
                Ok(a) if roles_include_admin(&a) => {}
                Ok(_) => warnings.push(
                    "The admin token is not a system administrator's: it will be ignored."
                        .to_owned(),
                ),
                Err(e) if e.code == ErrorCode::CredentialDead => {
                    return Err(OpError::new(
                        ErrorCode::Invalid,
                        "the admin token was rejected by Mattermost",
                    ));
                }
                Err(_) => warnings.push("The admin token could not be checked.".to_owned()),
            },
        }
        let res = CredentialsValidateResult {
            tenant_ref: String::new(),
            tenant_label: format!("@{bot_name} on {}", mm.host()),
            shared_bot: None,
            capabilities: None,
            choices: BTreeMap::from([("team".to_owned(), choices)]),
            warnings,
        };
        Ok(json!(res))
    }

    /// `credentials.validate` (docs/20 §Integration setup fields).
    async fn validate(&self, v: CredentialsValidateOp) -> R<Value> {
        let invalid = |m: &str| OpError::new(ErrorCode::Invalid, m);
        let text = |o: &Value, k: &str| {
            o.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let server =
            text(&v.config, "server_url").ok_or_else(|| invalid("server_url is required"))?;
        let bot_token =
            text(&v.credentials, "bot_token").ok_or_else(|| invalid("bot_token is required"))?;
        let admin_token = text(&v.credentials, "admin_token");
        // The URL guard runs here, before any request, in probe mode too (C-711).
        let mm = Mm::with_policy(&server, &self.shared.settings.net)
            .map_err(|e| OpError::new(ErrorCode::Invalid, e))?;
        if v.is_probe() {
            return Self::probe(
                &mm,
                &bot_token,
                admin_token.as_deref(),
                text(&v.config, "team"),
            )
            .await;
        }
        let team = text(&v.config, "team").ok_or_else(|| invalid("team is required"))?;

        // The bot identity (401 → credential_dead).
        let me = mm.get(&bot_token, "/users/me").await?;
        let bot_id = me
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let bot_name = me
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let bot_is_admin = roles_include_admin(&me);

        // The team, by name or id.
        let team_path = if crate::api::is_mm_id(&team) {
            format!("/teams/{team}")
        } else {
            format!("/teams/name/{team}")
        };
        let t = mm.get(&bot_token, &team_path).await.map_err(|e| {
            if e.code == ErrorCode::NotFound {
                invalid("team not found, or the bot cannot see it: add the bot to the team")
            } else {
                e
            }
        })?;
        let team_id = t
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let team_label = t
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(&team)
            .to_owned();
        mm.get(&bot_token, &format!("/teams/{team_id}/members/{bot_id}"))
            .await
            .map_err(|e| {
                if e.code == ErrorCode::NotFound {
                    invalid("the bot is not a member of the team: add it in the team's member list")
                } else {
                    e
                }
            })?;

        // Server limits and privacy.
        let cc = mm
            .get_q(&bot_token, "/config/client", &[("format", "old")])
            .await
            .unwrap_or(Value::Null);
        let hides_email = cc.get("ShowEmailAddress").and_then(Value::as_str) == Some("false");

        // The admin token must be a working system-admin token to count.
        let admin_ok = match &admin_token {
            Some(tok) => match mm.get(tok, "/users/me").await {
                Ok(a) => roles_include_admin(&a),
                Err(e) if e.code == ErrorCode::CredentialDead => {
                    return Err(OpError::new(
                        ErrorCode::Invalid,
                        "the admin token was rejected by Mattermost",
                    ));
                }
                Err(_) => false,
            },
            None => false,
        };

        // Apply the display name when the bot may be edited (best effort).
        let wanted = text(&v.config, "shared_bot_display_name");
        // The name Mattermost shows for the bot is the bot record's `display_name` (the user
        // record's `first_name` is something else and was once a single letter).
        let bot_record = {
            let path = format!("/bots/{bot_id}");
            match admin_token.as_deref().filter(|_| admin_ok) {
                Some(a) => mm.get(a, &path).await.ok(),
                None => None,
            }
        };
        let bot_record = match bot_record {
            Some(r) => Some(r),
            None => mm.get(&bot_token, &format!("/bots/{bot_id}")).await.ok(),
        };
        let mut display = bot_display_name(bot_record.as_ref(), &bot_name);
        if let Some(w) = wanted {
            let tok = admin_token
                .as_deref()
                .filter(|_| admin_ok)
                .unwrap_or(&bot_token);
            let patched = mm
                .json::<Value>(
                    tok,
                    Method::PUT,
                    &format!("/bots/{bot_id}"),
                    &[],
                    Some(&json!({"display_name": w})),
                )
                .await;
            match patched {
                Ok(_) => display = w,
                Err(e) => tracing::info!(error = %e.message, "bot display name not applied"),
            }
        }

        let mut caps: BTreeMap<String, Value> = BTreeMap::new();
        if !admin_ok {
            caps.insert("ephemeral".into(), json!(false));
            caps.insert("presence_dedicated".into(), json!(false));
        }
        if hides_email && !bot_is_admin {
            caps.insert("email_verified".into(), json!(false));
        }
        let res = CredentialsValidateResult {
            tenant_ref: format!("{}/{}", mm.host(), team_id),
            tenant_label: format!("{team_label} on {}", mm.host()),
            shared_bot: Some(SharedBot {
                platform_user_ref: bot_id,
                handle: bot_name,
                display_name: display,
            }),
            capabilities: (!caps.is_empty()).then_some(caps),
            choices: BTreeMap::new(),
            warnings: Vec::new(),
        };
        Ok(json!(res))
    }

    /// Registers (or updates) the team slash command `/aurora` and writes its
    /// verification token back with the integration's credentials.
    async fn register_command(shared: Arc<Shared>, integ: Arc<Integ>) {
        let d = integ.snapshot();
        let (Some(admin), Some(public)) =
            (d.admin_token.clone(), shared.settings.public_url.clone())
        else {
            return;
        };
        if d.command_token.is_some() || !d.caps.contains("commands") {
            return;
        }
        let url = format!("{}/mm/commands", public.trim_end_matches('/'));
        let peers: Vec<Uuid> = shared
            .all()
            .iter()
            .filter(|i| i.team_id == integ.team_id)
            .map(|i| i.id)
            .collect();
        let trigger = command_trigger(integ.id, &peers);
        let existing = integ
            .mm
            .get_q(
                &admin,
                "/commands",
                &[("team_id", &integ.team_id), ("custom_only", "true")],
            )
            .await
            .ok()
            .and_then(|v| v.as_array().cloned())
            .and_then(|a| {
                a.into_iter()
                    .find(|c| c.get("trigger").and_then(Value::as_str) == Some(trigger.as_str()))
            });
        let body = json!({
            "team_id": integ.team_id,
            "trigger": trigger,
            "method": "P",
            "url": url,
            "display_name": "Aurora",
            "description": "Talk to your Aurora agents",
            "auto_complete": true,
            "auto_complete_desc": "help, agents, use, link, unlink, bind, unbind, approve, deny",
            "auto_complete_hint": "[command] [args]",
        });
        let cmd = match existing {
            Some(c) => {
                let id = c
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let mut upd = body.clone();
                upd["id"] = json!(id);
                integ
                    .mm
                    .json::<Value>(
                        &admin,
                        Method::PUT,
                        &format!("/commands/{id}"),
                        &[],
                        Some(&upd),
                    )
                    .await
            }
            None => integ.mm.post(&admin, "/commands", &body).await,
        };
        let token = match cmd.map(|c| c.get("token").and_then(Value::as_str).map(str::to_owned)) {
            Ok(Some(t)) if !t.is_empty() => t,
            Ok(_) => return tracing::warn!("slash command registered without a token"),
            Err(e) => {
                return tracing::warn!(error = %e.message, "slash command registration failed");
            }
        };
        let Some(core) = shared.core_client() else {
            return;
        };
        let mut creds =
            json!({"bot_token": d.bot_token, "admin_token": admin, "command_token": token});
        if !creds.is_object() {
            creds = json!({});
        }
        let req = CredentialsRequest {
            target: CredentialTarget {
                integration_id: Some(integ.id),
                presence_id: None,
            },
            expected_version: d.credentials_version,
            credentials: creds,
        };
        match core.write_credentials(&req).await {
            Ok(r) => {
                let mut w = integ.dynamic.write().unwrap_or_else(|e| e.into_inner());
                w.command_token = Some(token);
                w.credentials_version = r.version;
            }
            Err(e) => tracing::warn!(error = %e, "could not store the slash command token"),
        }
    }
}

/// Decrements `n` if it is above zero; true when it did.
fn take_one(n: &AtomicU32) -> bool {
    let mut cur = n.load(Ordering::SeqCst);
    while cur > 0 {
        match n.compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(seen) => cur = seen,
        }
    }
    false
}

fn roles_include_admin(user: &Value) -> bool {
    user.get("roles")
        .and_then(Value::as_str)
        .is_some_and(|r| r.split_whitespace().any(|x| x == "system_admin"))
}

#[async_trait]
impl Connector for MattermostConnector {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            package: ManifestPackage {
                id: "mattermost".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                contract: 1,
                publisher: self.publisher.clone(),
            },
            about: ManifestAbout {
                name: "Mattermost".into(),
                icon_png_b64: Some(base64::engine::general_purpose::STANDARD.encode(ICON_PNG)),
                setup_guide_md: Some(Self::setup_guide()),
            },
            capabilities: self.manifest_caps(),
            integration_setup: Self::setup_fields(),
            presence_setup: json!({"dedicated": {"fields": []}}),
            limits: ManifestLimits {
                message_max_chars: Some(ops::MESSAGE_MAX_CHARS as u32),
                writes_per_space_per_minute: Some(30),
            },
            public_callbacks: vec!["actions".into(), "commands".into()],
            instance_setup: self.managed.then(Self::instance_setup),
        }
    }

    fn attach_core_client(&self, client: Arc<CoreClient>) {
        *lock(&self.shared.core) = Some(client);
    }

    async fn open_integration(
        &self,
        integration: Integration,
        presences: Vec<Presence>,
        sink: EventSink,
    ) -> Result<IntegrationHandle, ConnectorError> {
        let handle = IntegrationHandle::new(integration.id);
        let integ = Arc::new(
            Integ::from_config(
                &integration,
                &presences,
                &self.declared,
                sink,
                handle.clone(),
                self.shared.settings.clone(),
            )
            .map_err(ConnectorError)?,
        );
        self.shared
            .integrations
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(integration.id, integ.clone());
        tokio::spawn(integ.clone().run_socket());
        integ.sync_bot_sockets();
        tokio::spawn(Self::register_command(self.shared.clone(), integ.clone()));
        let shared = self.shared.clone();
        let (h, id) = (handle.clone(), integration.id);
        tokio::spawn(async move {
            h.closed().await;
            shared
                .integrations
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        });
        Ok(handle)
    }

    async fn update_integration(
        &self,
        handle: &IntegrationHandle,
        integration: Integration,
        presences: Vec<Presence>,
    ) -> Result<(), ConnectorError> {
        let integ = self
            .shared
            .integration(handle.id())
            .ok_or_else(|| ConnectorError("integration is not open".into()))?;
        integ.update(&integration, &presences, &self.declared);
        integ.sync_bot_sockets();
        Ok(())
    }

    async fn execute(&self, op: Operation) -> OpResult {
        if op.kind == OpKind::CredentialsValidate {
            let parsed = match op.parse() {
                Ok(OpPayload::CredentialsValidate(v)) => v,
                Ok(_) | Err(_) => {
                    return OpResult::err(
                        op.op_id,
                        OpError::new(ErrorCode::Invalid, "bad payload"),
                    );
                }
            };
            return match self.validate(parsed).await {
                Ok(data) => OpResult::ok(op.op_id, data),
                Err(e) => OpResult::err(op.op_id, e),
            };
        }
        if op.kind == OpKind::PresenceDeprovision {
            // Self-contained (RFE-024): payload + `credentials` alone; the integration may be
            // gone and this replica never owned it.
            return match self.deprovision(&op).await {
                Ok(data) => OpResult::ok(op.op_id, data),
                Err(e) => OpResult::err(op.op_id, e),
            };
        }
        let Some(id) = op.integration_id else {
            return OpResult::err(
                op.op_id,
                OpError::new(ErrorCode::Invalid, "operation without an integration"),
            );
        };
        // Ops can be claimed in the same moment the config poll hands us the
        // integration: give the open a few seconds before giving up.
        let mut integ = self.shared.integration(id);
        for _ in 0..100 {
            if integ.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            integ = self.shared.integration(id);
        }
        let Some(integ) = integ else {
            return OpResult::err(
                op.op_id,
                OpError::new(
                    ErrorCode::Unavailable,
                    "integration is not open on this replica",
                ),
            );
        };
        match ops::execute(&integ, self.shared.core_client(), &op).await {
            Ok(data) => {
                if op.kind == OpKind::MessagePost
                    && self.shared.settings.test_control
                    && take_one(&self.shared.lose_results)
                {
                    tracing::warn!("test profile: dropping the result of a message.post");
                    return std::future::pending().await;
                }
                OpResult::ok(op.op_id, data)
            }
            Err(e) => OpResult::err(op.op_id, e),
        }
    }

    fn router(&self) -> Option<axum::Router> {
        Some(crate::callbacks::router(self.shared.clone()))
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// FIX-128: the bot's name is the bot record's display name; a one-letter value (the `R` owners
    /// saw in "Daneel · via R") or a missing record falls back to the username.
    #[test]
    fn bot_display_name_comes_from_the_bot_record() {
        let rec = json!({"user_id": "x", "username": "daneel", "display_name": "R. Daneel Olivaw"});
        assert_eq!(bot_display_name(Some(&rec), "daneel"), "R. Daneel Olivaw");
        let padded = json!({"display_name": "  Daneel  "});
        assert_eq!(bot_display_name(Some(&padded), "daneel"), "Daneel");
        let short = json!({"display_name": "R"});
        assert_eq!(bot_display_name(Some(&short), "daneel"), "daneel");
        let empty = json!({"display_name": ""});
        assert_eq!(bot_display_name(Some(&empty), "daneel"), "daneel");
        assert_eq!(bot_display_name(Some(&json!({})), "daneel"), "daneel");
        assert_eq!(bot_display_name(None, "daneel"), "daneel");
    }

    use crate::api::NetPolicy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn connector(net: NetPolicy) -> MattermostConnector {
        MattermostConnector::new(Settings {
            public_url: None,
            hmac_key: Vec::new(),
            catchup_window: std::time::Duration::from_secs(60),
            api_url: None,
            token_check: std::time::Duration::from_secs(3600),
            capabilities: None,
            test_control: false,
            net,
        })
    }

    fn probe_op(server: &str, config_extra: Value, credentials: Value) -> CredentialsValidateOp {
        let mut config = json!({"server_url": server});
        if let (Some(c), Some(x)) = (config.as_object_mut(), config_extra.as_object()) {
            c.extend(x.clone());
        }
        CredentialsValidateOp {
            config,
            credentials,
            mode: Some(CredentialsValidateOp::PROBE.to_owned()),
        }
    }

    /// C-710/C-711: a probe lists the bot's teams (sorted by label, id as the value) and only
    /// ever reads: no PUT/POST/PATCH/DELETE reaches Mattermost (no slash command, no profile edit,
    /// no webhook), and the only paths are the two identity reads.
    #[tokio::test]
    async fn probe_lists_the_bots_teams_and_never_writes() {
        let mm = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/users/me"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id": "u1", "username": "daneel"})),
            )
            .mount(&mm)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/users/me/teams"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": "t2", "name": "zeta", "display_name": "Zeta"},
                {"id": "t1", "name": "alpha", "display_name": ""},
            ])))
            .mount(&mm)
            .await;
        let c = connector(NetPolicy::allow_all());
        let v = c
            .validate(probe_op(&mm.uri(), json!({}), json!({"bot_token": "tok"})))
            .await
            .unwrap();
        assert_eq!(
            v["choices"]["team"],
            json!([
                {"value": "alpha", "label": "alpha"},
                {"value": "zeta", "label": "Zeta"}
            ])
        );
        assert_eq!(v["tenant_ref"], "");
        assert!(
            v["tenant_label"]
                .as_str()
                .unwrap()
                .starts_with("@daneel on "),
            "{v}"
        );
        assert!(
            v["warnings"][0]
                .as_str()
                .unwrap()
                .contains("No admin token"),
            "{v}"
        );
        assert!(!v.to_string().contains("tok\""), "{v}");
        let reqs = mm.received_requests().await.unwrap();
        assert!(!reqs.is_empty());
        for r in &reqs {
            assert_eq!(r.method.as_str(), "GET", "{} {}", r.method, r.url);
            assert!(
                matches!(r.url.path(), "/api/v4/users/me" | "/api/v4/users/me/teams"),
                "unexpected read {}",
                r.url
            );
        }
    }

    /// A team typed in probe mode must still resolve; an unset one is not an error.
    #[tokio::test]
    async fn probe_checks_a_typed_team_and_reports_an_unknown_one() {
        let mm = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/users/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"username": "daneel"})))
            .mount(&mm)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/users/me/teams"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&mm)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/teams/name/nope"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "x"})))
            .mount(&mm)
            .await;
        let c = connector(NetPolicy::allow_all());
        let e = c
            .validate(probe_op(
                &mm.uri(),
                json!({"team": "nope"}),
                json!({"bot_token": "tok"}),
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid);
        assert!(!e.message.contains("tok"), "{}", e.message);
    }

    /// The URL guard (C-711 amendment, docs/20 §Transport): a loopback server URL is refused as
    /// `invalid` before **any** request leaves the connector, probe mode included.
    #[tokio::test]
    async fn probe_with_a_guarded_server_url_makes_zero_requests() {
        let mm = MockServer::start().await; // listens on 127.0.0.1
        let c = connector(NetPolicy::default());
        for mode in [Some(CredentialsValidateOp::PROBE.to_owned()), None] {
            let mut op = probe_op(&mm.uri(), json!({"team": "t"}), json!({"bot_token": "tok"}));
            op.mode = mode;
            let e = c.validate(op).await.unwrap_err();
            assert_eq!(e.code, ErrorCode::Invalid);
            assert!(
                !e.message.contains(&mm.uri()),
                "no target text: {}",
                e.message
            );
        }
        assert!(
            mm.received_requests().await.unwrap().is_empty(),
            "the guard must refuse before dialling"
        );
    }

    /// A manifest copy change must keep the digest pins in step: the managed manifest's setup
    /// fields carry the new labels and the `advanced` / `choices` flags.
    #[test]
    fn manifest_copy_and_flags() {
        let c = connector(NetPolicy::allow_all()).managed();
        let m = serde_json::to_value(c.manifest()).unwrap();
        let fields = |k: &str| m[k]["fields"].as_array().unwrap().clone();
        let find = |fs: &[Value], n: &str| fs.iter().find(|f| f["name"] == n).unwrap().clone();
        let inst = fields("instance_setup");
        assert_eq!(
            find(&inst, "public_url")["label"],
            "Address Mattermost uses to reach Aurora"
        );
        for n in ["public_url", "catchup_window_secs", "token_check_secs"] {
            assert_eq!(find(&inst, n)["advanced"], true, "{n}");
        }
        assert!(find(&inst, "callback_hmac_key").get("advanced").is_none());
        let integ = fields("integration_setup");
        assert_eq!(
            find(&integ, "server_url")["label"],
            "Your Mattermost server"
        );
        assert_eq!(find(&integ, "team")["label"], "Team");
        assert_eq!(find(&integ, "team")["choices"], "from_validate");
    }
}

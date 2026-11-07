//! The SDK [`Connector`] implementation for Mattermost (shared mode).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use aurora_connector_sdk::{Connector, ConnectorError, CoreClient, EventSink, IntegrationHandle};
use aurora_protocol::connector::*;
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::{Mm, R};
use crate::integ::{Integ, Settings, lock};
use crate::ops;

/// Capabilities of this task's manifest (`docs/20 §Manifest`). `presence_dedicated`
/// stays off until MM-002 ships bot provisioning.
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
    "email_verified",
    "ephemeral",
];

pub const MAX_FILE_BYTES: u64 = 52_428_800;

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
}

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
            {"name": "server_url", "label": "Server URL", "type": "text", "required": true,
             "store": "config", "placeholder": "https://chat.example.com"},
            {"name": "team", "label": "Team name or id", "type": "text", "required": true,
             "store": "config"},
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
        let team = text(&v.config, "team").ok_or_else(|| invalid("team is required"))?;
        let bot_token =
            text(&v.credentials, "bot_token").ok_or_else(|| invalid("bot_token is required"))?;
        let admin_token = text(&v.credentials, "admin_token");
        let mm = Mm::new(&server).map_err(|e| OpError::new(ErrorCode::Invalid, e))?;

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
        let mut display = me
            .get("first_name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&bot_name)
            .to_owned();
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
                    .find(|c| c.get("trigger").and_then(Value::as_str) == Some("aurora"))
            });
        let body = json!({
            "team_id": integ.team_id,
            "trigger": "aurora",
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
                icon_png_b64: None,
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

//! Outbound helpers of the inbox, commands and mirror: the integration's effective
//! capabilities and typed operation builders (docs/13 §4.2, §4.5).

use crate::domains::channels::capability_enabled;
use crate::domains::connectors::ops::{self, NewOp};
use aurora_channels::{Capability, CapabilitySet, Content};
use aurora_core::ApiError;
use aurora_db::repo::channel_integrations::IntegrationRow;
use aurora_protocol::connector::{MessagePostOp, OutFile, ReactionOp, Sender, StatusOp, Target};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

const CAPABILITY_NAMES: [&str; 15] = [
    "threads",
    "edits",
    "buttons",
    "menus",
    "text_input",
    "reactions",
    "ephemeral",
    "native_status",
    "presence_dedicated",
    "presence_shared",
    "user_tokens",
    "email_verified",
    "files",
    "commands",
    "history",
];

/// An integration with the capabilities it effectively has: the connector manifest's, narrowed
/// by what `credentials.validate` reported (`config._capabilities`).
pub struct Ctx {
    pub integ: IntegrationRow,
    pub caps: CapabilitySet,
    pub manifest: Option<Value>,
}

impl Ctx {
    pub fn new(integ: IntegrationRow, manifest: Option<Value>) -> Self {
        let caps = CapabilitySet::new(CAPABILITY_NAMES.iter().filter_map(|n| {
            capability_enabled(manifest.as_ref(), &integ.config, n)
                .then(|| Capability::from_wire(n).ok())
                .flatten()
        }));
        Self {
            integ,
            caps,
            manifest,
        }
    }

    pub fn id(&self) -> Uuid {
        self.integ.id
    }

    fn shared_field(&self, key: &str) -> Option<String> {
        self.integ
            .shared_bot
            .as_ref()?
            .get(key)?
            .as_str()
            .map(str::to_owned)
    }

    /// The shared bot's platform user.
    pub fn shared_ref(&self) -> Option<String> {
        self.shared_field("platform_user_ref")
    }

    pub fn shared_display_name(&self) -> String {
        self.shared_field("display_name")
            .unwrap_or_else(|| "the bot".to_owned())
    }

    /// The manifest's per-message character limit.
    pub fn message_max_chars(&self) -> usize {
        self.manifest
            .as_ref()
            .and_then(|m| m.pointer("/limits/message_max_chars"))
            .and_then(Value::as_u64)
            .map_or(aurora_channels::content::MAX_TEXT_CHARS, |n| {
                usize::try_from(n).unwrap_or(usize::MAX)
            })
            .min(aurora_channels::content::MAX_TEXT_CHARS)
    }
}

pub fn presence_of(s: &Sender) -> Option<Uuid> {
    match s {
        Sender::Presence { presence_id } => Some(*presence_id),
        Sender::Shared(_) => None,
    }
}

/// `Some(thread)` only for a non-empty thread ref.
pub fn thread_opt(thread_ref: &str) -> Option<String> {
    (!thread_ref.is_empty()).then(|| thread_ref.to_owned())
}

pub fn space_target(space_ref: &str, thread_ref: &str) -> Target {
    Target::Space {
        space_ref: space_ref.to_owned(),
        thread_ref: thread_opt(thread_ref),
    }
}

pub struct Post {
    pub sender: Sender,
    pub target: Target,
    pub content: Content,
    pub files: Vec<OutFile>,
    pub ephemeral_to: Option<String>,
    pub dedupe: String,
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError::internal(e.to_string()))
}

/// Queue a `message.post` (the op engine stamps `op_marker` with the op id).
pub async fn post(conn: &mut PgConnection, ctx: &Ctx, p: Post) -> Result<Uuid, ApiError> {
    let payload = to_value(&MessagePostOp {
        target: p.target,
        sender: p.sender.clone(),
        content: to_value(&p.content)?,
        files: p.files,
        ephemeral_to: p.ephemeral_to,
        op_marker: String::new(),
    })?;
    ops::enqueue(
        conn,
        NewOp {
            kind: "message.post".to_owned(),
            connector_id: None,
            integration_id: Some(ctx.id()),
            presence_id: presence_of(&p.sender),
            payload,
            dedupe_key: Some(p.dedupe),
        },
    )
    .await
}

/// A platform-voice message in the thread everybody sees.
pub async fn say(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sender: &Sender,
    space_ref: &str,
    thread_ref: &str,
    text: &str,
    dedupe: String,
) -> Result<Uuid, ApiError> {
    post(
        conn,
        ctx,
        Post {
            sender: sender.clone(),
            target: space_target(space_ref, thread_ref),
            content: Content::platform(text),
            files: Vec::new(),
            ephemeral_to: None,
            dedupe,
        },
    )
    .await
}

/// A platform-voice message for one person: ephemeral where the platform has it, otherwise a
/// DM that names the space it concerns (docs/13 §4.5).
#[allow(clippy::too_many_arguments)]
pub async fn private(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sender: &Sender,
    space_ref: &str,
    thread_ref: &str,
    user_ref: &str,
    text: &str,
    dedupe: String,
) -> Result<Uuid, ApiError> {
    use aurora_channels::capability::PrivateNotice;
    let (target, ephemeral_to, text) = match ctx.caps.private_notice() {
        PrivateNotice::Ephemeral => (
            space_target(space_ref, thread_ref),
            Some(user_ref.to_owned()),
            text.to_owned(),
        ),
        PrivateNotice::DirectMessage => (
            Target::User {
                user_ref: user_ref.to_owned(),
            },
            None,
            format!(
                "In {}: {text}",
                aurora_channels::content::space_mention(space_ref)
            ),
        ),
    };
    post(
        conn,
        ctx,
        Post {
            sender: sender.clone(),
            target,
            content: Content::platform(&text),
            files: Vec::new(),
            ephemeral_to,
            dedupe,
        },
    )
    .await
}

/// A DM to a person regardless of capabilities.
pub async fn direct(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sender: &Sender,
    user_ref: &str,
    text: &str,
    dedupe: String,
) -> Result<Uuid, ApiError> {
    post(
        conn,
        ctx,
        Post {
            sender: sender.clone(),
            target: Target::User {
                user_ref: user_ref.to_owned(),
            },
            content: Content::platform(text),
            files: Vec::new(),
            ephemeral_to: None,
            dedupe,
        },
    )
    .await
}

async fn enqueue_kind(
    conn: &mut PgConnection,
    ctx: &Ctx,
    kind: &str,
    sender: &Sender,
    payload: Value,
    dedupe: String,
) -> Result<Uuid, ApiError> {
    ops::enqueue(
        conn,
        NewOp {
            kind: kind.to_owned(),
            connector_id: None,
            integration_id: Some(ctx.id()),
            presence_id: presence_of(sender),
            payload,
            dedupe_key: Some(dedupe),
        },
    )
    .await
}

/// `reaction.add` / `reaction.remove` with the neutral emoji name.
pub async fn reaction(
    conn: &mut PgConnection,
    ctx: &Ctx,
    kind: &str,
    sender: &Sender,
    space_ref: &str,
    message_ref: &str,
    dedupe: String,
) -> Result<Uuid, ApiError> {
    let payload = to_value(&ReactionOp {
        space_ref: space_ref.to_owned(),
        message_ref: message_ref.to_owned(),
        sender: sender.clone(),
        emoji: "eyes".to_owned(),
    })?;
    enqueue_kind(conn, ctx, kind, sender, payload, dedupe).await
}

/// `status.set` / `status.clear`.
pub async fn status(
    conn: &mut PgConnection,
    ctx: &Ctx,
    kind: &str,
    sender: &Sender,
    space_ref: &str,
    thread_ref: &str,
    dedupe: String,
) -> Result<Uuid, ApiError> {
    let payload = to_value(&StatusOp {
        space_ref: space_ref.to_owned(),
        thread_ref: thread_opt(thread_ref),
        sender: sender.clone(),
        text: (kind == "status.set").then(|| "Working…".to_owned()),
    })?;
    enqueue_kind(conn, ctx, kind, sender, payload, dedupe).await
}

/// `message.delete`.
pub async fn delete(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sender: &Sender,
    space_ref: &str,
    message_ref: &str,
    dedupe: String,
) -> Result<Uuid, ApiError> {
    let payload = json!({"space_ref": space_ref, "message_ref": message_ref, "sender": sender});
    enqueue_kind(conn, ctx, "message.delete", sender, payload, dedupe).await
}

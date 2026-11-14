//! The platform side of the conformance scenarios.
//!
//! Scenarios must make "the platform" do things (a person posts, clicks a
//! button, the bot is added to a channel, credentials die) and read back what
//! the platform shows (posts, reactions, files). That is inherently specific to
//! one platform, so scenarios talk to a [`PlatformDriver`]. This crate ships
//! [`crate::fake_driver::FakeDriver`] for `connectors/fake` (its `/fake/*`
//! control API). **A real connector's suite needs its own driver** implementing
//! this trait against a test instance of its platform (or a stub of it).

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type Result<T> = anyhow::Result<T>;

#[derive(Debug, Clone)]
pub struct UserSpec {
    pub reference: String,
    pub display_name: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub is_bot: bool,
}

impl UserSpec {
    pub fn person(reference: &str, email: Option<&str>, verified: bool) -> Self {
        Self {
            reference: reference.into(),
            display_name: reference.into(),
            email: email.map(Into::into),
            email_verified: verified,
            is_bot: false,
        }
    }

    pub fn bot(reference: &str) -> Self {
        Self {
            reference: reference.into(),
            display_name: reference.into(),
            email: None,
            email_verified: false,
            is_bot: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpaceSpec {
    pub reference: String,
    /// `dm`, `group_dm`, `channel` or `private`.
    pub kind: String,
    pub label: String,
}

/// A file hosted by the platform (what an inbound message's file ref points at).
#[derive(Debug, Clone)]
pub struct FileSpec {
    pub reference: String,
    pub name: String,
    pub content_type: String,
    /// UTF-8 text content.
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct MessageSpec {
    pub author: String,
    /// `None`: the author's DM with the bot.
    pub space: Option<String>,
    pub thread_ref: Option<String>,
    pub text: String,
    /// Refs of hosted files to attach (see [`PlatformDriver::host_file`]).
    pub files: Vec<(String, String, u64, String)>,
    /// Platform event id; `None` = a fresh one. Reusing an id is a redelivery.
    pub event_id: Option<String>,
    /// Platform message ref; `None` = a fresh one.
    pub message_ref: Option<String>,
}

/// The core's answer to an event the connector submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ack {
    Accepted,
    Duplicate,
    Rejected(String),
}

#[derive(Debug, Clone)]
pub struct Delivered {
    pub ack: Ack,
    pub message_ref: String,
}

#[derive(Debug, Clone)]
pub struct Click {
    pub clicker: String,
    /// The opaque token exactly as the platform showed it on the button/select.
    pub token: String,
    /// The chosen option value (selects).
    pub value: Option<String>,
    pub space_ref: Option<String>,
    pub message_ref: Option<String>,
}

/// A message as the platform shows it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Post {
    pub message_ref: String,
    pub space_ref: String,
    pub thread_ref: String,
    pub author_ref: String,
    pub text: String,
    pub card: Option<Value>,
    pub ephemeral_to: Option<String>,
    pub op_marker: Option<String>,
    pub files: Vec<String>,
    pub deleted: bool,
    pub reactions: Vec<(String, String)>,
}

/// A bot account as the platform shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotProfile {
    pub username: String,
    pub display_name: String,
    /// Disabled/deleted bots cannot post or log in.
    pub active: bool,
    /// The bot has a custom picture.
    pub has_icon: bool,
}

/// An interactive element the platform shows on a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Select {
    pub token: String,
    /// `(label, value)`.
    pub options: Vec<(String, String)>,
}

/// Failure injection every platform stub must be able to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Every platform call answers "credentials are dead".
    DeadCredentials,
    /// The next `n` `message.post` ops take effect on the platform but their
    /// result is lost (the claim lease lapses).
    LoseResults(u32),
}

/// What a platform cannot do the way the neutral scenarios assume. Every field
/// defaults to `true`; a driver turns a field off only for a documented platform
/// limit, and the scenario then skips exactly that assertion (see the listed
/// exclusions in `scripts/conformance.sh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformQuirks {
    /// The platform lets the connector read other users' `email_verified` flag.
    pub email_verified_visible: bool,
    /// A person can have no email address at all.
    pub users_without_email: bool,
    /// The connector serves dedicated presences (bots of its own per agent).
    pub dedicated_presences: bool,
    /// The platform has a person-initiated action that makes the connector call `POST /link-start`.
    pub link_start_action: bool,
    /// The connector exposes public HTTP callbacks (interactive actions, slash
    /// commands) that a stranger could call, so the refusal scenario forges them.
    pub public_callbacks: bool,
}

impl Default for PlatformQuirks {
    fn default() -> Self {
        Self {
            email_verified_visible: true,
            users_without_email: true,
            dedicated_presences: true,
            link_start_action: true,
            public_callbacks: true,
        }
    }
}

/// A forged call to one of the connector's public callbacks. The platform
/// makes these calls itself in normal operation; a stranger can make them too,
/// so each must be refused (HTTP 403) and must never reach the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forgery {
    /// An action callback whose signature is wrong.
    ActionBadSignature,
    /// A correctly signed action naming an integration this connector does not serve.
    ActionUnknownIntegration,
    /// A correctly signed action with no clicker.
    ActionNoClicker,
    /// An action callback with no context at all.
    ActionNoContext,
    /// A slash command carrying a token no integration issued.
    CommandWrongToken,
    /// A slash command for a team no integration serves.
    CommandUnknownTeam,
    /// A slash command with no token.
    CommandNoToken,
}

#[async_trait]
pub trait PlatformDriver: Send + Sync {
    /// Documented platform limits (default: none).
    fn quirks(&self) -> PlatformQuirks {
        PlatformQuirks::default()
    }

    /// Waits until the connector serves an integration (it has polled `config`
    /// after approval and opened the integration).
    async fn wait_ready(&self, timeout: Duration) -> Result<()>;
    /// The capability names the running connector declares.
    async fn capabilities(&self) -> Result<Vec<String>>;

    async fn add_user(&self, u: UserSpec) -> Result<()>;
    async fn add_space(&self, s: SpaceSpec) -> Result<()>;
    async fn host_file(&self, f: FileSpec) -> Result<()>;

    /// A person posts; resolves with the core's answer, which the connector
    /// relays only after it has the answer (ack-after-accept).
    async fn post_message(&self, m: MessageSpec) -> Result<Delivered>;
    async fn click(&self, c: Click) -> Result<Ack>;
    async fn bot_added(&self, space_ref: &str, bot_ref: &str, actor_ref: &str) -> Result<Ack>;
    async fn presence_removed(
        &self,
        bot_ref: &str,
        presence_id: Option<Uuid>,
        reason: &str,
    ) -> Result<Ack>;
    async fn inject(&self, f: Failure) -> Result<()>;
    /// A platform-side "link my account" action: the connector calls the
    /// core's `POST /link-start` for `user_ref` and returns the sign-in URL.
    async fn request_link(&self, user_ref: &str) -> Result<String>;

    /// Calls the connector's public callback with `f`, built from the real
    /// action `token` of a card the connector rendered, and returns the HTTP
    /// status it answered. Only drivers whose quirks say `public_callbacks`.
    async fn forge_callback(&self, _f: Forgery, _message_ref: &str, _token: &str) -> Result<u16> {
        anyhow::bail!("this driver has no public callbacks (PlatformQuirks::public_callbacks)")
    }

    /// Whether `member` (a non-admin person), fetching `message_ref` through
    /// the platform's ordinary client API, is shown the interactive-action
    /// context (the token) of its buttons. `None`: the driver cannot tell.
    async fn member_sees_action_context(
        &self,
        _message_ref: &str,
        _member: &str,
        _token: &str,
    ) -> Result<Option<bool>> {
        Ok(None)
    }

    /// A dedicated bot as the platform shows it (`None`: the driver cannot tell).
    async fn bot_profile(&self, _bot_ref: &str) -> Result<Option<BotProfile>> {
        Ok(None)
    }

    /// Everything posted on the platform (optionally one space), oldest first.
    async fn posts(&self, space_ref: Option<&str>) -> Result<Vec<Post>>;
    /// Bytes of a file attached to a post the connector made.
    async fn attachment(&self, message_ref: &str, name: &str) -> Result<Option<Vec<u8>>>;

    /// Buttons the platform renders on `post`.
    fn buttons(&self, post: &Post) -> Vec<Button>;
    /// The select menu the platform renders on `post`, if any.
    fn select(&self, post: &Post) -> Option<Select>;
    /// Platform-wide broadcast mentions (`@channel`, `@all`, `@here`) that are
    /// *live* in `post` (would notify everyone).
    fn live_broadcasts(&self, post: &Post) -> Vec<String>;
}

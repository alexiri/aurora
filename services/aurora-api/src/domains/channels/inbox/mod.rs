//! Inbox handlers (docs/13 §5.1, BE-072): every inbound event kind except `action.invoked`
//! (BE-073). Registered on the BE-071 processor through [`handlers`].
//!
//! Decisions:
//! * the listen gate (step 6) is evaluated as soon as the target agent is known, before the
//!   speaker is resolved, so channel chatter that nobody addressed never triggers link prompts,
//!   refusals or owner notifications; nothing it drops would have reached an agent either way;
//! * a bot author with no approved person reach is ignored silently (no refusal, no pending
//!   grant), which keeps bots from talking to each other;
//! * `earlier messages` context is fetched before the thread link is created, so a retry after
//!   a transient `connector_unavailable` still sees the thread as new;
//! * the dashboard "notice" for owners is the pending reach grant itself plus an audit entry;
//!   the card DM is raised through [`crate::domains::channels::hooks`].
mod events;
mod message;
pub mod send;
pub(crate) mod speaker;

use crate::app::AppState;
use crate::domains::connectors::inbox::{InboxError, InboxHandler};
use aurora_db::repo::channel_inbox::InboxRow;
use serde_json::{Value, json};
use std::sync::Arc;

pub use speaker::{
    LINK_PROMPT_INTERVAL_SECS, approval_code, approval_code_matches, approval_code_prefix,
    send_link_prompt,
};

/// The handlers this module contributes to [`crate::domains::connectors::inbox::handlers`].
pub fn handlers() -> Vec<Arc<dyn InboxHandler>> {
    vec![Arc::new(ChannelInbox)]
}

const KINDS: [&str; 8] = [
    "message.posted",
    "message.edited",
    "message.deleted",
    "reaction.added",
    "bot.added",
    "bot.removed",
    "presence.removed",
    "command.invoked",
];

struct ChannelInbox;

#[async_trait::async_trait]
impl InboxHandler for ChannelInbox {
    fn handles(&self, kind: &str) -> bool {
        KINDS.contains(&kind)
    }

    async fn handle(&self, state: &AppState, row: &InboxRow) -> Result<Value, InboxError> {
        super::mirror::init(&state.config.external_url);
        match row.kind.as_str() {
            "message.posted" => message::handle(state, row).await,
            _ => events::handle(state, row).await,
        }
    }
}

/// Turns infrastructure errors into a retry.
pub(crate) trait Retry<T> {
    fn retry(self) -> Result<T, InboxError>;
}

impl<T> Retry<T> for Result<T, aurora_core::ApiError> {
    fn retry(self) -> Result<T, InboxError> {
        self.map_err(|e| InboxError::Retry(e.message().to_owned()))
    }
}
impl<T> Retry<T> for Result<T, aurora_db::DbError> {
    fn retry(self) -> Result<T, InboxError> {
        self.map_err(|e| InboxError::Retry(e.to_string()))
    }
}
impl<T> Retry<T> for Result<T, sqlx::Error> {
    fn retry(self) -> Result<T, InboxError> {
        self.map_err(|e| InboxError::Retry(e.to_string()))
    }
}

pub(crate) fn outcome(result: &str) -> Value {
    json!({"result": result})
}

pub(crate) fn ignored(reason: &str) -> Value {
    json!({"result": "ignored", "reason": reason})
}

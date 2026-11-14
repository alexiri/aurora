//! Neutral content `{ text?, card? }` (docs/13 §4.3).
//!
//! Text is CommonMark 0.31 + GFM + github.com's additions + GitHub's raw-HTML allowlist
//! (D-227, `13 §4.3`). Three constructors make the voice explicit:
//! [`Content::untrusted`] turns model/user/plugin/connector text into inert
//! data (SEC-18); [`Content::agent_markdown`] is an agent's own message body, which keeps
//! its Markdown except for D-227's four security exceptions; [`Content::platform`] is for
//! strings authored by the core. `<@here>` comes only from the core: [`Content::with_here`],
//! or a standalone `@here` in an agent's message body ([`Content::agent_markdown`]).

use aurora_core::sanitize;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::card::Card;

/// Maximum characters of neutral text per message.
pub const MAX_TEXT_CHARS: usize = 8000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Content {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
}

impl Content {
    /// Untrusted text rendered as data: controls/bidi stripped, markdown and
    /// mention syntax escaped so nothing becomes active (no links, no `<@here>`).
    pub fn untrusted(raw: &str) -> Self {
        Self {
            text: Some(sanitize::markdown_as_data(raw, MAX_TEXT_CHARS)),
            card: None,
        }
    }

    /// An agent's own message body, for a chat platform (D-226, D-227): turn answers mirrored
    /// to a thread and `send_message` text, **nothing else** (labels, names, card text, notices,
    /// approval summaries and other people's text stay [`Content::untrusted`]). The text keeps
    /// its Markdown byte for byte ([`sanitize::markdown_safe`]: no inline images, link schemes
    /// `http`/`https`/`mailto`, raw HTML within GitHub's allowlist, typed neutral tokens inert);
    /// a standalone `@here` becomes the core's `<@here>` broadcast.
    pub fn agent_markdown(raw: &str) -> Self {
        Self {
            text: Some(sanitize::broadcast_here(&sanitize::markdown_safe(
                raw,
                MAX_TEXT_CHARS,
            ))),
            card: None,
        }
    }

    /// Core-authored text (platform voice). Controls/bidi are still stripped
    /// and mention tokens other than the core's own are neutralised.
    pub fn platform(text: &str) -> Self {
        Self {
            text: Some(defuse_here(&sanitize::block(text, MAX_TEXT_CHARS))),
            card: None,
        }
    }

    pub fn card(card: Card) -> Self {
        Self {
            text: None,
            card: Some(card),
        }
    }

    pub fn with_card(mut self, card: Card) -> Self {
        self.card = Some(card);
        self
    }

    /// Appends an explicit `<@here>` broadcast. Core use only, never from model text.
    pub fn with_here(mut self) -> Self {
        let t = self.text.take().unwrap_or_default();
        self.text = Some(if t.is_empty() {
            "<@here>".to_owned()
        } else {
            format!("<@here> {t}")
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty) && self.card.is_none()
    }
}

/// Mention token for a platform-qualified user. `reference` is sanitized and
/// stripped of token delimiters.
pub fn user_mention(reference: &str) -> String {
    format!("<@user:{}>", mention_ref(reference))
}

/// Mention token for a space.
pub fn space_mention(reference: &str) -> String {
    format!("<#space:{}>", mention_ref(reference))
}

fn mention_ref(reference: &str) -> String {
    sanitize::one_line(reference, 128)
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ' '))
        .collect()
}

/// Platform-voice text may not smuggle a `<@here>` through the plain constructor.
fn defuse_here(s: &str) -> String {
    s.replace("<@here>", "@here")
}

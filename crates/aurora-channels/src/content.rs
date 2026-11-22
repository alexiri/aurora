//! Neutral content `{ text?, card? }` (docs/13 §4.3).
//!
//! Text is CommonMark 0.31 + GFM + the common code-forge additions + the raw-HTML allowlist of 13 §4.3
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
    /// (P4-NARR, C-847) A platform-authored progress card, rendered as data by the connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narration: Option<Narration>,
}

/// Where a narration card's turn stands (C-847).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NarrationStatus {
    Running,
    Done,
    Failed,
    Stopped,
}

/// One tool step of a narration card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NarrationStep {
    /// The tool's name, at most `NARR_STEP_CHARS`.
    pub name: String,
    /// The sanitized summary; present only in a `direct` conversation (C-850).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `running` | `ok` | `failed`.
    pub status: NarrationStepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NarrationStepStatus {
    Running,
    Ok,
    Failed,
}

/// The narration card (C-847): the running form lists the last steps; the collapsed form
/// (D-286, `collapsed: true`) carries only the counts and the elapsed time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Narration {
    pub turn_id: String,
    pub status: NarrationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<NarrationStep>,
    /// Steps cut from the front of `steps` ("N earlier").
    #[serde(default)]
    pub earlier: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Content {
    /// A narration card (C-847). Core use only: never built from model or user text.
    pub fn narration(n: Narration) -> Self {
        Self {
            text: None,
            card: None,
            narration: Some(n),
        }
    }

    /// Untrusted text rendered as data: controls/bidi stripped, markdown and
    /// mention syntax escaped so nothing becomes active (no links, no `<@here>`).
    pub fn untrusted(raw: &str) -> Self {
        Self {
            text: Some(sanitize::markdown_as_data(raw, MAX_TEXT_CHARS)),
            card: None,
            narration: None,
        }
    }

    /// An agent's own message body, for a chat platform (D-226, D-227): turn answers mirrored
    /// to a thread and `send_message` text, **nothing else** (labels, names, card text, notices,
    /// approval summaries and other people's text stay [`Content::untrusted`]). The text keeps
    /// its Markdown byte for byte ([`sanitize::markdown_safe`]: no inline images, link schemes
    /// `http`/`https`/`mailto`, raw HTML within the allowlist of 13 §4.3, typed neutral tokens inert);
    /// a standalone `@here` becomes the core's `<@here>` broadcast.
    pub fn agent_markdown(raw: &str) -> Self {
        Self {
            text: Some(sanitize::broadcast_here(&sanitize::markdown_safe(
                raw,
                MAX_TEXT_CHARS,
            ))),
            card: None,
            narration: None,
        }
    }

    /// Core-authored text (platform voice). Controls/bidi are still stripped
    /// and mention tokens other than the core's own are neutralised.
    pub fn platform(text: &str) -> Self {
        Self {
            text: Some(defuse_here(&sanitize::block(text, MAX_TEXT_CHARS))),
            card: None,
            narration: None,
        }
    }

    pub fn card(card: Card) -> Self {
        Self {
            text: None,
            card: Some(card),
            narration: None,
        }
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
        self.text.as_deref().is_none_or(str::is_empty)
            && self.card.is_none()
            && self.narration.is_none()
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

/// A neutral mention token as it stands between `<` and `>` (`13 §4.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionToken<'a> {
    /// `<@user:REF>`
    User(&'a str),
    /// `<@bot:REF>`
    Bot(&'a str),
    /// `<#space:REF>`
    Space(&'a str),
    /// `<@here>`
    Here,
}

/// A token reference: 1..=128 bytes, no `<`, `>`, space or newline.
pub fn valid_mention_ref(r: &str) -> bool {
    !r.is_empty() && r.len() <= 128 && !r.contains(['<', '>', ' ', '\n'])
}

/// Parses the text between `<` and `>`; `None` when it is not a mention token.
pub fn parse_mention_token(inner: &str) -> Option<MentionToken<'_>> {
    if inner == "@here" {
        return Some(MentionToken::Here);
    }
    if let Some(r) = inner.strip_prefix("@user:") {
        return valid_mention_ref(r).then_some(MentionToken::User(r));
    }
    if let Some(r) = inner.strip_prefix("@bot:") {
        return valid_mention_ref(r).then_some(MentionToken::Bot(r));
    }
    if let Some(r) = inner.strip_prefix("#space:") {
        return valid_mention_ref(r).then_some(MentionToken::Space(r));
    }
    None
}

/// C-1146: a mention handle is `^[@~#][A-Za-z0-9][A-Za-z0-9._-]{0,63}$`.
pub fn valid_mention_handle(h: &str) -> bool {
    let mut cs = h.chars();
    matches!(cs.next(), Some('@' | '~' | '#'))
        && cs.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && h.len() <= 65
        && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Platform-voice text may not smuggle a `<@here>` through the plain constructor.
fn defuse_here(s: &str) -> String {
    s.replace("<@here>", "@here")
}

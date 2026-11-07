//! Capability model and fallback rendering (docs/13 §4.5).

use aurora_core::sanitize;
use aurora_protocol::open_vocab;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::card::{Block, Card};

open_vocab! {
    /// Capability names; unknown names are carried and ignored.
    pub enum Capability {
        Threads = "threads",
        Edits = "edits",
        Buttons = "buttons",
        Menus = "menus",
        TextInput = "text_input",
        Reactions = "reactions",
        Ephemeral = "ephemeral",
        NativeStatus = "native_status",
        PresenceDedicated = "presence_dedicated",
        PresenceShared = "presence_shared",
        UserTokens = "user_tokens",
        EmailVerified = "email_verified",
        Files = "files",
        Commands = "commands",
        History = "history",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct CapabilitySet(Vec<Capability>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleMode {
    EditInPlace,
    PostSettledMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenReceipt {
    Reaction,
    /// Post "Working…" and delete it (or edit it into the answer) when done.
    PlaceholderPost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateNotice {
    Ephemeral,
    /// DM the person, linking to the message concerned.
    DirectMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMethod {
    VerifiedEmail,
    SignInLink,
}

impl CapabilitySet {
    pub fn new(caps: impl IntoIterator<Item = Capability>) -> Self {
        let mut v: Vec<Capability> = Vec::new();
        for c in caps {
            if !v.contains(&c) {
                v.push(c);
            }
        }
        Self(v)
    }

    pub fn has(&self, c: &Capability) -> bool {
        self.0.contains(c)
    }

    /// Without `threads`, `thread_ref` collapses to `""` (whole DM/space).
    pub fn thread_ref<'a>(&self, thread_ref: &'a str) -> &'a str {
        if self.has(&Capability::Threads) {
            thread_ref
        } else {
            ""
        }
    }

    pub fn settle_mode(&self) -> SettleMode {
        if self.has(&Capability::Edits) {
            SettleMode::EditInPlace
        } else {
            SettleMode::PostSettledMessage
        }
    }

    pub fn seen_receipt(&self) -> SeenReceipt {
        if self.has(&Capability::Reactions) {
            SeenReceipt::Reaction
        } else {
            SeenReceipt::PlaceholderPost
        }
    }

    /// `native_status` absent falls back to the same placeholder post.
    pub fn working_status_is_placeholder(&self) -> bool {
        !self.has(&Capability::NativeStatus)
    }

    pub fn private_notice(&self) -> PrivateNotice {
        if self.has(&Capability::Ephemeral) {
            PrivateNotice::Ephemeral
        } else {
            PrivateNotice::DirectMessage
        }
    }

    pub fn link_method(&self) -> LinkMethod {
        if self.has(&Capability::EmailVerified) {
            LinkMethod::VerifiedEmail
        } else {
            LinkMethod::SignInLink
        }
    }

    /// Presence modes offered: (dedicated, shared). If neither is declared,
    /// both are offered (nothing to restrict by).
    pub fn presence_modes(&self) -> (bool, bool) {
        let d = self.has(&Capability::PresenceDedicated);
        let s = self.has(&Capability::PresenceShared);
        if !d && !s { (true, true) } else { (d, s) }
    }

    /// `user_tokens` absent hides read/send-as-user features.
    pub fn user_token_features(&self) -> bool {
        self.has(&Capability::UserTokens)
    }

    /// `history` absent: no mid-thread context, `read_conversation` unavailable.
    pub fn can_read_history(&self) -> bool {
        self.has(&Capability::History)
    }

    /// `commands` absent: settings only in the dashboard.
    pub fn settings_via_commands(&self) -> bool {
        self.has(&Capability::Commands)
    }

    /// `files`: the effective limit is min(core cap, platform cap); no `files` => 0.
    pub fn file_limit(&self, core_cap: u64, platform_cap: u64) -> u64 {
        if self.has(&Capability::Files) {
            core_cap.min(platform_cap)
        } else {
            0
        }
    }
}

/// Inputs for card fallbacks.
#[derive(Debug, Clone, Default)]
pub struct FallbackContext<'a> {
    /// Dashboard URL for the thing the card concerns.
    pub dashboard_url: Option<&'a str>,
    /// First 8 base32 chars of the approval id; enables `approve`/`deny` commands.
    pub approval_code: Option<&'a str>,
}

/// A card as it can be shown on a given platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Card with unsupported interactive blocks removed (`None` if nothing is left).
    pub card: Option<Card>,
    /// Platform-voice text that replaces the removed interactions.
    pub text: Option<String>,
}

/// Degrades `card` to `caps`: interactive blocks the platform lacks (`buttons`,
/// `menus`, `text_input`) are dropped and replaced by text: a dashboard link
/// plus, when `commands` exists, `approve <code>` / `deny <code>` (approval
/// cards) or numbered options to reply with.
pub fn render_card(card: &Card, caps: &CapabilitySet, ctx: &FallbackContext<'_>) -> Rendered {
    let mut kept = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut degraded = false;
    let commands = caps.has(&Capability::Commands);
    let approval = ctx.approval_code.map(code_clean).filter(|c| !c.is_empty());

    for b in &card.blocks {
        match b {
            Block::Buttons { items } if !caps.has(&Capability::Buttons) => {
                degraded = true;
                if commands && approval.is_none() {
                    for (i, it) in items.iter().enumerate() {
                        lines.push(format!("{}. {}", i + 1, sanitize::label(&it.label)));
                    }
                }
            }
            Block::Select { options, .. } if !caps.has(&Capability::Menus) => {
                degraded = true;
                if commands {
                    for (i, o) in options.iter().enumerate() {
                        lines.push(format!("{}. {}", i + 1, sanitize::label(&o.label)));
                    }
                }
            }
            Block::Input { .. } if !caps.has(&Capability::TextInput) => degraded = true,
            other => kept.push(other.clone()),
        }
    }

    let mut text = Vec::new();
    if degraded {
        if let Some(url) = ctx.dashboard_url {
            text.push(format!(
                "Open the dashboard: {}",
                sanitize::one_line(url, 2048)
            ));
        }
        if commands && let Some(code) = &approval {
            text.push(format!("Reply `approve {code}` or `deny {code}`."));
        }
        if !lines.is_empty() {
            text.push("Reply with a number:".to_owned());
            text.append(&mut lines);
        }
    }
    Rendered {
        card: (!kept.is_empty()).then_some(Card { blocks: kept }),
        text: (!text.is_empty()).then(|| text.join("\n")),
    }
}

fn code_clean(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
}

//! In-memory platform simulation: users, spaces, threads, DMs, bots, messages,
//! reactions and failure injection.

use std::collections::{BTreeMap, BTreeSet};

use aurora_channels::Card;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakeUser {
    #[serde(rename = "ref")]
    pub reference: String,
    pub display_name: String,
    #[serde(default)]
    pub handle: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default)]
    pub is_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakeSpace {
    #[serde(rename = "ref")]
    pub reference: String,
    /// `dm`, `group_dm`, `channel` or `private`.
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub members: BTreeSet<String>,
}

/// A message as the platform shows it (what tests read back).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostedMessage {
    pub message_ref: String,
    pub space_ref: String,
    pub thread_ref: String,
    pub author_ref: String,
    /// Platform-markup text after conversion, broadcast defusing and card fallback.
    pub text: String,
    pub card: Option<Card>,
    pub ephemeral_to: Option<String>,
    pub op_marker: Option<String>,
    pub files: Vec<String>,
    pub deleted: bool,
    pub reactions: Vec<(String, String)>,
    pub at: String,
}

/// Failure injection switches (`POST /fake/failures`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failures {
    /// Every op answers `credential_dead`.
    #[serde(default)]
    pub dead_credentials: bool,
    /// The next N ops answer `rate_limited`.
    #[serde(default)]
    pub rate_limit_next: u32,
    #[serde(default)]
    pub rate_limit_retry_after_secs: Option<u32>,
    /// The next N `message.post` ops take effect but their result never leaves
    /// (the lease lapses; the core re-claims with `maybe_sent`).
    #[serde(default)]
    pub lose_results: u32,
    /// The next N ops answer `unavailable`.
    #[serde(default)]
    pub unavailable_next: u32,
}

/// A file hosted by the simulated platform (what `file.fetch` downloads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeFile {
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct Platform {
    /// Inbound files by platform file ref.
    pub files: BTreeMap<String, FakeFile>,
    /// Bytes of files attached to posted messages, by `(message_ref, file name)`.
    pub attachments: BTreeMap<(String, String), Vec<u8>>,
    pub users: BTreeMap<String, FakeUser>,
    pub spaces: BTreeMap<String, FakeSpace>,
    pub messages: Vec<PostedMessage>,
    pub failures: Failures,
    pub counter: u64,
}

impl Platform {
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}{}", self.counter)
    }

    pub fn upsert_user(&mut self, mut u: FakeUser) {
        if u.handle.is_empty() {
            u.handle = u.reference.to_lowercase();
        }
        self.users.insert(u.reference.clone(), u);
    }

    pub fn upsert_space(&mut self, s: FakeSpace) {
        self.spaces.insert(s.reference.clone(), s);
    }

    /// The DM space between `user_ref` and any bot (created on demand).
    pub fn dm_with(&mut self, user_ref: &str) -> String {
        let r = format!("dm-{user_ref}");
        self.spaces.entry(r.clone()).or_insert_with(|| FakeSpace {
            reference: r.clone(),
            kind: "dm".into(),
            label: String::new(),
            members: BTreeSet::from([user_ref.to_owned()]),
        });
        r
    }

    pub fn is_bot(&self, user_ref: &str) -> bool {
        self.users.get(user_ref).is_some_and(|u| u.is_bot)
    }

    pub fn find(&mut self, message_ref: &str) -> Option<&mut PostedMessage> {
        self.messages
            .iter_mut()
            .find(|m| m.message_ref == message_ref)
    }
}

/// Neutral markup -> fake platform markup. Platform broadcast syntax typed by
/// anyone is defused (zero-width space); only the neutral `<@here>` token
/// (which only the core emits) becomes a live `@here`.
pub fn to_platform_text(neutral: &str, p: &Platform) -> String {
    const HERE: char = '\u{E000}';
    let mut s = neutral.replace("<@here>", &HERE.to_string());
    for word in ["@channel", "@all", "@here"] {
        let defused = format!("@\u{200b}{}", &word[1..]);
        s = s.replace(word, &defused);
    }
    s = s.replace(HERE, "@here");
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(start) = rest.find("<") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let token = &tail[1..end];
        if let Some(r) = token.strip_prefix("@user:").or(token.strip_prefix("@bot:")) {
            let handle = p.users.get(r).map_or(r, |u| u.handle.as_str());
            out.push('@');
            out.push_str(handle);
        } else if let Some(r) = token.strip_prefix("#space:") {
            let label = p.spaces.get(r).map_or(r, |s| s.label.as_str());
            out.push('#');
            out.push_str(label);
        } else {
            out.push_str(&tail[..=end]);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

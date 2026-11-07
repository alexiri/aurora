//! Mattermost payloads → neutral inbound events (docs/20 §Transport).
//!
//! Pure mapping: the connector fetches the channel, author, mentioned users
//! and file metadata, then calls these functions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use aurora_protocol::connector::{
    Author, AuthorKind, FileRef, MAX_EVENT_TEXT_CHARS, Mention, MentionKind, MessagePosted, Space,
    SpaceKind,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::render::neutralize_tokens;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MmPost {
    pub id: String,
    pub create_at: i64,
    pub update_at: i64,
    pub edit_at: i64,
    pub delete_at: i64,
    pub user_id: String,
    pub channel_id: String,
    pub root_id: String,
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub props: Value,
    pub file_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MmChannel {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub display_name: String,
    pub name: String,
    pub team_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MmUser {
    pub id: String,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub nickname: String,
    pub email: String,
    pub email_verified: bool,
    pub is_bot: bool,
    pub delete_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MmFileInfo {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime_type: String,
}

/// One WebSocket frame that is an event (`{"event": …, "data": …, "broadcast": …, "seq": …}`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WsEvent {
    pub event: String,
    pub data: Value,
    pub broadcast: Value,
    pub seq: i64,
}

impl WsEvent {
    pub fn parse(frame: &str) -> Option<Self> {
        let e: WsEvent = serde_json::from_str(frame).ok()?;
        (!e.event.is_empty()).then_some(e)
    }

    /// `data.post` (Mattermost double-encodes it as a JSON string).
    pub fn post(&self) -> Option<MmPost> {
        let raw = self.data.get("post")?;
        match raw {
            Value::String(s) => serde_json::from_str(s).ok(),
            other => serde_json::from_value(other.clone()).ok(),
        }
    }

    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(Value::as_str)
    }

    /// The user a membership event is about: `data.user_id`, or, in the copy a removed
    /// user receives about themself (`data` then has only `channel_id` and `remover_id`),
    /// the broadcast's `user_id`.
    pub fn member_id(&self) -> Option<&str> {
        self.str_field("user_id").or_else(|| {
            self.broadcast
                .get("user_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
    }

    pub fn channel_id(&self) -> Option<&str> {
        self.broadcast
            .get("channel_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }
}

/// Mattermost channel type → neutral space kind.
pub fn space_kind(channel_type: &str) -> SpaceKind {
    match channel_type {
        "D" => SpaceKind::Dm,
        "G" => SpaceKind::GroupDm,
        "P" => SpaceKind::Private,
        _ => SpaceKind::Channel,
    }
}

pub fn space(ch: &MmChannel) -> Space {
    let label = match ch.kind.as_str() {
        "D" => String::new(),
        _ if !ch.display_name.is_empty() => ch.display_name.clone(),
        _ => ch.name.clone(),
    };
    Space {
        reference: ch.id.clone(),
        kind: space_kind(&ch.kind),
        label,
    }
}

pub fn display_name(u: &MmUser) -> String {
    if !u.nickname.is_empty() {
        return u.nickname.clone();
    }
    let full = format!("{} {}", u.first_name, u.last_name);
    let full = full.trim();
    if full.is_empty() {
        u.username.clone()
    } else {
        full.to_owned()
    }
}

/// The author block. `email` is whatever Mattermost shows the bot (it only
/// pre-fills the sign-in flow, RFE-001). `email_verified` is `Some(true)` only
/// when the API says so; Mattermost omits the flag when false, and (12.x) for
/// every user other than the caller, so absence means "unknown", never
/// "verified". The core auto-links only on `Some(true)`.
pub fn author(u: &MmUser) -> Author {
    let has_email = !u.email.is_empty();
    Author {
        reference: u.id.clone(),
        kind: if u.is_bot {
            AuthorKind::Bot
        } else {
            AuthorKind::Person
        },
        display_name: display_name(u),
        email: has_email.then(|| u.email.clone()),
        email_verified: (has_email && u.email_verified).then_some(true),
    }
}

static MENTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)@([a-z0-9][a-z0-9._-]*)").unwrap_or_else(|_| unreachable!()));

/// Usernames written as `@name` in `text` (lowercased, trailing `.`/`-` trimmed variants included).
pub fn mentioned_usernames(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in MENTION_RE.captures_iter(text) {
        let name = c[1].to_ascii_lowercase();
        let trimmed = name.trim_end_matches(['.', '-']).to_owned();
        for n in [name, trimmed] {
            if !n.is_empty() && !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// The bots of this integration (echo guard, `bot_mentioned`, `addressed_bot_ref`).
#[derive(Debug, Clone, Default)]
pub struct Bots {
    /// `(user id, username)`
    pub list: Vec<(String, String)>,
}

impl Bots {
    pub fn contains(&self, id: &str) -> bool {
        self.list.iter().any(|(i, _)| i == id)
    }
}

/// Rewrites `@name` to neutral mention tokens for the users in `known`
/// (keyed by lowercase username) and collects the mentions.
fn convert_mentions(text: &str, known: &HashMap<String, MmUser>) -> (String, Vec<Mention>) {
    let mut out = String::with_capacity(text.len());
    let mut mentions: Vec<Mention> = Vec::new();
    let mut last = 0;
    for m in MENTION_RE.find_iter(text) {
        // `@` must start a word: `a@b.c` is an email, not a mention.
        let prev = text[..m.start()].chars().next_back();
        if prev.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-')) {
            continue;
        }
        let word = &m.as_str()[1..];
        let lower = word.to_ascii_lowercase();
        let mut cand: &str = &lower;
        let resolved = loop {
            if let Some(u) = known.get(cand) {
                break Some((cand.len(), u));
            }
            match cand.strip_suffix(['.', '-']) {
                Some(shorter) if !shorter.is_empty() => cand = shorter,
                _ => break None,
            }
        };
        if let Some((len, u)) = resolved {
            out.push_str(&text[last..m.start()]);
            let (kind, tag) = if u.is_bot {
                (MentionKind::Bot, "bot")
            } else {
                (MentionKind::User, "user")
            };
            out.push_str(&format!("<@{tag}:{}>", u.id));
            last = m.start() + 1 + len;
            if !mentions.iter().any(|x| x.reference == u.id) {
                mentions.push(Mention {
                    kind,
                    reference: u.id.clone(),
                });
            }
        } else if matches!(lower.as_str(), "channel" | "all" | "here")
            && !mentions.iter().any(|x| x.reference == lower)
        {
            mentions.push(Mention {
                kind: MentionKind::Broadcast,
                reference: lower,
            });
        }
    }
    out.push_str(&text[last..]);
    (out, mentions)
}

/// Builds the `message.posted` payload. `known` maps lowercase usernames to the
/// users resolved from the post text.
pub fn message_posted(
    post: &MmPost,
    channel: &MmChannel,
    author_user: &MmUser,
    files: &[MmFileInfo],
    known: &HashMap<String, MmUser>,
    bots: &Bots,
) -> MessagePosted {
    let escaped = neutralize_tokens(&post.message);
    let (mut text, mentions) = convert_mentions(&escaped, known);
    if text.chars().count() > MAX_EVENT_TEXT_CHARS {
        text = text.chars().take(MAX_EVENT_TEXT_CHARS).collect();
    }
    let mentioned_bot = mentions
        .iter()
        .find(|m| bots.contains(&m.reference))
        .map(|m| m.reference.clone());
    let dm_bot = (channel.kind == "D")
        .then(|| {
            bots.list
                .iter()
                .find(|(id, _)| channel.name.split("__").any(|p| p == id))
                .map(|(id, _)| id.clone())
        })
        .flatten();
    let space = space(channel);
    MessagePosted {
        space,
        thread_ref: (!post.root_id.is_empty()).then(|| post.root_id.clone()),
        message_ref: post.id.clone(),
        author: author(author_user),
        text,
        mentions,
        bot_mentioned: mentioned_bot.is_some() || dm_bot.is_some(),
        addressed_bot_ref: mentioned_bot.or(dm_bot).unwrap_or_default(),
        files: files
            .iter()
            .map(|f| FileRef {
                reference: f.id.clone(),
                name: f.name.clone(),
                size: f.size,
                content_type: f.mime_type.clone(),
            })
            .collect(),
    }
}

/// Commands recognised in plain text when slash commands are not registered.
pub const TEXT_COMMANDS: &[&str] = &[
    "help", "agents", "use", "link", "unlink", "bind", "unbind", "approve", "deny",
];

/// `!<command> args` in a DM, `@bot <command> args` in a channel.
pub fn command_from_text(
    text: &str,
    channel_type: &str,
    bot_username: &str,
) -> Option<(String, String)> {
    let t = text.trim();
    let body = if channel_type == "D" {
        t.strip_prefix('!')?
    } else {
        let rest = t.strip_prefix('@')?;
        let name_len = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        if !rest[..name_len].eq_ignore_ascii_case(bot_username) {
            return None;
        }
        rest[name_len..].trim_start()
    };
    let (name, args) = match body.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (body, ""),
    };
    let name = name.to_ascii_lowercase();
    TEXT_COMMANDS
        .contains(&name.as_str())
        .then(|| (name, args.to_owned()))
}

/// Deduplicates a catch-up fetch: one entry per post id, no system posts, no
/// deleted posts, oldest first. (Replays across passes are the core's job.)
pub fn dedupe_posts(posts: impl IntoIterator<Item = MmPost>, since_ms: i64) -> Vec<MmPost> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: BTreeMap<(i64, String), MmPost> = BTreeMap::new();
    for p in posts {
        if p.id.is_empty() || !p.kind.is_empty() || p.delete_at > 0 || p.create_at < since_ms {
            continue;
        }
        if seen.insert(p.id.clone()) {
            out.insert((p.create_at, p.id.clone()), p);
        }
    }
    out.into_values().collect()
}

/// Epoch milliseconds → RFC 3339 (UTC).
pub fn rfc3339_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
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

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn user(id: &str, name: &str, bot: bool) -> MmUser {
        MmUser {
            id: id.into(),
            username: name.into(),
            first_name: "Al".into(),
            last_name: "Ice".into(),
            email: format!("{name}@example.com"),
            email_verified: true,
            is_bot: bot,
            ..Default::default()
        }
    }

    fn known(us: &[MmUser]) -> HashMap<String, MmUser> {
        us.iter()
            .map(|u| (u.username.to_ascii_lowercase(), u.clone()))
            .collect()
    }

    fn post(text: &str, root: &str) -> MmPost {
        MmPost {
            id: "p1".into(),
            channel_id: "c1".into(),
            user_id: "u1".into(),
            root_id: root.into(),
            message: text.into(),
            create_at: 1_700_000_000_000,
            ..Default::default()
        }
    }

    fn chan(kind: &str, name: &str) -> MmChannel {
        MmChannel {
            id: "c1".into(),
            kind: kind.into(),
            display_name: "General".into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn bots() -> Bots {
        Bots {
            list: vec![("b1".into(), "daneel".into())],
        }
    }

    #[test]
    fn channel_types_map_to_space_kinds() {
        assert_eq!(space_kind("D"), SpaceKind::Dm);
        assert_eq!(space_kind("G"), SpaceKind::GroupDm);
        assert_eq!(space_kind("O"), SpaceKind::Channel);
        assert_eq!(space_kind("P"), SpaceKind::Private);
        assert_eq!(space(&chan("D", "a__b")).label, "");
        assert_eq!(space(&chan("O", "general")).label, "General");
    }

    #[test]
    fn channel_message_with_bot_mention() {
        let k = known(&[user("b1", "daneel", true), user("u2", "bob", false)]);
        let m = message_posted(
            &post("hey @daneel and @Bob, @here: summarize.", ""),
            &chan("O", "general"),
            &user("u1", "alice", false),
            &[],
            &k,
            &bots(),
        );
        assert_eq!(m.text, "hey <@bot:b1> and <@user:u2>, @here: summarize.");
        assert!(m.bot_mentioned);
        assert_eq!(m.addressed_bot_ref, "b1");
        assert_eq!(m.thread_ref, None);
        assert_eq!(m.space.kind, SpaceKind::Channel);
        assert!(
            m.mentions
                .iter()
                .any(|x| x.kind == MentionKind::Bot && x.reference == "b1")
        );
        assert!(
            m.mentions
                .iter()
                .any(|x| x.kind == MentionKind::User && x.reference == "u2")
        );
        assert!(
            m.mentions
                .iter()
                .any(|x| x.kind == MentionKind::Broadcast && x.reference == "here")
        );
        assert_eq!(m.author.reference, "u1");
        assert_eq!(m.author.kind, AuthorKind::Person);
    }

    #[test]
    fn dm_with_the_bot_is_always_addressed() {
        let m = message_posted(
            &post("hello", ""),
            &chan("D", "b1__u1"),
            &user("u1", "alice", false),
            &[],
            &HashMap::new(),
            &bots(),
        );
        assert!(m.bot_mentioned);
        assert_eq!(m.addressed_bot_ref, "b1");
        assert_eq!(m.space.kind, SpaceKind::Dm);
        // A group DM is not addressed unless mentioned.
        let g = message_posted(
            &post("hello", ""),
            &chan("G", "hash"),
            &user("u1", "alice", false),
            &[],
            &HashMap::new(),
            &bots(),
        );
        assert!(!g.bot_mentioned);
        assert_eq!(g.addressed_bot_ref, "");
    }

    #[test]
    fn thread_ref_is_the_root_only_for_replies() {
        let c = chan("O", "general");
        let u = user("u1", "alice", false);
        let top = message_posted(&post("x", ""), &c, &u, &[], &HashMap::new(), &bots());
        assert_eq!(top.thread_ref, None);
        let reply = message_posted(&post("x", "root1"), &c, &u, &[], &HashMap::new(), &bots());
        assert_eq!(reply.thread_ref.as_deref(), Some("root1"));
        assert!(!reply.bot_mentioned);
    }

    #[test]
    fn emails_and_unknown_names_are_not_mentions() {
        let k = known(&[user("u2", "bob", false)]);
        let m = message_posted(
            &post("mail me at x@bob.example or @nobody; @bob.", ""),
            &chan("O", "general"),
            &user("u1", "alice", false),
            &[],
            &k,
            &bots(),
        );
        assert_eq!(m.text, "mail me at x@bob.example or @nobody; <@user:u2>.");
        assert_eq!(m.mentions.len(), 1);
    }

    #[test]
    fn people_cannot_forge_neutral_tokens() {
        let m = message_posted(
            &post("<@here> <@user:u9> <#space:c9>", ""),
            &chan("O", "general"),
            &user("u1", "alice", false),
            &[],
            &HashMap::new(),
            &bots(),
        );
        assert!(!m.text.contains("<@"));
        assert!(!m.text.contains("<#"));
    }

    #[test]
    fn verified_is_only_claimed_with_evidence() {
        let mut u = user("u1", "alice", false);
        let a = author(&u);
        assert_eq!(a.email.as_deref(), Some("alice@example.com"));
        assert_eq!(a.email_verified, Some(true));
        // The API omitted the flag (false, or hidden): the email is only a hint.
        u.email_verified = false;
        let a = author(&u);
        assert_eq!(a.email.as_deref(), Some("alice@example.com"));
        assert_eq!(a.email_verified, None);
        // No visible email: nothing to report, whatever the flag says.
        u.email_verified = true;
        u.email.clear();
        let a = author(&u);
        assert_eq!((a.email, a.email_verified), (None, None));
        u.is_bot = true;
        assert_eq!(author(&u).kind, AuthorKind::Bot);
    }

    #[test]
    fn files_are_mapped() {
        let f = MmFileInfo {
            id: "f1".into(),
            name: "notes.txt".into(),
            size: 10,
            mime_type: "text/plain".into(),
        };
        let m = message_posted(
            &post("see", ""),
            &chan("D", "b1__u1"),
            &user("u1", "alice", false),
            &[f],
            &HashMap::new(),
            &bots(),
        );
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].reference, "f1");
        assert_eq!(m.files[0].content_type, "text/plain");
    }

    #[test]
    fn text_commands() {
        let c = |t, ch| command_from_text(t, ch, "daneel");
        assert_eq!(c("!help", "D"), Some(("help".into(), String::new())));
        assert_eq!(
            c("  !Use research  ", "D"),
            Some(("use".into(), "research".into()))
        );
        assert_eq!(c("!frobnicate", "D"), None);
        assert_eq!(c("help", "D"), None);
        assert_eq!(
            c("@daneel bind research", "O"),
            Some(("bind".into(), "research".into()))
        );
        assert_eq!(
            c("@Daneel approve ABCD1234", "P"),
            Some(("approve".into(), "ABCD1234".into()))
        );
        assert_eq!(c("@someone bind x", "O"), None);
        assert_eq!(c("@daneel what is up", "O"), None);
        assert_eq!(c("!help", "O"), None);
        assert_eq!(c("@daneel", "O"), None);
    }

    #[test]
    fn catch_up_dedup() {
        let mk = |id: &str, at: i64| MmPost {
            id: id.into(),
            create_at: at,
            ..Default::default()
        };
        let mut sys = mk("sys", 5000);
        sys.kind = "system_join_channel".into();
        let mut del = mk("del", 5000);
        del.delete_at = 6000;
        let out = dedupe_posts(
            vec![
                mk("b", 3000),
                mk("a", 2000),
                mk("b", 3000), // the same post listed twice (e.g. two channel passes)
                mk("old", 100),
                sys,
                del,
                mk("", 4000),
            ],
            1000,
        );
        let ids: Vec<&str> = out.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn ws_frames() {
        let frame = r#"{"event":"posted","data":{"channel_type":"O","post":"{\"id\":\"p1\",\"message\":\"hi\",\"channel_id\":\"c1\"}"},"broadcast":{"channel_id":"c1"},"seq":4}"#;
        let e = WsEvent::parse(frame).unwrap();
        assert_eq!(e.event, "posted");
        assert_eq!(e.seq, 4);
        assert_eq!(e.channel_id(), Some("c1"));
        let p = e.post().unwrap();
        assert_eq!(p.id, "p1");
        assert_eq!(p.message, "hi");
        // Replies to our own requests carry no event name.
        assert!(WsEvent::parse(r#"{"status":"OK","seq_reply":1}"#).is_none());
        assert!(WsEvent::parse("garbage").is_none());
    }

    #[test]
    fn removed_user_gets_its_id_from_the_broadcast() {
        // As Mattermost 11.7 sends it to the user who was removed.
        let own = r#"{"event":"user_removed","data":{"channel_id":"c1","remover_id":"r1"},"broadcast":{"user_id":"u1","channel_id":""},"seq":6}"#;
        let e = WsEvent::parse(own).unwrap();
        assert_eq!(e.member_id(), Some("u1"));
        assert_eq!(e.str_field("channel_id"), Some("c1"));
        assert_eq!(e.str_field("remover_id"), Some("r1"));
        // The copy other members get names the user in `data`.
        let other = r#"{"event":"user_removed","data":{"user_id":"u1","channel_id":"c1"},"broadcast":{"channel_id":"c1"},"seq":7}"#;
        assert_eq!(WsEvent::parse(other).unwrap().member_id(), Some("u1"));
    }

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_ms(1_700_000_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn mentioned_usernames_include_trimmed_variants() {
        let n = mentioned_usernames("hi @Bob. and @a-b- @x@y");
        assert!(n.contains(&"bob.".to_owned()));
        assert!(n.contains(&"bob".to_owned()));
        assert!(n.contains(&"a-b".to_owned()));
    }
}

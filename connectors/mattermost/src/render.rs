//! Neutral content → Mattermost markdown / message attachments (docs/20 §Rendering).
//!
//! Everything here is pure: callers resolve mention references first
//! ([`mention_refs`]) and hand the answers in as [`Lookups`].

use std::collections::HashMap;

use aurora_channels::{Block, ButtonStyle, Card};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

/// Marker for the core's own `<@here>` while broadcast syntax is being defused.
const HERE: char = '\u{E000}';

/// Mention references found in neutral text that need resolving.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MentionRefs {
    pub users: Vec<String>,
    pub spaces: Vec<String>,
}

/// Pre-resolved platform names for mention tokens.
#[derive(Debug, Default)]
pub struct Lookups {
    /// user id → username
    pub users: HashMap<String, String>,
    /// channel id → channel name
    pub spaces: HashMap<String, String>,
}

/// Parsed `<@user:ID>`, `<@bot:ID>`, `<#space:ID>` token.
enum Token<'a> {
    User(&'a str),
    Space(&'a str),
    Here,
}

fn parse_token(inner: &str) -> Option<Token<'_>> {
    if inner == "@here" {
        return Some(Token::Here);
    }
    if let Some(r) = inner
        .strip_prefix("@user:")
        .or_else(|| inner.strip_prefix("@bot:"))
    {
        return valid_ref(r).then_some(Token::User(r));
    }
    if let Some(r) = inner.strip_prefix("#space:") {
        return valid_ref(r).then_some(Token::Space(r));
    }
    None
}

fn valid_ref(r: &str) -> bool {
    !r.is_empty() && r.len() <= 128 && !r.contains(['<', '>', ' ', '\n'])
}

fn for_each_token<'a>(neutral: &'a str, mut f: impl FnMut(&'a str, Token<'a>) -> bool) {
    let mut rest = neutral;
    while let Some(start) = rest.find('<') {
        let tail = &rest[start..];
        match tail.find('>') {
            Some(end) => {
                let whole = &tail[..=end];
                if let Some(t) = parse_token(&tail[1..end])
                    && !f(whole, t)
                {
                    return;
                }
                rest = &tail[end + 1..];
            }
            None => return,
        }
    }
}

/// The user and space references mentioned in `neutral`.
pub fn mention_refs(neutral: &str) -> MentionRefs {
    let mut m = MentionRefs::default();
    for_each_token(neutral, |_, t| {
        match t {
            Token::User(r) if !m.users.iter().any(|x| x == r) => m.users.push(r.to_owned()),
            Token::Space(r) if !m.spaces.iter().any(|x| x == r) => m.spaces.push(r.to_owned()),
            _ => {}
        }
        true
    });
    m
}

/// Neutral markdown → Mattermost markdown: mention tokens become `@username` /
/// `~channel`, platform broadcast syntax typed by anyone is defused, and only
/// the core's `<@here>` token becomes a live `@here`.
pub fn render_text(neutral: &str, lk: &Lookups) -> String {
    // The marker is private to this function: text carrying it cannot forge `@here`.
    let cleaned = neutral.replace(HERE, "");
    let neutral = cleaned.as_str();
    let mut out = String::with_capacity(neutral.len());
    let mut rest = neutral;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        match parse_token(&tail[1..end]) {
            Some(Token::Here) => out.push(HERE),
            Some(Token::User(id)) => match lk.users.get(id) {
                Some(name) => {
                    out.push('@');
                    out.push_str(name);
                }
                None => out.push_str("[unknown user]"),
            },
            Some(Token::Space(id)) => match lk.spaces.get(id) {
                Some(name) => {
                    out.push('~');
                    out.push_str(name);
                }
                None => out.push_str("[unknown channel]"),
            },
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    // `defuse` skips the private-use marker, so the core's `<@here>` survives.
    defuse(&out).replace(HERE, "@here")
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Broadcast word at the start of `s` (which begins with `@`), if any.
fn broadcast_at(s: &str) -> Option<usize> {
    let after = s.strip_prefix('@')?;
    for w in ["channel", "all", "here"] {
        if after.len() >= w.len()
            && after.is_char_boundary(w.len())
            && after[..w.len()].eq_ignore_ascii_case(w)
            && !after[w.len()..].chars().next().is_some_and(is_word)
        {
            return Some(1 + w.len());
        }
    }
    None
}

/// Wraps `@channel`, `@all` and `@here` in inline code so they notify nobody.
/// Text already inside inline code or a fenced block is left alone.
pub fn defuse(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let at_line_start = i == 0 || text.as_bytes()[i - 1] == b'\n';
        if at_line_start && rest.starts_with("```") {
            // Fenced block: copy through the closing fence line (or to the end).
            let end = rest[3..].find("\n```").map_or(rest.len(), |p| 3 + p + 4);
            out.push_str(&rest[..end]);
            i += end;
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        if c == '`' {
            let n = rest.bytes().take_while(|b| *b == b'`').count();
            let fence = "`".repeat(n);
            let mut end = n;
            let mut found = None;
            while let Some(p) = rest[end..].find(&fence) {
                let at = end + p;
                if rest.as_bytes().get(at + n) == Some(&b'`') {
                    end = at + n + rest[at + n..].bytes().take_while(|b| *b == b'`').count();
                    continue;
                }
                found = Some(at + n);
                break;
            }
            let take = found.unwrap_or(n);
            out.push_str(&rest[..take]);
            i += take;
            continue;
        }
        if c == '@'
            && let Some(len) = broadcast_at(rest)
        {
            out.push('`');
            out.push_str(&rest[..len]);
            out.push('`');
            i += len;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Keeps a person from smuggling neutral mention tokens through message text
/// (`<@here>`, `<@user:…>`): a zero-width space breaks the token.
pub fn neutralize_tokens(text: &str) -> String {
    text.replace("<@", "<\u{200b}@").replace("<#", "<\u{200b}#")
}

/// `base64url(HMAC-SHA256(key, token))`: the defence-in-depth signature of an
/// interactive action (Mattermost does not sign its callbacks).
pub fn sign(key: &[u8], token: &str) -> String {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return String::new();
    };
    mac.update(token.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Constant-time check of [`sign`].
pub fn verify(key: &[u8], token: &str, sig: &str) -> bool {
    let Ok(raw) = URL_SAFE_NO_PAD.decode(sig.as_bytes()) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return false;
    };
    mac.update(token.as_bytes());
    mac.verify_slice(&raw).is_ok()
}

/// What card rendering needs beyond the card itself.
pub struct CardCtx<'a> {
    /// `<public>/mm/actions`; `None` when no public URL is configured.
    pub actions_url: Option<&'a str>,
    pub hmac_key: &'a [u8],
    pub integration_id: Uuid,
}

fn style(s: ButtonStyle) -> &'static str {
    match s {
        ButtonStyle::Primary => "primary",
        ButtonStyle::Danger => "danger",
        ButtonStyle::Default => "default",
    }
}

/// A card → one Mattermost message attachment. `lk` resolves mention tokens
/// inside card text. All text is defused.
pub fn render_attachment(card: &Card, ctx: &CardCtx<'_>, lk: &Lookups) -> Value {
    let t = |s: &str| render_text(s, lk);
    let mut title: Option<String> = None;
    let mut text: Vec<String> = Vec::new();
    let mut fields: Vec<Value> = Vec::new();
    let mut footer: Vec<String> = Vec::new();
    let mut actions: Vec<Value> = Vec::new();
    let mut n = 0u32;
    for b in &card.blocks {
        match b {
            Block::Header { text: h } => {
                if title.is_none() {
                    title = Some(t(h));
                } else {
                    text.push(format!("**{}**", t(h)));
                }
            }
            Block::Text { text: x } => text.push(t(x)),
            Block::Fields { pairs } => {
                for p in pairs {
                    let value = t(&p.value);
                    fields.push(json!({
                        "title": t(&p.label),
                        "short": value.chars().count() <= 40,
                        "value": value,
                    }));
                }
            }
            Block::Context { text: x } => footer.push(t(x)),
            Block::Link { label, url } => text.push(format!("[{}]({url})", t(label))),
            Block::Buttons { items } => {
                let Some(url) = ctx.actions_url else { continue };
                for it in items {
                    n += 1;
                    actions.push(json!({
                        "id": format!("act{n}"),
                        "name": t(&it.label),
                        "type": "button",
                        "style": style(it.style),
                        "integration": integration(url, ctx, &it.token),
                    }));
                }
            }
            Block::Select {
                placeholder,
                options,
                token,
            } => {
                let Some(url) = ctx.actions_url else { continue };
                n += 1;
                actions.push(json!({
                    "id": format!("act{n}"),
                    "name": t(placeholder),
                    "type": "select",
                    "options": options.iter()
                        .map(|o| json!({"text": t(&o.label), "value": o.value}))
                        .collect::<Vec<_>>(),
                    "integration": integration(url, ctx, token),
                }));
            }
            // `text_input` is not declared: the core's fallback removes inputs.
            Block::Input { .. } => {}
        }
    }
    let mut att = serde_json::Map::new();
    if let Some(title) = title {
        att.insert("title".into(), json!(title));
    }
    if !text.is_empty() {
        att.insert("text".into(), json!(text.join("\n\n")));
    }
    if !fields.is_empty() {
        att.insert("fields".into(), json!(fields));
    }
    if !footer.is_empty() {
        att.insert("footer".into(), json!(footer.join(" · ")));
    }
    if !actions.is_empty() {
        att.insert("actions".into(), json!(actions));
    }
    Value::Object(att)
}

fn integration(url: &str, ctx: &CardCtx<'_>, token: &str) -> Value {
    json!({
        "url": url,
        "context": {
            "token": token,
            "sig": sign(ctx.hmac_key, token),
            "iid": ctx.integration_id,
        }
    })
}

/// Neutral reaction name → Mattermost emoji name (1:1 for the known set).
pub fn emoji_name(neutral: &str) -> &'static str {
    match neutral {
        "white_check_mark" => "white_check_mark",
        "x" => "x",
        "hourglass_flowing_sand" => "hourglass_flowing_sand",
        _ => "eyes",
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use aurora_channels::{ButtonItem, Field};

    fn lk() -> Lookups {
        let mut l = Lookups::default();
        l.users.insert("u1".into(), "alice".into());
        l.spaces.insert("c1".into(), "general".into());
        l
    }

    #[test]
    fn mentions_are_resolved() {
        let t = render_text(
            "hi <@user:u1> and <@bot:u1> in <#space:c1> <@user:zz>",
            &lk(),
        );
        assert_eq!(t, "hi @alice and @alice in ~general [unknown user]");
        let r = mention_refs("<@user:a> <@bot:b> <@user:a> <#space:c> <@here> <x>");
        assert_eq!(r.users, ["a", "b"]);
        assert_eq!(r.spaces, ["c"]);
    }

    #[test]
    fn broadcasts_are_defused_but_core_here_is_live() {
        let t = render_text("ping @channel, @all and @here now. @ALL! <@here>", &lk());
        assert_eq!(t, "ping `@channel`, `@all` and `@here` now. `@ALL`! @here");
    }

    #[test]
    fn defuse_keeps_ordinary_mentions_and_code() {
        assert_eq!(
            defuse("@allison @channels @here_x"),
            "@allison @channels @here_x"
        );
        assert_eq!(defuse("`@channel` already"), "`@channel` already");
        assert_eq!(defuse("``a @all b`` @all"), "``a @all b`` `@all`");
        assert_eq!(defuse("```\n@here\n```\n@here"), "```\n@here\n```\n`@here`");
        assert_eq!(defuse("unclosed ` @all"), "unclosed ` `@all`");
        assert_eq!(defuse("é@all"), "é`@all`");
    }

    #[test]
    fn typed_tokens_cannot_forge_mentions() {
        let n = neutralize_tokens("x <@here> <@user:u1> <#space:c1>");
        assert!(!n.contains("<@"));
        assert!(!n.contains("<#"));
        assert!(mention_refs(&n).users.is_empty());
    }

    #[test]
    fn signatures_round_trip_and_reject_tampering() {
        let key = b"k".repeat(32);
        let s = sign(&key, "v1.token");
        assert!(verify(&key, "v1.token", &s));
        assert!(!verify(&key, "v1.other", &s));
        assert!(!verify(b"other-key", "v1.token", &s));
        assert!(!verify(&key, "v1.token", ""));
        assert!(!verify(&key, "v1.token", "not base64 !!"));
    }

    fn ctx(key: &[u8]) -> CardCtx<'_> {
        CardCtx {
            actions_url: Some("https://c.example/mm/actions"),
            hmac_key: key,
            integration_id: Uuid::nil(),
        }
    }

    #[test]
    fn card_maps_to_an_attachment() {
        let key = b"k".repeat(32);
        let card = Card::new(vec![
            Block::header("Approval needed"),
            Block::text("Run <@user:u1>'s report"),
            Block::Fields {
                pairs: vec![
                    Field {
                        label: "Agent".into(),
                        value: "Research".into(),
                    },
                    Field {
                        label: "Why".into(),
                        value: "x".repeat(41),
                    },
                ],
            },
            Block::context("Research · via Daneel"),
            Block::link("Dashboard", "https://d.example/a").unwrap(),
            Block::buttons(vec![
                ButtonItem::new("Approve", ButtonStyle::Primary, "tok-a".into()),
                ButtonItem::new("Deny", ButtonStyle::Danger, "tok-d".into()),
            ])
            .unwrap(),
            Block::select("Pick", [("One".to_owned(), "1".to_owned())], "tok-s".into()).unwrap(),
        ])
        .unwrap();
        let a = render_attachment(&card, &ctx(&key), &lk());
        assert_eq!(a["title"], "Approval needed");
        assert_eq!(
            a["text"],
            "Run @alice's report\n\n[Dashboard](https://d.example/a)"
        );
        assert_eq!(a["footer"], "Research · via Daneel");
        assert_eq!(a["fields"][0]["short"], true);
        assert_eq!(a["fields"][1]["short"], false);
        let acts = a["actions"].as_array().unwrap();
        assert_eq!(acts.len(), 3);
        assert_eq!(acts[0]["type"], "button");
        assert_eq!(acts[0]["style"], "primary");
        assert_eq!(acts[1]["style"], "danger");
        assert_eq!(acts[2]["type"], "select");
        assert_eq!(acts[2]["options"][0]["text"], "One");
        let c = &acts[0]["integration"]["context"];
        assert_eq!(c["token"], "tok-a");
        assert!(verify(&key, "tok-a", c["sig"].as_str().unwrap()));
        assert_eq!(
            acts[0]["integration"]["url"],
            "https://c.example/mm/actions"
        );
        // Action ids are alphanumeric (Mattermost requirement) and unique.
        assert_eq!(acts[0]["id"], "act1");
        assert_eq!(acts[2]["id"], "act3");
    }

    #[test]
    fn card_text_is_defused_everywhere() {
        let key = b"k".repeat(32);
        let card = Card {
            blocks: vec![
                Block::Header {
                    text: "@channel header".into(),
                },
                Block::Fields {
                    pairs: vec![Field {
                        label: "@all".into(),
                        value: "@here".into(),
                    }],
                },
                Block::Context {
                    text: "@channel".into(),
                },
                Block::Buttons {
                    items: vec![ButtonItem {
                        label: "@here".into(),
                        style: ButtonStyle::Default,
                        token: "t".into(),
                    }],
                },
            ],
        };
        let s = render_attachment(&card, &ctx(&key), &lk()).to_string();
        for w in ["@channel", "@all", "@here"] {
            assert!(s.contains(&format!("`{w}`")), "{w} not defused in {s}");
        }
        assert!(!s.contains("\"@here\""));
    }

    #[test]
    fn no_public_url_means_no_actions() {
        let key = b"k".repeat(32);
        let card = Card::new(vec![
            Block::header("h"),
            Block::buttons(vec![ButtonItem::new(
                "Go",
                ButtonStyle::Default,
                "t".into(),
            )])
            .unwrap(),
        ])
        .unwrap();
        let c = CardCtx {
            actions_url: None,
            hmac_key: &key,
            integration_id: Uuid::nil(),
        };
        assert!(render_attachment(&card, &c, &lk()).get("actions").is_none());
    }

    #[test]
    fn reactions_map_one_to_one() {
        assert_eq!(emoji_name("eyes"), "eyes");
        assert_eq!(emoji_name("white_check_mark"), "white_check_mark");
        assert_eq!(emoji_name("x"), "x");
        assert_eq!(
            emoji_name("hourglass_flowing_sand"),
            "hourglass_flowing_sand"
        );
        assert_eq!(emoji_name("mystery"), "eyes");
    }
}

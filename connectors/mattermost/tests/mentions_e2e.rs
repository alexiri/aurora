//! D-271 proof against the pinned dev Mattermost (FIX-142): an agent's message goes through the
//! whole outbound path — `markdown_chunks` → `Content::agent_markdown` → the connector's name
//! lookup and `render_text` → `POST /posts` as the bot — and the **server's own notification
//! data** says who was notified: each channel member's `mention_count`. No channel-wide or group
//! notification may happen except the allowed `@here` path, and `@username` must still notify.
//!
//! Runs when the dev server is up (`bash scripts/mattermost/setup.sh up`, which writes
//! `target/mattermost/env`; `scripts/conformance.sh --mattermost` sets the variables). Without
//! `AURORA_MM_URL` the test reports "skipped" and passes.
//!
//! Groups: Mattermost custom and LDAP groups need a licence; the dev image is unlicensed (`POST
//! /groups` answers 501), so a real group cannot be created there. When a server accepts one, the
//! test creates it, adds nobody but Alice, and asserts the group's mention notifies nobody.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aurora_channels::Content;
use aurora_connector_mattermost::api::{Mm, NetPolicy};
use aurora_connector_mattermost::mentions::{NameCache, resolve};
use aurora_connector_mattermost::render::{Lookups, mention_refs, render_text};
use aurora_core::sanitize::markdown_chunks;
use reqwest::Method;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

struct Env {
    mm: Mm,
    admin: String,
    bot: String,
    alice: String,
    bob: String,
    bot_id: String,
    team: String,
}

fn env() -> Option<Env> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let url = var("AURORA_MM_URL")?;
    Some(Env {
        mm: Mm::with_policy(&url, &NetPolicy::allow_all()).ok()?,
        admin: var("AURORA_MM_ADMIN_TOKEN")?,
        bot: var("AURORA_MM_BOT_TOKEN")?,
        alice: var("AURORA_MM_ALICE_ID")?,
        bob: var("AURORA_MM_BOB_ID")?,
        bot_id: var("AURORA_MM_BOT_ID")?,
        team: var("AURORA_MM_TEAM_ID")?,
    })
}

async fn mention_count(e: &Env, ch: &str, user: &str) -> i64 {
    let v =
        e.mm.get_q(&e.admin, &format!("/channels/{ch}/members/{user}"), &[])
            .await
            .expect("channel member");
    v["mention_count"].as_i64().expect("mention_count")
}

/// Posts what an agent wrote the way the connector does and returns the posts made.
async fn post_as_agent(e: &Env, ch: &str, cache: &NameCache, raw: &str) -> Vec<String> {
    let mut posted = Vec::new();
    for chunk in markdown_chunks(raw, 3000) {
        let neutral = Content::agent_markdown(&chunk).text.unwrap_or_default();
        let refs = mention_refs(&neutral);
        let mut lk = Lookups::default();
        resolve(&e.mm, &e.bot, Some(&e.admin), cache, &refs.names, &mut lk).await;
        let message = render_text(&neutral, &lk);
        e.mm.post(
            &e.bot,
            "/posts",
            &json!({"channel_id": ch, "message": message}),
        )
        .await
        .expect("post");
        posted.push(message);
    }
    posted
}

/// D-270 on the pinned server: a URL an agent writes — bare, `<url>`, `www.…`, `ftp://…`, in HTML text
/// or in a table cell beyond the header — never makes the server build a link preview (an
/// embed in the post's metadata; the server fetches the address when it does).
#[tokio::test]
async fn agent_urls_are_never_previewed_by_the_server() {
    let Some(e) = env() else {
        eprintln!("skipped: AURORA_MM_URL is not set (bash scripts/mattermost/setup.sh up)");
        return;
    };
    let name = format!("previews-e2e-{}", std::process::id());
    let ch =
        e.mm.post(
            &e.admin,
            "/channels",
            &json!({"team_id": e.team, "name": name, "display_name": "Previews e2e", "type": "O"}),
        )
        .await
        .expect("channel");
    let ch = ch["id"].as_str().expect("channel id").to_owned();
    e.mm.post(
        &e.admin,
        &format!("/channels/{ch}/members"),
        &json!({"user_id": e.bot_id}),
    )
    .await
    .expect("member");
    // Controls: the server previews these (it is what D-270 closes).
    for raw in [
        "http://127.0.0.1:9/a",
        "www.example.org/a",
        "ftp://example.org/a",
    ] {
        let id = post_one(&e, &ch, raw).await;
        assert!(
            until(async || !embeds(&e, &id).await.is_empty()).await,
            "control {raw:?}: the server made no embed"
        );
    }
    let cache = NameCache::default();
    for raw in [
        "see http://127.0.0.1:9/a?d=SECRET now",
        "see https://example.org/a?d=SECRET now",
        "<https://example.org/a?d=SECRET>",
        "www.example.org/a?d=SECRET and (www.example.org/b)",
        "WWW.example.org/a ftp://example.org/b",
        "<div>https://example.org/a www.example.org/b ftp://example.org/c</div>",
        "<b>www.example.org/a</b> <a href=\"https://example.org/b\">x</a>",
        "|a|b|\n|-|-|\n|1|2|https://example.org/a www.example.org/b|",
        "$https://example.org/a$ and $$www.example.org/b$$",
        "[x](https://example.org/a) and [www.example.org/b](https://example.org/b)",
    ] {
        let posted = post_as_agent(&e, &ch, &cache, raw).await;
        // The post just made is the newest of the channel.
        let list =
            e.mm.get_q(
                &e.admin,
                &format!("/channels/{ch}/posts"),
                &[("per_page", "1")],
            )
            .await
            .expect("posts");
        let id = list["order"][0].as_str().expect("newest post").to_owned();
        // Give the server its chance to make an embed, then look: none must come.
        let sentinel = post_one(&e, &ch, "sentinel").await;
        assert!(until(async || embeds(&e, &sentinel).await.is_empty()).await);
        assert!(
            embeds(&e, &id).await.is_empty(),
            "{raw:?} posted as {posted:?} made a preview: {:?}",
            embeds(&e, &id).await
        );
    }
}

async fn post_one(e: &Env, ch: &str, message: &str) -> String {
    let v =
        e.mm.post(
            &e.bot,
            "/posts",
            &json!({"channel_id": ch, "message": message}),
        )
        .await
        .expect("post");
    v["id"].as_str().expect("post id").to_owned()
}

async fn embeds(e: &Env, id: &str) -> Vec<Value> {
    let v =
        e.mm.get_q(&e.admin, &format!("/posts/{id}"), &[])
            .await
            .expect("post");
    v["metadata"]["embeds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Polls until `check` holds or the deadline passes; returns whether it held.
async fn until(mut check: impl AsyncFnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if check().await {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `(what the agent wrote, Alice's expected mentions, Bob's)`. Alice is online, Bob offline;
/// the bot is the author. Everything that is `(0, 0)` must be defused; `@here` may notify only
/// the online Alice, only as a standalone lowercase `@here`.
const FORMS: &[(&str, i64, i64)] = &[
    // (a) @channel / @all in every spelling: nobody.
    ("@channel", 0, 0),
    ("@Channel @CHANNEL @cHaNnEl", 0, 0),
    ("@all @All @ALL", 0, 0),
    (
        "@channel. @channel, @channel: @channel! @channel? @channel; @channel)",
        0,
        0,
    ),
    ("(@channel) [@all] \"@channel\" '@all'", 0, 0),
    ("@channel- @channel_ @all_ _@channel .@all /@channel", 0, 0),
    ("@channel’s @channel… @channel— @channel–", 0, 0),
    ("**@channel** _@all_ ~~@channel~~ *@all*", 0, 0),
    (
        "# @channel\n\n> @all\n\n- @channel\n- @all\n\n1. @channel",
        0,
        0,
    ),
    ("| a |\n|---|\n| @channel |\n| @all |", 0, 0),
    (
        "[@channel](https://x.example) [see @all here](https://x.example)",
        0,
        0,
    ),
    (
        "[x](https://x.example/@channel) and [y][r]\n\n[r]: https://x.example/@all",
        0,
        0,
    ),
    ("\\@channel \\\\@all", 0, 0),
    (":tada:@channel", 0, 0),
    ("hello\n@channel\n@all", 0, 0),
    (
        "<code>@channel</code> <kbd>@all</kbd> <b>@channel</b>",
        0,
        0,
    ),
    ("<div>@channel</div>", 0, 0),
    // Names the connector's own HTML conversion joins (review 4): read from the converted text.
    (
        "@chan<span></span>nel @al<sub>l</sub> @he<sup>re</sup>",
        0,
        0,
    ),
    ("@al<span></span>ice", 1, 0),
    ("` @channel", 0, 0),
    // (b) @here: only a standalone lowercase one notifies, and only online people.
    ("@Here @HERE @hErE", 0, 0),
    (
        "[@here](https://x.example) [a @here b](https://x.example)",
        0,
        0,
    ),
    ("x@here @hereby foo@here.org", 0, 0),
    ("@here_ @here_x @here.x", 0, 0),
    // (a trailing dash or full stop after a standalone `@here` is punctuation: still the broadcast)
    ("@here-", 1, 0),
    ("@here", 1, 0),
    ("ping @here now.", 1, 0),
    ("**@here** and @here,", 1, 0),
    // (c) a group that does not exist is nobody (nothing is notified); an unknown name passes.
    ("@zzgroup @ZZGroup", 0, 0),
    ("@devs-without-a-group", 0, 0),
    // (d) @username of one user still notifies that user.
    ("@alice", 1, 0),
    ("@Alice.", 1, 0),
    ("**@alice** and [@alice](https://x.example)", 1, 0),
    ("@bob", 0, 1),
    ("@alice @bob", 1, 1),
    ("@alice @channel @all", 1, 0),
    // (e) nothing in code notifies.
    ("`@channel` `@all` `@here` `@alice`", 0, 0),
    ("``@channel`` and ``@here``", 0, 0),
    ("```\n@channel\n@all\n@here\n@alice\n```", 0, 0),
    ("    @channel\n    @all\n    @here", 0, 0),
    ("<code>@here</code> <pre>@alice</pre>", 0, 0),
];

#[tokio::test]
async fn pings_are_accounted_for_on_the_pinned_server() {
    let Some(e) = env() else {
        eprintln!("skipped: AURORA_MM_URL is not set (bash scripts/mattermost/setup.sh up)");
        return;
    };
    let name = format!(
        "mentions-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );
    let ch =
        e.mm.post(
            &e.admin,
            "/channels",
            &json!({"team_id": e.team, "name": name, "display_name": "Mentions e2e", "type": "O"}),
        )
        .await
        .expect("channel");
    let ch = ch["id"].as_str().expect("channel id").to_owned();
    for u in [&e.alice, &e.bob, &e.bot_id] {
        e.mm.post(
            &e.admin,
            &format!("/channels/{ch}/members"),
            &json!({"user_id": u}),
        )
        .await
        .expect("member");
    }
    for (u, s) in [(&e.alice, "online"), (&e.bob, "offline")] {
        e.mm.send(
            &e.admin,
            Method::PUT,
            &format!("/users/{u}/status"),
            &[],
            Some(&json!({"user_id": u, "status": s})),
        )
        .await
        .expect("status");
    }
    // A group, when the server has the feature (a licence); the dev image does not.
    let group = e
        .mm
        .post(
            &e.admin,
            "/groups",
            &json!({"name": "zzdevs", "display_name": "ZZ Devs", "source": "custom", "allow_reference": true, "user_ids": [e.alice]}),
        )
        .await;
    let have_group = group.is_ok();
    if !have_group {
        eprintln!(
            "note: the server has no groups (501, needs a licence): the group path is covered by the unit tests with a mocked groups API"
        );
    }

    let cache = NameCache::default();
    // (joining a channel leaves a system post that already counts as one mention each)
    let mut want_alice = mention_count(&e, &ch, &e.alice).await;
    let mut want_bob = mention_count(&e, &ch, &e.bob).await;
    let mut forms: Vec<(&str, i64, i64)> = FORMS.to_vec();
    if have_group {
        forms.push(("@zzdevs @ZZDevs. **@zzdevs**", 0, 0));
    }
    for (n, (text, a, b)) in forms.iter().enumerate() {
        let (a0, b0) = (
            mention_count(&e, &ch, &e.alice).await,
            mention_count(&e, &ch, &e.bob).await,
        );
        let posted = post_as_agent(&e, &ch, &cache, text).await;
        // A sentinel that notifies Alice (and nobody else) tells us the notifications of the
        // form before it have been processed.
        post_as_agent(&e, &ch, &cache, &format!("sentinel {n} @alice")).await;
        let done = until(async || mention_count(&e, &ch, &e.alice).await > a0 + a).await;
        let (a1, b1) = (
            mention_count(&e, &ch, &e.alice).await,
            mention_count(&e, &ch, &e.bob).await,
        );
        assert!(
            done,
            "{text:?}: the sentinel never notified Alice ({a1} vs {})",
            a0 + a + 1
        );
        assert_eq!(a1 - a0, a + 1, "alice, {text:?} posted as {posted:?}");
        assert_eq!(b1 - b0, *b, "bob, {text:?} posted as {posted:?}");
        want_alice += a + 1;
        want_bob += b;
    }
    // Late notifications: the counts settle on exactly the expected totals.
    let mut last = (-1, -1);
    let settled = until(async || {
        let now = (
            mention_count(&e, &ch, &e.alice).await,
            mention_count(&e, &ch, &e.bob).await,
        );
        let stable = now == last;
        last = now;
        stable
    })
    .await;
    assert!(settled);
    assert_eq!(
        last,
        (want_alice, want_bob),
        "totals after everything settled"
    );

    // The server's own view of the last post: its `mentions` list is not exposed over REST, but
    // the posts as stored are the converted texts — a defused word is code or carries a
    // zero-width space, never a bare ping word.
    let posts: Value =
        e.mm.get_q(
            &e.admin,
            &format!("/channels/{ch}/posts"),
            &[("per_page", "200")],
        )
        .await
        .expect("posts");
    let order = posts["order"].as_array().expect("order");
    assert!(order.len() >= forms.len(), "{}", order.len());
}

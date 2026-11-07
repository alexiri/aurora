//! The twelve connector conformance scenarios (docs/13 §7, TEST-015).
//!
//! Each test starts its own fake core + connector under test (the in-process
//! fake connector, or the binary in `CONFORMANCE_CONNECTOR_BIN`), plays the
//! core's side by enqueueing operations, plays the platform's side through the
//! [`PlatformDriver`], and asserts on what crossed the wire and what the
//! platform shows. `Harness::finish` fails the test if the fake core saw any
//! protocol violation (bad body, missing header, foreign replica, …).

// Test helpers outside `#[test]` fns assert by panicking.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use aurora_channels::{Block, ButtonItem, ButtonStyle, Card, Content, Field};
use aurora_conformance_connector::driver::*;
use aurora_conformance_connector::fake_core::eventually;
use aurora_conformance_connector::target::{BOT_REF, Harness, StartOptions};
use aurora_protocol::connector::*;
use serde_json::{Value, json};
use uuid::Uuid;

type R = anyhow::Result<()>;

const T: Duration = Duration::from_secs(20);

// ------------------------------------------------------------- helpers

fn posted(ev: &InboundEvent) -> MessagePosted {
    match ev.parse() {
        Ok(EventPayload::MessagePosted(m)) => m,
        other => panic!("expected message.posted, got {other:?}"),
    }
}

fn ok_data(r: &OpResult) -> Value {
    assert_eq!(
        r.outcome,
        Outcome::Ok,
        "op {} failed: {:?}",
        r.op_id,
        r.error
    );
    r.data.clone().unwrap_or(Value::Null)
}

fn err_code(r: &OpResult) -> ErrorCode {
    assert_eq!(r.outcome, Outcome::Error, "op {} succeeded", r.op_id);
    r.error
        .clone()
        .expect("error outcome carries an error")
        .code
}

fn text(t: &str) -> Content {
    // Raw struct literal: tests hand the connector text exactly as given
    // (the constructors would sanitize what a scenario wants to be hostile).
    Content {
        text: Some(t.to_owned()),
        card: None,
    }
}

fn space_target(space: &str, thread: Option<&str>) -> Value {
    match thread {
        Some(t) => json!({"space_ref": space, "thread_ref": t}),
        None => json!({"space_ref": space}),
    }
}

impl Ctx {
    /// Queues a `message.post` (op_marker = op id) and waits for its result.
    async fn post(&self, target: Value, content: &Content) -> anyhow::Result<(Uuid, OpResult)> {
        self.post_with(target, content, json!({})).await
    }

    async fn post_with(
        &self,
        target: Value,
        content: &Content,
        extra: Value,
    ) -> anyhow::Result<(Uuid, OpResult)> {
        let id = Uuid::now_v7();
        let mut payload = json!({
            "target": target, "sender": "shared",
            "content": serde_json::to_value(content)?, "op_marker": id.to_string(),
        });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            payload[k] = v;
        }
        self.0
            .core
            .enqueue_with_id(id, "message.post", Some(self.0.integration), payload);
        let r = self
            .0
            .core
            .wait_result(id, T)
            .await
            .ok_or_else(|| anyhow::anyhow!("no result for message.post {id}"))?;
        Ok((id, r))
    }

    async fn op(&self, kind: &str, payload: Value) -> anyhow::Result<OpResult> {
        let id = self.0.core.enqueue(kind, Some(self.0.integration), payload);
        self.0
            .core
            .wait_result(id, T)
            .await
            .ok_or_else(|| anyhow::anyhow!("no result for {kind} {id}"))
    }

    async fn event(&self, pred: impl Fn(&InboundEvent) -> bool) -> anyhow::Result<InboundEvent> {
        self.0
            .core
            .wait_event(T, pred)
            .await
            .ok_or_else(|| anyhow::anyhow!("expected event never reached the core inbox"))
    }

    async fn event_of(&self, kind: EventKind) -> anyhow::Result<InboundEvent> {
        self.event(move |e| e.kind == kind).await
    }

    async fn message_event(&self, message_ref: &str) -> anyhow::Result<MessagePosted> {
        let m = message_ref.to_owned();
        let ev = self
            .event(move |e| e.kind == EventKind::MessagePosted && posted(e).message_ref == m)
            .await?;
        assert_eq!(ev.integration_id, self.0.integration);
        Ok(posted(&ev))
    }

    async fn find_post(&self, message_ref: &str) -> anyhow::Result<Post> {
        self.0
            .driver
            .posts(None)
            .await?
            .into_iter()
            .find(|p| p.message_ref == message_ref)
            .ok_or_else(|| anyhow::anyhow!("platform has no post {message_ref}"))
    }
}

/// A started harness plus convenience methods.
struct Ctx(Harness);

async fn ctx(opts: StartOptions) -> anyhow::Result<Ctx> {
    let c = Ctx(Harness::ready(opts).await?);
    c.0.driver
        .add_user(UserSpec::person("U1", Some("alice@example.com"), true))
        .await?;
    c.0.driver
        .add_space(SpaceSpec {
            reference: "C1".into(),
            kind: "channel".into(),
            label: "general".into(),
        })
        .await?;
    Ok(c)
}

fn msg(author: &str, space: Option<&str>, text: &str) -> MessageSpec {
    MessageSpec {
        author: author.into(),
        space: space.map(Into::into),
        text: text.into(),
        ..Default::default()
    }
}

// --------------------------------------------------- 1. register/approve

#[tokio::test]
async fn s01_register_then_approve() -> R {
    let h = Harness::start_fake(StartOptions::default()).await?;

    // Registered with a contract-1 manifest; first registration needs approval.
    let regs = h.core.registrations();
    assert_eq!(regs.len(), 1);
    let m = &regs[0].manifest;
    assert_eq!(m.package.contract, 1);
    assert!(!m.package.id.is_empty());
    assert!(!m.enabled_capabilities().is_empty());
    assert_eq!(h.core.status(), ConnectorStatus::PendingApproval);

    // Until approved nothing is served and nothing is claimable.
    assert!(
        h.driver
            .wait_ready(Duration::from_millis(700))
            .await
            .is_err(),
        "integration served before approval"
    );
    let early = h
        .core
        .enqueue("spaces.list", Some(h.integration), json!({"limit": 5}));
    assert!(
        h.core
            .wait_claims(early, 1, Duration::from_millis(700))
            .await
            .is_none(),
        "op claimed before approval"
    );

    // Approved: the integration is served and the queued op is executed.
    h.approve_and_wait().await?;
    let r = h
        .core
        .wait_result(early, T)
        .await
        .expect("op executed after approval");
    ok_data(&r);

    // Clean shutdown retires (releases owned integrations).
    let core = h.finish().await?;
    assert!(core.retired(), "connector did not call /retire on shutdown");
    Ok(())
}

// ------------------------------------------------------- 2. DM round trip

#[tokio::test]
async fn s02_dm_round_trip() -> R {
    let c = ctx(StartOptions::default()).await?;
    let d = c.0.driver.post_message(msg("U1", None, "hello")).await?;
    assert_eq!(d.ack, Ack::Accepted);

    let m = c.message_event(&d.message_ref).await?;
    assert_eq!(m.space.kind, SpaceKind::Dm);
    assert_eq!(m.text, "hello");
    assert_eq!(m.author.reference, "U1");
    assert_eq!(m.author.kind, AuthorKind::Person);
    assert_eq!(m.author.email.as_deref(), Some("alice@example.com"));
    // A platform that does not show other users' verification flag reports none
    // (never a guess); the core then only uses the email as a sign-in hint.
    let q = c.0.driver.quirks();
    assert_eq!(
        m.author.email_verified,
        q.email_verified_visible.then_some(true)
    );
    assert_eq!(m.addressed_bot_ref, BOT_REF);
    assert_eq!(m.thread_ref, None);

    // Receipt, then the answer back through the same door.
    let r = c
        .op(
            "reaction.add",
            json!({"space_ref": m.space.reference, "message_ref": d.message_ref,
                   "sender": "shared", "emoji": "eyes"}),
        )
        .await?;
    ok_data(&r);
    let (id, r) = c
        .post(space_target(&m.space.reference, None), &text("hi Alice"))
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let answer = c.find_post(&res.message_ref).await?;
    assert_eq!(answer.space_ref, m.space.reference);
    assert_eq!(answer.text, "hi Alice");
    assert_eq!(answer.op_marker.as_deref(), Some(id.to_string().as_str()));
    let user_msg = c.find_post(&d.message_ref).await?;
    assert!(
        user_msg.reactions.iter().any(|(e, _)| e == "eyes"),
        "receipt reaction missing: {:?}",
        user_msg.reactions
    );
    c.0.finish().await?;
    Ok(())
}

// ------------------------------- 3. channel mention, then thread follow-up

#[tokio::test]
async fn s03_channel_mention_then_thread_follow_up() -> R {
    let c = ctx(StartOptions::default()).await?;
    let d =
        c.0.driver
            .post_message(msg("U1", Some("C1"), "hey <@bot:B1> summarize this"))
            .await?;
    assert_eq!(d.ack, Ack::Accepted);
    let m = c.message_event(&d.message_ref).await?;
    assert_eq!(m.space.kind, SpaceKind::Channel);
    assert!(m.bot_mentioned);
    assert!(
        m.mentions
            .iter()
            .any(|x| x.kind == MentionKind::Bot && x.reference == BOT_REF)
    );
    assert_eq!(m.addressed_bot_ref, BOT_REF);
    assert_eq!(m.thread_ref, None, "top-level message has no thread yet");

    // The core answers in the thread rooted at the mention.
    let (_, r) = c
        .post(space_target("C1", Some(&d.message_ref)), &text("Summary…"))
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    assert_eq!(res.thread_ref, d.message_ref);
    assert_eq!(
        c.find_post(&res.message_ref).await?.thread_ref,
        d.message_ref
    );

    // Follow-up inside the thread, without a mention.
    let f =
        c.0.driver
            .post_message(MessageSpec {
                thread_ref: Some(d.message_ref.clone()),
                ..msg("U1", Some("C1"), "and the second part?")
            })
            .await?;
    assert_eq!(f.ack, Ack::Accepted);
    let fm = c.message_event(&f.message_ref).await?;
    assert_eq!(fm.thread_ref.as_deref(), Some(d.message_ref.as_str()));
    assert!(!fm.bot_mentioned);
    let (_, r) = c
        .post(
            space_target("C1", fm.thread_ref.as_deref()),
            &text("Second part…"),
        )
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    assert_eq!(res.thread_ref, d.message_ref, "reply stays in the thread");

    // A top-level post in a channel roots a new thread at itself.
    let (_, r) = c.post(space_target("C1", None), &text("proactive")).await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    assert_eq!(res.thread_ref, res.message_ref);
    c.0.finish().await?;
    Ok(())
}

// -------------------------------------------- 4. bot added -> binding card

#[tokio::test]
async fn s04_bot_added_emits_event_and_renders_binding_card() -> R {
    let c = ctx(StartOptions::default()).await?;
    assert_eq!(
        c.0.driver.bot_added("C1", BOT_REF, "U1").await?,
        Ack::Accepted
    );
    // The event of *this* add: a platform may also report the bot's earlier membership
    // (it was put in the channel when the scenario created it).
    let ev = c
        .event(|e| {
            matches!(e.parse(), Ok(EventPayload::BotAdded(b)) if b.actor_ref.as_deref() == Some("U1"))
        })
        .await?;
    let EventPayload::BotAdded(b) = ev.parse()? else {
        panic!("not bot.added")
    };
    assert_eq!(b.space.reference, "C1");
    assert_eq!(b.space.kind, SpaceKind::Channel);
    assert_eq!(b.bot_ref, BOT_REF);
    assert_eq!(b.actor_ref.as_deref(), Some("U1"));

    // The core DMs the adder a binding.choose select (opaque token).
    let token = "v1.eyJqdGkiOiJiLTEifQ==.c2ln_-+/AAAA==";
    let card = Card::new(vec![
        Block::header("Choose an agent for #general"),
        Block::select(
            "Pick an agent",
            [
                ("Research".to_owned(), "agent-1".to_owned()),
                ("Ops".to_owned(), "agent-2".to_owned()),
            ],
            token.into(),
        )?,
    ])?;
    let (_, r) = c
        .post(json!({"user_ref": "U1"}), &Content::card(card))
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    let sel = c.0.driver.select(&post).expect("select menu rendered");
    assert_eq!(sel.token, token, "token must reach the platform unchanged");
    assert_eq!(
        sel.options,
        vec![
            ("Research".to_owned(), "agent-1".to_owned()),
            ("Ops".to_owned(), "agent-2".to_owned())
        ]
    );

    // The pick comes back as action.invoked with the same token and the value.
    let ack =
        c.0.driver
            .click(Click {
                clicker: "U1".into(),
                token: sel.token.clone(),
                value: Some("agent-1".into()),
                space_ref: Some(post.space_ref.clone()),
                message_ref: Some(post.message_ref.clone()),
            })
            .await?;
    assert_eq!(ack, Ack::Accepted);
    let ev = c.event_of(EventKind::ActionInvoked).await?;
    let EventPayload::ActionInvoked(a) = ev.parse()? else {
        panic!("not action.invoked")
    };
    assert_eq!(a.token, token);
    assert_eq!(a.value.as_deref(), Some("agent-1"));
    assert_eq!(a.clicker_ref, "U1");

    // The binding is made by the core; the bot then announces itself.
    let (_, r) = c
        .post(space_target("C1", None), &text("Research is now here."))
        .await?;
    ok_data(&r);
    c.0.finish().await?;
    Ok(())
}

// ------------------------------------------------ 5. unlinked user -> link

#[tokio::test]
async fn s05_unlinked_user_gets_a_link_privately() -> R {
    let c = ctx(StartOptions::default()).await?;
    // No email at all: the core cannot auto-link, it must send a sign-in link.
    c.0.driver
        .add_user(UserSpec::person("U2", None, false))
        .await?;
    let d = c.0.driver.post_message(msg("U2", None, "hi")).await?;
    assert_eq!(d.ack, Ack::Accepted);
    let m = c.message_event(&d.message_ref).await?;
    assert_eq!(m.author.reference, "U2");
    let q = c.0.driver.quirks();
    if q.users_without_email {
        assert_eq!(m.author.email, None);
    }
    assert_eq!(m.author.email_verified, None);

    // The connector asks the core for a sign-in link for that person (on platforms
    // with a person-initiated "link my account" action).
    let url = if q.link_start_action {
        let url = c.0.driver.request_link("U2").await?;
        let calls = c.0.core.link_starts();
        assert_eq!(calls.len(), 1, "link-start must be called once");
        assert_eq!(calls[0].integration_id, c.0.integration);
        assert_eq!(calls[0].platform_user_ref, "U2");
        assert!(url.contains("aui_"), "{url}");
        url
    } else {
        "https://aurora.example/link/channel?token=aui_0123456789abcdef".to_owned()
    };

    // The core delivers it privately: ephemeral where possible.
    let caps = c.0.driver.capabilities().await?;
    assert!(caps.iter().any(|x| x == "ephemeral"));
    let (_, r) = c
        .post_with(
            space_target(&m.space.reference, None),
            &text(&format!("Sign in to link your account: {url}")),
            json!({"ephemeral_to": "U2"}),
        )
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    assert_eq!(post.ephemeral_to.as_deref(), Some("U2"));
    assert!(post.text.contains(&url));
    c.0.finish().await?;
    Ok(())
}

// ------------------------------------------------------- 6. files in/out

#[tokio::test]
async fn s06_file_in_and_out() -> R {
    let c = ctx(StartOptions::default()).await?;

    // ---- in: message with a file -> file.fetch -> PUT /files/{op_id}
    c.0.driver
        .host_file(FileSpec {
            reference: "F1".into(),
            name: "notes.txt".into(),
            content_type: "text/plain".into(),
            content: "hello file".into(),
        })
        .await?;
    let d =
        c.0.driver
            .post_message(MessageSpec {
                files: vec![("F1".into(), "notes.txt".into(), 10, "text/plain".into())],
                ..msg("U1", None, "see attached")
            })
            .await?;
    assert_eq!(d.ack, Ack::Accepted);
    let m = c.message_event(&d.message_ref).await?;
    assert_eq!(m.files.len(), 1);
    assert_eq!(m.files[0].reference, "F1");
    assert_eq!(m.files[0].name, "notes.txt");

    let op = c.0.core.enqueue(
        "file.fetch",
        Some(c.0.integration),
        json!({"file_ref": "F1", "max_bytes": 1024}),
    );
    let r =
        c.0.core
            .wait_result(op, T)
            .await
            .expect("file.fetch result");
    let res: FileFetchResult = serde_json::from_value(ok_data(&r))?;
    assert!(res.upload_url_used);
    let ups = c.0.core.uploads();
    assert_eq!(ups.len(), 1);
    assert_eq!(ups[0].op_id, Some(op));
    assert_eq!(ups[0].attachment_id, res.attachment_id);
    assert_eq!(ups[0].filename, "notes.txt");
    assert!(ups[0].content_type.starts_with("text/plain"));
    assert_eq!(ups[0].bytes, b"hello file");

    // Over max_bytes: refused, nothing uploaded. Unknown file: not_found.
    c.0.driver
        .host_file(FileSpec {
            reference: "F2".into(),
            name: "big.txt".into(),
            content_type: "text/plain".into(),
            content: "x".repeat(2000),
        })
        .await?;
    let r = c
        .op("file.fetch", json!({"file_ref": "F2", "max_bytes": 100}))
        .await?;
    assert_eq!(err_code(&r), ErrorCode::TooLarge);
    let r = c
        .op("file.fetch", json!({"file_ref": "nope", "max_bytes": 100}))
        .await?;
    assert_eq!(err_code(&r), ErrorCode::NotFound);
    assert_eq!(c.0.core.uploads().len(), 1, "no upload for refused fetches");

    // ---- out: message.post with files -> GET /files/{attachment_id}
    let att =
        c.0.core
            .add_attachment("report.txt", "text/plain", b"outbound bytes");
    let (_, r) = c
        .post_with(
            space_target("C1", None),
            &text("report attached"),
            json!({"files": [{
                "attachment_id": att, "name": "report.txt",
                "download_url": format!("{}/v1/connector/files/{att}", c.0.core.url()),
            }]}),
        )
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    assert_eq!(post.files, vec!["report.txt".to_owned()]);
    assert_eq!(
        c.0.driver
            .attachment(&res.message_ref, "report.txt")
            .await?
            .as_deref(),
        Some(&b"outbound bytes"[..])
    );

    // An attachment the core does not know: the download 404s -> not_found.
    let ghost = Uuid::now_v7();
    let (_, r) = c
        .post_with(
            space_target("C1", None),
            &text("ghost"),
            json!({"files": [{
                "attachment_id": ghost, "name": "ghost.txt",
                "download_url": format!("{}/v1/connector/files/{ghost}", c.0.core.url()),
            }]}),
        )
        .await?;
    assert_eq!(err_code(&r), ErrorCode::NotFound);
    c.0.finish().await?;
    Ok(())
}

// ---------------------------------- 7. approval card -> click -> settle

#[tokio::test]
async fn s07_approval_card_click_and_settle() -> R {
    let c = ctx(StartOptions::default()).await?;
    let dm =
        c.0.driver
            .post_message(msg("U1", None, "run the report"))
            .await?;
    let m = c.message_event(&dm.message_ref).await?;
    let space = m.space.reference;

    let approve = "v1.eyJraW5kIjoiYXBwcm92YWwuZGVjaWRlIn0.QVBQUk9WRS_-==";
    let deny = "v1.eyJraW5kIjoiYXBwcm92YWwuZGVjaWRlIn0.REVOWQ==";
    let card = Card::new(vec![
        Block::header("Approval needed"),
        Block::fields([("Agent", "Research"), ("Action", "send email")])?,
        Block::buttons(vec![
            ButtonItem::new("Approve", ButtonStyle::Primary, approve.into()),
            ButtonItem::new("Deny", ButtonStyle::Danger, deny.into()),
        ])?,
    ])?;
    let (_, r) = c
        .post(space_target(&space, None), &Content::card(card))
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    let buttons = c.0.driver.buttons(&post);
    assert_eq!(
        buttons.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(),
        ["Approve", "Deny"]
    );
    assert_eq!(buttons[0].token, approve);
    assert_eq!(buttons[1].token, deny);

    // Click Approve: action.invoked carries the opaque token unchanged.
    let ack =
        c.0.driver
            .click(Click {
                clicker: "U1".into(),
                token: buttons[0].token.clone(),
                value: None,
                space_ref: Some(space.clone()),
                message_ref: Some(res.message_ref.clone()),
            })
            .await?;
    assert_eq!(ack, Ack::Accepted);
    let ev = c.event_of(EventKind::ActionInvoked).await?;
    let EventPayload::ActionInvoked(a) = ev.parse()? else {
        panic!("not action.invoked")
    };
    assert_eq!(a.token, approve);
    assert_eq!(a.clicker_ref, "U1");
    assert_eq!(a.message_ref.as_deref(), Some(res.message_ref.as_str()));
    assert_eq!(a.space_ref.as_deref(), Some(space.as_str()));

    // Settle the card in place: no buttons left, outcome visible.
    let settled = Card::new(vec![
        Block::header("Approved"),
        Block::context("Approved by U1"),
    ])?;
    let r = c
        .op(
            "message.update",
            json!({"space_ref": space, "message_ref": res.message_ref,
                   "sender": "shared", "content": Content::card(settled)}),
        )
        .await?;
    ok_data(&r);
    let post = c.find_post(&res.message_ref).await?;
    assert!(
        c.0.driver.buttons(&post).is_empty(),
        "settled card kept its buttons"
    );
    assert!(
        serde_json::to_string(&post.card)?.contains("Approved by U1"),
        "{:?}",
        post.card
    );
    c.0.finish().await?;
    Ok(())
}

// --------------------------------------------- 8. credential dies

#[tokio::test]
async fn s08_credential_dies() -> R {
    let c = ctx(StartOptions::default()).await?;

    if c.0.driver.quirks().dedicated_presences {
        // (a) the platform tells the connector a bot is gone -> presence.removed.
        let pid = Uuid::now_v7();
        let ack =
            c.0.driver
                .presence_removed("B2", Some(pid), "token_revoked")
                .await?;
        assert_eq!(ack, Ack::Accepted);
        let ev = c.event_of(EventKind::PresenceRemoved).await?;
        let EventPayload::PresenceRemoved(p) = ev.parse()? else {
            panic!("not presence.removed")
        };
        assert_eq!(p.presence_id, Some(pid));
        assert_eq!(p.bot_ref, "B2");
        assert_eq!(p.reason, PresenceRemovedReason::TokenRevoked);
    }

    // (b) the platform rejects the credentials -> ops answer credential_dead
    // (a definitive error, reported once; never a silent drop or a hang).
    c.0.driver.inject(Failure::DeadCredentials).await?;
    let (id, r) = c
        .post(space_target("C1", None), &text("never delivered"))
        .await?;
    assert_eq!(err_code(&r), ErrorCode::CredentialDead);
    assert!(
        c.0.driver
            .posts(Some("C1"))
            .await?
            .iter()
            .all(|p| p.op_marker.as_deref() != Some(id.to_string().as_str())),
        "post must not take effect with dead credentials"
    );
    let r = c.op("user.lookup", json!({"user_ref": "U1"})).await?;
    assert_eq!(err_code(&r), ErrorCode::CredentialDead);
    assert_eq!(
        c.0.core.results().iter().filter(|r| r.op_id == id).count(),
        1
    );
    c.0.finish().await?;
    Ok(())
}

// ------------------------------------ 9. a redelivered event is processed once

#[tokio::test]
async fn s09_redelivered_event_is_processed_once() -> R {
    let c = ctx(StartOptions::default()).await?;

    // Same platform event id twice (and the same message): the core reports a
    // duplicate the second time; the inbox holds it once.
    let spec = MessageSpec {
        event_id: Some("platform-evt-1".into()),
        message_ref: Some("pm-1".into()),
        ..msg("U1", None, "once please")
    };
    let first = c.0.driver.post_message(spec.clone()).await?;
    let second = c.0.driver.post_message(spec).await?;
    assert_eq!(first.ack, Ack::Accepted);
    assert_eq!(second.ack, Ack::Duplicate);
    assert_eq!(
        c.0.core
            .events()
            .iter()
            .filter(|e| e.event_id == "platform-evt-1")
            .count(),
        1
    );
    assert_eq!(c.0.core.duplicates(), vec!["platform-evt-1".to_owned()]);

    // The platform is acked only after the core accepted: while the core holds
    // its answer, the platform-side delivery stays pending.
    c.0.core.hold_events(true);
    let driver = c.0.driver.clone();
    let pending = tokio::spawn(async move {
        driver
            .post_message(MessageSpec {
                event_id: Some("platform-evt-2".into()),
                ..msg("U1", None, "held")
            })
            .await
    });
    let arrived = eventually(T, || (c.0.core.event_posts() >= 3).then_some(())).await;
    assert!(arrived.is_some(), "event never reached the core");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !pending.is_finished(),
        "platform acked before the core accepted"
    );
    assert!(
        c.0.core
            .events()
            .iter()
            .all(|e| e.event_id != "platform-evt-2")
    );
    c.0.core.hold_events(false);
    let d = tokio::time::timeout(T, pending).await???;
    assert_eq!(d.ack, Ack::Accepted);

    // A core outage: the same event id is retried until accepted, never lost
    // and never doubled.
    c.0.core.fail_next_event_posts(1);
    let before = c.0.core.event_posts();
    let d =
        c.0.driver
            .post_message(MessageSpec {
                event_id: Some("platform-evt-3".into()),
                ..msg("U1", None, "after outage")
            })
            .await?;
    assert_eq!(d.ack, Ack::Accepted);
    assert!(c.0.core.event_posts() >= before + 2, "no retry after 503");
    assert_eq!(
        c.0.core
            .events()
            .iter()
            .filter(|e| e.event_id == "platform-evt-3")
            .count(),
        1
    );
    assert_eq!(c.0.core.duplicates().len(), 1, "no spurious duplicates");
    c.0.finish().await?;
    Ok(())
}

// --------------------------------------- 10. a re-claimed post is not duplicated

#[tokio::test]
async fn s10_reclaimed_post_is_not_duplicated() -> R {
    let c = ctx(StartOptions::default()).await?;
    // The post takes effect on the platform but its result never leaves.
    c.0.driver.inject(Failure::LoseResults(1)).await?;
    let id = Uuid::now_v7();
    c.0.core.enqueue_with_id(
        id,
        "message.post",
        Some(c.0.integration),
        json!({"target": {"space_ref": "C1"}, "sender": "shared",
               "content": text("exactly once"), "op_marker": id.to_string()}),
    );
    let claims = c.0.core.wait_claims(id, 1, T).await.expect("first claim");
    assert!(!claims[0].maybe_sent);
    let marker = id.to_string();
    // Wait until the effect is visible on the platform.
    let mut landed = None;
    for _ in 0..200 {
        landed =
            c.0.driver
                .posts(Some("C1"))
                .await?
                .into_iter()
                .find(|p| p.op_marker.as_deref() == Some(marker.as_str()));
        if landed.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let landed = landed.expect("the lost-result post must still take effect");
    assert!(
        c.0.core.results().iter().all(|r| r.op_id != id),
        "result was supposed to be lost"
    );

    // The lease lapses: the core hands the op out again with maybe_sent.
    assert!(c.0.core.expire_lease(id));
    let claims = c.0.core.wait_claims(id, 2, T).await.expect("re-claim");
    assert!(claims[1].maybe_sent, "re-claim must carry maybe_sent");
    assert_eq!(claims[1].attempt, 2);
    let r =
        c.0.core
            .wait_result(id, T)
            .await
            .expect("result after re-claim");
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    assert_eq!(
        res.message_ref, landed.message_ref,
        "must report the first post"
    );

    let with_marker: Vec<Post> =
        c.0.driver
            .posts(Some("C1"))
            .await?
            .into_iter()
            .filter(|p| p.op_marker.as_deref() == Some(marker.as_str()))
            .collect();
    assert_eq!(with_marker.len(), 1, "post duplicated: {with_marker:?}");
    c.0.finish().await?;
    Ok(())
}

// ------------------------------------------- 11. broadcast mention defused

#[tokio::test]
async fn s11_broadcast_mentions_are_defused() -> R {
    let c = ctx(StartOptions::default()).await?;
    let hostile = "ping @channel, @all and @here now, thanks <@user:U1>";

    // In message text.
    let (_, r) = c.post(space_target("C1", None), &text(hostile)).await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    assert_eq!(
        c.0.driver.live_broadcasts(&post),
        Vec::<String>::new(),
        "text reached the platform with live broadcasts: {:?}",
        post.text
    );
    assert!(
        post.text.contains("channel"),
        "defusing must keep the words"
    );

    // In card text, fields and button labels.
    let card = Card {
        blocks: vec![
            Block::Header {
                text: "@channel header".into(),
            },
            Block::Text {
                text: hostile.into(),
            },
            Block::Fields {
                pairs: vec![Field {
                    label: "@all".into(),
                    value: "@here".into(),
                }],
            },
            Block::Context {
                text: "@everyone? @channel".into(),
            },
            Block::Buttons {
                items: vec![ButtonItem {
                    label: "@here".into(),
                    style: ButtonStyle::Default,
                    token: "t-1".into(),
                }],
            },
        ],
    };
    let (_, r) = c
        .post(space_target("C1", None), &Content::card(card))
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    assert_eq!(
        c.0.driver.live_broadcasts(&post),
        Vec::<String>::new(),
        "card reached the platform with live broadcasts: {:?}",
        post.card
    );

    // Positive control: the core's own `<@here>` token is the only way to a
    // live broadcast (it only emits it when explicitly intended).
    let (_, r) = c
        .post(space_target("C1", None), &text("deploy done <@here>"))
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    assert_eq!(c.0.driver.live_broadcasts(&post), vec!["@here".to_owned()]);
    c.0.finish().await?;
    Ok(())
}

// ------------------------------------------------ 12. capability fallbacks

#[tokio::test]
async fn s12_capabilities_declared_are_honoured() -> R {
    // A deliberately poor platform: no threads, buttons, menus, edits,
    // reactions, ephemeral, history, files or native status.
    let declared = vec!["commands".to_owned()];
    let c = ctx(StartOptions {
        capabilities: Some(declared.clone()),
    })
    .await?;

    // The manifest sent at register is exactly what the connector can do.
    let regs = c.0.core.registrations();
    let mut caps: Vec<String> = regs[0]
        .manifest
        .enabled_capabilities()
        .into_iter()
        .map(str::to_owned)
        .collect();
    caps.sort();
    assert_eq!(caps, declared);
    assert_eq!(c.0.driver.capabilities().await?, declared);

    // Ops needing an undeclared capability answer `unsupported` and have no effect.
    let (_, r) = c
        .post(space_target("C1", None), &text("plain post"))
        .await?;
    let plain: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    assert_eq!(
        plain.thread_ref, "",
        "without `threads` a space is one thread"
    );
    let posts_before = c.0.driver.posts(None).await?.len();
    let cases: Vec<(&str, Value)> = vec![
        (
            "message.update",
            json!({"space_ref": "C1", "message_ref": plain.message_ref,
                   "sender": "shared", "content": text("edited")}),
        ),
        (
            "reaction.add",
            json!({"space_ref": "C1", "message_ref": plain.message_ref,
                   "sender": "shared", "emoji": "eyes"}),
        ),
        (
            "status.set",
            json!({"space_ref": "C1", "sender": "shared", "text": "working"}),
        ),
        ("history.read", json!({"space_ref": "C1", "limit": 5})),
        ("file.fetch", json!({"file_ref": "F1", "max_bytes": 100})),
    ];
    for (kind, payload) in cases {
        let r = c.op(kind, payload).await?;
        assert_eq!(err_code(&r), ErrorCode::Unsupported, "{kind}");
    }
    let (_, r) = c
        .post_with(
            space_target("C1", None),
            &text("private"),
            json!({"ephemeral_to": "U1"}),
        )
        .await?;
    assert_eq!(err_code(&r), ErrorCode::Unsupported, "ephemeral post");
    let att = c.0.core.add_attachment("a.txt", "text/plain", b"a");
    let (_, r) = c
        .post_with(
            space_target("C1", None),
            &text("with file"),
            json!({"files": [{"attachment_id": att, "name": "a.txt",
                   "download_url": format!("{}/v1/connector/files/{att}", c.0.core.url())}]}),
        )
        .await?;
    assert_eq!(err_code(&r), ErrorCode::Unsupported, "post with files");
    // An operation kind the connector has never heard of.
    let r = c.op("frobnicate.widget", json!({})).await?;
    assert_eq!(err_code(&r), ErrorCode::Unsupported, "unknown kind");
    assert_eq!(
        c.0.driver.posts(None).await?.len(),
        posts_before,
        "unsupported ops must not take effect"
    );
    let unchanged = c.find_post(&plain.message_ref).await?;
    assert_eq!(unchanged.text, "plain post");
    assert!(unchanged.reactions.is_empty());

    // Interactive blocks the platform cannot render are not rendered as such.
    let card = Card::new(vec![
        Block::header("Approval needed"),
        Block::buttons(vec![ButtonItem::new(
            "Approve",
            ButtonStyle::Primary,
            "tok-a".into(),
        )])?,
        Block::select("Pick", [("One".to_owned(), "1".to_owned())], "tok-s".into())?,
    ])?;
    let (_, r) = c
        .post(space_target("C1", None), &Content::card(card))
        .await?;
    let res: MessagePostResult = serde_json::from_value(ok_data(&r))?;
    let post = c.find_post(&res.message_ref).await?;
    assert!(
        c.0.driver.buttons(&post).is_empty(),
        "buttons rendered without `buttons`"
    );
    assert!(
        c.0.driver.select(&post).is_none(),
        "select rendered without `menus`"
    );
    c.0.finish().await?;
    Ok(())
}

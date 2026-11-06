//! Test control API (`/fake/*`): inject platform events, read what was posted,
//! switch failures. Mounted only when explicitly enabled (test profile).

use std::sync::{Arc, Mutex, MutexGuard};

use aurora_connector_sdk::EventAck;
use aurora_protocol::connector::*;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::connector::{FakeConnector, now};
use crate::platform::{Failures, FakeSpace, FakeUser, PostedMessage};

type Ctl = State<Arc<FakeConnector>>;
type Reply = (StatusCode, Json<Value>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn bad(msg: impl ToString) -> Reply {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": msg.to_string()})),
    )
}

fn ack_json(a: &EventAck) -> Value {
    match a {
        EventAck::Accepted => json!({"ack": "accepted"}),
        EventAck::Duplicate => json!({"ack": "duplicate"}),
        EventAck::Rejected(c) => json!({"ack": "rejected", "code": c.as_str()}),
    }
}

async fn emit<T: serde::Serialize>(
    f: &FakeConnector,
    integration: Option<Uuid>,
    kind: EventKind,
    payload: &T,
) -> Reply {
    match f.post_event(integration, kind, payload).await {
        Ok(a) => (StatusCode::OK, Json(ack_json(&a))),
        Err(e) => (StatusCode::CONFLICT, Json(json!({"error": e}))),
    }
}

pub fn router(f: Arc<FakeConnector>) -> Router {
    Router::new()
        .route("/fake/users", post(put_user))
        .route("/fake/spaces", post(put_space))
        .route("/fake/messages", post(inject_message))
        .route("/fake/messages/edit", post(inject_edit))
        .route("/fake/actions", post(inject_action))
        .route("/fake/commands", post(inject_command))
        .route("/fake/bot/added", post(bot_added))
        .route("/fake/bot/removed", post(bot_removed))
        .route("/fake/presence-removed", post(presence_removed))
        .route("/fake/posted", get(posted))
        .route("/fake/failures", post(set_failures))
        .route("/fake/reset", post(reset))
        .with_state(f)
}

async fn put_user(State(f): Ctl, Json(u): Json<FakeUser>) -> Reply {
    lock(&f.platform).upsert_user(u);
    (StatusCode::OK, Json(json!({"ok": true})))
}

async fn put_space(State(f): Ctl, Json(s): Json<FakeSpace>) -> Reply {
    lock(&f.platform).upsert_space(s);
    (StatusCode::OK, Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct InjectMessage {
    #[serde(default)]
    integration_id: Option<Uuid>,
    /// Existing space, or omitted together with `dm_with` for a DM.
    #[serde(default)]
    space_ref: Option<String>,
    #[serde(default)]
    thread_ref: Option<String>,
    author_ref: String,
    text: String,
    #[serde(default)]
    files: Vec<FileRef>,
}

async fn inject_message(State(f): Ctl, Json(m): Json<InjectMessage>) -> Reply {
    let (payload, message_ref) = {
        let mut p = lock(&f.platform);
        let Some(author) = p.users.get(&m.author_ref).cloned() else {
            return bad("unknown author");
        };
        let space_ref = match &m.space_ref {
            Some(s) => s.clone(),
            None => p.dm_with(&m.author_ref),
        };
        let Some(space) = p.spaces.get(&space_ref).cloned() else {
            return bad("unknown space");
        };
        let message_ref = p.next_id("m");
        let mut mentions = vec![];
        let mut rest = m.text.as_str();
        while let Some(i) = rest.find("<@") {
            let tail = &rest[i + 2..];
            let Some(end) = tail.find('>') else { break };
            let token = &tail[..end];
            if let Some(r) = token.strip_prefix("user:").or(token.strip_prefix("bot:")) {
                let kind = if p.is_bot(r) {
                    MentionKind::Bot
                } else {
                    MentionKind::User
                };
                mentions.push(Mention {
                    kind,
                    reference: r.to_owned(),
                });
            } else if token == "here" {
                mentions.push(Mention {
                    kind: MentionKind::Broadcast,
                    reference: "here".into(),
                });
            }
            rest = &tail[end + 1..];
        }
        let addressed = mentions
            .iter()
            .find(|x| x.kind == MentionKind::Bot)
            .map(|x| x.reference.clone())
            .or_else(|| {
                // In a DM every message is addressed to the DM's bot.
                (space.kind == "dm")
                    .then(|| {
                        p.users
                            .values()
                            .find(|u| u.is_bot)
                            .map(|u| u.reference.clone())
                    })
                    .flatten()
            })
            .unwrap_or_default();
        let payload = MessagePosted {
            space: Space {
                reference: space.reference.clone(),
                kind: SpaceKind::from_wire(&space.kind).unwrap_or(SpaceKind::Channel),
                label: space.label.clone(),
            },
            thread_ref: m.thread_ref.clone(),
            message_ref: message_ref.clone(),
            author: Author {
                reference: author.reference.clone(),
                kind: if author.is_bot {
                    AuthorKind::Bot
                } else {
                    AuthorKind::Person
                },
                display_name: author.display_name.clone(),
                email: author.email.clone(),
                email_verified: author.email.as_ref().map(|_| author.email_verified),
            },
            text: m.text.clone(),
            bot_mentioned: mentions.iter().any(|x| x.kind == MentionKind::Bot),
            mentions,
            addressed_bot_ref: addressed,
            files: m.files.clone(),
        };
        p.messages.push(PostedMessage {
            message_ref: message_ref.clone(),
            space_ref,
            thread_ref: m.thread_ref.clone().unwrap_or_default(),
            author_ref: m.author_ref.clone(),
            text: m.text.clone(),
            card: None,
            ephemeral_to: None,
            op_marker: None,
            files: m.files.iter().map(|x| x.name.clone()).collect(),
            deleted: false,
            reactions: vec![],
            at: now(),
        });
        (payload, message_ref)
    };
    let (status, Json(mut body)) =
        emit(&f, m.integration_id, EventKind::MessagePosted, &payload).await;
    body["message_ref"] = json!(message_ref);
    (status, Json(body))
}

#[derive(Deserialize)]
struct InjectEdit {
    #[serde(default)]
    integration_id: Option<Uuid>,
    message_ref: String,
    /// `None` deletes the message.
    #[serde(default)]
    text: Option<String>,
}

async fn inject_edit(State(f): Ctl, Json(m): Json<InjectEdit>) -> Reply {
    let (space_ref, thread) = {
        let mut p = lock(&f.platform);
        let Some(msg) = p.find(&m.message_ref) else {
            return bad("unknown message");
        };
        match &m.text {
            Some(t) => msg.text = t.clone(),
            None => msg.deleted = true,
        }
        (msg.space_ref.clone(), msg.thread_ref.clone())
    };
    let payload = MessageChanged {
        space_ref,
        thread_ref: (!thread.is_empty()).then_some(thread),
        message_ref: m.message_ref,
        text: m.text.clone(),
    };
    let kind = if m.text.is_some() {
        EventKind::MessageEdited
    } else {
        EventKind::MessageDeleted
    };
    emit(&f, m.integration_id, kind, &payload).await
}

#[derive(Deserialize)]
struct InjectAction {
    #[serde(default)]
    integration_id: Option<Uuid>,
    #[serde(flatten)]
    action: ActionInvoked,
}

async fn inject_action(State(f): Ctl, Json(a): Json<InjectAction>) -> Reply {
    emit(&f, a.integration_id, EventKind::ActionInvoked, &a.action).await
}

#[derive(Deserialize)]
struct InjectCommand {
    #[serde(default)]
    integration_id: Option<Uuid>,
    #[serde(flatten)]
    command: CommandInvoked,
}

async fn inject_command(State(f): Ctl, Json(c): Json<InjectCommand>) -> Reply {
    emit(&f, c.integration_id, EventKind::CommandInvoked, &c.command).await
}

#[derive(Deserialize)]
struct BotChange {
    #[serde(default)]
    integration_id: Option<Uuid>,
    space_ref: String,
    bot_ref: String,
    #[serde(default)]
    actor_ref: Option<String>,
}

async fn bot_change(f: Arc<FakeConnector>, c: BotChange, add: bool) -> Reply {
    let space = {
        let mut p = lock(&f.platform);
        let Some(s) = p.spaces.get_mut(&c.space_ref) else {
            return bad("unknown space");
        };
        if add {
            s.members.insert(c.bot_ref.clone());
        } else {
            s.members.remove(&c.bot_ref);
        }
        Space {
            reference: s.reference.clone(),
            kind: SpaceKind::from_wire(&s.kind).unwrap_or(SpaceKind::Channel),
            label: s.label.clone(),
        }
    };
    let payload = BotMembership {
        space,
        bot_ref: c.bot_ref,
        actor_ref: c.actor_ref,
    };
    let kind = if add {
        EventKind::BotAdded
    } else {
        EventKind::BotRemoved
    };
    emit(&f, c.integration_id, kind, &payload).await
}

async fn bot_added(State(f): Ctl, Json(c): Json<BotChange>) -> Reply {
    bot_change(f, c, true).await
}

async fn bot_removed(State(f): Ctl, Json(c): Json<BotChange>) -> Reply {
    bot_change(f, c, false).await
}

#[derive(Deserialize)]
struct PresenceGone {
    #[serde(default)]
    integration_id: Option<Uuid>,
    #[serde(flatten)]
    removed: PresenceRemoved,
}

async fn presence_removed(State(f): Ctl, Json(c): Json<PresenceGone>) -> Reply {
    emit(&f, c.integration_id, EventKind::PresenceRemoved, &c.removed).await
}

#[derive(Deserialize)]
struct PostedQuery {
    #[serde(default)]
    space_ref: Option<String>,
    #[serde(default)]
    include_deleted: bool,
}

async fn posted(State(f): Ctl, Query(q): Query<PostedQuery>) -> Reply {
    let p = lock(&f.platform);
    let msgs: Vec<&PostedMessage> = p
        .messages
        .iter()
        .filter(|m| q.space_ref.as_ref().is_none_or(|s| &m.space_ref == s))
        .filter(|m| q.include_deleted || !m.deleted)
        .collect();
    (StatusCode::OK, Json(json!({"messages": msgs})))
}

async fn set_failures(State(f): Ctl, Json(x): Json<Failures>) -> Reply {
    lock(&f.platform).failures = x;
    (StatusCode::OK, Json(json!({"ok": true})))
}

async fn reset(State(f): Ctl) -> Reply {
    *lock(&f.platform) = Default::default();
    (StatusCode::OK, Json(json!({"ok": true})))
}

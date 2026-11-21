//! Outbound operations (docs/20 §Operations).

use std::sync::Arc;

use aurora_channels::{Capability, CapabilitySet, Content, FallbackContext, render_card};
use aurora_connector_sdk::{ClientError, CoreClient};
use aurora_protocol::common::Validate;
use aurora_protocol::connector::*;
use reqwest::Method;
use serde_json::{Value, json};

use crate::api::{R, is_mm_id};
use crate::events::{MmChannel, MmPost, MmUser, author, display_name, rfc3339_ms};
use crate::integ::{Integ, bad_shape, posts_of};
use crate::presence;
use crate::render::{CardCtx, emoji_name, neutralize_tokens, render_attachment, render_text};

/// Mattermost's default `MaxPostSize` (`message_max_chars` of the manifest).
pub const MESSAGE_MAX_CHARS: usize = 16_383;
/// Files per post (manifest `files.max_count`).
pub const MAX_FILES: usize = 10;
/// Posts searched for the op marker when `maybe_sent`.
const MARKER_SEARCH: &str = "50";

fn err(code: ErrorCode, msg: impl Into<String>) -> OpError {
    OpError::new(code, msg)
}

fn need(integ: &Integ, cap: Capability) -> R<()> {
    if integ.has(cap.as_str()) {
        Ok(())
    } else {
        Err(err(
            ErrorCode::Unsupported,
            format!("capability {} not declared", cap.as_str()),
        ))
    }
}

fn need_id(id: &str, what: &str) -> R<()> {
    if is_mm_id(id) {
        Ok(())
    } else {
        Err(err(ErrorCode::NotFound, format!("no such {what}")))
    }
}

fn map_client_error(e: &ClientError) -> OpError {
    let code = match e.status() {
        Some(404) => ErrorCode::NotFound,
        Some(413) => ErrorCode::TooLarge,
        Some(400 | 422) => ErrorCode::Invalid,
        Some(403) => ErrorCode::Forbidden,
        _ => ErrorCode::Unavailable,
    };
    err(code, e.to_string())
}

/// The token and user id a message is sent as.
fn sender(integ: &Integ, s: &Sender) -> R<(String, String)> {
    let d = integ.snapshot();
    match s {
        Sender::Shared(_) => Ok((d.bot_token, d.bot_id)),
        Sender::Presence { presence_id } => d
            .presences
            .get(presence_id)
            .map(|p| (p.token.clone(), p.user_id.clone()))
            .ok_or_else(|| err(ErrorCode::NotFound, "unknown presence")),
        // (P4-UTOK) A person's token never goes through the bot's client: only the send and
        // verify paths below use it, through `PersonClient`.
        Sender::UserToken { .. } => Err(err(
            ErrorCode::Invalid,
            "a person's token is used only by message.post and user_token.verify",
        )),
    }
}

/// The person's token of a claimed op (`Operation.user_credential`).
fn person_token(op: &Operation) -> R<&str> {
    op.user_credential
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            err(
                ErrorCode::Invalid,
                "the operation carries no person's token",
            )
        })
}

/// `user_token.verify` (C-854): `GET /users/me` with the person's token only. A `401` answers
/// `credential_dead`; the core compares the account with the person's link.
async fn user_verify(integ: &Arc<Integ>, op: &Operation) -> R<Value> {
    need(integ, Capability::UserTokens)?;
    let me = integ.mm.person_client().me(person_token(op)?).await?;
    let id = me
        .get("id")
        .and_then(Value::as_str)
        .filter(|i| is_mm_id(i))
        .ok_or_else(|| {
            err(
                ErrorCode::Unavailable,
                "unexpected mattermost response to users/me",
            )
        })?;
    let handle = me
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(json!({"platform_user_ref": id, "handle": handle}))
}

/// `message.post` with `sender.user_token` (C-857): `POST /posts` with the person's token and
/// nothing else — no ephemeral post, no file, no card, no direct-channel creation. The post ends
/// with the footer naming the agent and the owner (D-283(c)), added here and not by the model.
async fn person_post(integ: &Arc<Integ>, op: &Operation, m: MessagePostOp) -> R<Value> {
    need(integ, Capability::UserTokens)?;
    if m.ephemeral_to.is_some() || !m.files.is_empty() {
        return Err(err(
            ErrorCode::Invalid,
            "a post as a person has no ephemeral or file part",
        ));
    }
    let token = person_token(op)?;
    let footer = m.user_footer.as_ref().ok_or_else(|| {
        err(
            ErrorCode::Invalid,
            "a post as a person needs the names of its footer",
        )
    })?;
    let (channel_id, root_in) = match &m.target {
        Target::Space {
            space_ref,
            thread_ref,
        } => {
            need_id(space_ref, "space")?;
            (space_ref.clone(), thread_ref.clone().unwrap_or_default())
        }
        Target::User { .. } => {
            return Err(err(
                ErrorCode::Invalid,
                "a post as a person goes to a channel or thread",
            ));
        }
    };
    let c: Content = serde_json::from_value(m.content.clone())
        .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
    if c.card.is_some() || c.narration.is_some() {
        return Err(err(ErrorCode::Invalid, "a post as a person is plain text"));
    }
    let (body, _) = render_content(integ, &m.content).await?;
    let message = format!(
        "{body}\n\n{}",
        crate::render::user_footer(&footer.agent_name, &footer.owner_label)
    );
    if message.chars().count() > MESSAGE_MAX_CHARS {
        return Err(err(ErrorCode::TooLarge, "message too long"));
    }
    // At-most-once in practice: a re-claimed post may already be there. The person's token has no
    // other endpoint, so the bot looks for the marker; when it cannot tell, the post is not
    // repeated (the op is retried or fails rather than posting twice as a person).
    if op.maybe_sent {
        let bot = integ.snapshot().bot_token;
        let v = integ
            .mm
            .get_q(
                &bot,
                &format!("/channels/{channel_id}/posts"),
                &[("per_page", MARKER_SEARCH)],
            )
            .await?;
        if let Some(prev) = posts_of(&v)
            .into_iter()
            .filter(|p| {
                p.props.get("aurora_op").and_then(Value::as_str) == Some(m.op_marker.as_str())
            })
            .min_by_key(|p| p.create_at)
        {
            let channel = integ.channel(&channel_id).await.ok();
            return Ok(json!({
                "message_ref": prev.id,
                "thread_ref": thread_of(integ, channel.as_ref(), &prev.root_id, &prev.id),
            }));
        }
    }
    let root = if integ.has("threads") {
        root_in
    } else {
        String::new()
    };
    let created = integ
        .mm
        .person_client()
        .post(
            token,
            &json!({
                "channel_id": channel_id,
                "message": message,
                "root_id": root,
                "props": props(Some(&m.op_marker), Vec::new()),
            }),
        )
        .await?;
    let post: MmPost = serde_json::from_value(created).map_err(bad_shape)?;
    let channel = integ.channel(&channel_id).await.ok();
    Ok(json!({
        "message_ref": post.id,
        "thread_ref": thread_of(integ, channel.as_ref(), &root, &post.id),
    }))
}

/// Neutral content → `(message, props.attachments)`, honouring the integration's capabilities.
async fn render_content(integ: &Integ, content: &Value) -> R<(String, Vec<Value>)> {
    let c: Content = serde_json::from_value(content.clone())
        .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
    // (P4-NARR, C-851) A narration card is a platform-authored post, rendered as data.
    if let Some(n) = &c.narration {
        let message = crate::render::render_narration(n);
        if message.chars().count() > MESSAGE_MAX_CHARS {
            return Err(err(ErrorCode::TooLarge, "message too long"));
        }
        return Ok((message, Vec::new()));
    }
    let actions_url = integ.settings.actions_url();
    let mut caps_vec: Vec<Capability> = integ
        .snapshot()
        .caps
        .iter()
        .filter_map(|n| Capability::from_wire(n).ok())
        .collect();
    if actions_url.is_none() {
        // Without a public URL Mattermost has nowhere to send clicks.
        caps_vec.retain(|c| !matches!(c, Capability::Buttons | Capability::Menus));
    }
    let caps = CapabilitySet::new(caps_vec);
    let mut text = c.text.clone().unwrap_or_default();
    let mut card = None;
    if let Some(cd) = &c.card {
        cd.validate()
            .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
        let r = render_card(cd, &caps, &FallbackContext::default());
        if let Some(extra) = r.text {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&extra);
        }
        card = r.card;
    }
    let mut refs = crate::render::mention_refs(&text);
    if let Some(cd) = &card
        && let Ok(s) = serde_json::to_string(cd)
    {
        let more = crate::render::mention_refs(&s);
        refs.users.extend(more.users);
        refs.spaces.extend(more.spaces);
        for n in more.names {
            if !refs.names.contains(&n) {
                refs.names.push(n);
            }
        }
    }
    let lk = integ.lookups(&refs).await;
    let message = render_text(&text, &lk);
    let mut attachments = Vec::new();
    if let Some(cd) = card {
        let ctx = CardCtx {
            actions_url: actions_url.as_deref(),
            hmac_key: &integ.settings.hmac_key,
            integration_id: integ.id,
        };
        attachments.push(render_attachment(&cd, &ctx, &lk));
    }
    if message.chars().count() > MESSAGE_MAX_CHARS {
        return Err(err(ErrorCode::TooLarge, "message too long"));
    }
    if message.is_empty() && attachments.is_empty() {
        return Err(err(ErrorCode::Invalid, "empty content"));
    }
    Ok((message, attachments))
}

/// (FIX-203) Debug aid: the button and menu fields of `attachments` (as sent to Mattermost, or as
/// Mattermost returned them): action id, type, callback URL and integration id. The action token
/// and its signature are never logged, only their lengths; labels and text are not logged.
fn action_summary(attachments: &[Value]) -> Vec<Value> {
    attachments
        .iter()
        .filter_map(|a| a.get("actions").and_then(Value::as_array))
        .flatten()
        .map(|a| {
            let integration = a.get("integration");
            let ctx = integration.and_then(|i| i.get("context"));
            let len = |k: &str| {
                ctx.and_then(|c| c.get(k))
                    .and_then(Value::as_str)
                    .map(str::len)
            };
            json!({
                "id": a.get("id"),
                "type": a.get("type"),
                "url": integration.and_then(|i| i.get("url")),
                "iid": ctx.and_then(|c| c.get("iid")),
                "token_len": len("token"),
                "sig_len": len("sig"),
            })
        })
        .collect()
}

/// (FIX-203) Logs, at debug, the actions of a card going to Mattermost and of the post Mattermost
/// answered with (`returned`), so an operator can see the exact callback URL the buttons carry.
fn log_actions(op: &str, channel_id: &str, sent: &[Value], returned: Option<&Value>) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    let sent = action_summary(sent);
    if sent.is_empty() {
        return;
    }
    let post_id = returned.and_then(|p| p.get("id")).and_then(Value::as_str);
    let stored = returned
        .and_then(|p| p.pointer("/props/attachments"))
        .and_then(Value::as_array)
        .map_or(Value::Null, |a| Value::Array(action_summary(a)))
        .to_string();
    let sent = Value::Array(sent).to_string();
    tracing::debug!(
        op,
        channel_id,
        post_id,
        sent = %sent,
        returned = %stored,
        "card actions posted to Mattermost"
    );
}

fn props(marker: Option<&str>, attachments: Vec<Value>) -> Value {
    let mut p = serde_json::Map::new();
    if let Some(m) = marker {
        p.insert("aurora_op".into(), json!(m));
    }
    p.insert("attachments".into(), json!(attachments));
    Value::Object(p)
}

fn thread_of(integ: &Integ, channel: Option<&MmChannel>, root: &str, post_id: &str) -> String {
    if !integ.has("threads") {
        return String::new();
    }
    if !root.is_empty() {
        return root.to_owned();
    }
    match channel {
        Some(c) if c.kind == "D" => String::new(),
        _ => post_id.to_owned(),
    }
}

async fn message_post(
    integ: &Arc<Integ>,
    core: Option<Arc<CoreClient>>,
    op: &Operation,
    m: MessagePostOp,
) -> R<Value> {
    if matches!(m.sender, Sender::UserToken { .. }) {
        return person_post(integ, op, m).await;
    }
    let (token, bot_id) = sender(integ, &m.sender)?;
    let d = integ.snapshot();
    let ephemeral_token = match &m.ephemeral_to {
        Some(_) => {
            need(integ, Capability::Ephemeral)?;
            Some(d.admin_token.clone().ok_or_else(|| {
                err(
                    ErrorCode::Unsupported,
                    "ephemeral posts need an admin token",
                )
            })?)
        }
        None => None,
    };
    if !m.files.is_empty() {
        need(integ, Capability::Files)?;
        if m.files.len() > MAX_FILES {
            return Err(err(ErrorCode::Invalid, "too many files"));
        }
    }
    let (channel_id, root_in) = match &m.target {
        Target::Space {
            space_ref,
            thread_ref,
        } => {
            need_id(space_ref, "space")?;
            (space_ref.clone(), thread_ref.clone().unwrap_or_default())
        }
        Target::User { user_ref } => {
            need_id(user_ref, "user")?;
            let v = integ
                .mm
                .post(&token, "/channels/direct", &json!([bot_id, user_ref]))
                .await?;
            let ch: MmChannel = serde_json::from_value(v).map_err(bad_shape)?;
            (ch.id, String::new())
        }
    };
    let (message, attachments) = render_content(integ, &m.content).await?;

    // At-most-once in practice: a re-claimed post may already be there.
    if op.maybe_sent && m.ephemeral_to.is_none() {
        let v = integ
            .mm
            .get_q(
                &token,
                &format!("/channels/{channel_id}/posts"),
                &[("per_page", MARKER_SEARCH)],
            )
            .await?;
        if let Some(prev) = posts_of(&v)
            .into_iter()
            .filter(|p| {
                p.props.get("aurora_op").and_then(Value::as_str) == Some(m.op_marker.as_str())
            })
            .min_by_key(|p| p.create_at)
        {
            let channel = integ.channel(&channel_id).await.ok();
            return Ok(json!({
                "message_ref": prev.id,
                "thread_ref": thread_of(integ, channel.as_ref(), &prev.root_id, &prev.id),
            }));
        }
    }

    let mut file_ids: Vec<String> = Vec::new();
    if !m.files.is_empty() {
        let core = core.ok_or_else(|| err(ErrorCode::Unavailable, "not connected to a core"))?;
        for f in &m.files {
            let bytes = core
                .get_file(f.attachment_id)
                .await
                .map_err(|e| map_client_error(&e))?;
            file_ids.push(
                integ
                    .mm
                    .upload_file(&token, &channel_id, &f.name, bytes.to_vec())
                    .await?,
            );
        }
    }

    let root = if integ.has("threads") {
        root_in
    } else {
        String::new()
    };
    let sent_attachments = attachments.clone();
    let body = json!({
        "channel_id": channel_id,
        "message": message,
        "root_id": root,
        "file_ids": file_ids,
        "props": props(Some(&m.op_marker), attachments),
    });
    let op_name = if m.ephemeral_to.is_some() && ephemeral_token.is_some() {
        "message.post (ephemeral)"
    } else {
        "message.post"
    };
    let created = match (&m.ephemeral_to, ephemeral_token) {
        (Some(user), Some(admin)) => {
            need_id(user, "user")?;
            integ
                .mm
                .post(
                    &admin,
                    "/posts/ephemeral",
                    &json!({"user_id": user, "post": body}),
                )
                .await?
        }
        _ => integ.mm.post(&token, "/posts", &body).await?,
    };
    log_actions(op_name, &channel_id, &sent_attachments, Some(&created));
    let post: MmPost = serde_json::from_value(created).map_err(bad_shape)?;
    let channel = integ.channel(&channel_id).await.ok();
    Ok(json!({
        "message_ref": post.id,
        "thread_ref": thread_of(integ, channel.as_ref(), &root, &post.id),
    }))
}

async fn message_update(integ: &Arc<Integ>, m: MessageUpdateOp) -> R<Value> {
    need(integ, Capability::Edits)?;
    let (token, _) = sender(integ, &m.sender)?;
    need_id(&m.message_ref, "message")?;
    let existing: MmPost = serde_json::from_value(
        integ
            .mm
            .get(&token, &format!("/posts/{}", m.message_ref))
            .await?,
    )
    .map_err(bad_shape)?;
    if existing.channel_id != m.space_ref {
        return Err(err(ErrorCode::NotFound, "no such message"));
    }
    let (message, attachments) = render_content(integ, &m.content).await?;
    let marker = existing.props.get("aurora_op").and_then(Value::as_str);
    let sent_attachments = attachments.clone();
    let patched = integ
        .mm
        .json::<Value>(
            &token,
            Method::PUT,
            &format!("/posts/{}/patch", m.message_ref),
            &[],
            Some(&json!({"message": message, "props": props(marker, attachments)})),
        )
        .await?;
    log_actions(
        "message.update",
        &m.space_ref,
        &sent_attachments,
        Some(&patched),
    );
    Ok(json!({}))
}

async fn message_delete(integ: &Arc<Integ>, m: MessageDeleteOp) -> R<Value> {
    let (token, _) = sender(integ, &m.sender)?;
    need_id(&m.message_ref, "message")?;
    let existing: MmPost = serde_json::from_value(
        integ
            .mm
            .get(&token, &format!("/posts/{}", m.message_ref))
            .await?,
    )
    .map_err(bad_shape)?;
    if existing.channel_id != m.space_ref {
        return Err(err(ErrorCode::NotFound, "no such message"));
    }
    integ
        .mm
        .send(
            &token,
            Method::DELETE,
            &format!("/posts/{}", m.message_ref),
            &[],
            None,
        )
        .await?;
    Ok(json!({}))
}

async fn reaction(integ: &Arc<Integ>, r: ReactionOp, add: bool) -> R<Value> {
    need(integ, Capability::Reactions)?;
    let (token, user) = sender(integ, &r.sender)?;
    need_id(&r.message_ref, "message")?;
    let emoji = emoji_name(&r.emoji);
    if add {
        integ
            .mm
            .post(
                &token,
                "/reactions",
                &json!({"user_id": user, "post_id": r.message_ref, "emoji_name": emoji}),
            )
            .await?;
    } else {
        integ
            .mm
            .send(
                &token,
                Method::DELETE,
                &format!("/users/{user}/posts/{}/reactions/{emoji}", r.message_ref),
                &[],
                None,
            )
            .await?;
    }
    Ok(json!({}))
}

/// `status.set` / `status.clear`: the sender "is typing…" in the thread until cleared (docs/20
/// §Typing indicator). The refresher lives in the connector; a bad id is refused, a sender that
/// may not post there (403/404) just never types.
async fn status(integ: &Arc<Integ>, s: &StatusOp, set: bool) -> R<Value> {
    let (token, user) = sender(integ, &s.sender)?;
    need_id(&s.space_ref, "space")?;
    let parent = s.thread_ref.clone().unwrap_or_default();
    if !parent.is_empty() {
        need_id(&parent, "thread")?;
    }
    if set {
        integ
            .typing
            .start(integ.mm.clone(), token, user, s.space_ref.clone(), parent);
    } else {
        integ.typing.stop(&user, &s.space_ref, &parent);
    }
    Ok(json!({}))
}

async fn file_fetch(
    integ: &Arc<Integ>,
    core: Option<Arc<CoreClient>>,
    op: &Operation,
    f: FileFetchOp,
) -> R<Value> {
    need(integ, Capability::Files)?;
    need_id(&f.file_ref, "file")?;
    let token = integ.snapshot().bot_token;
    let info: crate::events::MmFileInfo = serde_json::from_value(
        integ
            .mm
            .get(&token, &format!("/files/{}/info", f.file_ref))
            .await?,
    )
    .map_err(bad_shape)?;
    if info.size > f.max_bytes {
        return Err(err(
            ErrorCode::TooLarge,
            "file exceeds the operation's max_bytes",
        ));
    }
    let bytes = integ
        .mm
        .download_file(&token, &f.file_ref, f.max_bytes)
        .await?;
    let core = core.ok_or_else(|| err(ErrorCode::Unavailable, "not connected to a core"))?;
    let content_type = if info.mime_type.is_empty() {
        "application/octet-stream"
    } else {
        &info.mime_type
    };
    let r = core
        .put_file(op.op_id, &info.name, content_type, bytes)
        .await
        .map_err(|e| map_client_error(&e))?;
    Ok(json!(FileFetchResult {
        upload_url_used: true,
        attachment_id: r.attachment_id,
    }))
}

fn platform_user(u: &MmUser) -> PlatformUser {
    let a = author(u);
    PlatformUser {
        reference: u.id.clone(),
        display_name: display_name(u),
        handle: u.username.clone(),
        email: a.email,
        email_verified: a.email_verified.unwrap_or(false),
        is_bot: u.is_bot,
    }
}

async fn user_lookup(integ: &Arc<Integ>, l: UserLookupOp) -> R<Value> {
    let token = integ.snapshot().bot_token;
    let users: Vec<MmUser> = match (&l.user_ref, &l.query) {
        (Some(r), _) => vec![integ.user(r).await?],
        (None, Some(q)) => {
            let limit = l.limit.unwrap_or(25).clamp(1, 100);
            let v = integ
                .mm
                .post(
                    &token,
                    "/users/search",
                    &json!({"term": q, "team_id": integ.team_id, "limit": limit}),
                )
                .await?;
            serde_json::from_value(v).map_err(bad_shape)?
        }
        (None, None) => return Err(err(ErrorCode::Invalid, "user_ref or query required")),
    };
    let users: Vec<PlatformUser> = users.iter().map(platform_user).collect();
    Ok(json!({"users": users}))
}

async fn spaces_list(integ: &Arc<Integ>, l: SpacesListOp) -> R<Value> {
    let d = integ.snapshot();
    let v = integ
        .mm
        .get(
            &d.bot_token,
            &format!("/users/{}/teams/{}/channels", d.bot_id, integ.team_id),
        )
        .await?;
    let channels: Vec<MmChannel> = serde_json::from_value(v).map_err(bad_shape)?;
    let q = l.query.as_ref().map(|q| q.to_lowercase());
    let spaces: Vec<Space> = channels
        .iter()
        .filter(|c| matches!(c.kind.as_str(), "O" | "P"))
        .filter(|c| {
            q.as_ref().is_none_or(|q| {
                c.display_name.to_lowercase().contains(q) || c.name.to_lowercase().contains(q)
            })
        })
        .take(l.limit as usize)
        .map(crate::events::space)
        .collect();
    Ok(json!({"spaces": spaces}))
}

async fn space_members(integ: &Arc<Integ>, m: SpaceMembersOp) -> R<Value> {
    need_id(&m.space_ref, "space")?;
    let token = integ.snapshot().bot_token;
    let members = integ
        .mm
        .get_q(
            &token,
            &format!("/channels/{}/members", m.space_ref),
            &[("per_page", "200")],
        )
        .await?;
    let ids: Vec<String> = members
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("user_id").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let v = integ.mm.post(&token, "/users/ids", &json!(ids)).await?;
    let users: Vec<MmUser> = serde_json::from_value(v).map_err(bad_shape)?;
    let users: Vec<PlatformUser> = users.iter().map(platform_user).collect();
    Ok(json!({"users": users}))
}

async fn history_read(integ: &Arc<Integ>, h: HistoryReadOp) -> R<Value> {
    need(integ, Capability::History)?;
    h.validate()
        .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
    need_id(&h.space_ref, "space")?;
    let token = integ.snapshot().bot_token;
    let limit = h.limit.to_string();
    let v = match &h.thread_ref {
        Some(t) if !t.is_empty() => {
            need_id(t, "thread")?;
            integ.mm.get(&token, &format!("/posts/{t}/thread")).await?
        }
        _ => {
            integ
                .mm
                .get_q(
                    &token,
                    &format!("/channels/{}/posts", h.space_ref),
                    &[("per_page", &limit)],
                )
                .await?
        }
    };
    let mut posts: Vec<MmPost> = posts_of(&v)
        .into_iter()
        .filter(|p| p.kind.is_empty() && p.delete_at == 0 && p.channel_id == h.space_ref)
        .collect();
    posts.sort_by_key(|p| (p.create_at, p.id.clone()));
    if let Some(b) = &h.before
        && let Some(i) = posts.iter().position(|p| &p.id == b)
    {
        posts.truncate(i);
    }
    let start = posts.len().saturating_sub(h.limit as usize);
    let mut messages = Vec::new();
    for p in &posts[start..] {
        let Ok(u) = integ.user(&p.user_id).await else {
            continue;
        };
        messages.push(HistoryMessage {
            message_ref: p.id.clone(),
            author: author(&u),
            text: neutralize_tokens(&p.message),
            at: rfc3339_ms(p.create_at),
        });
    }
    Ok(json!({"messages": messages}))
}

/// Runs one claimed operation against Mattermost (everything except `credentials.validate`).
pub async fn execute(
    integ: &Arc<Integ>,
    core: Option<Arc<CoreClient>>,
    op: &Operation,
) -> R<Value> {
    let payload = op
        .parse()
        .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
    match payload {
        OpPayload::MessagePost(m) => message_post(integ, core, op, m).await,
        OpPayload::MessageUpdate(m) => message_update(integ, m).await,
        OpPayload::MessageDelete(m) => message_delete(integ, m).await,
        OpPayload::ReactionAdd(r) => reaction(integ, r, true).await,
        OpPayload::ReactionRemove(r) => reaction(integ, r, false).await,
        OpPayload::StatusSet(s) => {
            need(integ, Capability::NativeStatus)?;
            status(integ, &s, true).await
        }
        OpPayload::StatusClear(s) => {
            need(integ, Capability::NativeStatus)?;
            status(integ, &s, false).await
        }
        OpPayload::FileFetch(f) => file_fetch(integ, core, op, f).await,
        OpPayload::UserLookup(l) => user_lookup(integ, l).await,
        OpPayload::SpacesList(l) => spaces_list(integ, l).await,
        OpPayload::SpaceMembers(m) => space_members(integ, m).await,
        OpPayload::HistoryRead(h) => history_read(integ, h).await,
        OpPayload::PresenceProvision(p) => {
            need(integ, Capability::PresenceDedicated)?;
            presence::provision(integ, op, p).await
        }
        OpPayload::PresenceDeprovision(_) => Err(err(
            ErrorCode::Invalid,
            "presence.deprovision is self-contained and handled by the connector, not an integration",
        )),
        OpPayload::PresenceRename(c) => {
            need(integ, Capability::PresenceDedicated)?;
            presence::rename(integ, op, c).await
        }
        OpPayload::PresenceSetIcon(c) => {
            need(integ, Capability::PresenceDedicated)?;
            presence::set_icon(integ, op, c).await
        }
        OpPayload::CredentialsValidate(_) => Err(err(
            ErrorCode::Invalid,
            "credentials.validate is handled by the connector, not an integration",
        )),
        OpPayload::UserTokenVerify(_) => user_verify(integ, op).await,
        OpPayload::Other => Err(err(ErrorCode::Unsupported, "unknown operation kind")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (FIX-203) The debug summary carries the callback URL and never the token or its signature.
    #[test]
    fn action_summary_shows_the_url_and_hides_the_token() {
        let attachments = vec![json!({
            "text": "secret text",
            "actions": [{
                "id": "act1", "name": "Allow once", "type": "button",
                "integration": {"url": "https://aurora.example/connectors/mattermost/mm/actions",
                    "context": {"token": "tok-SECRET", "sig": "sig-SECRET", "iid": "i-1"}}
            }]
        })];
        let s = Value::Array(action_summary(&attachments)).to_string();
        assert!(s.contains("https://aurora.example/connectors/mattermost/mm/actions"));
        assert!(s.contains("\"act1\"") && s.contains("\"i-1\""));
        assert!(s.contains("\"token_len\":10"));
        for hidden in ["tok-SECRET", "sig-SECRET", "secret text", "Allow once"] {
            assert!(!s.contains(hidden), "{hidden} leaked: {s}");
        }
    }
}

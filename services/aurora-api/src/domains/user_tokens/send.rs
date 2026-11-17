//! `send_as_me`: the tool's eligibility, its held approval and the approved send (P4-UTOK,
//! D-283, C-856, C-857, SEC-113; docs/13 §5.11).
//!
//! The tool never sends. At the call it checks that the turn is **the owner's own**: its speaker
//! is the token owner, it came through a chat thread, the conversation is the owner's direct
//! conversation with the agent or a group thread the owner started, the agent holds the owner's
//! grant and the token and link stand. A permitted call creates a held action approval whose
//! frozen payload and platform-composed summary never leave the owner's sight; only the owner
//! decides it (`action_approvals::service::decide`), there is no `allow` path, and on approval the
//! post is queued as a `message.post` with `sender: { user_token }` and its own per-op copy.

use super::custody::{self, CopyError};
use crate::app::AppState;
use crate::domains::action_approvals::handlers::{
    ActionContext, ActionError, ActionHandler, ActionOutcome,
};
use crate::domains::action_approvals::service::{self as approvals, NewAction};
use crate::domains::channels::messaging::{
    self as m, Fail, For, RKind, Where, recipient_json, resolve_recipient,
};
use crate::domains::connectors::ops::{self, NewOp};
use aurora_channels::{Capability, Content};
use aurora_core::limits::USER_TOKEN_PENDING_MAX;
use aurora_core::sanitize;
use aurora_db::DbError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::user_tokens::{self as repo, UserTokenRow};
use aurora_db::repo::{agents, conversations, thread_links, turns, user_links, users};
use aurora_protocol::connector::{MessagePostOp, Sender, Target, UserFooter};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// Why `send_as_me` was refused to the agent (C-856). The message is for the model: it never
/// names anyone.
pub struct Refusal {
    pub code: &'static str,
    pub message: &'static str,
}

const NOT_ELIGIBLE: Refusal = Refusal {
    code: "not_eligible",
    message: "You can post as a person only while answering that person in their own conversation with you.",
};
const NOT_GRANTED: Refusal = Refusal {
    code: "not_granted",
    message: "This person has not allowed you to post as them.",
};
const TOKEN_INACTIVE: Refusal = Refusal {
    code: "token_inactive",
    message: "This person's token is not usable right now.",
};
const PENDING_LIMIT: Refusal = Refusal {
    code: "pending_limit",
    message: "Too many posts as this person are waiting for their approval.",
};

impl From<Refusal> for Fail {
    fn from(r: Refusal) -> Self {
        Fail::Denied(format!("{}: {}", r.code, r.message))
    }
}

/// The owner, the token and the thread a permitted call rides on.
pub struct Eligible {
    pub owner: Uuid,
    pub token: UserTokenRow,
}

/// Whether the token, the agent's grant and the owner's link stand right now.
pub async fn standing(
    conn: &mut PgConnection,
    token: &UserTokenRow,
    agent: Uuid,
) -> Result<bool, DbError> {
    if token.status != "active" || !repo::has_grant(&mut *conn, token.id, agent).await? {
        return Ok(false);
    }
    let link =
        user_links::user_link_for(&mut *conn, token.integration_id, &token.platform_user_ref)
            .await?;
    Ok(link.is_some_and(|l| l.user_id == token.user_id))
}

/// D-283(a) at the call: Ok(Err(refusal)) names why not.
pub async fn eligibility(
    conn: &mut PgConnection,
    agent: &AgentRow,
    w: &Where,
) -> Result<Result<Eligible, Refusal>, Fail> {
    // The turn that is running: its speaker, its door.
    let Some(turn) = (match w.turn_id {
        Some(t) => turns::get(&mut *conn, t).await?,
        None => None,
    }) else {
        return Ok(Err(NOT_ELIGIBLE));
    };
    if turn.conversation_id != w.conversation_id || turn.agent_id != agent.id {
        return Ok(Err(NOT_ELIGIBLE));
    }
    let owner = match (turn.speaker_kind.as_str(), turn.speaker_user_id) {
        ("person", Some(u)) => u,
        _ => return Ok(Err(NOT_ELIGIBLE)),
    };
    // Only a chat thread door (a dashboard, API, schedule, webhook, watch, peer or system door
    // never qualifies).
    if turn.door.get("kind").and_then(Value::as_str) != Some("thread") {
        return Ok(Err(NOT_ELIGIBLE));
    }
    let link = match turn
        .door
        .get("thread_link_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        Some(id) => thread_links::get(&mut *conn, id).await?,
        None => None,
    };
    let Some(link) = link.filter(|l| l.conversation_id == w.conversation_id) else {
        return Ok(Err(NOT_ELIGIBLE));
    };
    // The owner's own direct conversation, or a group thread the owner started.
    let Some(conv) = conversations::get(&mut *conn, w.conversation_id).await? else {
        return Ok(Err(NOT_ELIGIBLE));
    };
    let own_conversation = match conv.kind.as_str() {
        "direct" => conv.direct_user_id == Some(owner),
        "group" => {
            let first: Option<Option<Uuid>> = sqlx::query_scalar(
                "SELECT speaker_user_id FROM turns
                  WHERE conversation_id = $1 AND role = 'turn' AND speaker_kind = 'person'
                  ORDER BY created_at, id LIMIT 1",
            )
            .bind(w.conversation_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
            first.flatten() == Some(owner)
        }
        _ => false,
    };
    if !own_conversation {
        return Ok(Err(NOT_ELIGIBLE));
    }
    // The owner's grant of this agent, on the platform of the thread.
    let tokens = repo::granted_for(&mut *conn, owner, link.integration_id, agent.id).await?;
    if tokens.is_empty() {
        return Ok(Err(NOT_GRANTED));
    }
    for t in tokens {
        if standing(conn, &t, agent.id).await? {
            return Ok(Ok(Eligible { owner, token: t }));
        }
    }
    Ok(Err(TOKEN_INACTIVE))
}

/// `send_as_me {to, text}`: eligibility, recipient, then the held approval.
pub async fn send_as_me(
    s: &AppState,
    agent_id: Uuid,
    w: &Where,
    to: &str,
    text: &str,
) -> Result<Value, Fail> {
    let agent = agents::get(&s.pool, agent_id)
        .await?
        .ok_or(Fail::Internal)?;
    let mut conn = s.pool.acquire().await?;
    let eligible = eligibility(&mut conn, &agent, w).await??;
    drop(conn);
    // Before any transaction: an exact handle is looked up on the platform (correlated wait).
    let to = resolve_recipient(s, &agent, w, to, None, For::Send).await?;

    let mut tx = s.pool.begin().await?;
    let r = m::resolve_to(&mut tx, w, &to).await?;
    if r.kind != RKind::Space {
        return Err(Fail::Invalid(
            "send_as_me posts in a channel or thread, not to a person.".into(),
        ));
    }
    if r.integration_id != eligible.token.integration_id {
        return Err(Fail::Denied(
            "That space is on another platform than this person's token.".into(),
        ));
    }
    let platform = m::load_platform(&mut tx, r.integration_id, agent_id).await?;
    if !platform.ctx.caps.has(&Capability::UserTokens) {
        return Err(Fail::Denied(
            "This platform does not support posting as a person.".into(),
        ));
    }
    if let Some(msg) = m::bad_text(text, &platform) {
        return Err(Fail::Invalid(msg));
    }
    let own = m::own_thread(&mut tx, w, &r).await?;
    // The agent's reach is not widened by a person's token: the thread it is in, a bound space
    // or an approved grant.
    if own.is_none() && !m::reachable(&mut tx, &agent, &r).await? {
        return Err(Fail::Denied(format!(
            "You cannot reach {} on {}: nobody has allowed it.",
            sanitize::label(&r.label),
            sanitize::label(&platform.ctx.integ.tenant_label)
        )));
    }
    // Serialize concurrent calls on this token, then re-check what the call relied on.
    let token = repo::get_own_for_update(&mut tx, eligible.owner, eligible.token.id)
        .await?
        .ok_or(Fail::Denied(format!(
            "{}: {}",
            TOKEN_INACTIVE.code, TOKEN_INACTIVE.message
        )))?;
    if !standing(&mut tx, &token, agent_id).await? {
        return Err(TOKEN_INACTIVE.into());
    }
    if repo::count_pending_sends(&mut tx, token.id).await? >= USER_TOKEN_PENDING_MAX as i64 {
        return Err(PENDING_LIMIT.into());
    }
    let owner_label = match users::find_by_id(&mut *tx, eligible.owner).await? {
        Some(u) => sanitize::label(&u.display_name),
        None => return Err(Fail::Internal),
    };
    let recipient = recipient_json(&mut tx, w, &to, &r).await?;
    let (space_ref, thread_ref) = match &own {
        Some(l) => (l.space_ref.clone(), l.thread_ref.clone()),
        None => (r.reference.clone(), String::new()),
    };
    let label = sanitize::label(&r.label);
    // The summary names the conversation and the length, never the text (D-283(b)).
    let summary = format!(
        "Post as {owner_label} in {label} ({} characters)",
        text.chars().count()
    );
    let payload = json!({
        "token_id": token.id,
        "conversation_id": w.conversation_id,
        "text": text,
        // Internal to the handler (the owner check of the decision reads `owner_id`).
        "owner_id": eligible.owner,
        "agent_id": agent.id,
        "integration_id": token.integration_id,
        "space_ref": space_ref,
        "thread_ref": thread_ref,
        "label": label,
    });
    let row = approvals::create(
        &mut tx,
        NewAction {
            workspace_id: agent.workspace_id,
            agent_id: agent.id,
            conversation_id: Some(w.conversation_id),
            turn_id: w.turn_id,
            action_key: "send_as_me",
            payload: &payload,
            summary: &summary,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(json!({
        "status": "pending_approval",
        "approval_id": row.id,
        "recipient": recipient,
    }))
}

/// The `send_as_me` action handler: queues the approved post.
pub struct SendAsMe;

#[derive(Deserialize)]
struct Frozen {
    token_id: Uuid,
    conversation_id: Uuid,
    text: String,
    owner_id: Uuid,
    agent_id: Uuid,
    integration_id: Uuid,
    space_ref: String,
    thread_ref: String,
    label: String,
}

fn failed(msg: &str) -> ActionError {
    ActionError(msg.to_owned())
}

#[async_trait::async_trait]
impl ActionHandler for SendAsMe {
    fn key(&self) -> &'static str {
        "send_as_me"
    }

    async fn execute(
        &self,
        tx: &mut PgConnection,
        ctx: &ActionContext<'_>,
    ) -> Result<ActionOutcome, ActionError> {
        let p: Frozen = serde_json::from_value(ctx.approval.payload.clone()).map_err(|e| {
            tracing::error!(error = %e, "send_as_me payload unreadable");
            failed("Internal error.")
        })?;
        let db = |e: DbError| {
            tracing::error!(error = %e, "send_as_me execution failed");
            failed("Internal error.")
        };
        // The owner alone decides (defence in depth; the decision route refuses anyone else).
        if ctx.decided_by != p.owner_id {
            return Err(failed("Only the token's owner can approve this."));
        }
        let gone = || failed("The token is no longer usable, so nothing was posted.");
        let token = repo::get(&mut *tx, p.token_id)
            .await
            .map_err(db)?
            .filter(|t| t.user_id == p.owner_id)
            .ok_or_else(gone)?;
        if !standing(tx, &token, p.agent_id).await.map_err(db)? {
            return Err(gone());
        }
        let agent = agents::get(&mut *tx, p.agent_id)
            .await
            .map_err(db)?
            .ok_or_else(|| failed("The agent no longer exists."))?;
        let platform = m::load_platform(tx, p.integration_id, p.agent_id)
            .await
            .map_err(|_| failed("The platform is not available, so nothing was posted."))?;
        if !platform.ctx.caps.has(&Capability::UserTokens) {
            return Err(failed("The platform does not support posting as a person."));
        }
        let owner_label = match users::find_by_id(&mut *tx, p.owner_id).await.map_err(db)? {
            Some(u) => sanitize::label(&u.display_name),
            None => return Err(gone()),
        };
        let content = Content::agent_markdown(&p.text);
        let op_id = Uuid::now_v7();
        let copy = custody::copy_for_send(
            ctx.state,
            token.id,
            token.org_id,
            token.token_ref.as_deref().ok_or_else(gone)?,
            op_id,
        )
        .await
        .map_err(|e| match e {
            CopyError::Gone => gone(),
            CopyError::Unavailable => {
                failed("The secret store is unavailable; nothing was posted. Try again.")
            }
        })?;
        let mut payload = serde_json::to_value(MessagePostOp {
            target: Target::Space {
                space_ref: p.space_ref.clone(),
                thread_ref: (!p.thread_ref.is_empty()).then(|| p.thread_ref.clone()),
            },
            sender: Sender::UserToken {
                user_token: token.id,
            },
            content: serde_json::to_value(&content).map_err(|_| failed("Internal error."))?,
            files: vec![],
            ephemeral_to: None,
            op_marker: String::new(),
            user_footer: Some(UserFooter {
                agent_name: sanitize::label(&agent.name),
                owner_label,
            }),
        })
        .map_err(|_| failed("Internal error."))?;
        if let Some(o) = payload.as_object_mut() {
            // Internal keys: stripped at claim, never sent to the connector.
            o.insert("user_credential_ref".into(), json!(copy.to_column()));
            o.insert("user_org_id".into(), json!(token.org_id));
            o.insert(
                "user_meta".into(),
                json!({
                    "agent_id": p.agent_id,
                    "conversation_id": p.conversation_id,
                    "approval_id": ctx.approval.id,
                    "owner_id": p.owner_id,
                }),
            );
        }
        let queued = ops::enqueue_with_id(
            tx,
            op_id,
            NewOp {
                kind: "message.post".to_owned(),
                connector_id: None,
                integration_id: Some(p.integration_id),
                presence_id: None,
                payload,
                dedupe_key: Some(format!("send-as-me:{}", ctx.approval.id)),
            },
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e.message(), "send_as_me not queued");
            failed("Internal error.")
        })?;
        if queued != op_id {
            // A replay: the first execution queued the post and holds the copy of record.
            custody::discard_unused_copy(ctx.state, op_id, token.org_id, &copy).await;
        }
        Ok(ActionOutcome {
            line: format!(
                "Your post in {} was approved and is being sent.",
                sanitize::label(&p.label)
            ),
            notify_agent: true,
        })
    }
}

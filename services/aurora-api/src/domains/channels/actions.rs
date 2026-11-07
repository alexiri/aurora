//! `action.invoked` inbox handler and action router (docs/13 §4.4, §5.5, §6.1; BE-073).
//!
//! Order of checks for every click: token signature, expiry and integration; not already used;
//! the clicker's user link (unlinked -> link prompt, nothing else); the access law for the
//! subject's workspace **at click time** (`can_use` for approvals, action approvals, stop /
//! send-now, agent-link policies and DM defaults; `can_own` for reach and bindings); only then is
//! the token consumed (single use) and the action performed through the same service functions
//! the dashboard uses (same audit, same NOTIFY). Refused clicks do not burn the token. Transient
//! failures release the token and retry the inbox row. The platform never authorizes.
//!
//! Refusals and prompts are sent to the clicker as operations (ephemeral or DM), then the
//! card(s) of the subject are settled.

use super::cards::{self, fanout};
use super::dto::{ReachDecision, ReachDecisionRequest};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::action_approvals::dto::ActionDecision;
use crate::domains::agent_links::dto::{LinkPolicy, SetPolicyBody};
use crate::domains::approvals::dto::Decision as GateDecision;
use crate::domains::connectors::inbox::{InboxError, InboxHandler};
use crate::http::auth::{Caller, Principal};
use aurora_channels::{ActionKind, ActionToken, Content, Decision, TokenError};
use aurora_core::ApiError;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::DbError;
use aurora_db::repo::channel_cards as card_repo;
use aurora_db::repo::channel_inbox::InboxRow;
use aurora_db::repo::channel_integrations::IntegrationRow;
use aurora_db::repo::{
    action_approvals, agent_links, agents, approvals, channel_integrations, channel_presences,
    dm_defaults, link_requests, reach_grants, space_bindings, turns, user_links, users,
};
use aurora_protocol::connector::ActionInvoked;
use chrono::{DateTime, Utc};
use http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

const LINK_TTL_SECS: f64 = 30.0 * 60.0;

pub struct ActionHandler;

enum Stop {
    /// Tell the clicker why nothing happened; the row is done.
    Refuse(&'static str, String),
    /// The clicker has no user link: send a sign-in link.
    LinkPrompt,
    Inbox(InboxError),
}

impl From<DbError> for Stop {
    fn from(e: DbError) -> Self {
        Stop::Inbox(InboxError::Retry(e.to_string()))
    }
}

impl From<ApiError> for Stop {
    fn from(e: ApiError) -> Self {
        match e.status() {
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => {
                Stop::Refuse("forbidden", "You do not have access to do that.".to_owned())
            }
            StatusCode::CONFLICT => Stop::Refuse("conflict", e.message().to_owned()),
            s if s.is_client_error() => Stop::Refuse("invalid", e.message().to_owned()),
            _ => Stop::Inbox(InboxError::Retry(e.message().to_owned())),
        }
    }
}

fn refuse<T>(code: &'static str, text: &str) -> Result<T, Stop> {
    Err(Stop::Refuse(code, text.to_owned()))
}

struct Click<'a> {
    state: &'a AppState,
    row: &'a InboxRow,
    ev: ActionInvoked,
    integ: IntegrationRow,
}

#[async_trait::async_trait]
impl InboxHandler for ActionHandler {
    fn handles(&self, kind: &str) -> bool {
        kind == "action.invoked"
    }

    async fn handle(&self, state: &AppState, row: &InboxRow) -> Result<Value, InboxError> {
        let ev: ActionInvoked = serde_json::from_value(row.payload.clone())
            .map_err(|e| InboxError::Fatal(format!("bad action.invoked payload: {e}")))?;
        let Some(integ_id) = row.integration_id else {
            return Ok(json!({"result": "refused", "reason": "no_integration"}));
        };
        let Some(integ) = channel_integrations::get(&state.pool, integ_id)
            .await
            .map_err(|e| InboxError::Retry(e.to_string()))?
        else {
            return Ok(json!({"result": "refused", "reason": "no_integration"}));
        };
        let click = Click {
            state,
            row,
            ev,
            integ,
        };
        match click.run().await {
            Ok(outcome) => Ok(outcome),
            Err(Stop::Inbox(e)) => Err(e),
            Err(Stop::Refuse(code, text)) => {
                click
                    .tell(Content::platform(&text))
                    .await
                    .map_err(|e| InboxError::Retry(e.message().to_owned()))?;
                Ok(json!({"result": "refused", "reason": code}))
            }
            Err(Stop::LinkPrompt) => {
                click
                    .link_prompt()
                    .await
                    .map_err(|e| InboxError::Retry(e.message().to_owned()))?;
                Ok(json!({"result": "link_prompt"}))
            }
        }
    }
}

impl Click<'_> {
    /// A private notice to the clicker.
    async fn tell(&self, content: Content) -> Result<(), ApiError> {
        let mut tx = self.state.pool.begin().await.map_err(DbError::from)?;
        let caps = cards::capabilities(&mut tx, self.integ.id).await?;
        cards::notify_person(
            &mut tx,
            &caps,
            self.integ.id,
            &self.ev.clicker_ref,
            self.ev.space_ref.as_deref(),
            content,
            &format!("action-notice:{}", self.row.id),
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    async fn link_prompt(&self) -> Result<(), ApiError> {
        let tok = token::generate(TokenPrefix::Invitation);
        link_requests::insert(
            &self.state.pool,
            self.integ.id,
            &self.ev.clicker_ref,
            &token::hash(tok.expose()),
            LINK_TTL_SECS,
            None,
        )
        .await?;
        let url = format!(
            "{}/link/channel?token={}",
            self.state.config.external_url.trim_end_matches('/'),
            tok.expose()
        );
        self.tell(Content::platform(&format!(
            "Link your Aurora account to use this button: {url}"
        )))
        .await
    }

    async fn run(&self) -> Result<Value, Stop> {
        let s = self.state;
        let now = Utc::now();
        let claims =
            match ActionToken::verify(&s.derived, &self.ev.token, self.integ.id, now.timestamp()) {
                Ok(c) => c,
                Err(TokenError::Expired) => return refuse("expired", "This action has expired."),
                Err(TokenError::WrongIntegration) => {
                    return refuse("wrong_integration", "This action does not belong here.");
                }
                Err(_) => return refuse("invalid_token", "This action is not valid."),
            };
        if card_repo::token_used(&s.pool, claims.jti).await? {
            return refuse("already_used", "This action was already used.");
        }
        let Some(link) =
            user_links::user_link_for(&s.pool, self.integ.id, &self.ev.clicker_ref).await?
        else {
            return Err(Stop::LinkPrompt);
        };
        let Some(user) = users::find_by_id(&s.pool, link.user_id).await? else {
            return refuse("forbidden", "You do not have access to do that.");
        };
        if user.disabled_at.is_some() {
            return refuse("forbidden", "You do not have access to do that.");
        }
        let caller = Caller {
            user_id: user.id,
            email: user.email.clone(),
            is_instance_admin: false,
            principal: Principal::Channel {
                integration_id: self.integ.id,
            },
            ip: None,
        };
        let exp = DateTime::from_timestamp(claims.exp, 0).unwrap_or(now);
        let done = self.act(&caller, &claims, exp, &user.display_name).await;
        match done {
            Ok(settle) => {
                self.after(&settle, &claims).await;
                Ok(json!({"result": "done", "kind": settle.kind}))
            }
            Err(Stop::Inbox(e)) => {
                // Not performed: let the retry use the token.
                let _ = card_repo::release_token(&s.pool, claims.jti).await;
                Err(Stop::Inbox(e))
            }
            Err(Stop::Refuse(code, text)) if code == "conflict" => {
                // Already resolved elsewhere: the token is spent, the card may need settling.
                self.after(&Done::none("conflict"), &claims).await;
                Err(Stop::Refuse(code, text))
            }
            Err(e) => Err(e),
        }
    }

    /// Record the use of the token. Called after the access law passed.
    async fn consume(
        &self,
        claims: &aurora_channels::Claims,
        exp: DateTime<Utc>,
    ) -> Result<(), Stop> {
        if card_repo::consume_token(&self.state.pool, claims.jti, exp).await? {
            Ok(())
        } else {
            refuse("already_used", "This action was already used.")
        }
    }

    /// `can_use` / `can_own` of the clicker on a workspace, which must belong to the
    /// integration's organization.
    async fn authorize(&self, c: &Caller, ws: Uuid, own: bool) -> Result<(), Stop> {
        let auth = c.workspace(self.state, ws).await?;
        if auth.org_id != self.integ.org_id || !auth.can_use() || (own && !auth.can_own()) {
            return refuse("forbidden", "You do not have access to do that.");
        }
        Ok(())
    }

    async fn act(
        &self,
        c: &Caller,
        claims: &aurora_channels::Claims,
        exp: DateTime<Utc>,
        user_name: &str,
    ) -> Result<Done, Stop> {
        let s = self.state;
        let gone = || Stop::Refuse("not_found", "That request no longer exists.".to_owned());
        match &claims.action {
            ActionKind::ApprovalDecide { decision } => {
                let a = approvals::get(&s.pool, claims.subject_id)
                    .await?
                    .ok_or_else(gone)?;
                self.authorize(c, a.workspace_id, false).await?;
                self.consume(claims, exp).await?;
                let d = gate(*decision);
                crate::domains::approvals::service::decide(s, c, a.id, d).await?;
                Ok(Done::new("approval.decide"))
            }
            ActionKind::ApprovalBatchDecide {
                batch_id,
                agent_id,
                decision,
            } => {
                let a = approvals::get(&s.pool, claims.subject_id)
                    .await?
                    .ok_or_else(gone)?;
                let Some(batch) = a.batch_id.clone() else {
                    return refuse("invalid_token", "This action is not valid.");
                };
                if cards::build::batch_uuid(&batch) != *batch_id || a.agent_id != *agent_id {
                    return refuse("invalid_token", "This action is not valid.");
                }
                self.authorize(c, a.workspace_id, false).await?;
                self.consume(claims, exp).await?;
                crate::domains::approvals::service::decide_batch(
                    s,
                    c,
                    &batch,
                    *agent_id,
                    gate(*decision),
                )
                .await?;
                Ok(Done::new("approval.batch_decide"))
            }
            ActionKind::ActionApprovalDecide { decision } => {
                let a = action_approvals::get(&s.pool, claims.subject_id)
                    .await?
                    .ok_or_else(gone)?;
                self.authorize(c, a.workspace_id, false).await?;
                self.consume(claims, exp).await?;
                let d = match decision {
                    Decision::Approve => ActionDecision::Approve,
                    Decision::Deny => ActionDecision::Deny,
                };
                crate::domains::action_approvals::service::decide(s, c, a.id, d).await?;
                Ok(Done::new("action_approval.decide"))
            }
            ActionKind::ReachDecide { decision } => {
                let g = reach_grants::get(&s.pool, claims.subject_id)
                    .await?
                    .ok_or_else(gone)?;
                if g.integration_id != self.integ.id {
                    return refuse("invalid_token", "This action is not valid.");
                }
                let agent = agents::get(&s.pool, g.agent_id).await?.ok_or_else(gone)?;
                self.authorize(c, agent.workspace_id, true).await?;
                if g.state != "pending" {
                    // Resolved already (e.g. from the dashboard): nothing to do but settle.
                    self.consume(claims, exp).await?;
                    return Ok(Done::new("reach.decide"));
                }
                self.consume(claims, exp).await?;
                super::reach::decide(
                    s,
                    c,
                    g.id,
                    ReachDecisionRequest {
                        decision: match decision {
                            Decision::Approve => ReachDecision::Approve,
                            Decision::Deny => ReachDecision::Block,
                        },
                        listen: None,
                        revision: g.revision,
                    },
                )
                .await?;
                Ok(Done::new("reach.decide"))
            }
            ActionKind::BindingChoose {
                space_ref,
                allowed_agent_ids,
            } => {
                let agent = self.chosen_agent(allowed_agent_ids).await?;
                let card = card_repo::get(&s.pool, claims.subject_id)
                    .await?
                    .ok_or_else(gone)?;
                if card.integration_id != self.integ.id {
                    return refuse("invalid_token", "This action is not valid.");
                }
                self.authorize(c, agent.workspace_id, true).await?;
                if channel_presences::active_shared(&s.pool, agent.id, self.integ.id)
                    .await?
                    .is_none()
                {
                    return refuse(
                        "unavailable",
                        "That agent is not available in this workspace chat.",
                    );
                }
                self.consume(claims, exp).await?;
                let space = space_ref.as_str();
                let res = space_bindings::insert(
                    &s.pool,
                    &space_bindings::NewBinding {
                        id: Uuid::now_v7(),
                        integration_id: self.integ.id,
                        space_ref: space,
                        space_label: space,
                        agent_id: agent.id,
                        listen: "mentions",
                        bound_by_user_id: Some(c.user_id),
                    },
                )
                .await;
                match res {
                    Ok(()) => {}
                    Err(DbError::UniqueViolation { .. }) => {
                        return refuse("conflict", "This space is already bound to an agent.");
                    }
                    Err(e) => return Err(e.into()),
                }
                audit::record(
                    s,
                    AuditEvent::new(
                        AuditActor::from_caller(c),
                        "space_binding.create",
                        Outcome::Success,
                    )
                    .org(agent.org_id)
                    .workspace(agent.workspace_id)
                    .target("space_binding", agent.id)
                    .metadata(json!({"integration_id": self.integ.id, "space_ref": space,
                        "agent_id": agent.id, "via": "channel"})),
                );
                Ok(Done::card(
                    "binding.choose",
                    card.id,
                    format!(
                        "Bound to {} by {}.",
                        aurora_core::sanitize::markdown_as_data(&agent.name, 80),
                        aurora_core::sanitize::markdown_as_data(user_name, 80)
                    ),
                ))
            }
            ActionKind::DmDefaultChoose { allowed_agent_ids } => {
                let agent = self.chosen_agent(allowed_agent_ids).await?;
                let card = card_repo::get(&s.pool, claims.subject_id)
                    .await?
                    .ok_or_else(gone)?;
                // The menu was DMed to one person; only they may set their own default.
                let mine = format!("{}{}", cards::DM_PREFIX, self.ev.clicker_ref);
                let learned_elsewhere = !card.space_ref.starts_with(cards::DM_PREFIX);
                if card.integration_id != self.integ.id
                    || (!learned_elsewhere && card.space_ref != mine)
                {
                    return refuse("forbidden", "This menu was not sent to you.");
                }
                self.authorize(c, agent.workspace_id, false).await?;
                self.consume(claims, exp).await?;
                dm_defaults::upsert(&s.pool, self.integ.id, &self.ev.clicker_ref, agent.id).await?;
                audit::record(
                    s,
                    AuditEvent::new(
                        AuditActor::from_caller(c),
                        "dm_default.set",
                        Outcome::Success,
                    )
                    .org(agent.org_id)
                    .workspace(agent.workspace_id)
                    .target("agent", agent.id)
                    .metadata(json!({"integration_id": self.integ.id, "via": "channel"})),
                );
                Ok(Done::card(
                    "dm_default.choose",
                    card.id,
                    format!(
                        "{} will answer your direct messages.",
                        aurora_core::sanitize::markdown_as_data(&agent.name, 80)
                    ),
                ))
            }
            ActionKind::TurnStop { turn_id } | ActionKind::FollowupSendNow { turn_id } => {
                let stop = matches!(claims.action, ActionKind::TurnStop { .. });
                let t = turns::get(&s.pool, *turn_id).await?.ok_or_else(gone)?;
                // The conversation's workspace and (for directs) privacy decide.
                let conv = crate::domains::conversations::service::load_conversation(
                    s,
                    c,
                    t.conversation_id,
                )
                .await?;
                self.authorize(c, conv.workspace_id, false).await?;
                self.consume(claims, exp).await?;
                if stop {
                    crate::domains::conversations::service::abort(s, c, t.id).await?;
                    Ok(Done::new("turn.stop"))
                } else {
                    crate::domains::conversations::service::send_now(s, c, t.id).await?;
                    Ok(Done::new("followup.send_now"))
                }
            }
            ActionKind::AgentLinkDecide { side, decision } => {
                let link = agent_links::get(&s.pool, claims.subject_id)
                    .await?
                    .ok_or_else(gone)?;
                let agent_id = match side.as_str() {
                    "a" => link.agent_a_id,
                    "b" => link.agent_b_id,
                    _ => return refuse("invalid_token", "This action is not valid."),
                };
                let agent = agents::get(&s.pool, agent_id).await?.ok_or_else(gone)?;
                self.authorize(c, agent.workspace_id, false).await?;
                if link.policy_of(agent_id) != Some("ask") {
                    return refuse("conflict", "This request was already answered.");
                }
                self.consume(claims, exp).await?;
                crate::domains::agent_links::service::set_policy(
                    s,
                    c,
                    link.id,
                    SetPolicyBody {
                        agent_id,
                        policy: match decision {
                            Decision::Approve => LinkPolicy::Allow,
                            Decision::Deny => LinkPolicy::Blocked,
                        },
                        revision: link.revision,
                    },
                )
                .await?;
                Ok(Done::new("agent_link.decide"))
            }
        }
    }

    /// The agent named by the select `value`, which must be one the card offered.
    async fn chosen_agent(
        &self,
        allowed: &[Uuid],
    ) -> Result<aurora_db::repo::agents::AgentRow, Stop> {
        let Some(id) = self
            .ev
            .value
            .as_deref()
            .and_then(|v| v.parse::<Uuid>().ok())
            .filter(|id| allowed.contains(id))
        else {
            return refuse("invalid_choice", "That choice is not available.");
        };
        match agents::get(&self.state.pool, id).await? {
            Some(a) => Ok(a),
            None => refuse("invalid_choice", "That choice is not available."),
        }
    }

    /// Learn the DM space of the clicked card, then settle: the clicked card by `text`, and any
    /// card whose subject is now decided (approvals, action approvals, reach).
    async fn after(&self, d: &Done, _claims: &aurora_channels::Claims) {
        let s = self.state;
        if let (Some(space), Some(m)) = (&self.ev.space_ref, &self.ev.message_ref) {
            let _ = card_repo::learn_space(&s.pool, self.integ.id, m, space).await;
        }
        if let Err(e) = fanout::sync_posting(s).await {
            tracing::warn!(error = %e.message(), "card sync after click failed");
        }
        if let (Some(id), Some(text)) = (d.card, &d.text)
            && let Ok(Some(card)) = card_repo::get(&s.pool, id).await
            && let Ok(mut tx) = s.pool.begin().await
            && let Ok(caps) = cards::capabilities(&mut tx, card.integration_id).await
            && cards::settle_card(&mut tx, &card, &caps, text)
                .await
                .is_ok()
        {
            let _ = tx.commit().await;
        }
        if let Err(e) = fanout::settle_resolved(s).await {
            tracing::warn!(error = %e.message(), "card settle after click failed");
        }
    }
}

fn gate(d: Decision) -> GateDecision {
    match d {
        Decision::Approve => GateDecision::Approve,
        Decision::Deny => GateDecision::Deny,
    }
}

struct Done {
    kind: &'static str,
    card: Option<Uuid>,
    text: Option<String>,
}

impl Done {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            card: None,
            text: None,
        }
    }
    fn none(kind: &'static str) -> Self {
        Self::new(kind)
    }
    fn card(kind: &'static str, card: Uuid, text: String) -> Self {
        Self {
            kind,
            card: Some(card),
            text: Some(text),
        }
    }
}

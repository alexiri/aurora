//! The `message_agent` action handler: approving a link side flips it to `allow`; once no side
//! is `ask` the frozen message is delivered as a peer task.

use crate::domains::action_approvals::handlers::{
    ActionContext, ActionError, ActionHandler, ActionOutcome,
};
use crate::domains::peer_tasks::service::{self as peers, OpenRequest, SendOutcome};
use aurora_db::repo::agent_links as links;
use aurora_db::repo::{agents, conversations as convs};
use serde::Deserialize;
use sqlx::PgConnection;
use uuid::Uuid;

pub struct MessageAgent;

#[derive(Deserialize)]
struct Payload {
    link_id: Uuid,
    side_agent_id: Uuid,
    from_agent_id: Uuid,
    to_agent_id: Uuid,
    requester_user_id: Uuid,
    origin_conversation_id: Uuid,
    text: String,
}

fn internal<E: std::fmt::Display>(e: E) -> ActionError {
    tracing::error!(error = %e, "message_agent action failed");
    ActionError("Internal error.".into())
}

#[async_trait::async_trait]
impl ActionHandler for MessageAgent {
    fn key(&self) -> &'static str {
        "message_agent"
    }

    async fn execute(
        &self,
        tx: &mut PgConnection,
        ctx: &ActionContext<'_>,
    ) -> Result<ActionOutcome, ActionError> {
        let p: Payload = serde_json::from_value(ctx.approval.payload.clone()).map_err(internal)?;
        let link = links::get_for_update(&mut *tx, p.link_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| ActionError("The link no longer exists.".into()))?;
        let blocked = || ActionError("Messaging between these agents is blocked.".into());
        match link.policy_of(p.side_agent_id) {
            None => {
                return Err(ActionError(
                    "The link no longer includes this agent.".into(),
                ));
            }
            Some("blocked") => return Err(blocked()),
            Some("ask") => {
                links::set_policy(&mut *tx, link.id, p.side_agent_id, "allow", None)
                    .await
                    .map_err(internal)?;
            }
            Some(_) => {}
        }
        let link = links::get(&mut *tx, link.id)
            .await
            .map_err(internal)?
            .ok_or_else(|| ActionError("The link no longer exists.".into()))?;
        if link.policy_a == "blocked" || link.policy_b == "blocked" {
            return Err(blocked());
        }
        let to = agents::get(&mut *tx, p.to_agent_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| ActionError("The receiving agent no longer exists.".into()))?;
        let who = peers::agent_label(&to);
        if link.policy_a == "ask" || link.policy_b == "ask" {
            return Ok(ActionOutcome {
                line: format!(
                    "Your message to {who} was approved on one side; it is still waiting for \
                     approval from the other side."
                ),
                notify_agent: false,
            });
        }
        let from = agents::get(&mut *tx, p.from_agent_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| ActionError("The sending agent no longer exists.".into()))?;
        let origin = convs::get(&mut *tx, p.origin_conversation_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| ActionError("The original conversation no longer exists.".into()))?;
        let out = peers::open_task(
            tx,
            &OpenRequest {
                link: &link,
                from: &from,
                to: &to,
                requester: p.requester_user_id,
                origin: &origin,
                text: &p.text,
            },
        )
        .await
        .map_err(|e| internal(e.message()))?;
        match out {
            SendOutcome::Opened(_) => Ok(ActionOutcome {
                line: format!("Your message to {who} was approved and sent."),
                notify_agent: true,
            }),
            SendOutcome::Queued(_) => Ok(ActionOutcome {
                line: format!(
                    "Your message to {who} was approved; it is queued behind an earlier task."
                ),
                notify_agent: true,
            }),
            SendOutcome::Refused(msg) => Err(ActionError(msg)),
            SendOutcome::Delivered(_) | SendOutcome::PendingApproval(_) => {
                Err(internal("unexpected outcome"))
            }
        }
    }
}

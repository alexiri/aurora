//! Due-work arms 4-8 (docs/05 §The due-work engine) plus the pre-start `no_model_key` check
//! (docs/10 §Failure semantics). All run inside the poll's transaction; every transition is
//! fenced, and turn changes take the conversation lock first.

use super::{home, lifecycle, model};
use crate::app::AppState;
use crate::domains::conversations::{dto::door_name, service as conv};
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, limits};
use aurora_db::repo::conversations as convs;
use aurora_db::repo::hosted::{self, Stale};
use aurora_db::repo::{agents, credentials, turns};
use aurora_protocol::runner::{
    AttachmentRef, Recall, SessionInfo, TurnDeliver, TurnInput, TurnMessage, TurnNotice, TurnRef,
    WorkItem, WorkItemBody,
};
use serde_json::json;
use sqlx::PgConnection;
use std::time::Duration;
use uuid::Uuid;

const ARM_LIMIT: i64 = 16;
/// Lease on a home sync / abort delivery before it is re-sent.
const SYNC_LEASE_SECS: f64 = 60.0;
const ABORT_LEASE_SECS: f64 = 30.0;
const PURGE_LEASE_SECS: f64 = 30.0;

/// Side effects the caller performs after the transaction commits.
#[derive(Default)]
pub struct After {
    /// `(conversation, turn)` whose buffered deltas must be flushed.
    pub flush: Vec<(Uuid, Uuid)>,
}

fn item(sandbox_id: Uuid, boot_id: Uuid, body: WorkItemBody) -> WorkItem {
    WorkItem {
        item_id: Uuid::now_v7(),
        sandbox_id,
        boot_id,
        body,
    }
}

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

/// `no_model_key` before any sandbox start: queued hosted turns of agents without a usable key.
pub async fn sweep_no_model_key(
    s: &AppState,
    tx: &mut PgConnection,
    after: &mut After,
) -> Result<(), ApiError> {
    for (turn, conversation, agent) in hosted::queued_hosted_turns(tx, ARM_LIMIT).await? {
        let Some(a) = agents::get(&mut *tx, agent).await? else {
            continue;
        };
        let harness = a
            .harness
            .clone()
            .unwrap_or_else(|| s.config.hosted.default_harness.clone());
        if matches!(
            model::select(s, tx, agent, a.org_id, a.workspace_id, &harness).await?,
            model::Selection::NoKey
        ) {
            convs::lock(tx, conversation).await?;
            if lifecycle::fail(tx, turn, conversation, "no_model_key", NO_KEY_MSG)
                .await?
                .is_some()
            {
                after.flush.push((conversation, turn));
            }
        }
    }
    Ok(())
}

const NO_KEY_MSG: &str =
    "No compatible LLM key is granted to this agent. Store a key and grant it to the agent.";

/// Arms 4-8 for one poll.
pub async fn arms(
    s: &AppState,
    tx: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    items: &mut Vec<WorkItem>,
    after: &mut After,
) -> Result<(), ApiError> {
    arm4_signals(tx, runner, replica, items).await?;
    arm4b_purges(tx, runner, replica, items).await?;
    arm5_home_sync(s, tx, runner, replica, items).await?;
    arm6_dispatch(s, tx, runner, replica, items, after).await?;
    arm7_steer(tx, runner, replica, items).await?;
    arm8_stale(s, tx, runner, replica, items, after).await?;
    Ok(())
}

async fn arm4_signals(
    tx: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    items: &mut Vec<WorkItem>,
) -> Result<(), ApiError> {
    for t in hosted::claim_aborts(tx, runner, replica, ABORT_LEASE_SECS, ARM_LIMIT).await? {
        items.push(item(
            t.sandbox_id,
            t.boot_id,
            WorkItemBody::TurnAbort(TurnRef { turn_id: t.turn_id }),
        ));
    }
    for t in hosted::claim_send_nows(tx, runner, replica, ARM_LIMIT).await? {
        items.push(item(
            t.sandbox_id,
            t.boot_id,
            WorkItemBody::TurnSendNow(TurnRef { turn_id: t.turn_id }),
        ));
    }
    Ok(())
}

/// (RFE-010) Live `session.purge` for deleted conversations; re-sent each lease until the
/// supervisor reports `session.purged`. Sandboxes that are not running get it with the home sync
/// at their next boot.
async fn arm4b_purges(
    tx: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    items: &mut Vec<WorkItem>,
) -> Result<(), ApiError> {
    for p in
        aurora_db::repo::lifecycle::claim_live(tx, runner, replica, PURGE_LEASE_SECS, ARM_LIMIT)
            .await?
    {
        let Some(conversation_id) = p.conversation_id else {
            continue;
        };
        items.push(item(
            p.sandbox_id,
            p.boot_id,
            WorkItemBody::SessionPurge(aurora_protocol::runner::SessionPurge {
                conversation_id,
                session_ref: p.session_ref,
            }),
        ));
    }
    Ok(())
}

async fn arm5_home_sync(
    s: &AppState,
    tx: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    items: &mut Vec<WorkItem>,
) -> Result<(), ApiError> {
    for c in hosted::claim_home_syncs(tx, runner, replica, SYNC_LEASE_SECS, 4).await? {
        for part in home::render(s, tx, &c).await? {
            items.push(item(c.sandbox_id, c.boot_id, WorkItemBody::HomeSync(part)));
        }
    }
    Ok(())
}

async fn arm6_dispatch(
    s: &AppState,
    tx: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    items: &mut Vec<WorkItem>,
    after: &mut After,
) -> Result<(), ApiError> {
    let ttl = s.config.hosted.turn_ceiling_secs as f64 + secs(limits::CONTEXT_CREDENTIAL_GRACE);
    for cand in hosted::dispatch_candidates(tx, runner, replica, ARM_LIMIT).await? {
        convs::lock(tx, cand.conversation_id).await?;
        let Some(turn) = turns::get(&mut *tx, cand.turn_id)
            .await?
            .filter(|t| t.status == "queued" && t.role == "turn")
        else {
            continue;
        };
        let Some(agent) = agents::get(&mut *tx, cand.agent_id).await? else {
            continue;
        };
        let harness = agent
            .harness
            .clone()
            .unwrap_or_else(|| s.config.hosted.default_harness.clone());
        let (choice, mut notices) =
            match model::select(s, tx, agent.id, agent.org_id, agent.workspace_id, &harness).await?
            {
                model::Selection::Chosen { choice, notices } => (choice, notices),
                model::Selection::NoKey => {
                    if lifecycle::fail(
                        tx,
                        turn.id,
                        turn.conversation_id,
                        "no_model_key",
                        NO_KEY_MSG,
                    )
                    .await?
                    .is_some()
                    {
                        after.flush.push((turn.conversation_id, turn.id));
                    }
                    continue;
                }
            };
        if !hosted::mark_dispatched(tx, turn.id, cand.boot_id).await? {
            continue;
        }
        let tok = token::generate(TokenPrefix::Context);
        let revoked = hosted::mint_turn_credential(
            tx,
            agent.id,
            turn.conversation_id,
            turn.id,
            &token::hash(tok.expose()),
            &token::display_prefix(tok.expose()),
            ttl,
        )
        .await?;
        for id in revoked {
            credentials::notify_gateway_invalidate(&mut *tx, "credential", id).await?;
        }
        for n in &notices {
            conv::append_event(
                tx,
                turn.conversation_id,
                Some(turn.id),
                "notice",
                &json!({"code": "model_override_dropped", "text": n}),
            )
            .await?;
        }
        let (resume_ref, epoch) = hosted::session_of(tx, turn.conversation_id).await?;
        let mems = hosted::memories(tx, agent.id).await?;
        let snippets = crate::domains::memory::recall_snippets(tx, agent.id, &turn.text).await?;
        let attachments = hosted::turn_attachments(tx, turn.id)
            .await?
            .into_iter()
            .map(|a| AttachmentRef {
                attachment_id: a.id,
                filename: a.filename,
                size: u64::try_from(a.size_bytes).unwrap_or(0),
            })
            .collect();
        items.push(item(
            cand.sandbox_id,
            cand.boot_id,
            WorkItemBody::TurnDeliver(Box::new(TurnDeliver {
                conversation_id: turn.conversation_id,
                turn_id: turn.id,
                session: SessionInfo {
                    resume_ref,
                    epoch: u64::try_from(epoch).unwrap_or(0),
                },
                model: choice,
                context_credential: aurora_protocol::common::SecretString::new(
                    tok.expose().as_str(),
                ),
                input: TurnInput {
                    text: turn.text.clone(),
                    speaker_label: turn.speaker_label.clone(),
                    door: door_name(&turn.door),
                    recall: Recall {
                        index: home::memory_index(&mems),
                        snippets,
                    },
                    notices: std::mem::take(&mut notices),
                },
                attachments,
                gate: crate::domains::schedules::fire::gate_for_turn(tx, turn.id).await?,
            })),
        ));
    }
    Ok(())
}

async fn arm7_steer(
    tx: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    items: &mut Vec<WorkItem>,
) -> Result<(), ApiError> {
    for c in hosted::steer_candidates(tx, runner, replica, ARM_LIMIT).await? {
        convs::lock(tx, c.conversation_id).await?;
        if !turns::mark_steer_delivered(tx, c.followup_id).await? {
            continue;
        }
        items.push(item(
            c.sandbox_id,
            c.boot_id,
            WorkItemBody::TurnMessage(TurnMessage {
                turn_id: c.turn_id,
                followup_id: c.followup_id,
                text: c.text,
                speaker_label: c.speaker_label,
            }),
        ));
    }
    Ok(())
}

async fn arm8_stale(
    s: &AppState,
    tx: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    items: &mut Vec<WorkItem>,
    after: &mut After,
) -> Result<(), ApiError> {
    let h = &s.config.hosted;
    let abort = |items: &mut Vec<WorkItem>, t: &hosted::StaleTurn| {
        if let (true, Some(sb), Some(b)) = (t.channel_here, t.sandbox_id, t.boot_id) {
            items.push(item(
                sb,
                b,
                WorkItemBody::TurnAbort(TurnRef { turn_id: t.turn_id }),
            ));
        }
    };

    // Never started: revive once, then fail.
    for t in hosted::stale_turns(
        tx,
        runner,
        replica,
        Stale::NeverStarted,
        secs(limits::STALE_DISPATCH),
        ARM_LIMIT,
    )
    .await?
    {
        convs::lock(tx, t.conversation_id).await?;
        if hosted::revive(tx, t.turn_id).await? {
            conv::notify_work(tx).await?;
            continue;
        }
        let (code, msg) = if t.sandbox_dead {
            (
                "agent_restarted",
                "The agent's computer restarted before the turn began.",
            )
        } else {
            (
                "never_started",
                "The agent did not start working on this message.",
            )
        };
        if lifecycle::fail(tx, t.turn_id, t.conversation_id, code, msg)
            .await?
            .is_some()
        {
            after.flush.push((t.conversation_id, t.turn_id));
            abort(items, &t);
        }
    }
    // Stalled.
    for t in hosted::stale_turns(
        tx,
        runner,
        replica,
        Stale::Stalled,
        h.turn_stall_secs as f64,
        ARM_LIMIT,
    )
    .await?
    {
        convs::lock(tx, t.conversation_id).await?;
        if lifecycle::fail(
            tx,
            t.turn_id,
            t.conversation_id,
            "turn_stalled",
            "The turn made no progress and was stopped.",
        )
        .await?
        .is_some()
        {
            after.flush.push((t.conversation_id, t.turn_id));
            abort(items, &t);
        }
    }
    // Ceiling: abort + time_limit.
    for t in hosted::stale_turns(
        tx,
        runner,
        replica,
        Stale::Ceiling,
        h.turn_ceiling_secs as f64,
        ARM_LIMIT,
    )
    .await?
    {
        convs::lock(tx, t.conversation_id).await?;
        if lifecycle::fail(
            tx,
            t.turn_id,
            t.conversation_id,
            "time_limit",
            "The turn reached its time limit.",
        )
        .await?
        .is_some()
        {
            after.flush.push((t.conversation_id, t.turn_id));
            abort(items, &t);
        }
    }
    // Ceiling warning, once.
    let warn_at = h
        .turn_ceiling_secs
        .saturating_sub(limits::TURN_CEILING_WARNING.as_secs());
    for t in hosted::stale_turns(
        tx,
        runner,
        replica,
        Stale::CeilingWarn,
        warn_at as f64,
        ARM_LIMIT,
    )
    .await?
    {
        convs::lock(tx, t.conversation_id).await?;
        if !hosted::mark_ceiling_warned(tx, t.turn_id).await? {
            continue;
        }
        let text = "This turn is approaching its time limit. Wrap up and report what you have.";
        conv::append_event(
            tx,
            t.conversation_id,
            Some(t.turn_id),
            "notice",
            &json!({"code": "turn_ceiling_warning", "text": text}),
        )
        .await?;
        if let (true, Some(sb), Some(b)) = (t.channel_here, t.sandbox_id, t.boot_id) {
            items.push(item(
                sb,
                b,
                WorkItemBody::TurnNotice(TurnNotice {
                    turn_id: t.turn_id,
                    text: text.to_owned(),
                }),
            ));
        }
    }
    Ok(())
}

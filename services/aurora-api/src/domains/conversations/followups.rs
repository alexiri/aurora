//! Waiting follow-ups: list, Steer, Edit, Cancel (FIX-225, D-469, D-470, C-1152..C-1156, SEC-152).
//!
//! A follow-up (`turns.role = 'followup'`) is a message sent while a turn runs. It **waits** until
//! a person steers it into the running turn, edits it or cancels it; untouched it runs as the next
//! turn. Every action takes the conversation lock, then the follow-up row `FOR UPDATE`, and acts only
//! while the row is *waiting* (`joining`, not yet handed to the harness, or handed and failed).
//! Due-work arm 7's claim and the promotion at turn end re-check that state under the same
//! conversation lock, so a follow-up is never both changed and delivered, and what the agent
//! receives is what is stored.
//!
//! Authority (SEC-152): the conversation must be visible to the caller first (404 otherwise, as
//! for abort); editing is the follow-up's own speaker only (403 `followup_forbidden`); steering and
//! cancelling need Stop authority, which is exactly visibility of the conversation (`can_use`, plus
//! `can_see_conversation` for a direct conversation). Agent and sandbox tokens never reach these
//! routes (their own auth scheme), and a chat-platform click can only steer.

use super::dto::{Followup, FollowupControls, FollowupList};
use super::service::{self, append_event, check_text, notify_work};
use crate::app::AppState;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::hosted;
use aurora_db::repo::turns::{self, TurnRow};
use aurora_protocol::runner::{TurnControls, harness_turn_controls};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// Most follow-ups the list returns.
pub const LIST_MAX: i64 = 50;
/// Largest slice of text copied into `followup.edited` (payload bound is 64 KiB).
const EVENT_TEXT_MAX_BYTES: usize = 48 * 1024;

fn not_found() -> ApiError {
    ApiError::not_found("Follow-up not found.")
}

fn not_waiting() -> ApiError {
    ApiError::conflict(
        "followup_not_waiting",
        "This message is no longer waiting: it was already sent to the agent, cancelled or run.",
    )
}

fn forbidden() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "followup_forbidden",
        "Only the person who wrote this message can edit it.",
    )
}

fn unsupported() -> ApiError {
    ApiError::conflict(
        "steer_unsupported",
        "This agent can't take messages while it works; the message runs when the current turn ends.",
    )
}

/// What the agent's harness and sandbox can do with a message that arrives mid-turn (C-1152).
#[derive(Debug, Clone, Copy)]
pub struct AgentFacts {
    pub tc: TurnControls,
    /// The agent is effectively frozen: no work starts, so nothing is steered or sent now (423).
    pub frozen: bool,
    /// What the running sandbox announced at `supervisor.ready`: `None` = nothing yet.
    pub reported_steer: Option<bool>,
}

pub async fn agent_facts(
    state: &AppState,
    conn: &mut PgConnection,
    agent_id: Uuid,
) -> Result<AgentFacts, ApiError> {
    let a = agents::get(&mut *conn, agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    // A BYO agent has no harness of ours: it has neither control.
    let tc = if a.kind == "hosted" {
        let id = a
            .harness
            .clone()
            .unwrap_or_else(|| state.ops().hosted.default_harness.clone());
        harness_turn_controls(&id)
    } else {
        harness_turn_controls("")
    };
    let frozen = crate::domains::freeze::service::ensure_agent_active(conn, agent_id)
        .await
        .is_err();
    let reported_steer = hosted::reported_steer(conn, agent_id).await?;
    Ok(AgentFacts {
        tc,
        frozen,
        reported_steer,
    })
}

fn is_active(status: &str) -> bool {
    matches!(status, "queued" | "dispatched" | "running")
}

/// The harness can steer a message into `parent` now: the table says it steers, and a sandbox that
/// already announced its capabilities for this run says so too (C-1155).
fn steer_supported(a: &AgentFacts, parent: Option<&TurnRow>) -> bool {
    let started = parent.is_some_and(|p| matches!(p.status.as_str(), "dispatched" | "running"));
    a.tc.steer && !(started && a.reported_steer == Some(false))
}

/// *Waiting* (C-1155): `joining` and the harness has not been handed it (or was, and failed).
pub fn is_waiting(f: &TurnRow) -> bool {
    f.status == "joining" && (f.steer_delivered_at.is_none() || f.steer_error.is_some())
}

/// What `user` may do with `f` now (C-1154). Pure, so every combination is unit-tested.
///
/// - `steer`: the harness steers, the parent turn is active, nobody asked yet and it was not
///   handed over, the agent is not frozen. (Stop authority is the caller's visibility.)
/// - `send_now`: the harness declares `background_tool` and does not steer, and the parent runs.
/// - `edit`: waiting, and `user` wrote it.
/// - `cancel`: waiting (Stop authority is the caller's visibility).
pub fn controls(
    f: &TurnRow,
    parent: Option<&TurnRow>,
    a: &AgentFacts,
    user: Uuid,
) -> FollowupControls {
    let joining = f.status == "joining";
    let parent_active = parent.is_some_and(|p| is_active(&p.status));
    let parent_running = parent.is_some_and(|p| p.status == "running");
    let waiting = is_waiting(f);
    FollowupControls {
        steer: joining
            && parent_active
            && f.steer_requested_at.is_none()
            && f.steer_delivered_at.is_none()
            && steer_supported(a, parent)
            && !a.frozen,
        send_now: joining && parent_running && a.tc.background_tool && !a.tc.steer && !a.frozen,
        edit: waiting && f.speaker_user_id == Some(user),
        cancel: waiting,
    }
}

/// The follow-up as `c` sees it, `controls` included. Reads the parent and agent facts itself.
pub async fn view(
    state: &AppState,
    conn: &mut PgConnection,
    c: &Caller,
    row: TurnRow,
) -> Result<Followup, ApiError> {
    let parent = match row.parent_turn_id {
        Some(p) => turns::get(&mut *conn, p).await?,
        None => None,
    };
    let a = agent_facts(state, conn, row.agent_id).await?;
    let promoted = if row.status == "promoted" {
        turns::promoted_turn_id(&mut *conn, row.conversation_id, row.id).await?
    } else {
        None
    };
    let ctl = controls(&row, parent.as_ref(), &a, c.user_id);
    let mut f = Followup::new(row, ctl);
    f.promoted_turn_id = promoted;
    Ok(f)
}

/// A follow-up the caller may see: the follow-up exists and so does its conversation for them
/// (404 otherwise, checked before anything about its state).
async fn load(state: &AppState, c: &Caller, id: Uuid) -> Result<TurnRow, ApiError> {
    let f = turns::get(&state.pool, id)
        .await?
        .filter(|t| t.role == "followup")
        .ok_or_else(not_found)?;
    service::load_conversation(state, c, f.conversation_id)
        .await
        .map_err(|_| not_found())?;
    Ok(f)
}

/// `GET /v1/conversations/{id}/followups` (C-1154).
pub async fn list(
    state: &AppState,
    c: &Caller,
    conversation_id: Uuid,
) -> Result<FollowupList, ApiError> {
    let conv = service::load_conversation(state, c, conversation_id).await?;
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let rows = turns::waiting_of_conversation(&mut *conn, conv.id, LIST_MAX).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        items.push(view(state, &mut conn, c, r).await?);
    }
    Ok(FollowupList { items })
}

/// Lock the conversation, then the follow-up row, in the caller's transaction (the lock order of
/// `turns.rs`). `404` when either is gone.
async fn lock_both(conn: &mut PgConnection, f: &TurnRow) -> Result<TurnRow, ApiError> {
    if !convs::lock(&mut *conn, f.conversation_id).await? {
        return Err(not_found());
    }
    turns::lock_followup(&mut *conn, f.id)
        .await?
        .ok_or_else(not_found)
}

/// The result of one steer: the follow-up, and whether this call asked for it (a repeat did not).
pub struct Steered {
    pub followup: Followup,
    pub newly: bool,
}

/// `POST /v1/followups/{id}/steer` (C-1155): ask for the follow-up to be handed to the running turn
/// at its next step. A repeat is a no-op.
pub async fn steer(state: &AppState, c: &Caller, id: Uuid) -> Result<Steered, ApiError> {
    let f0 = load(state, c, id).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let f = lock_both(&mut tx, &f0).await?;
    // 423 `frozen`: arm 7 hands nothing to a frozen agent.
    crate::domains::freeze::service::ensure_agent_active(&mut tx, f.agent_id).await?;
    if f.status != "joining" || f.steer_delivered_at.is_some() {
        return Err(not_waiting());
    }
    if f.steer_requested_at.is_some() {
        let followup = view(state, &mut tx, c, f).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(Steered {
            followup,
            newly: false,
        });
    }
    let parent = match f.parent_turn_id {
        Some(p) => turns::get(&mut *tx, p).await?,
        None => None,
    };
    let a = agent_facts(state, &mut tx, f.agent_id).await?;
    if !steer_supported(&a, parent.as_ref()) {
        return Err(unsupported());
    }
    let row = turns::request_steer(&mut tx, f.id)
        .await?
        .ok_or_else(not_waiting)?;
    append_event(
        &mut tx,
        row.conversation_id,
        row.parent_turn_id,
        "followup.steer_requested",
        &json!({"followup_id": row.id}),
    )
    .await?;
    notify_work(&mut tx).await?;
    let followup = view(state, &mut tx, c, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Steered {
        followup,
        newly: true,
    })
}

/// `POST /v1/followups/{id}/cancel` (C-1155): withdraw a waiting follow-up (never promoted).
pub async fn cancel(state: &AppState, c: &Caller, id: Uuid) -> Result<Followup, ApiError> {
    let f0 = load(state, c, id).await?;
    let by_label = service::speaker_label(state, c).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let f = lock_both(&mut tx, &f0).await?;
    if !is_waiting(&f) {
        return Err(not_waiting());
    }
    let row = turns::cancel_followup(&mut tx, f.id)
        .await?
        .ok_or_else(not_waiting)?;
    append_event(
        &mut tx,
        row.conversation_id,
        row.parent_turn_id,
        "followup.cancelled",
        // The canceller as a person (`{ id, display_name }`, C-1102): the dashboard shows them with `personName`.
        &json!({"followup_id": row.id, "by": {"id": c.user_id, "display_name": by_label}}),
    )
    .await?;
    let followup = view(state, &mut tx, c, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(followup)
}

/// `PATCH /v1/followups/{id}` (C-1155): replace the text of a waiting follow-up. Its own speaker
/// only; the same text rules as a new message; attachments and the quoted message are unchanged.
pub async fn edit(
    state: &AppState,
    c: &Caller,
    id: Uuid,
    text: &str,
) -> Result<Followup, ApiError> {
    let f0 = load(state, c, id).await?;
    // Authority before anything about its state or the text.
    if f0.speaker_user_id != Some(c.user_id) {
        return Err(forbidden());
    }
    check_text(text, !f0.attachment_ids.is_empty())?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let f = lock_both(&mut tx, &f0).await?;
    if !is_waiting(&f) {
        return Err(not_waiting());
    }
    let row = turns::edit_followup(&mut tx, f.id, text)
        .await?
        .ok_or_else(not_waiting)?;
    let shown = &text[..text.floor_char_boundary(EVENT_TEXT_MAX_BYTES)];
    append_event(
        &mut tx,
        row.conversation_id,
        row.parent_turn_id,
        "followup.edited",
        &json!({"followup_id": row.id, "text": shown}),
    )
    .await?;
    let followup = view(state, &mut tx, c, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(followup)
}

/// What a chat card's Steer click did (`13 §5.5`, C-1156).
pub enum CardSteer {
    /// This many waiting messages were asked to join the running turn.
    Steered(usize),
    /// No waiting message of the turn could be steered.
    NothingWaiting,
    /// The agent's harness cannot take a message mid-turn.
    Unsupported,
}

/// `followup.steer { turn_id }`: steer every waiting follow-up of the turn the clicker may steer
/// (the same authority as Stop: the caller already passed the conversation's visibility).
pub async fn steer_turn_followups(
    state: &AppState,
    c: &Caller,
    turn_id: Uuid,
) -> Result<CardSteer, ApiError> {
    let t = turns::get(&state.pool, turn_id)
        .await?
        .filter(|t| t.role == "turn")
        .ok_or_else(|| ApiError::not_found("Turn not found."))?;
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let a = agent_facts(state, &mut conn, t.agent_id).await?;
    if !steer_supported(&a, Some(&t)) {
        return Ok(CardSteer::Unsupported);
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM turns WHERE parent_turn_id = $1 AND role = 'followup'
           AND status = 'joining' AND steer_delivered_at IS NULL AND steer_requested_at IS NULL
         ORDER BY created_at, id",
    )
    .bind(turn_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    drop(conn);
    let mut n = 0;
    for id in ids {
        match steer(state, c, id).await {
            Ok(s) if s.newly => n += 1,
            Ok(_) => {}
            // Joined, cancelled or promoted since it was listed: not waiting any more.
            Err(e) if e.code() == "followup_not_waiting" => {}
            Err(e) => return Err(e),
        }
    }
    Ok(if n == 0 {
        CardSteer::NothingWaiting
    } else {
        CardSteer::Steered(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn row(status: &str) -> TurnRow {
        TurnRow {
            id: Uuid::nil(),
            conversation_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            role: if matches!(status, "joining" | "joined" | "promoted" | "cancelled") {
                "followup".into()
            } else {
                "turn".into()
            },
            status: status.into(),
            parent_turn_id: None,
            door: json!({"kind": "web"}),
            speaker_kind: "person".into(),
            speaker_user_id: Some(Uuid::from_u128(1)),
            speaker_label: "Ann".into(),
            text: "t".into(),
            source_event_id: None,
            abort_requested_at: None,
            send_now_requested_at: None,
            error_code: None,
            error_message: None,
            usage: None,
            started_at: None,
            finished_at: None,
            created_at: Utc::now(),
            attachment_ids: vec![],
            reply_to_seq: None,
            trace_context: None,
            steer_delivered_at: None,
            steer_requested_at: None,
            steer_error: None,
            edited_at: None,
        }
    }

    fn facts(steer: bool, background_tool: bool) -> AgentFacts {
        AgentFacts {
            tc: TurnControls {
                steer,
                background_tool,
            },
            frozen: false,
            reported_steer: None,
        }
    }

    const ANN: Uuid = Uuid::from_u128(1);
    const BOB: Uuid = Uuid::from_u128(2);

    fn flags(c: FollowupControls) -> (bool, bool, bool, bool) {
        (c.steer, c.send_now, c.edit, c.cancel)
    }

    #[test]
    fn a_steering_harness_offers_steer_edit_cancel_and_never_send_now() {
        let f = row("joining");
        let p = row("running");
        let c = controls(&f, Some(&p), &facts(true, false), ANN);
        assert_eq!(flags(c), (true, false, true, true));
        // `fake` declares both: it steers, so Steer applies and Send now does not (D-469(b)).
        let c = controls(&f, Some(&p), &facts(true, true), ANN);
        assert_eq!(flags(c), (true, false, true, true));
    }

    #[test]
    fn a_harness_that_only_backgrounds_offers_send_now_while_the_turn_runs() {
        let f = row("joining");
        let c = controls(&f, Some(&row("running")), &facts(false, true), ANN);
        assert_eq!(flags(c), (false, true, true, true));
        // Not while the parent has not started running.
        let c = controls(&f, Some(&row("queued")), &facts(false, true), ANN);
        assert_eq!(flags(c), (false, false, true, true));
    }

    #[test]
    fn a_harness_with_neither_offers_only_edit_and_cancel() {
        let f = row("joining");
        let c = controls(&f, Some(&row("running")), &facts(false, false), ANN);
        assert_eq!(flags(c), (false, false, true, true));
    }

    #[test]
    fn edit_is_the_authors_alone_cancel_is_anyones_who_may_stop() {
        let f = row("joining");
        let c = controls(&f, Some(&row("running")), &facts(true, false), BOB);
        assert_eq!(flags(c), (true, false, false, true));
    }

    #[test]
    fn once_steering_was_asked_steer_goes_and_edit_cancel_stay_until_delivery() {
        let mut f = row("joining");
        f.steer_requested_at = Some(Utc::now());
        let c = controls(&f, Some(&row("running")), &facts(true, false), ANN);
        assert_eq!(flags(c), (false, false, true, true));
        // Handed to the harness: nothing may change it any more...
        f.steer_delivered_at = Some(Utc::now());
        let c = controls(&f, Some(&row("running")), &facts(true, false), ANN);
        assert_eq!(flags(c), (false, false, false, false));
        // ...unless the harness did not take it: it waits for the next turn, editable and cancellable.
        f.steer_error = Some("missed".into());
        let c = controls(&f, Some(&row("running")), &facts(true, false), ANN);
        assert_eq!(flags(c), (false, false, true, true));
    }

    #[test]
    fn a_follow_up_that_is_no_longer_waiting_has_no_controls() {
        for status in ["joined", "promoted", "cancelled"] {
            let c = controls(&row(status), Some(&row("running")), &facts(true, true), ANN);
            assert_eq!(flags(c), (false, false, false, false), "{status}");
        }
    }

    #[test]
    fn a_frozen_agent_takes_no_steer_or_send_now_but_a_message_can_still_be_withdrawn() {
        let mut a = facts(true, false);
        a.frozen = true;
        let c = controls(&row("joining"), Some(&row("running")), &a, ANN);
        assert_eq!(flags(c), (false, false, true, true));
    }

    #[test]
    fn a_sandbox_that_announced_no_steer_for_the_running_turn_overrules_the_table() {
        let mut a = facts(true, false);
        a.reported_steer = Some(false);
        let c = controls(&row("joining"), Some(&row("running")), &a, ANN);
        assert!(!c.steer);
        // Before the turn started, the announcement is of an older run: the table decides.
        let c = controls(&row("joining"), Some(&row("queued")), &a, ANN);
        assert!(c.steer);
    }

    #[test]
    fn a_finished_parent_leaves_nothing_to_steer_into() {
        let c = controls(&row("joining"), Some(&row("done")), &facts(true, true), ANN);
        assert_eq!(flags(c), (false, false, true, true));
    }
}

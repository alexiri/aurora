//! (P4-011B-2, D-264, C-823..C-826) What a decision on a **parked** approval does besides the
//! status change: one platform-composed system follow-up per conversation in the deciding
//! transaction, and the transcript notices that end a grant or a parked approval honestly.
//!
//! The follow-up text is built only from stored sanitized fields (`one_line`): the summary title,
//! method, host, the stored (redacted) path and the decider's display name — never model or
//! request text (SEC-74).

use crate::app::AppState;
use crate::domains::conversations::service::{self as conv, NewMessage};
use aurora_core::{ApiError, limits, sanitize};
use aurora_db::repo::approvals::ApprovalRow;
use aurora_db::repo::turns::Created;
use aurora_db::repo::{approvals, conversations as convs, freeze};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

const FIELD_MAX: usize = 120;

/// The one-line fact of an approval in a follow-up: `<verb> by <label>: <title> — METHOD host path`.
fn line(verb: &str, label: &str, r: &ApprovalRow) -> String {
    let title = r
        .summary
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    format!(
        "- {verb} by {}: {} — {} {}{}",
        sanitize::one_line(label, FIELD_MAX),
        sanitize::one_line(title, FIELD_MAX),
        sanitize::one_line(&r.method, 16),
        sanitize::one_line(&r.host, FIELD_MAX),
        sanitize::one_line(&r.path, FIELD_MAX),
    )
}

/// At most `PARK_FOLLOWUP_LINES_MAX` lines, then "…and N more" (C-824).
fn capped(lines: Vec<String>) -> Vec<String> {
    let max = limits::PARK_FOLLOWUP_LINES_MAX;
    if lines.len() <= max {
        return lines;
    }
    let more = lines.len() - max;
    let mut out: Vec<String> = lines.into_iter().take(max).collect();
    out.push(format!("…and {more} more"));
    out
}

/// The follow-up text of one conversation (C-824). `approved` and `denied` are the decided rows
/// of this transaction; a transaction with both writes both blocks.
pub fn followup_text(
    approved: &[&ApprovalRow],
    denied: &[&ApprovalRow],
    window_secs: u64,
) -> String {
    let mut blocks = Vec::new();
    if !approved.is_empty() {
        let label = |r: &ApprovalRow| r.decided_by_name.clone().unwrap_or_default();
        let mut b = vec!["Aurora: a person approved a request you were waiting for.".to_owned()];
        b.extend(capped(
            approved
                .iter()
                .map(|r| line("Approved", &label(r), r))
                .collect(),
        ));
        let until = approved
            .iter()
            .filter_map(|r| r.resume_expires_at)
            .max()
            .map_or_else(
                || "the window's end".to_owned(),
                |t| t.format("%H:%M").to_string(),
            );
        b.push(format!(
            "Repeat exactly the same request once, byte for byte, before {until} UTC ({} minutes). \
             A changed request needs a new approval.",
            window_secs / 60
        ));
        blocks.push(b.join("\n"));
    }
    if !denied.is_empty() {
        let label = |r: &ApprovalRow| r.decided_by_name.clone().unwrap_or_default();
        let mut b = vec!["Aurora: a person denied a request you were waiting for.".to_owned()];
        b.extend(capped(
            denied
                .iter()
                .map(|r| line("Denied", &label(r), r))
                .collect(),
        ));
        b.push(
            "Do not try it again or look for another way to do it; tell the user it was denied."
                .to_owned(),
        );
        blocks.push(b.join("\n"));
    }
    blocks.join("\n\n")
}

/// Appends a platform `notice` to the conversation's transcript, when one remains.
pub async fn notice(
    tx: &mut PgConnection,
    conversation: Option<Uuid>,
    approval: Uuid,
    code: &str,
    text: &str,
) -> Result<(), ApiError> {
    let Some(cid) = conversation else {
        return Ok(());
    };
    if !convs::lock(&mut *tx, cid).await? {
        return Ok(());
    }
    conv::append_event(
        tx,
        cid,
        None,
        "notice",
        &json!({"code": code, "text": text, "approval_id": approval}),
    )
    .await?;
    Ok(())
}

/// Notice text of `approval_grant_revoked` for a cause (`frozen`, `access_removed`,
/// `session_reset`).
pub fn revoked_text(cause: &str) -> String {
    let why = match cause {
        "frozen" => "the agent was paused by an administrator",
        "session_reset" => "the agent's session was reset",
        _ => "the access it would use was removed",
    };
    format!("An approved request will not be accepted any more: {why}. A new approval is needed.")
}

/// After the decision statement, in the same transaction (C-823/C-824): for the **parked** rows
/// among `rows` (already decided; `approve` tells which way), write one follow-up per
/// conversation and set `resume_turn_id`; a frozen agent or a conversation that is gone gets no
/// follow-up and an approve ends `revoked` instead of `ready`.
pub async fn after_decide(
    tx: &mut PgConnection,
    s: &AppState,
    rows: &[ApprovalRow],
    approve: bool,
) -> Result<(), ApiError> {
    let parked: Vec<&ApprovalRow> = rows.iter().filter(|r| r.parked).collect();
    let mut conversations: Vec<Uuid> = parked.iter().filter_map(|r| r.conversation_id).collect();
    conversations.sort();
    conversations.dedup();
    let window = s.ops().approvals.park.resume_window_secs;
    for cid in conversations {
        let of: Vec<&ApprovalRow> = parked
            .iter()
            .copied()
            .filter(|r| r.conversation_id == Some(cid))
            .collect();
        let ids: Vec<Uuid> = of.iter().map(|r| r.id).collect();
        let conversation = if convs::lock(&mut *tx, cid).await? {
            convs::get(&mut *tx, cid).await?
        } else {
            None
        };
        let Some(c) = conversation else {
            if approve {
                approvals::revoke_ready(tx, &ids).await?;
            }
            continue;
        };
        if freeze::effective_of_agent(&mut *tx, c.agent_id)
            .await?
            .is_some()
        {
            if approve {
                approvals::revoke_ready(tx, &ids).await?;
            }
            continue;
        }
        let (approved, denied): (&[&ApprovalRow], &[&ApprovalRow]) =
            if approve { (&of, &[]) } else { (&[], &of) };
        let text = followup_text(approved, denied, window);
        let created = conv::create_turn_tx(
            tx,
            &c,
            &NewMessage {
                door: json!({"kind": "system", "ref": format!("approval:{}", ids[0])}),
                speaker_kind: "system",
                speaker_user_id: None,
                speaker_label: "Approval",
                text: &text,
                source_event_id: None,
                attachment_ids: &[],
                reply_to_seq: None,
            },
        )
        .await?;
        let (Created::Turn(t) | Created::Followup(t)) = created;
        approvals::set_resume_turn(tx, &ids, t.id).await?;
    }
    Ok(())
}

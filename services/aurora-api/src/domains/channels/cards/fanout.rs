//! Approval card fan-out job (docs/05 §Background jobs, docs/13 §5.6): on `aurora_approval`
//! notifications and every 10 s it (1) resolves `posting` cards from their finished post op,
//! (2) posts cards for pending approvals / action approvals of conversations with thread links,
//! (3) settles cards whose subject was decided or expired, (4) prunes the token ledger.
//! Idempotent and safe on every replica: posts and settles are claimed through
//! `channel_ops.dedupe_key` and the `channel_cards` state.

use super::{CardRow, Minter, NewPost, build, capabilities, post_card, repo, settle_card};
use crate::app::AppState;
use crate::domains::approvals::bus::BusMsg;
use aurora_channels::CapabilitySet;
use aurora_core::{ApiError, sanitize};
use aurora_db::DbError;
use aurora_db::repo::{
    action_approvals, agents, approvals, channel_integrations, channel_ops, peer_tasks,
    reach_grants,
};
use aurora_protocol::connector::{Sender, Target};
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

pub const EVERY: Duration = Duration::from_secs(10);
const BATCH: i64 = 50;

/// One pass; returns how many cards were posted or settled.
pub async fn run_once(state: &AppState) -> Result<usize, ApiError> {
    let mut n = sync_posting(state).await?;
    n += post_new(state).await?;
    n += settle_resolved(state).await?;
    n += resettle_ended(state).await?;
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    repo::prune_tokens(&mut conn).await?;
    Ok(n)
}

pub async fn sync_posting(state: &AppState) -> Result<usize, ApiError> {
    let mut n = 0;
    for c in repo::unsettled(&state.pool, 200).await? {
        if c.state != "posting" {
            continue;
        }
        let Some(op) = channel_ops::get(&state.pool, c.post_op_id).await? else {
            repo::set_failed(&state.pool, c.id).await?;
            continue;
        };
        match op.status.as_str() {
            "done" => {
                let r = op.result.unwrap_or_default();
                let mref = r.get("message_ref").and_then(|v| v.as_str()).unwrap_or("");
                let tref = r.get("thread_ref").and_then(|v| v.as_str()).unwrap_or("");
                if mref.is_empty() {
                    repo::set_failed(&state.pool, c.id).await?;
                } else if repo::set_posted(&state.pool, c.id, mref, tref).await? {
                    n += 1;
                }
            }
            "failed" | "dropped" => {
                repo::set_failed(&state.pool, c.id).await?;
            }
            _ => {}
        }
    }
    Ok(n)
}

async fn post_new(state: &AppState) -> Result<usize, ApiError> {
    let mut n = 0;
    for id in repo::approvals_without_card(&state.pool, BATCH).await? {
        match post_approval(state, id).await {
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(approval = %id, error = %e.message(), "approval card failed"),
        }
    }
    for id in repo::action_approvals_without_card(&state.pool, BATCH).await? {
        match post_action_approval(state, id).await {
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(approval = %id, error = %e.message(), "action approval card failed");
            }
        }
    }
    Ok(n)
}

fn sender_of(v: &serde_json::Value) -> Sender {
    serde_json::from_value(v.clone()).unwrap_or_else(|_| Sender::shared())
}

async fn post_approval(state: &AppState, id: Uuid) -> Result<bool, ApiError> {
    let Some(row) = approvals::get(&state.pool, id).await? else {
        return Ok(false);
    };
    let Some(conv) = row.conversation_id else {
        return Ok(false);
    };
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    // (FIX-252) An approval raised in a peer task conversation has no thread link of its own; its
    // card goes to the origin conversation the request came from, (FIX-288) up the delegation
    // chain while the same person asked (`card_home`). The card carries the same facts as every
    // approval card plus the delegating agent ("asked for by …"); D-178 redaction follows that
    // conversation's link kind. Deciding stays `can_approve` at the approval's workspace (the
    // action handler re-checks at click time); the card's buttons change nothing about that.
    let peer = peer_tasks::by_task_conversation(&mut *tx, conv).await?;
    let link = match repo::latest_thread_link(&mut *tx, conv).await? {
        Some(l) => Some(l),
        None => match peer.as_ref() {
            Some(pt) => match peer_tasks::card_home(&mut *tx, pt.id).await? {
                Some(home) => repo::latest_thread_link(&mut *tx, home).await?,
                None => None,
            },
            None => None,
        },
    };
    let Some(link) = link else {
        return Ok(false);
    };
    if !integration_active(&mut tx, link.integration_id).await? {
        return Ok(false);
    }
    let caps = capabilities(&mut tx, link.integration_id).await?;
    let m = Minter {
        keys: &state.derived,
        integration_id: link.integration_id,
    };
    // D-178: only a 1:1 conversation (`direct`) gets the request's details on the card.
    let peer_card = match peer.as_ref() {
        Some(pt) => {
            let asked = agents::get(&mut *tx, pt.from_agent_id)
                .await?
                .map(|a| crate::domains::peer_tasks::service::agent_label(&a));
            // (FIX-252) The card's buttons follow the requester's rights at the approval's
            // workspace, read at post time; every decide re-checks them there.
            let requester_may_decide = match pt.requester_user_id {
                Some(u) => aurora_db::authz::can_approve(&state.pool, u, row.workspace_id)
                    .await
                    .unwrap_or(false),
                None => false,
            };
            let workspace_name =
                aurora_db::repo::approval_notify::workspace_labels(&state.pool, row.workspace_id)
                    .await?
                    .map_or_else(|| "the workspace".into(), |w| w.0);
            Some(build::PeerOrigin {
                asked_for_by: asked.unwrap_or_else(|| "another agent".into()),
                workspace_name,
                requester_may_decide,
            })
        }
        None => None,
    };
    let card = build::approval(
        &m,
        &row,
        link.kind != "direct",
        state.ops().approvals.park.resume_window_secs,
        peer_card.as_ref(),
    )?;
    let approvals_url =
        super::workspace_page_url(&mut tx, state, row.workspace_id, "approvals").await?;
    let posted = post_card(
        &mut tx,
        &caps,
        NewPost {
            integration_id: link.integration_id,
            subject_kind: "approval",
            subject_id: id,
            card_id: Uuid::now_v7(),
            target: Target::Space {
                space_ref: link.space_ref.clone(),
                thread_ref: Some(link.thread_ref.clone()),
            },
            sender: sender_of(&link.sender),
            card,
            dashboard_url: Some(approvals_url),
            approval_code: Some(super::approval_code(id)),
            dedupe_key: format!("approval:{id}"),
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(posted)
}

async fn post_action_approval(state: &AppState, id: Uuid) -> Result<bool, ApiError> {
    let Some(row) = action_approvals::get(&state.pool, id).await? else {
        return Ok(false);
    };
    let Some(conv) = row.conversation_id else {
        return Ok(false);
    };
    let agent = agents::get(&state.pool, row.agent_id).await?;
    let label = agent.as_ref().map_or("unknown", |a| a.name.as_str());
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let Some(link) = repo::latest_thread_link(&mut *tx, conv).await? else {
        return Ok(false);
    };
    if !integration_active(&mut tx, link.integration_id).await? {
        return Ok(false);
    }
    let caps = capabilities(&mut tx, link.integration_id).await?;
    let m = Minter {
        keys: &state.derived,
        integration_id: link.integration_id,
    };
    let card = build::action_approval(&m, &row, label)?;
    let approvals_url =
        super::workspace_page_url(&mut tx, state, row.workspace_id, "approvals").await?;
    let posted = post_card(
        &mut tx,
        &caps,
        NewPost {
            integration_id: link.integration_id,
            subject_kind: "action_approval",
            subject_id: id,
            card_id: Uuid::now_v7(),
            target: Target::Space {
                space_ref: link.space_ref.clone(),
                thread_ref: Some(link.thread_ref.clone()),
            },
            sender: sender_of(&link.sender),
            card,
            dashboard_url: Some(approvals_url),
            approval_code: Some(super::approval_code(id)),
            dedupe_key: format!("action_approval:{id}"),
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(posted)
}

async fn integration_active(conn: &mut sqlx::PgConnection, id: Uuid) -> Result<bool, ApiError> {
    Ok(channel_integrations::get(&mut *conn, id)
        .await?
        .is_some_and(|i| i.status == "active"))
}

fn who(name: Option<&str>) -> String {
    sanitize::markdown_as_data(name.unwrap_or("someone"), 80)
}

fn how(via: Option<&str>) -> &'static str {
    match via {
        Some("channel") => "in chat",
        Some("api") => "through the API",
        _ => "in the dashboard",
    }
}

/// (C-825) Card text once a parked approval's window ended unused.
fn window_ended_card_text() -> String {
    "Approval window ended: the approved request was not repeated in time.".into()
}

/// (C-825) Card text once an approved parked grant was revoked.
fn revoked_card_text() -> String {
    "Revoked: the approval can no longer be used.".into()
}

/// (C-825) Edit the settled card of a parked approval once more when its grant ended afterwards.
pub async fn resettle_ended(state: &AppState) -> Result<usize, ApiError> {
    let mut n = 0;
    let mut caps: HashMap<Uuid, CapabilitySet> = HashMap::new();
    for c in repo::settled_with_ended_grant(&state.pool, 100).await? {
        let Some(a) = approvals::get(&state.pool, c.subject_id).await? else {
            continue;
        };
        let text = match a.resume_state.as_deref() {
            Some("revoked") => revoked_card_text(),
            Some("expired") => window_ended_card_text(),
            _ => continue,
        };
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        let cs = match caps.get(&c.integration_id) {
            Some(x) => x.clone(),
            None => {
                let x = capabilities(&mut tx, c.integration_id).await?;
                caps.insert(c.integration_id, x.clone());
                x
            }
        };
        super::resettle_card(&mut tx, &c, &cs, &text).await?;
        n += 1;
        tx.commit().await.map_err(DbError::from)?;
    }
    Ok(n)
}

/// Settle text of a card whose subject is resolved; `None` while it is still open.
pub async fn settle_text(state: &AppState, c: &CardRow) -> Result<Option<String>, ApiError> {
    Ok(match c.subject_kind.as_str() {
        "approval" => {
            let Some(a) = approvals::get(&state.pool, c.subject_id).await? else {
                return Ok(Some("This request no longer exists.".into()));
            };
            let by = who(a.decided_by_name.as_deref());
            let agent = who(a.agent_name.as_deref());
            match (a.status.as_str(), a.parked) {
                ("pending", _) => None,
                // (C-825) A parked grant that ended before this settle shows its end directly.
                ("approved", true) => Some(match a.resume_state.as_deref() {
                    Some("revoked") => revoked_card_text(),
                    Some("expired") => window_ended_card_text(),
                    _ => format!(
                        "Approved by {by} {} — {agent} will be told.",
                        how(a.decided_via.as_deref())
                    ),
                }),
                ("approved", false) => Some(format!(
                    "Approved by {by} {}.",
                    how(a.decided_via.as_deref())
                )),
                ("denied", _) => Some(format!("Denied by {by} {}.", how(a.decided_via.as_deref()))),
                ("expired", _) => Some("Expired without a decision.".into()),
                // (C-825) A parked approval abandoned by a revocation event.
                ("abandoned", true) => Some("No longer needed.".into()),
                _ => Some("Cancelled: the request was abandoned.".into()),
            }
        }
        "action_approval" => {
            let Some(a) = action_approvals::get(&state.pool, c.subject_id).await? else {
                return Ok(Some("This request no longer exists.".into()));
            };
            let by = who(a.decided_by_name.as_deref());
            match a.status.as_str() {
                "pending" => None,
                "approved" | "executed" => Some(format!("Approved by {by}.")),
                "failed" => Some(format!("Approved by {by}, but it could not be completed.")),
                "rejected" => Some(format!("Denied by {by}.")),
                _ => Some("Expired without a decision.".into()),
            }
        }
        "reach" => {
            let Some(g) = reach_grants::get(&state.pool, c.subject_id).await? else {
                return Ok(Some("This request no longer exists.".into()));
            };
            let by = who(g.decided_by_name.as_deref());
            match g.state.as_str() {
                "pending" => None,
                "approved" => Some(format!("Allowed by {by}.")),
                "blocked" => Some(format!("Blocked by {by}.")),
                _ => Some("This request is no longer open.".into()),
            }
        }
        _ => None,
    })
}

/// Settle every posted card whose subject is resolved.
pub async fn settle_resolved(state: &AppState) -> Result<usize, ApiError> {
    let mut n = 0;
    let mut caps: HashMap<Uuid, CapabilitySet> = HashMap::new();
    for c in repo::unsettled(&state.pool, 200).await? {
        if c.state != "posted" {
            continue;
        }
        let Some(text) = settle_text(state, &c).await? else {
            continue;
        };
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        let cs = match caps.get(&c.integration_id) {
            Some(x) => x.clone(),
            None => {
                let x = capabilities(&mut tx, c.integration_id).await?;
                caps.insert(c.integration_id, x.clone());
                x
            }
        };
        if settle_card(&mut tx, &c, &cs, &text).await? {
            n += 1;
        }
        tx.commit().await.map_err(DbError::from)?;
    }
    Ok(n)
}

/// Start the job on the current runtime (every replica).
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut rx = state.approval_bus.subscribe(&state.pool).await.ok();
        loop {
            match run_once(&state).await {
                Ok(n) if n > 0 => tracing::info!(cards = n, "approval card fan-out"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "approval card fan-out failed"),
            }
            let wait = async {
                match rx.as_mut() {
                    Some(r) => loop {
                        match r.recv().await {
                            Ok(BusMsg::Event { event, .. }) if event == "created" => break,
                            Ok(BusMsg::Resync) => break,
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                            Err(_) => std::future::pending::<()>().await,
                        }
                    },
                    None => std::future::pending::<()>().await,
                }
            };
            let _ = tokio::time::timeout(EVERY, wait).await;
        }
    });
}
